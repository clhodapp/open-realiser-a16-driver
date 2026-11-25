// SPDX-License-Identifier: MIT
//! Realtime scheduling for the calling thread through rtkit, on the system D-Bus.
//!
//! The USB thread moves audio between the queues and the A16's transfers, a few
//! milliseconds ahead of the device. At normal priority a busy CPU delays it past that
//! margin and the A16 runs short. PipeWire's realtime module raises only the threads it
//! creates, so the driver asks rtkit for the USB thread itself, at the highest priority
//! rtkit grants. rtkit refuses a process whose `RLIMIT_RTTIME` is unset or above its
//! `RTTimeUSecMax`, so the limit is set to that maximum first: a realtime thread that runs
//! that long without blocking gets `SIGXCPU`, then `SIGKILL` at the same limit.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;

const DESTINATION: &CStr = c"org.freedesktop.RealtimeKit1";
const PATH: &CStr = c"/org/freedesktop/RealtimeKit1";
const INTERFACE: &CStr = c"org.freedesktop.RealtimeKit1";

#[repr(C)]
struct BusError {
    name: *const c_char,
    message: *const c_char,
    need_free: c_int,
}

impl BusError {
    fn empty() -> BusError {
        BusError {
            name: ptr::null(),
            message: ptr::null(),
            need_free: 0,
        }
    }
}

unsafe extern "C" {
    fn sd_bus_open_system(bus: *mut *mut c_void) -> c_int;
    fn sd_bus_flush_close_unref(bus: *mut c_void) -> *mut c_void;
    fn sd_bus_get_property_trivial(
        bus: *mut c_void,
        destination: *const c_char,
        path: *const c_char,
        interface: *const c_char,
        member: *const c_char,
        error: *mut BusError,
        kind: c_char,
        value: *mut c_void,
    ) -> c_int;
    fn sd_bus_call_method(
        bus: *mut c_void,
        destination: *const c_char,
        path: *const c_char,
        interface: *const c_char,
        member: *const c_char,
        error: *mut BusError,
        reply: *mut *mut c_void,
        types: *const c_char,
        ...
    ) -> c_int;
    fn sd_bus_message_unref(message: *mut c_void) -> *mut c_void;
    fn sd_bus_error_free(error: *mut BusError);
}

/// A failed sd-bus call as `context: message`, from the D-Bus error or the errno.
fn failure(context: &str, ret: c_int, error: &BusError) -> String {
    let message = if error.message.is_null() {
        crate::strerror(-ret)
    } else {
        // SAFETY: sd-bus sets a NUL-terminated message with the error
        unsafe { CStr::from_ptr(error.message) }
            .to_string_lossy()
            .into_owned()
    };
    format!("{context}: {message}")
}

/// The system bus, closed on drop.
struct Bus(*mut c_void);

impl Drop for Bus {
    fn drop(&mut self) {
        // SAFETY: the bus this value opened
        unsafe { sd_bus_flush_close_unref(self.0) };
    }
}

impl Bus {
    fn open() -> Result<Bus, String> {
        let mut bus = ptr::null_mut();
        // SAFETY: an out pointer for the new bus
        let ret = unsafe { sd_bus_open_system(&raw mut bus) };
        if ret < 0 {
            return Err(format!(
                "Cannot open the system bus: {}",
                crate::strerror(-ret)
            ));
        }
        Ok(Bus(bus))
    }

    /// The rtkit property `member`, of D-Bus type `kind`, read into a `T` of that type.
    fn property<T: Default>(&self, member: &CStr, kind: u8) -> Result<T, String> {
        let mut value = T::default();
        let mut error = BusError::empty();
        // SAFETY: the open bus, NUL-terminated names, and a `T` of the D-Bus type `kind`
        let ret = unsafe {
            sd_bus_get_property_trivial(
                self.0,
                DESTINATION.as_ptr(),
                PATH.as_ptr(),
                INTERFACE.as_ptr(),
                member.as_ptr(),
                &raw mut error,
                kind as c_char,
                (&raw mut value).cast(),
            )
        };
        let result = if ret < 0 {
            Err(failure(
                &format!(
                    "Cannot read the rtkit property {}",
                    member.to_string_lossy()
                ),
                ret,
                &error,
            ))
        } else {
            Ok(value)
        };
        // SAFETY: the error the call filled, or the empty one
        unsafe { sd_bus_error_free(&raw mut error) };
        result
    }

    fn make_realtime(&self, tid: u64, priority: u32) -> Result<(), String> {
        let mut error = BusError::empty();
        let mut reply = ptr::null_mut();
        // SAFETY: the open bus, NUL-terminated names, and arguments of the types "tu" names
        let ret = unsafe {
            sd_bus_call_method(
                self.0,
                DESTINATION.as_ptr(),
                PATH.as_ptr(),
                INTERFACE.as_ptr(),
                c"MakeThreadRealtime".as_ptr(),
                &raw mut error,
                &raw mut reply,
                c"tu".as_ptr(),
                tid,
                priority,
            )
        };
        let result = if ret < 0 {
            Err(failure("rtkit refused realtime", ret, &error))
        } else {
            Ok(())
        };
        // SAFETY: the reply and error the call filled, or null and the empty error
        unsafe {
            sd_bus_message_unref(reply);
            sd_bus_error_free(&raw mut error);
        }
        result
    }
}

/// Make the calling thread realtime through rtkit: the priority rtkit granted.
pub fn make_current_thread_realtime() -> Result<u32, String> {
    let bus = Bus::open()?;
    let max_priority: i32 = bus.property(c"MaxRealtimePriority", b'i')?;
    let max_rttime: i64 = bus.property(c"RTTimeUSecMax", b'x')?;
    let priority = u32::try_from(max_priority)
        .ok()
        .filter(|&p| p > 0)
        .ok_or_else(|| format!("rtkit grants no realtime priority (maximum {max_priority})"))?;
    let rttime = u64::try_from(max_rttime)
        .ok()
        .filter(|&t| t > 0)
        .ok_or_else(|| format!("rtkit grants no realtime time (maximum {max_rttime} µs)"))?;
    lower_rttime(rttime)?;
    // SAFETY: gettid has no preconditions
    let tid = unsafe { libc::gettid() };
    bus.make_realtime(tid as u64, priority)?;
    Ok(priority)
}

/// Set `RLIMIT_RTTIME` to at most `max` µs, keeping a lower limit already set.
fn lower_rttime(max: u64) -> Result<(), String> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: an out pointer for the current limit
    if unsafe { libc::getrlimit(libc::RLIMIT_RTTIME, &raw mut limit) } != 0 {
        return Err(crate::perror(
            "Cannot read RLIMIT_RTTIME",
            &std::io::Error::last_os_error(),
        ));
    }
    let (soft, hard) = rttime_limit(limit.rlim_cur, limit.rlim_max, max)?;
    if (soft, hard) == (limit.rlim_cur, limit.rlim_max) {
        return Ok(());
    }
    let limit = libc::rlimit {
        rlim_cur: soft,
        rlim_max: hard,
    };
    // SAFETY: a valid limit
    if unsafe { libc::setrlimit(libc::RLIMIT_RTTIME, &raw const limit) } != 0 {
        return Err(crate::perror(
            "Cannot set RLIMIT_RTTIME",
            &std::io::Error::last_os_error(),
        ));
    }
    Ok(())
}

/// The soft and hard `RLIMIT_RTTIME` from the current ones and rtkit's maximum: each at
/// most `max`, the soft limit at most the hard one. A limit of 0 is an error: the kernel
/// kills a realtime thread under it on the first scheduler tick that finds the thread
/// running, and an unprivileged process cannot raise its hard limit again. PipeWire's
/// realtime module sets that limit in a client when the Realtime portal reports an
/// `RTTimeUSecMax` of 0, as the portal does when it started before rtkit was installed.
fn rttime_limit(soft: u64, hard: u64, max: u64) -> Result<(u64, u64), String> {
    let hard = hard.min(max);
    let soft = soft.min(hard);
    if soft == 0 {
        return Err(
            "RLIMIT_RTTIME is 0, under which the kernel kills a realtime thread".to_owned(),
        );
    }
    Ok((soft, hard))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unlimited_rttime_takes_the_maximum() {
        assert_eq!(
            rttime_limit(libc::RLIM_INFINITY, libc::RLIM_INFINITY, 200_000),
            Ok((200_000, 200_000))
        );
    }

    #[test]
    fn a_lower_rttime_stays() {
        assert_eq!(
            rttime_limit(50_000, 100_000, 200_000),
            Ok((50_000, 100_000))
        );
        assert_eq!(
            rttime_limit(150_000, 300_000, 200_000),
            Ok((150_000, 200_000))
        );
    }

    #[test]
    fn a_zero_rttime_refuses_realtime() {
        assert!(rttime_limit(0, 0, 200_000).is_err());
        assert!(rttime_limit(0, 100_000, 200_000).is_err());
    }
}
