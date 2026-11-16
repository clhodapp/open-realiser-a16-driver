// SPDX-License-Identifier: MIT
//
// Readiness for systemd's Type=notify: `READY=1`, sent once as a datagram to the socket
// systemd names in NOTIFY_SOCKET. Without that variable, as outside a notify service,
// nothing is sent.

use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixDatagram};
use std::sync::atomic::{AtomicBool, Ordering};

static SENT: AtomicBool = AtomicBool::new(false);

/// Tell systemd the driver is ready, the first time this is called.
pub fn ready() {
    if SENT.swap(true, Ordering::SeqCst) {
        return;
    }
    let Some(path) = std::env::var_os("NOTIFY_SOCKET") else {
        return;
    };
    let bytes = std::os::unix::ffi::OsStrExt::as_bytes(path.as_os_str());
    // A leading '@' names a socket in the abstract namespace
    let addr = match bytes.split_first() {
        Some((b'@', name)) => SocketAddr::from_abstract_name(name),
        _ => SocketAddr::from_pathname(&path),
    };
    let sent = addr.and_then(|addr| {
        let socket = UnixDatagram::unbound()?;
        socket.send_to_addr(b"READY=1", &addr)
    });
    if let Err(error) = sent {
        eprintln!("Cannot notify systemd of readiness: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sends_ready_once_to_a_path_socket() {
        let dir = std::env::temp_dir().join(format!("a16-notify-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("notify.sock");
        let _ = std::fs::remove_file(&path);
        let listener = UnixDatagram::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        // SAFETY: this test is the only code in the crate's unit tests that reads or writes
        // NOTIFY_SOCKET
        unsafe { std::env::set_var("NOTIFY_SOCKET", &path) };
        ready();
        ready();
        let mut buf = [0u8; 64];
        let n = listener.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"READY=1");
        assert!(listener.recv(&mut buf).is_err(), "sent more than once");
        unsafe { std::env::remove_var("NOTIFY_SOCKET") };
        let _ = std::fs::remove_dir_all(&dir);
    }
}
