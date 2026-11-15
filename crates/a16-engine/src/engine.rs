// SPDX-License-Identifier: MIT
//! The engine: the A16's protocol on top of a [`Bus`], moving audio between USB transfers
//! and the PCM rings.

use std::fmt;
use std::time::Duration;

use crate::bus::{Bus, Status, Transfer, TransferId};
use crate::codec::{self, IN_FRAME_BYTES, IN_SAMPLES_BYTES, OUT_FRAME_BYTES, OUT_SAMPLES_BYTES};
use crate::handshake::{self, HandshakeError, RESTART_QUIET};
use crate::protocol::{FRAMES_PER_IN_SUBPACKET, FRAMES_PER_OUT_SUBPACKET, OutLayout};
use crate::settings::{self, MAX_TRANSFER_FRAMES, Setting, SettingChange, SettingError, Settings};
use crate::usb::{Pipe, UsbEndpoint};
use crate::{Direction, PcmRing, PerDirection, SampleRate};

/// The least safety offset reported at any rate: 1 ms, room for the jitter of the
/// transfers' completions. A guess, to be measured on hardware; far below what the defaults
/// queue.
const SAFETY_MIN: Duration = Duration::from_millis(1);

/// Where the engine is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EngineState {
    /// Not started, or stopped: the device isn't streaming and the host should drop it.
    #[default]
    Stopped,
    /// Paused for a restart: no transfers, but the host keeps the device published.
    Suspended,
    /// Transfers in flight; each completion queues the next.
    Streaming,
}

/// Why a start failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartError {
    /// The engine is streaming already: suspend or stop it first, or use
    /// [`Engine::restart`].
    AlreadyStreaming,
    /// The device didn't take the handshake.
    Handshake(HandshakeError),
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StartError::AlreadyStreaming => f.write_str("the engine is streaming already"),
            StartError::Handshake(error) => write!(f, "handshake: {error}"),
        }
    }
}

impl std::error::Error for StartError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StartError::AlreadyStreaming => None,
            StartError::Handshake(error) => Some(error),
        }
    }
}

/// What the engine logs through: one line per call, without a newline.
pub type Logger = Box<dyn FnMut(&str) + Send>;

/// A ring attached to the engine and its position: the ring frame of the next transfer's
/// first frame is `position % ring.frames()`.
#[derive(Debug)]
struct Attached {
    ring: PcmRing,
    position: u64,
}

/// The A16's protocol engine.
///
/// A platform:
/// 1. claims [`INTERFACES`](crate::INTERFACES) at their alternate settings, lists their
///    endpoints for [`Engine::bind_endpoints`] and opens the pipes it picks
/// 2. attaches the PCM rings ([`Engine::attach_pcm_ring`]) and calls [`Engine::start`]
/// 3. hands every completion to [`Engine::complete`]
/// 4. calls [`Engine::stop`], which aborts every transfer ([`Bus::abort`])
///
/// The engine keeps no locks and no threads: the platform calls it from one thread at a
/// time, its USB event thread, and passes its [`Bus`] to each call that uses it. PCM reaches
/// the engine through the rings it owns, which the platform reads and writes on that same
/// thread ([`Bus::pcm_progress`], [`Engine::pcm_ring_mut`]); audio from other threads goes
/// through the platform's queues into the rings there.
pub struct Engine {
    settings: Settings,
    requested_rate: Option<SampleRate>,
    pipes: PerDirection<Option<Pipe>>,
    layout: OutLayout,
    state: EngineState,
    /// Has streamed since it was made: a start is a restart, which settles first.
    streamed: bool,
    frames: PerDirection<u32>,
    in_flight: PerDirection<u32>,
    sample_rate: Option<SampleRate>,
    safety_offset: PerDirection<u32>,
    rings: PerDirection<Option<Attached>>,
    log: Logger,
}

impl fmt::Debug for Engine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Engine")
            .field("state", &self.state)
            .field("settings", &self.settings)
            .field("requested_rate", &self.requested_rate)
            .field("sample_rate", &self.sample_rate)
            .field("pipes", &self.pipes)
            .field("frames", &self.frames)
            .field("in_flight", &self.in_flight)
            .finish_non_exhaustive()
    }
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new()
    }
}

/// `frames` as whole packets of `step` frames (0: any): the next size up, at most `max`.
fn whole_packets(frames: u32, step: u32, max: u32) -> u32 {
    if step == 0 || frames.is_multiple_of(step) {
        return frames;
    }
    let whole = (frames / step + 1) * step;
    if whole > max { whole - step } else { whole }
}

/// The safety offset reported for `queued` frames at `rate`: at least [`SAFETY_MIN`].
fn safety_offset(queued: u32, rate: SampleRate) -> u32 {
    let floor = (u64::from(rate.hz()) * SAFETY_MIN.as_micros() as u64).div_ceil(1_000_000);
    queued.max(floor as u32)
}

impl Engine {
    /// An engine at the default settings, with no pipes, rings or rate request, logging to
    /// stderr.
    pub fn new() -> Engine {
        let settings = Settings::default();
        Engine {
            frames: PerDirection::new(
                settings::pcm_frames(Direction::Out, settings[Setting::PcmOutFrames]),
                settings::pcm_frames(Direction::In, settings[Setting::PcmInFrames]),
            ),
            in_flight: PerDirection::new(
                settings::pcm_queue(settings[Setting::PcmOutQueue]),
                settings::pcm_queue(settings[Setting::PcmInQueue]),
            ),
            settings,
            requested_rate: None,
            pipes: PerDirection::default(),
            layout: OutLayout::default(),
            state: EngineState::Stopped,
            streamed: false,
            sample_rate: None,
            safety_offset: PerDirection::default(),
            rings: PerDirection::default(),
            log: Box::new(|line| eprintln!("a16-engine: {line}")),
        }
    }

    /// Log through `logger` from now on.
    pub fn set_logger(&mut self, logger: Logger) {
        self.log = logger;
    }

    fn log(&mut self, args: fmt::Arguments<'_>) {
        (self.log)(&args.to_string());
    }

    // --- Endpoints ---

    /// Pick the PCM pipes from `endpoints`, every endpoint of the claimed interfaces at their
    /// current setting, and run on them: each direction takes its fixed address
    /// ([`Direction::endpoint`]) on its interface. Every endpoint is logged, so a log doubles
    /// as a descriptor dump. Returns the endpoint picked for each direction, which the
    /// platform opens.
    pub fn bind_endpoints(
        &mut self,
        endpoints: &[UsbEndpoint],
    ) -> PerDirection<Option<UsbEndpoint>> {
        for ep in endpoints {
            self.log(format_args!(
                "  interface {}: EP 0x{:02x} {:<3} {:<9} wMaxPacketSize {} x{} bInterval {}",
                ep.interface,
                ep.address,
                if ep.address & 0x80 != 0 { "IN" } else { "OUT" },
                ep.transfer_type().to_string(),
                ep.max_packet & 0x07FF,
                ((ep.max_packet >> 11) & 3) + 1,
                ep.interval
            ));
        }
        let mut bound = PerDirection::default();
        for direction in Direction::ALL {
            let found = endpoints.iter().find(|ep| {
                ep.interface == direction.interface() && ep.address == direction.endpoint()
            });
            match found {
                Some(ep) => {
                    bound[direction] = Some(*ep);
                    self.set_pipe(direction, ep.pipe());
                }
                None => self.log(format_args!(
                    "  no endpoint for PCM {direction} on interface {}",
                    direction.interface()
                )),
            }
        }
        bound
    }

    /// Run `direction`'s PCM on `pipe` ([`Engine::bind_endpoints`] does this for each it
    /// picks). The out pipe's packet size picks the output layout.
    pub fn set_pipe(&mut self, direction: Direction, pipe: Pipe) {
        self.pipes[direction] = Some(pipe);
        if direction == Direction::Out {
            let layout = OutLayout::for_max_packet(pipe.max_packet_size);
            self.layout = layout;
            self.log(format_args!(
                "transport: {}, wMaxPacketSize {} ({layout} layout, {}-byte sub-packets)",
                pipe.transfer_type,
                pipe.max_packet_size,
                layout.subpacket_bytes()
            ));
        }
    }

    pub fn pipe(&self, direction: Direction) -> Option<Pipe> {
        self.pipes[direction]
    }

    /// The output packet layout, from the out pipe.
    pub fn out_layout(&self) -> OutLayout {
        self.layout
    }

    /// What a PCM transfer in `direction` must be a multiple of, in frames: one whole packet
    /// of its pipe (0: any).
    pub fn packet_frames(&self, direction: Direction) -> u32 {
        self.pipes[direction].map_or(0, |pipe| pipe.packet_frames(direction.wire_frame_bytes()))
    }

    // --- Settings and rate ---

    /// What the next start runs with. After a start, what it ran with: a size that wasn't
    /// whole packets shows as the size it ran at.
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Set `setting` for the next start, without checks: a size kept from before that isn't
    /// whole packets runs at the next size up, and a value out of range at the default.
    pub fn set_setting(&mut self, setting: Setting, value: u8) {
        self.settings.set(setting, value);
    }

    /// A setting from the user: checked against the setting's values and the pipes, logged,
    /// and set for the next start.
    pub fn change_setting(
        &mut self,
        setting: Setting,
        value: u8,
    ) -> Result<SettingChange, SettingError> {
        if let Err(error) = settings::check_value(setting, value, |d| self.packet_frames(d)) {
            self.log(format_args!("{error}"));
            return Err(error);
        }
        if self.settings[setting] == value {
            return Ok(SettingChange::Unchanged);
        }
        let name = setting.value_name(value).unwrap_or_default();
        self.log(format_args!("{setting}: {name}"));
        self.settings.set(setting, value);
        Ok(SettingChange::NeedsRestart)
    }

    /// The rate the next start asks for (None: 48 kHz). A streaming engine takes it with a
    /// restart.
    pub fn set_rate(&mut self, rate: Option<SampleRate>) {
        self.requested_rate = rate;
    }

    pub fn requested_rate(&self) -> Option<SampleRate> {
        self.requested_rate
    }

    /// A rate from the host, logged: unchanged if the device runs at it, else set for the
    /// next start.
    pub fn change_rate(&mut self, rate: SampleRate) -> SettingChange {
        if self.sample_rate == Some(rate) {
            return SettingChange::Unchanged;
        }
        let current = self.sample_rate.map_or(0, SampleRate::hz);
        self.log(format_args!("sample rate {current} -> {} Hz", rate.hz()));
        self.requested_rate = Some(rate);
        SettingChange::NeedsRestart
    }

    // --- The stream, as the last start set it up ---

    pub fn state(&self) -> EngineState {
        self.state
    }

    /// The rate the device runs at, from the last start that succeeded.
    pub fn sample_rate(&self) -> Option<SampleRate> {
        self.sample_rate
    }

    /// The frames one PCM transfer of `direction` moves: the smallest period a platform can
    /// let its audio stack ask for. Before the first start, the default sizes.
    pub fn transfer_frames(&self, direction: Direction) -> u32 {
        self.frames[direction]
    }

    /// The bytes of one PCM transfer of `direction`.
    pub fn transfer_bytes(&self, direction: Direction) -> usize {
        let frames = self.frames[direction];
        match direction {
            Direction::Out => self.layout.transfer_bytes(frames),
            Direction::In => frames as usize * IN_FRAME_BYTES,
        }
    }

    /// The PCM transfers kept in flight in `direction`.
    pub fn in_flight(&self, direction: Direction) -> u32 {
        self.in_flight[direction]
    }

    /// How far ahead of the device's clock the host must write output (out), or how far
    /// behind it the host may read input (in), in frames: the transfers queued, at least 1 ms.
    /// 0 before the first start.
    pub fn safety_offset(&self, direction: Direction) -> u32 {
        self.safety_offset[direction]
    }

    // --- PCM rings ---

    /// Attach `ring` for its direction, replacing (and returning) the one attached. The next
    /// transfer starts at ring position `start`. Without a ring, out transfers carry silence
    /// and in transfers are dropped.
    pub fn attach_pcm_ring(&mut self, ring: PcmRing, start: u64) -> Option<PcmRing> {
        let direction = ring.direction();
        self.rings[direction]
            .replace(Attached {
                ring,
                position: start,
            })
            .map(|attached| attached.ring)
    }

    pub fn detach_pcm_ring(&mut self, direction: Direction) -> Option<PcmRing> {
        self.rings[direction].take().map(|attached| attached.ring)
    }

    pub fn pcm_ring(&self, direction: Direction) -> Option<&PcmRing> {
        self.rings[direction]
            .as_ref()
            .map(|attached| &attached.ring)
    }

    pub fn pcm_ring_mut(&mut self, direction: Direction) -> Option<&mut PcmRing> {
        self.rings[direction]
            .as_mut()
            .map(|attached| &mut attached.ring)
    }

    /// The ring position of `direction`'s next transfer, if a ring is attached.
    pub fn pcm_position(&self, direction: Direction) -> Option<u64> {
        self.rings[direction]
            .as_ref()
            .map(|attached| attached.position)
    }

    // --- Start, stop, restart ---

    /// The handshake, then streaming: the settings and rate take effect, and the transfers
    /// in flight are queued.
    pub fn start<B: Bus + ?Sized>(&mut self, bus: &mut B) -> Result<(), StartError> {
        if self.state == EngineState::Streaming {
            return Err(StartError::AlreadyStreaming);
        }
        self.configure_transfers();

        // A restart first lets the device settle from the transfers just stopped
        if self.streamed {
            bus.sleep(RESTART_QUIET);
        }
        let rate = self.requested_rate.unwrap_or_default();
        handshake::handshake(bus, rate, &mut *self.log).map_err(StartError::Handshake)?;
        self.sample_rate = Some(rate);
        self.safety_offset = self
            .safety_offset
            .map(|_, queued| safety_offset(queued, rate));

        self.state = EngineState::Streaming;
        self.streamed = true;
        let (out_queue, in_queue) = (
            self.in_flight[Direction::Out],
            self.in_flight[Direction::In],
        );
        for i in 0..out_queue.max(in_queue) {
            if i < in_queue {
                let buffer = vec![0; self.transfer_bytes(Direction::In)];
                self.submit(bus, Transfer::new(Direction::In, TransferId(i), buffer));
            }
            if i < out_queue {
                // Silence and idle MIDI and control bytes: the ring is encoded from the
                // transfer after the ones queued here
                let mut buffer = vec![0; self.transfer_bytes(Direction::Out)];
                self.layout.init_subpackets(&mut buffer);
                self.submit(bus, Transfer::new(Direction::Out, TransferId(i), buffer));
            }
        }
        Ok(())
    }

    /// The transfer sizes and queues for this run, from the settings. Sizes kept from before
    /// that aren't whole packets take the next size up (down at the top of the list), and
    /// the setting follows.
    fn configure_transfers(&mut self) {
        for direction in Direction::ALL {
            let setting = Setting::frames(direction);
            let per_subpacket = match direction {
                Direction::Out => FRAMES_PER_OUT_SUBPACKET,
                Direction::In => FRAMES_PER_IN_SUBPACKET,
            };
            let asked = settings::pcm_frames(direction, self.settings[setting]);
            let frames = whole_packets(asked, self.packet_frames(direction), MAX_TRANSFER_FRAMES);
            if frames != asked {
                self.log(format_args!(
                    "transfers of {frames} frames, whole {}-frame packets instead of {asked}",
                    self.packet_frames(direction)
                ));
            }
            self.frames[direction] = frames;
            self.settings
                .set(setting, (frames / per_subpacket - 1) as u8);

            // What runs: one in flight (a setting stored before it went) runs as two
            let setting = Setting::queue(direction);
            let queue = settings::pcm_queue(self.settings[setting]);
            self.in_flight[direction] = queue;
            self.settings.set(setting, (queue - 1) as u8);
            self.safety_offset[direction] = queue * frames;
        }
        let frames = self.frames;
        self.log(format_args!(
            "model: {}, 2 in / 16 out, transfers of {} frames out, {} in",
            crate::PRODUCT_NAME,
            frames[Direction::Out],
            frames[Direction::In]
        ));
    }

    /// No more transfers: stop queueing, abort what is in flight, and tell the host the
    /// device is gone.
    pub fn stop<B: Bus + ?Sized>(&mut self, bus: &mut B) {
        self.state = EngineState::Stopped;
        self.log(format_args!("engine stopped"));
        drop(bus.abort());
    }

    /// Pause for a restart, or for a pause the platform makes around a USB reset: like stop,
    /// but the host keeps the device published. [`Engine::start`] runs the handshake again.
    pub fn suspend<B: Bus + ?Sized>(&mut self, bus: &mut B) {
        if self.state == EngineState::Streaming {
            self.state = EngineState::Suspended;
        }
        self.log(format_args!("engine suspended"));
        drop(bus.abort());
    }

    /// Take the rate and settings that changed: suspend, then start again. If the device
    /// doesn't take it, the engine is stopped.
    pub fn restart<B: Bus + ?Sized>(&mut self, bus: &mut B) -> Result<(), StartError> {
        self.suspend(bus);
        let result = self.start(bus);
        if result.is_err() {
            self.log(format_args!("restart failed"));
            self.stop(bus);
        }
        result
    }

    // --- The completion path ---

    /// A transfer the engine submitted came back. While streaming, the next one goes out:
    /// out, encoded from the ring; in, after the transfer's frames are decoded into the ring.
    pub fn complete<B: Bus + ?Sized>(
        &mut self,
        bus: &mut B,
        mut transfer: Transfer,
        status: Status,
    ) {
        let direction = transfer.direction();
        if self.state != EngineState::Streaming || status == Status::Aborted {
            return;
        }
        let next = transfer.id().after(self.in_flight[direction]);
        match direction {
            Direction::Out => self.encode_out(bus, transfer.buffer_mut()),
            Direction::In => {
                let received = match status {
                    Status::Completed(bytes) => bytes,
                    Status::Failed | Status::Aborted => 0,
                };
                self.decode_in(bus, transfer.buffer(), received);
            }
        }
        transfer.set_id(next);
        self.submit(bus, transfer);
    }

    fn submit<B: Bus + ?Sized>(&mut self, bus: &mut B, transfer: Transfer) {
        if let Err(transfer) = bus.submit(transfer) {
            self.log(format_args!(
                "PCM {} submission failed (idx: {})",
                transfer.direction(),
                transfer.id()
            ));
        }
    }

    /// The next out transfer's frames from the ring, encoded into `packet` (whose MIDI and
    /// control bytes are already in place); silence without a ring.
    fn encode_out<B: Bus + ?Sized>(&mut self, bus: &mut B, packet: &mut [u8]) {
        let frames = self.frames[Direction::Out];
        let layout = self.layout;
        let Some(attached) = self.rings[Direction::Out].as_mut() else {
            for f in 0..frames {
                let at = layout.frame_offset(f);
                packet[at..at + OUT_FRAME_BYTES].fill(0);
            }
            return;
        };
        let ring_frames = attached.ring.frames();
        let mut r = (attached.position % ring_frames as u64) as usize;
        for f in 0..frames {
            let at = layout.frame_offset(f);
            let wire: &mut [u8; OUT_FRAME_BYTES] = (&mut packet[at..at + OUT_FRAME_BYTES])
                .try_into()
                .expect("a frame slice");
            let samples: &[u8; OUT_SAMPLES_BYTES] = attached
                .ring
                .frame(r)
                .try_into()
                .expect("a 16-channel ring frame");
            codec::encode_frame(wire, samples);
            r += 1;
            if r == ring_frames {
                r = 0;
            }
        }
        attached.position += u64::from(frames);
        bus.pcm_progress(Direction::Out, attached.position, &mut attached.ring);
    }

    /// A completed in transfer's frames into the ring: the `received` bytes decoded, the
    /// frames that didn't arrive as silence, so the ring keeps the device's clock.
    fn decode_in<B: Bus + ?Sized>(&mut self, bus: &mut B, packet: &[u8], received: usize) {
        let frames = self.frames[Direction::In] as usize;
        let Some(attached) = self.rings[Direction::In].as_mut() else {
            return;
        };
        let got = (received / IN_FRAME_BYTES).min(frames);
        let ring_frames = attached.ring.frames();
        let mut r = (attached.position % ring_frames as u64) as usize;
        for f in 0..frames {
            let samples: &mut [u8; IN_SAMPLES_BYTES] = attached
                .ring
                .frame_mut(r)
                .try_into()
                .expect("a 2-channel ring frame");
            if f < got {
                let wire: &[u8; IN_FRAME_BYTES] = packet
                    [f * IN_FRAME_BYTES..(f + 1) * IN_FRAME_BYTES]
                    .try_into()
                    .expect("a frame slice");
                codec::decode_frame(samples, wire);
            } else {
                samples.fill(0);
            }
            r += 1;
            if r == ring_frames {
                r = 0;
            }
        }
        attached.position += frames as u64;
        bus.pcm_progress(Direction::In, attached.position, &mut attached.ring);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_and_floors() {
        assert_eq!(whole_packets(80, 20, 200), 80);
        assert_eq!(whole_packets(10, 20, 200), 20);
        assert_eq!(whole_packets(200, 16, 200), 192);
        assert_eq!(whole_packets(8, 0, 200), 8);
        assert_eq!(safety_offset(20, SampleRate::Hz48000), 48);
        assert_eq!(safety_offset(320, SampleRate::Hz44100), 320);
        assert_eq!(safety_offset(16, SampleRate::Hz44100), 45);
    }
}
