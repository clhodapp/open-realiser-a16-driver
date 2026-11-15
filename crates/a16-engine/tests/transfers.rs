// SPDX-License-Identifier: MIT
//! PCM transfer sizes on the A16, both out layouts (its 964-byte interrupt endpoint, and a
//! 512-byte bulk one), every size that is whole packets of the out endpoint (20 frames on the
//! 964 layout, 10 on 512; the rest is rounded up): a transfer is frames / 10 sub-packets out
//! and frames * 64 bytes in; the bytes on the wire are the same stream at every size (PCM
//! and control bytes), only cut differently, from the first transfer the ring is encoded into
//! (the ones queued at start are silence); input decodes to the same ring; the safety
//! offsets follow the sizes. Then the queues, 2 to 4 transfers in flight at the smallest
//! sizes: the same stream, safety offsets of queue x frames.

mod common;

use a16_engine::{Direction, PcmRing, Setting, SettingError, TransferType, pcm_frames, pcm_queue};
use common::{FakeBus, pipe, quiet_engine};

/// Frames compared, out and in.
const FRAMES: usize = 960;
const SUBPACKETS: usize = FRAMES / 10;
/// Room for the last in transfer past FRAMES.
const IN_RING: usize = FRAMES + 200;

#[derive(Default, PartialEq)]
struct Run {
    /// Out sub-packets from the first encoded transfer.
    wire: Vec<u8>,
    in_ring: Vec<u8>,
    safety: (u32, u32),
    rate: u32,
    transfer_frames: (u32, u32),
}

/// The safety offset the engine reports for `queued` frames at `rate`: at least 1 ms.
fn safety(queued: u32, rate: u32) -> u32 {
    queued.max(rate.div_ceil(1000))
}

/// Set `setting` to `value` as a user would, or, if the pipes refuse it, as a value kept
/// from before.
fn set(e: &mut a16_engine::Engine, setting: Setting, value: u8, whole: bool) {
    match e.change_setting(setting, value) {
        Ok(_) => assert!(whole, "{setting} {value}: not whole packets, but taken"),
        Err(SettingError::NotWholePackets { .. }) if !whole => e.set_setting(setting, value),
        Err(error) => panic!("{setting} {value}: {error}"),
    }
}

fn run(out_max_packet: u16, out_value: u8, in_value: u8, queue_value: u8) -> Run {
    let queued = pcm_queue(queue_value);
    let mut bus = FakeBus::new();
    let mut e = quiet_engine();
    let out_type = if out_max_packet <= 512 {
        TransferType::Bulk
    } else {
        TransferType::Interrupt
    };
    e.set_pipe(Direction::Out, pipe(out_type, out_max_packet, 1));
    e.set_pipe(Direction::In, pipe(TransferType::Bulk, 512, 0));
    let out_whole =
        pcm_frames(Direction::Out, out_value).is_multiple_of(e.packet_frames(Direction::Out));
    set(&mut e, Setting::PcmOutFrames, out_value, out_whole);
    set(&mut e, Setting::PcmInFrames, in_value, true);
    set(&mut e, Setting::PcmOutQueue, queue_value, true);
    set(&mut e, Setting::PcmInQueue, queue_value, true);
    let out_ring: Vec<u8> = (0..FRAMES * 48).map(|k| (k * 13 + 5) as u8).collect();
    e.attach_pcm_ring(PcmRing::from_bytes(Direction::Out, out_ring), 0);
    e.attach_pcm_ring(PcmRing::new(Direction::In, IN_RING), 0);
    e.start(&mut bus).expect("start");

    let mut res = Run {
        safety: (
            e.safety_offset(Direction::Out),
            e.safety_offset(Direction::In),
        ),
        rate: e.sample_rate().expect("a rate").hz(),
        transfer_frames: (
            e.transfer_frames(Direction::Out),
            e.transfer_frames(Direction::In),
        ),
        ..Run::default()
    };
    // What runs: a size that isn't whole packets is rounded up
    let (out_frames, in_frames) = res.transfer_frames;
    let subpacket = if out_max_packet <= 512 { 512 } else { 482 };
    let out_bytes = out_frames as usize / 10 * subpacket;
    let in_bytes = in_frames as usize * 64;
    let mut skip = (queued * out_frames / 10) as usize;
    let mut out_done = 0;
    let mut counter = 0u32;
    let in_full =
        |e: &a16_engine::Engine| e.pcm_position(Direction::In).unwrap_or(0) >= FRAMES as u64;

    // Out until the queued transfers are past and the wire has FRAMES frames; in until the
    // ring is full once
    while let Some(transfer) = bus.pop() {
        match transfer.direction() {
            Direction::Out => {
                assert_eq!(
                    transfer.len(),
                    out_bytes,
                    "out transfer for {out_frames} frames"
                );
                // Encoded at submission: its bytes are final now
                for sub in transfer.buffer().chunks_exact(subpacket) {
                    if skip > 0 {
                        skip -= 1;
                    } else if res.wire.len() < SUBPACKETS * subpacket {
                        res.wire.extend_from_slice(sub);
                    }
                }
                out_done += out_frames;
                if out_done > queued * out_frames
                    && res.wire.len() >= SUBPACKETS * subpacket
                    && in_full(&e)
                {
                    break;
                }
            }
            Direction::In => {
                assert_eq!(
                    transfer.len(),
                    in_bytes,
                    "in transfer for {in_frames} frames"
                );
                if in_full(&e) {
                    continue; // the ring is full: leave it as it is
                }
            }
        }
        bus.complete(&mut e, transfer, |t| {
            for b in t.buffer_mut() {
                *b = counter.wrapping_mul(37).wrapping_add(11) as u8;
                counter += 1;
            }
        });
    }
    e.stop(&mut bus);
    let ring = e.detach_pcm_ring(Direction::In).expect("in ring");
    res.in_ring = ring.as_bytes()[..FRAMES * 6].to_vec();
    res
}

fn compare(name: &str, out_max_packet: u16) {
    let out_type = if out_max_packet <= 512 {
        TransferType::Bulk
    } else {
        TransferType::Interrupt
    };
    let out_step = pipe(out_type, out_max_packet, 1).packet_frames(48);
    let small = (out_step / 10 - 1) as u8;
    let small_frames = pcm_frames(Direction::Out, small);

    // The default (80 / 80), then every out size, with the in sizes alongside
    let reference = run(
        out_max_packet,
        Setting::PcmOutFrames.default_value(),
        Setting::PcmInFrames.default_value(),
        3,
    );
    assert_eq!(
        reference.safety,
        (4 * 80, 4 * 80),
        "{name}: default safety offsets"
    );
    let subpacket = if out_max_packet <= 512 { 512 } else { 482 };
    assert_eq!(
        reference.wire.len(),
        SUBPACKETS * subpacket,
        "{name}: the wire compared"
    );
    assert!(
        reference.wire.iter().any(|&b| b != 0),
        "{name}: a silent wire"
    );
    assert!(
        reference.in_ring.iter().any(|&b| b != 0),
        "{name}: silent input"
    );
    for v in 0..Setting::PcmInFrames.value_count() {
        let ov = v.min(Setting::PcmOutFrames.value_count() - 1);
        let out_frames = pcm_frames(Direction::Out, ov);
        let in_frames = pcm_frames(Direction::In, v);
        let got = run(out_max_packet, ov, v, 3);
        if !out_frames.is_multiple_of(out_step) {
            // Not whole packets: the engine takes the next size up
            assert_eq!(
                got.transfer_frames.0,
                (out_frames / out_step + 1) * out_step,
                "{name}: {out_frames} out frames"
            );
            continue;
        }
        assert_eq!(
            got.transfer_frames,
            (out_frames, in_frames),
            "{name}: transfer frames"
        );
        assert_eq!(
            got.safety,
            (
                safety(4 * out_frames, got.rate),
                safety(4 * in_frames, got.rate)
            ),
            "{name} {out_frames}/{in_frames}: safety offsets"
        );
        assert!(
            got.wire == reference.wire,
            "{name} {out_frames}/{in_frames}: the wire differs"
        );
        assert!(
            got.in_ring == reference.in_ring,
            "{name} {out_frames}/{in_frames}: input differs"
        );
    }
    // The queues at the smallest sizes: one packet out (10 or 20 frames), 8 in (one in flight
    // isn't offered)
    for v in 1..Setting::PcmOutQueue.value_count() {
        let q = pcm_queue(v);
        let got = run(out_max_packet, small, 0, v);
        assert_eq!(
            got.safety,
            (safety(q * small_frames, got.rate), safety(q * 8, got.rate)),
            "{name} {small_frames}/8, {q} queued: safety offsets"
        );
        assert!(
            got.wire == reference.wire,
            "{name} {small_frames}/8, {q} queued: the wire differs"
        );
        assert!(
            got.in_ring == reference.in_ring,
            "{name} {small_frames}/8, {q} queued: input differs"
        );
    }
}

#[test]
fn interrupt_layout() {
    compare("Realiser A16 (964 interrupt)", 964);
}

#[test]
fn bulk_layout() {
    compare("Realiser A16 (512 bulk)", 512);
}
