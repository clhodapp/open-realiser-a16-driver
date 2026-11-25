// SPDX-License-Identifier: MIT
//! The Linux userspace driver of the Smyth Realiser A16 around the `a16-engine` crate: the
//! libusb bus, the PipeWire bridge, the profile catalog and the tools' shared parts.
//!
//! The binaries are `a16-driver` (the PipeWire bridge), `a16-usb` (USB inspection and
//! bounded WAV streaming) and `a16-static-dsp` (offline Custom #2 processing).

pub mod bridge;
pub mod bus;
pub mod graph_clock;
pub mod libusb;
pub mod live;
pub mod nodes;
pub mod notify;
pub mod profiles;
pub mod realtime;
pub mod samplerate;
pub mod usb;
pub mod wav;

/// `strerror` of an OS error code.
pub fn strerror(code: i32) -> String {
    // SAFETY: strerror returns a string that stays valid until the next call on this thread
    unsafe {
        std::ffi::CStr::from_ptr(libc::strerror(code))
            .to_string_lossy()
            .into_owned()
    }
}

/// An I/O error as `perror` prints it, after `context`.
pub fn perror(context: &str, error: &std::io::Error) -> String {
    match error.raw_os_error() {
        Some(code) => format!("{context}: {}", strerror(code)),
        None => format!("{context}: {error}"),
    }
}
