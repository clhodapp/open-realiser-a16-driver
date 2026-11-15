// SPDX-License-Identifier: MIT
//! The libusb calls the driver makes, behind a trait: [`crate::libusb::LibUsb`] makes them
//! on the A16, and the tests make them on a fake A16 with a fake clock.

use std::fmt;
use std::time::Duration;

use a16_engine::Transfer;

/// A libusb error code (`LIBUSB_ERROR_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsbError(pub i32);

impl UsbError {
    pub const IO: UsbError = UsbError(-1);
    pub const ACCESS: UsbError = UsbError(-3);
    pub const NO_DEVICE: UsbError = UsbError(-4);
    pub const NOT_FOUND: UsbError = UsbError(-5);
    pub const BUSY: UsbError = UsbError(-6);
    pub const TIMEOUT: UsbError = UsbError(-7);
    pub const PIPE: UsbError = UsbError(-9);
    pub const INTERRUPTED: UsbError = UsbError(-10);
    pub const NOT_SUPPORTED: UsbError = UsbError(-12);

    /// libusb's name for it, as `libusb_error_name` gives it.
    pub fn name(self) -> &'static str {
        // SAFETY: libusb_error_name returns a static string for any code
        unsafe {
            std::ffi::CStr::from_ptr(rusb::ffi::libusb_error_name(self.0))
                .to_str()
                .unwrap_or("**UNKNOWN**")
        }
    }
}

impl From<rusb::Error> for UsbError {
    /// The libusb code rusb read the error from.
    fn from(error: rusb::Error) -> UsbError {
        UsbError(match error {
            rusb::Error::Io => -1,
            rusb::Error::InvalidParam => -2,
            rusb::Error::Access => -3,
            rusb::Error::NoDevice => -4,
            rusb::Error::NotFound => -5,
            rusb::Error::Busy => -6,
            rusb::Error::Timeout => -7,
            rusb::Error::Overflow => -8,
            rusb::Error::Pipe => -9,
            rusb::Error::Interrupted => -10,
            rusb::Error::NoMem => -11,
            rusb::Error::NotSupported => -12,
            _ => -99,
        })
    }
}

impl fmt::Display for UsbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// `LIBUSB_TRANSFER_COMPLETED`, the status of a transfer that ended without error.
pub const TRANSFER_COMPLETED: i32 = 0;
/// `LIBUSB_TRANSFER_CANCELLED`.
pub const TRANSFER_CANCELLED: i32 = 3;
/// `LIBUSB_TRANSFER_NO_DEVICE`.
pub const TRANSFER_NO_DEVICE: i32 = 5;

/// The transfer types the bus submits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferKind {
    Bulk,
    Interrupt,
}

/// A transfer libusb handed back.
#[derive(Debug)]
pub struct Completion {
    pub transfer: Transfer,
    /// Its `libusb_transfer_status`.
    pub status: i32,
    /// Bytes moved.
    pub actual_length: usize,
}

/// An endpoint descriptor's fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EndpointDescriptor {
    pub address: u8,
    pub attributes: u8,
    pub max_packet_size: u16,
    pub interval: u8,
}

/// An interface descriptor: one alternate setting of an interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AltSetting {
    pub interface: u8,
    pub alt_setting: u8,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub endpoints: Vec<EndpointDescriptor>,
}

/// A configuration descriptor: its value, and every alternate setting of every interface,
/// interface by interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigDescriptor {
    pub value: u8,
    pub interfaces: u8,
    pub settings: Vec<AltSetting>,
}

/// The libusb context and the one A16 handle the driver holds, on the USB thread.
pub trait Usb {
    /// `CLOCK_MONOTONIC`, in nanoseconds.
    fn now(&self) -> u64;

    fn sleep(&mut self, duration: Duration);

    /// Is exactly one A16 on the bus, at high speed?
    fn find_a16(&mut self) -> bool;

    /// Open the A16 [`Usb::find_a16`] finds.
    fn open_a16(&mut self) -> Result<(), UsbError>;

    /// Close the handle, if one is open.
    fn close(&mut self);

    /// `libusb_reset_device` on the open handle.
    fn reset(&mut self) -> Result<(), UsbError>;

    /// Has a hotplug event reported the open A16 leaving the bus since
    /// [`Usb::clear_gone`]?
    fn gone(&self) -> bool;

    fn clear_gone(&mut self);

    fn active_config(&mut self) -> Result<ConfigDescriptor, UsbError>;

    fn kernel_driver_active(&mut self, interface: u8) -> Result<bool, UsbError>;

    fn detach_kernel_driver(&mut self, interface: u8) -> Result<(), UsbError>;

    fn attach_kernel_driver(&mut self, interface: u8) -> Result<(), UsbError>;

    fn claim_interface(&mut self, interface: u8) -> Result<(), UsbError>;

    fn release_interface(&mut self, interface: u8) -> Result<(), UsbError>;

    fn set_alt_setting(&mut self, interface: u8, alt_setting: u8) -> Result<(), UsbError>;

    /// A control transfer: `data` is read into for a request with the direction bit, and
    /// sent otherwise. The bytes moved.
    fn control(
        &mut self,
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        data: &mut [u8],
        timeout: Duration,
    ) -> Result<usize, UsbError>;

    fn clear_halt(&mut self, endpoint: u8) -> Result<(), UsbError>;

    /// Submit `transfer` on its endpoint; it comes back from [`Usb::handle_events`].
    fn submit(
        &mut self,
        kind: TransferKind,
        transfer: Transfer,
        timeout: Duration,
    ) -> Result<(), (UsbError, Transfer)>;

    /// `libusb_cancel_transfer` on every transfer in flight, newest first: the error of each
    /// that failed.
    fn cancel_all(&mut self) -> Vec<UsbError>;

    /// `libusb_handle_events_timeout`: the transfers that came back are appended to `done`.
    fn handle_events(
        &mut self,
        timeout: Duration,
        done: &mut Vec<Completion>,
    ) -> Result<(), UsbError>;
}
