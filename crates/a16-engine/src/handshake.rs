// SPDX-License-Identifier: MIT
//! The control-request sequence that arms the A16 for streaming at a sample rate.

use std::fmt;
use std::time::Duration;

use crate::bus::{Bus, BusError, ControlSetup};
use crate::protocol::{
    FirmwareVersion, RATE_CONTROL, REGISTER_INPUT_SELECTOR, REQTYPE_GET_RATE, REQTYPE_SET_RATE,
    REQTYPE_VENDOR_IN, REQTYPE_VENDOR_OUT, REQUEST_FIRMWARE, REQUEST_GET_RATE, REQUEST_SET_RATE,
    REQUEST_STATUS, SELECTOR_AUTOMATIC_CLOCK, SELECTOR_DIGITAL_LOCK, confirm_wvalue, decode_rate,
    encode_rate, register_wvalue,
};
use crate::{Direction, SampleRate};

/// What the device reported during the handshake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HandshakeInfo {
    pub firmware: FirmwareVersion,
    /// The rate the device ran at before the handshake set it, in Hz.
    pub boot_rate: u32,
    /// The rate the device was set to.
    pub rate: SampleRate,
    /// The input selector byte as read before the status confirm.
    pub input_selector: u8,
}

/// Why the handshake failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandshakeError {
    /// A control request failed on every attempt.
    Control {
        setup: ControlSetup,
        error: BusError,
    },
    /// The device reads back another rate than it was set to.
    RateMismatch { requested: SampleRate, actual: u32 },
    /// Clearing a PCM endpoint's halt failed.
    ClearHalt { endpoint: u8, error: BusError },
}

impl fmt::Display for HandshakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HandshakeError::Control { setup, error } => write!(
                f,
                "request 0x{:02x} (type 0x{:02x}) failed: {error}",
                setup.request, setup.request_type
            ),
            HandshakeError::RateMismatch { requested, actual } => {
                write!(f, "device runs at {actual} Hz instead of {requested}")
            }
            HandshakeError::ClearHalt { endpoint, error } => {
                write!(f, "clear halt on endpoint 0x{endpoint:02x} failed: {error}")
            }
        }
    }
}

impl std::error::Error for HandshakeError {}

/// Attempts per control request, 20 ms apart.
///
/// Right after a restart stopped the streaming transfers, the Xone:DB4 (firmware 1.4.1)
/// stalls whichever request comes in the next few ms and answers again about 100 ms later.
/// [`RESTART_QUIET`] waits that out. Should a request still stall, a stall on endpoint 0 ends
/// with the next setup packet, and every handshake request is a plain read or write, so the
/// engine asks again.
const CONTROL_ATTEMPTS: u32 = 10;
const CONTROL_RETRY: Duration = Duration::from_millis(20);

/// After the streaming transfers stop, no request for this long: the official Windows
/// driver's ~100 ms of silence before it talks to the device again. The DB4 stalls whatever
/// comes sooner and answers again at ~115 ms.
pub(crate) const RESTART_QUIET: Duration = Duration::from_millis(120);

struct Handshake<'a, B: ?Sized> {
    bus: &'a mut B,
    log: &'a mut dyn FnMut(&str),
}

impl<B: Bus + ?Sized> Handshake<'_, B> {
    fn log(&mut self, args: fmt::Arguments<'_>) {
        (self.log)(&args.to_string());
    }

    fn retry(
        &mut self,
        setup: ControlSetup,
        mut request: impl FnMut(&mut B) -> Result<(), BusError>,
    ) -> Result<(), HandshakeError> {
        let mut last = BusError(0);
        for attempt in 1..=CONTROL_ATTEMPTS {
            match request(self.bus) {
                Ok(()) => {
                    if attempt > 1 {
                        self.log(format_args!(
                            "request 0x{:02x} answered on attempt {attempt}",
                            setup.request
                        ));
                    }
                    return Ok(());
                }
                Err(error) => last = error,
            }
            self.bus.sleep(CONTROL_RETRY);
        }
        Err(HandshakeError::Control { setup, error: last })
    }

    fn read(&mut self, setup: ControlSetup, data: &mut [u8]) -> Result<(), HandshakeError> {
        self.retry(setup, |bus| {
            data.fill(0);
            bus.control_in(setup, data)
        })
    }

    fn write(&mut self, setup: ControlSetup, data: &[u8]) -> Result<(), HandshakeError> {
        self.retry(setup, |bus| bus.control_out(setup, data))
    }

    fn read_selector_byte(&mut self) -> Result<u8, HandshakeError> {
        let mut byte = [0];
        self.read(status_setup(REQTYPE_VENDOR_IN, 0), &mut byte)?;
        Ok(byte[0])
    }

    fn write_selector_byte(&mut self, value: u16) -> Result<(), HandshakeError> {
        self.write(status_setup(REQTYPE_VENDOR_OUT, value), &[])
    }

    /// 'V': the firmware version.
    fn read_firmware(&mut self) -> Result<FirmwareVersion, HandshakeError> {
        let mut answer = [0; 15];
        let setup = ControlSetup {
            request_type: REQTYPE_VENDOR_IN,
            request: REQUEST_FIRMWARE,
            value: 0,
            index: 0,
        };
        self.read(setup, &mut answer)?;
        let firmware = FirmwareVersion::parse([answer[0], answer[1], answer[2]]);
        self.log(format_args!(
            "firmware: v{firmware} (chip ID: 0x{:02X})",
            firmware.chip_id
        ));
        Ok(firmware)
    }

    /// 'I', the input selector, logged bit by bit.
    fn read_status(&mut self) -> Result<u8, HandshakeError> {
        let raw = self.read_selector_byte()?;
        let bit = |n: u8| (raw >> n) & 1;
        self.log(format_args!(
            "status: 0x{raw:02X} [InputSel:{}] [DigLock:{}] [mode: {}{}{}{}{}{}]",
            bit(1),
            bit(2),
            bit(7),
            bit(6),
            bit(5),
            bit(4),
            bit(3),
            bit(0)
        ));
        Ok(raw)
    }

    /// UAC1 endpoint GET_CUR: the current sample rate.
    fn read_rate(&mut self) -> Result<u32, HandshakeError> {
        let mut answer = [0; 3];
        let setup = ControlSetup {
            request_type: REQTYPE_GET_RATE,
            request: REQUEST_GET_RATE,
            value: RATE_CONTROL,
            index: 0,
        };
        self.read(setup, &mut answer)?;
        let rate = decode_rate(answer);
        self.log(format_args!(
            "hardware sample rate: {rate} Hz (raw: {:02X} {:02X} {:02X})",
            answer[0], answer[1], answer[2]
        ));
        Ok(rate)
    }

    /// UAC1 endpoint SET_CUR to the PCM in, then the PCM out endpoint, the pipes the
    /// official driver streams on.
    fn set_rate(&mut self, rate: SampleRate) -> Result<(), HandshakeError> {
        for direction in [Direction::In, Direction::Out] {
            let setup = ControlSetup {
                request_type: REQTYPE_SET_RATE,
                request: REQUEST_SET_RATE,
                value: RATE_CONTROL,
                index: u16::from(direction.endpoint()),
            };
            self.write(setup, &encode_rate(rate.hz()))?;
        }
        Ok(())
    }

    /// The official configurationDone's updateAjInputSelector with the clock source
    /// Automatic: the selector byte with bit 1 set, written back only if it changed, then
    /// 40 ms for the device to re-clock and a rate read.
    fn select_automatic_clock(&mut self) -> Result<(), HandshakeError> {
        self.bus.sleep(Duration::from_millis(5));
        let mut read = self.read_selector_byte()?;
        // Read again after 20 ms while the digital lock bit is set, as both selector
        // routines of the official driver do
        if read & SELECTOR_DIGITAL_LOCK != 0 {
            self.bus.sleep(Duration::from_millis(20));
            read = self.read_selector_byte()?;
        }
        let want = read | SELECTOR_AUTOMATIC_CLOCK;
        if want == read {
            return Ok(());
        }
        self.write_selector_byte(register_wvalue(want))?;
        self.bus.sleep(Duration::from_millis(40));
        self.log(format_args!(
            "selector: read 0x{read:02X}, wrote 0x{want:02X}"
        ));
        self.read_rate().map(|_| ())
    }

    /// Read-modify-write of the input selector, as the official setAJDMAInputChannels does at
    /// every stream start, which arms the device for streaming.
    fn confirm_status(&mut self) -> Result<(), HandshakeError> {
        let read = self.read_selector_byte()?;
        let wvalue = confirm_wvalue(read);
        self.write_selector_byte(wvalue)?;
        self.log(format_args!(
            "status confirmed (read=0x{read:02X}, wrote wValue=0x{wvalue:04X})"
        ));
        Ok(())
    }

    /// Reset both PCM endpoints before streaming starts, as the official Windows driver does
    /// after the status confirm: PCM in, then PCM out.
    fn clear_pcm_halts(&mut self) -> Result<(), HandshakeError> {
        self.bus.sleep(Duration::from_millis(500));
        for direction in [Direction::In, Direction::Out] {
            let endpoint = direction.endpoint();
            if let Err(error) = self.bus.clear_halt(endpoint) {
                self.log(format_args!(
                    "clear halt on PCM {direction} failed: {error}"
                ));
                return Err(HandshakeError::ClearHalt { endpoint, error });
            }
        }
        Ok(())
    }

    fn run(&mut self, rate: SampleRate) -> Result<HandshakeInfo, HandshakeError> {
        self.log(format_args!("[1/7] reading firmware version..."));
        let firmware = self.read_firmware()?;
        self.log(format_args!("[2/7] reading hardware status..."));
        self.read_status()?;
        self.log(format_args!("[3/7] reading current sample rate..."));
        let boot_rate = self.read_rate()?;
        self.select_automatic_clock()?;
        self.log(format_args!("[4/7] setting sample rate to {rate}..."));
        self.set_rate(rate)?;
        // The official driver reads the rate back and waits 50 ms
        self.log(format_args!("[5/7] verifying sample rate..."));
        let actual = self.read_rate()?;
        if actual != rate.hz() {
            return Err(HandshakeError::RateMismatch {
                requested: rate,
                actual,
            });
        }
        self.bus.sleep(Duration::from_millis(50));
        self.log(format_args!("[6/7] reading hardware status..."));
        let input_selector = self.read_status()?;
        self.log(format_args!("[7/7] confirming device status..."));
        self.confirm_status()?;
        self.clear_pcm_halts()?;
        Ok(HandshakeInfo {
            firmware,
            boot_rate,
            rate,
            input_selector,
        })
    }
}

fn status_setup(request_type: u8, value: u16) -> ControlSetup {
    ControlSetup {
        request_type,
        request: REQUEST_STATUS,
        value,
        index: REGISTER_INPUT_SELECTOR,
    }
}

/// Arm the device for streaming at `rate`.
pub(crate) fn handshake<B: Bus + ?Sized>(
    bus: &mut B,
    rate: SampleRate,
    log: &mut dyn FnMut(&str),
) -> Result<HandshakeInfo, HandshakeError> {
    let mut hs = Handshake { bus, log };
    hs.log(format_args!("--- begin handshake sequence ---"));
    let result = hs.run(rate);
    match &result {
        Ok(_) => hs.log(format_args!("--- handshake complete, device ready ---")),
        Err(error) => hs.log(format_args!("handshake failed: {error}")),
    }
    result
}
