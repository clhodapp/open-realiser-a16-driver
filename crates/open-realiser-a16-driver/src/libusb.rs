// SPDX-License-Identifier: MIT
//! [`Usb`] on libusb: rusb for the context, the device list and hotplug events, and libusb's
//! own calls for the handle, so that every error keeps its libusb code.

use std::ffi::{c_int, c_void};
use std::ptr::{self, NonNull};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::time::Duration;

use a16_engine::Transfer;
use rusb::UsbContext as _;
use rusb::ffi;

use crate::usb::{
    AltSetting, Completion, ConfigDescriptor, EndpointDescriptor, TransferKind, Usb, UsbError,
};

/// `CLOCK_MONOTONIC`, in nanoseconds.
pub fn monotonic_ns() -> u64 {
    let mut t = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock_gettime writes the timespec it is given
    unsafe {
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut t);
    }
    t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
}

fn check(ret: c_int) -> Result<(), UsbError> {
    if ret == 0 { Ok(()) } else { Err(UsbError(ret)) }
}

/// The descriptor of configuration `index` of `device`, or of its active configuration.
///
/// # Safety
///
/// `device` must be a live libusb device.
pub unsafe fn config_descriptor(
    device: *mut ffi::libusb_device,
    index: Option<u8>,
) -> Result<ConfigDescriptor, UsbError> {
    let mut config: *const ffi::libusb_config_descriptor = ptr::null();
    // SAFETY: the caller's device; libusb allocates the descriptor
    let ret = unsafe {
        match index {
            Some(i) => ffi::libusb_get_config_descriptor(device, i, &mut config),
            None => ffi::libusb_get_active_config_descriptor(device, &mut config),
        }
    };
    check(ret)?;
    // SAFETY: libusb filled in a descriptor with the counts it gives, freed below
    unsafe {
        let c = &*config;
        let mut settings = Vec::new();
        for j in 0..usize::from(c.bNumInterfaces) {
            let interface = &*c.interface.add(j);
            for k in 0..interface.num_altsetting.max(0) as usize {
                let d = &*interface.altsetting.add(k);
                let endpoints = (0..usize::from(d.bNumEndpoints))
                    .map(|e| {
                        let ep = &*d.endpoint.add(e);
                        EndpointDescriptor {
                            address: ep.bEndpointAddress,
                            attributes: ep.bmAttributes,
                            max_packet_size: ep.wMaxPacketSize,
                            interval: ep.bInterval,
                        }
                    })
                    .collect();
                settings.push(AltSetting {
                    interface: d.bInterfaceNumber,
                    alt_setting: d.bAlternateSetting,
                    class: d.bInterfaceClass,
                    subclass: d.bInterfaceSubClass,
                    protocol: d.bInterfaceProtocol,
                    endpoints,
                });
            }
        }
        let descriptor = ConfigDescriptor {
            value: c.bConfigurationValue,
            interfaces: c.bNumInterfaces,
            settings,
        };
        ffi::libusb_free_config_descriptor(config);
        Ok(descriptor)
    }
}

/// What a transfer's callback finds through its `user_data`.
struct Pending {
    transfer: Transfer,
    done: *mut Vec<*mut ffi::libusb_transfer>,
}

extern "system" fn transfer_done(t: *mut ffi::libusb_transfer) {
    // SAFETY: libusb calls this inside libusb_handle_events on the USB thread, with the
    // transfer whose user_data is the Pending submit made
    unsafe {
        let pending = (*t).user_data.cast::<Pending>();
        (*(*pending).done).push(t);
    }
}

/// Tells [`LibUsb`] when the open A16 leaves the bus.
struct Left {
    device: Arc<AtomicPtr<ffi::libusb_device>>,
    gone: Arc<AtomicBool>,
}

impl rusb::Hotplug<rusb::Context> for Left {
    fn device_arrived(&mut self, _device: rusb::Device<rusb::Context>) {}

    fn device_left(&mut self, device: rusb::Device<rusb::Context>) {
        let open = self.device.load(Ordering::Relaxed);
        if !open.is_null() && device.as_raw() == open {
            self.gone.store(true, Ordering::Relaxed);
        }
    }
}

/// A libusb context and at most one open A16.
pub struct LibUsb {
    hotplug: Option<rusb::Registration<rusb::Context>>,
    handle: Option<NonNull<ffi::libusb_device_handle>>,
    device: Arc<AtomicPtr<ffi::libusb_device>>,
    gone: Arc<AtomicBool>,
    in_flight: Vec<*mut ffi::libusb_transfer>,
    /// Boxed: the callbacks reach it through a pointer that must not move with the struct.
    #[allow(clippy::box_collection)]
    done: Box<Vec<*mut ffi::libusb_transfer>>,
    context: rusb::Context,
}

impl LibUsb {
    /// `libusb_init`.
    pub fn new() -> Result<LibUsb, rusb::Error> {
        Ok(LibUsb {
            hotplug: None,
            handle: None,
            device: Arc::new(AtomicPtr::new(ptr::null_mut())),
            gone: Arc::new(AtomicBool::new(false)),
            in_flight: Vec::new(),
            done: Box::new(Vec::new()),
            context: rusb::Context::new()?,
        })
    }

    pub fn context(&self) -> &rusb::Context {
        &self.context
    }

    /// Report the open A16 leaving the bus through [`Usb::gone`]; false when libusb has no
    /// hotplug events here.
    pub fn register_hotplug(&mut self) -> bool {
        if !rusb::has_hotplug() {
            return false;
        }
        let left = Left {
            device: Arc::clone(&self.device),
            gone: Arc::clone(&self.gone),
        };
        match rusb::HotplugBuilder::new()
            .vendor_id(a16_engine::VENDOR_ID)
            .product_id(a16_engine::PRODUCT_ID)
            .register(self.context.clone(), Box::new(left))
        {
            Ok(registration) => {
                self.hotplug = Some(registration);
                true
            }
            Err(_) => false,
        }
    }

    /// The A16, when exactly one is on the bus at high speed.
    fn find(&self) -> Option<rusb::Device<rusb::Context>> {
        let devices = self.context.devices().ok()?;
        let mut chosen = None;
        let mut matches = 0;
        for device in devices.iter() {
            if let Ok(d) = device.device_descriptor()
                && d.vendor_id() == a16_engine::VENDOR_ID
                && d.product_id() == a16_engine::PRODUCT_ID
            {
                chosen = Some(device);
                matches += 1;
            }
        }
        chosen.filter(|device| {
            // SAFETY: the device is referenced
            matches == 1
                && unsafe { ffi::libusb_get_device_speed(device.as_raw()) }
                    == ffi::constants::LIBUSB_SPEED_HIGH
        })
    }

    /// Open `device`.
    pub fn open_device(&mut self, device: &rusb::Device<rusb::Context>) -> Result<(), UsbError> {
        let mut handle = ptr::null_mut();
        // SAFETY: the device is referenced; libusb writes the handle
        check(unsafe { ffi::libusb_open(device.as_raw(), &mut handle) })?;
        self.handle = NonNull::new(handle);
        self.device.store(device.as_raw(), Ordering::Relaxed);
        Ok(())
    }

    fn raw(&self) -> *mut ffi::libusb_device_handle {
        self.handle.map_or(ptr::null_mut(), NonNull::as_ptr)
    }
}

impl Drop for LibUsb {
    fn drop(&mut self) {
        self.hotplug = None;
        self.close();
    }
}

impl Usb for LibUsb {
    fn now(&self) -> u64 {
        monotonic_ns()
    }

    fn sleep(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }

    fn find_a16(&mut self) -> bool {
        self.find().is_some()
    }

    fn open_a16(&mut self) -> Result<(), UsbError> {
        match self.find() {
            Some(device) => self.open_device(&device),
            None => Err(UsbError::NOT_FOUND),
        }
    }

    fn close(&mut self) {
        if let Some(handle) = self.handle.take() {
            self.device.store(ptr::null_mut(), Ordering::Relaxed);
            // SAFETY: the handle is open, and closed once
            unsafe { ffi::libusb_close(handle.as_ptr()) };
        }
    }

    fn reset(&mut self) -> Result<(), UsbError> {
        // SAFETY: an open handle
        check(unsafe { ffi::libusb_reset_device(self.raw()) })
    }

    fn gone(&self) -> bool {
        self.gone.load(Ordering::Relaxed)
    }

    fn clear_gone(&mut self) {
        self.gone.store(false, Ordering::Relaxed);
    }

    fn active_config(&mut self) -> Result<ConfigDescriptor, UsbError> {
        // SAFETY: the device of an open handle
        unsafe { config_descriptor(ffi::libusb_get_device(self.raw()), None) }
    }

    fn kernel_driver_active(&mut self, interface: u8) -> Result<bool, UsbError> {
        // SAFETY: an open handle
        match unsafe { ffi::libusb_kernel_driver_active(self.raw(), c_int::from(interface)) } {
            0 => Ok(false),
            1 => Ok(true),
            error => Err(UsbError(error)),
        }
    }

    fn detach_kernel_driver(&mut self, interface: u8) -> Result<(), UsbError> {
        // SAFETY: an open handle
        check(unsafe { ffi::libusb_detach_kernel_driver(self.raw(), c_int::from(interface)) })
    }

    fn attach_kernel_driver(&mut self, interface: u8) -> Result<(), UsbError> {
        // SAFETY: an open handle
        check(unsafe { ffi::libusb_attach_kernel_driver(self.raw(), c_int::from(interface)) })
    }

    fn claim_interface(&mut self, interface: u8) -> Result<(), UsbError> {
        // SAFETY: an open handle
        check(unsafe { ffi::libusb_claim_interface(self.raw(), c_int::from(interface)) })
    }

    fn release_interface(&mut self, interface: u8) -> Result<(), UsbError> {
        // SAFETY: an open handle
        check(unsafe { ffi::libusb_release_interface(self.raw(), c_int::from(interface)) })
    }

    fn set_alt_setting(&mut self, interface: u8, alt_setting: u8) -> Result<(), UsbError> {
        // SAFETY: an open handle
        check(unsafe {
            ffi::libusb_set_interface_alt_setting(
                self.raw(),
                c_int::from(interface),
                c_int::from(alt_setting),
            )
        })
    }

    fn control(
        &mut self,
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        data: &mut [u8],
        timeout: Duration,
    ) -> Result<usize, UsbError> {
        // SAFETY: an open handle; libusb reads or writes at most `data.len()` bytes
        let ret = unsafe {
            ffi::libusb_control_transfer(
                self.raw(),
                request_type,
                request,
                value,
                index,
                data.as_mut_ptr(),
                data.len() as u16,
                timeout.as_millis() as u32,
            )
        };
        if ret < 0 {
            Err(UsbError(ret))
        } else {
            Ok(ret as usize)
        }
    }

    fn clear_halt(&mut self, endpoint: u8) -> Result<(), UsbError> {
        // SAFETY: an open handle
        check(unsafe { ffi::libusb_clear_halt(self.raw(), endpoint) })
    }

    fn submit(
        &mut self,
        kind: TransferKind,
        transfer: Transfer,
        timeout: Duration,
    ) -> Result<(), (UsbError, Transfer)> {
        // SAFETY: libusb allocates the transfer
        let t = unsafe { ffi::libusb_alloc_transfer(0) };
        if t.is_null() {
            return Err((UsbError(-11), transfer));
        }
        let pending = Box::into_raw(Box::new(Pending {
            transfer,
            done: &mut *self.done,
        }));
        // SAFETY: libusb holds the transfer until it completes. Its buffer is the heap buffer
        // of the Transfer in `pending`, which stays put until the completion takes it back
        unsafe {
            let buffer = (*pending).transfer.buffer_mut();
            (*t).dev_handle = self.raw();
            (*t).endpoint = (*pending).transfer.endpoint();
            (*t).transfer_type = match kind {
                TransferKind::Bulk => ffi::constants::LIBUSB_TRANSFER_TYPE_BULK,
                TransferKind::Interrupt => ffi::constants::LIBUSB_TRANSFER_TYPE_INTERRUPT,
            };
            (*t).timeout = timeout.as_millis() as u32;
            (*t).buffer = buffer.as_mut_ptr();
            (*t).length = buffer.len() as c_int;
            (*t).user_data = pending.cast::<c_void>();
            (*t).callback = transfer_done;
            let ret = ffi::libusb_submit_transfer(t);
            if ret != 0 {
                ffi::libusb_free_transfer(t);
                return Err((UsbError(ret), Box::from_raw(pending).transfer));
            }
        }
        self.in_flight.push(t);
        Ok(())
    }

    fn cancel_all(&mut self) -> Vec<UsbError> {
        let mut errors = Vec::new();
        for &t in self.in_flight.iter().rev() {
            // SAFETY: a transfer in flight
            let ret = unsafe { ffi::libusb_cancel_transfer(t) };
            if ret != 0 {
                errors.push(UsbError(ret));
            }
        }
        errors
    }

    fn handle_events(
        &mut self,
        timeout: Duration,
        done: &mut Vec<Completion>,
    ) -> Result<(), UsbError> {
        let tv = libc::timeval {
            tv_sec: timeout.as_secs() as libc::time_t,
            tv_usec: timeout.subsec_micros() as libc::suseconds_t,
        };
        // SAFETY: the context is live; the callbacks run on this thread
        let ret = unsafe {
            ffi::libusb_handle_events_timeout(self.context.as_raw(), ptr::from_ref(&tv).cast())
        };
        for t in self.done.drain(..) {
            self.in_flight.retain(|&p| p != t);
            // SAFETY: a completed transfer; its Pending is taken back once, and the transfer
            // freed once
            unsafe {
                let pending = Box::from_raw((*t).user_data.cast::<Pending>());
                done.push(Completion {
                    transfer: pending.transfer,
                    status: (*t).status,
                    actual_length: (*t).actual_length.max(0) as usize,
                });
                ffi::libusb_free_transfer(t);
            }
        }
        check(ret)
    }
}
