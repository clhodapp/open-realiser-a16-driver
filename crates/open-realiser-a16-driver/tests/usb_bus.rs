// SPDX-License-Identifier: MIT
//! The libusb bus on scripted libusb calls: cancellation drains every transfer, a missing
//! endpoint, a submit failure and a vanished device fail the bus, a short input transfer
//! passes on and a short output one fails the stream, control requests must be complete
//! ('V' excepted), and after a control timeout no request reaches the device.

use std::collections::VecDeque;
use std::time::Duration;

use a16_engine::{Bus, ControlSetup, Status, Transfer};
use open_realiser_a16_driver::bus::UsbBus;
use open_realiser_a16_driver::usb::{
    Completion, ConfigDescriptor, TRANSFER_CANCELLED, TRANSFER_COMPLETED, TRANSFER_NO_DEVICE,
    TransferKind, Usb, UsbError,
};

/// libusb calls whose results the test sets.
#[derive(Default)]
struct Script {
    queued: VecDeque<(Transfer, Duration)>,
    done: Vec<Completion>,
    fail_submit: bool,
    /// The next completion's status and bytes, instead of a full one.
    next: Option<(i32, usize)>,
    control_result: Option<Result<usize, UsbError>>,
    short_control: bool,
    control_calls: u32,
    last_timeout: Duration,
}

impl Usb for Script {
    fn now(&self) -> u64 {
        0
    }
    fn sleep(&mut self, _duration: Duration) {}
    fn find_a16(&mut self) -> bool {
        true
    }
    fn open_a16(&mut self) -> Result<(), UsbError> {
        Ok(())
    }
    fn close(&mut self) {}
    fn reset(&mut self) -> Result<(), UsbError> {
        Ok(())
    }
    fn gone(&self) -> bool {
        false
    }
    fn clear_gone(&mut self) {}
    fn active_config(&mut self) -> Result<ConfigDescriptor, UsbError> {
        Err(UsbError::IO)
    }
    fn kernel_driver_active(&mut self, _interface: u8) -> Result<bool, UsbError> {
        Ok(false)
    }
    fn detach_kernel_driver(&mut self, _interface: u8) -> Result<(), UsbError> {
        Ok(())
    }
    fn attach_kernel_driver(&mut self, _interface: u8) -> Result<(), UsbError> {
        Ok(())
    }
    fn claim_interface(&mut self, _interface: u8) -> Result<(), UsbError> {
        Ok(())
    }
    fn release_interface(&mut self, _interface: u8) -> Result<(), UsbError> {
        Ok(())
    }
    fn set_alt_setting(&mut self, _interface: u8, _alt: u8) -> Result<(), UsbError> {
        Ok(())
    }
    fn control(
        &mut self,
        _request_type: u8,
        _request: u8,
        _value: u16,
        _index: u16,
        data: &mut [u8],
        _timeout: Duration,
    ) -> Result<usize, UsbError> {
        self.control_calls += 1;
        data.fill(0xaa);
        self.control_result
            .unwrap_or(Ok(if self.short_control && !data.is_empty() {
                data.len() - 1
            } else {
                data.len()
            }))
    }
    fn clear_halt(&mut self, _endpoint: u8) -> Result<(), UsbError> {
        Ok(())
    }
    fn submit(
        &mut self,
        _kind: TransferKind,
        transfer: Transfer,
        timeout: Duration,
    ) -> Result<(), (UsbError, Transfer)> {
        if self.fail_submit {
            return Err((UsbError::NO_DEVICE, transfer));
        }
        self.last_timeout = timeout;
        self.queued.push_back((transfer, timeout));
        Ok(())
    }
    fn cancel_all(&mut self) -> Vec<UsbError> {
        for (transfer, _) in self.queued.drain(..) {
            self.done.push(Completion {
                transfer,
                status: TRANSFER_CANCELLED,
                actual_length: 0,
            });
        }
        Vec::new()
    }
    fn handle_events(
        &mut self,
        _timeout: Duration,
        done: &mut Vec<Completion>,
    ) -> Result<(), UsbError> {
        done.append(&mut self.done);
        if let Some((transfer, _)) = self.queued.pop_back() {
            let (status, actual_length) = self
                .next
                .take()
                .unwrap_or((TRANSFER_COMPLETED, transfer.len()));
            done.push(Completion {
                transfer,
                status,
                actual_length,
            });
        }
        Ok(())
    }
}

fn bus() -> UsbBus<Script> {
    UsbBus::new(
        Script::default(),
        None,
        Box::new(|line| eprintln!("{line}")),
    )
}

/// A transfer the engine would make for `endpoint`, through a start on a bus that keeps
/// what it is given.
fn transfer(endpoint: u8) -> Transfer {
    use a16_engine::{Direction, Engine, PcmRing};
    struct Keep(Vec<Transfer>);
    impl Bus for Keep {
        fn submit(&mut self, transfer: Transfer) -> Result<(), Transfer> {
            self.0.push(transfer);
            Ok(())
        }
        fn control_in(
            &mut self,
            s: ControlSetup,
            data: &mut [u8],
        ) -> Result<(), a16_engine::BusError> {
            if (s.request_type, s.request) == (0xa2, 0x81) {
                data.copy_from_slice(&[0x80, 0xbb, 0]);
            }
            Ok(())
        }
        fn control_out(
            &mut self,
            _s: ControlSetup,
            _data: &[u8],
        ) -> Result<(), a16_engine::BusError> {
            Ok(())
        }
        fn clear_halt(&mut self, _endpoint: u8) -> Result<(), a16_engine::BusError> {
            Ok(())
        }
        fn sleep(&mut self, _duration: Duration) {}
        fn abort(&mut self) -> Vec<Transfer> {
            std::mem::take(&mut self.0)
        }
    }
    let mut engine = Engine::new();
    engine.set_logger(Box::new(|_| {}));
    engine.attach_pcm_ring(PcmRing::new(Direction::Out, 80), 0);
    let mut keep = Keep(Vec::new());
    engine.start(&mut keep).expect("start");
    let i = keep
        .0
        .iter()
        .position(|t| t.endpoint() == endpoint)
        .expect("endpoint");
    keep.0.swap_remove(i)
}

fn next(bus: &mut UsbBus<Script>) -> Status {
    let mut done = Vec::new();
    bus.usb
        .handle_events(Duration::ZERO, &mut done)
        .expect("events");
    assert_eq!(done.len(), 1);
    bus.finish(&done[0])
}

#[test]
fn transfers() {
    // Cancellation drains every transfer, without failing the bus
    let mut b = bus();
    b.set_endpoint_type(0x86, 2);
    for _ in 0..8 {
        assert!(b.submit(transfer(0x86)).is_ok());
    }
    assert_eq!(b.pending(), 8);
    assert_eq!(b.abort().len(), 8);
    assert!(b.pending() == 0 && b.usb.queued.is_empty() && !b.failed);

    // An endpoint the interface lacks fails the bus
    let mut b = bus();
    assert!(b.submit(transfer(0x86)).is_err());
    assert!(b.failed && b.pending() == 0);

    // A short input transfer reaches the engine with its byte count
    let mut b = bus();
    b.set_endpoint_type(0x86, 3);
    assert!(b.submit(transfer(0x86)).is_ok());
    assert_eq!(b.usb.last_timeout, Duration::from_millis(2000));
    let length = b.usb.queued[0].0.len();
    b.usb.next = Some((TRANSFER_COMPLETED, length - 1));
    assert_eq!(next(&mut b), Status::Completed(length - 1));
    assert!(!b.failed && b.pending() == 0 && b.short_in == 1);

    // A short output transfer fails the stream
    let mut b = bus();
    b.set_endpoint_type(0x05, 3);
    assert!(b.submit(transfer(0x05)).is_ok());
    let length = b.usb.queued[0].0.len();
    b.usb.next = Some((TRANSFER_COMPLETED, length - 1));
    assert_eq!(next(&mut b), Status::Aborted);
    assert!(b.failed && b.pending() == 0);

    // A transfer of a vanished device fails the bus
    let mut b = bus();
    b.set_endpoint_type(0x86, 2);
    assert!(b.submit(transfer(0x86)).is_ok());
    b.usb.next = Some((TRANSFER_NO_DEVICE, 0));
    assert_eq!(next(&mut b), Status::Aborted);
    assert!(b.failed && b.pending() == 0);

    // A submit libusb refuses fails the bus
    let mut b = bus();
    b.set_endpoint_type(0x86, 2);
    b.usb.fail_submit = true;
    assert!(b.submit(transfer(0x86)).is_err());
    assert!(b.failed && b.pending() == 0);
}

#[test]
fn control_requests() {
    let setup = |request_type, request, value, index| ControlSetup {
        request_type,
        request,
        value,
        index,
    };
    let mut b = bus();
    let mut reply = [0u8; 3];
    b.usb.short_control = true;
    assert!(
        b.control_in(setup(0xa2, 0x81, 0x100, 0x86), &mut reply)
            .is_err()
    );
    b.usb.short_control = false;
    assert!(
        b.control_in(setup(0xa2, 0x81, 0x100, 0x86), &mut reply)
            .is_ok()
    );
    let mut firmware = [0u8; 15];
    b.usb.control_result = Some(Ok(3));
    assert!(b.control_in(setup(0xc0, 0x56, 0, 0), &mut firmware).is_ok());
    b.usb.control_result = Some(Ok(2));
    assert!(
        b.control_in(setup(0xc0, 0x56, 0, 0), &mut firmware)
            .is_err()
    );
    assert!(!b.failed);

    // A timeout fails the bus, and later requests fail without reaching the device
    b.usb.control_result = Some(Err(UsbError::TIMEOUT));
    assert!(
        b.control_in(setup(0xa2, 0x81, 0x100, 0), &mut reply)
            .is_err()
    );
    assert!(b.failed);
    b.usb.control_result = None;
    let before = b.usb.control_calls;
    assert!(
        b.control_in(setup(0xa2, 0x81, 0x100, 0), &mut reply)
            .is_err()
    );
    assert_eq!(b.usb.control_calls, before);

    // So does a device that is gone
    let mut b = bus();
    b.usb.control_result = Some(Err(UsbError::NO_DEVICE));
    assert!(b.control_out(setup(0x40, 0x49, 0x16, 0), &[]).is_err());
    assert!(b.failed);
}
