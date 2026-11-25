// SPDX-License-Identifier: MIT
//! Log lines written by a logger thread, so that the thread that logs never waits for the
//! reader of stderr. Under systemd that reader is journald, which falls behind when the
//! machine is busy; a write then blocks until it catches up, and a USB thread blocked that
//! long stops the A16's transfers in both directions.
//!
//! A line goes into a bounded queue without blocking. When the queue is full the line is
//! dropped and counted, and the logger reports the count before its next line.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::time::Duration;

/// The lines the queue holds before it drops new ones.
pub const QUEUED_LINES: usize = 256;
/// How long [`flush`] waits for the logger to write the queued lines.
const FLUSH_LIMIT: Duration = Duration::from_secs(1);

enum Message {
    Line(String),
    Flush(SyncSender<()>),
}

/// A logger thread and the queue it writes from.
pub struct LogQueue {
    queue: SyncSender<Message>,
    dropped: &'static AtomicU64,
}

impl LogQueue {
    /// Start a logger thread that hands each queued line to `write`, at normal priority.
    pub fn spawn(write: impl FnMut(&str) + Send + 'static) -> LogQueue {
        let (queue, lines) = mpsc::sync_channel(QUEUED_LINES);
        let dropped: &'static AtomicU64 = Box::leak(Box::new(AtomicU64::new(0)));
        std::thread::Builder::new()
            .name("a16-log".to_owned())
            .spawn(move || run(lines, dropped, write))
            .expect("Cannot start the logger thread");
        LogQueue { queue, dropped }
    }

    /// Queue `line`, or drop it when the queue is full; never blocks.
    pub fn push(&self, line: &str) {
        match self.queue.try_send(Message::Line(line.to_owned())) {
            Ok(()) => {}
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Wait, for at most a second, until the lines queued so far are written.
    pub fn flush(&self) {
        let (done, finished) = mpsc::sync_channel(1);
        if self.queue.send(Message::Flush(done)).is_ok() {
            let _ = finished.recv_timeout(FLUSH_LIMIT);
        }
    }
}

fn run(lines: Receiver<Message>, dropped: &AtomicU64, mut write: impl FnMut(&str)) {
    // A thread starts with the scheduling policy of the thread that creates it, which may
    // be the realtime USB thread; the logger waits on stderr, so it runs at normal priority
    let normal = libc::sched_param { sched_priority: 0 };
    // SAFETY: a valid parameter for SCHED_OTHER, on the calling thread
    unsafe { libc::sched_setscheduler(0, libc::SCHED_OTHER, &raw const normal) };
    for message in lines {
        let lost = dropped.swap(0, Ordering::Relaxed);
        if lost != 0 {
            write(&format!(
                "{lost} log lines dropped while stderr was blocked"
            ));
        }
        match message {
            Message::Line(line) => write(&line),
            Message::Flush(done) => {
                let _ = done.try_send(());
            }
        }
    }
}

/// The process's logger, writing to stderr.
fn stderr() -> &'static LogQueue {
    static QUEUE: OnceLock<LogQueue> = OnceLock::new();
    QUEUE.get_or_init(|| LogQueue::spawn(|line| eprintln!("{line}")))
}

/// Log `line` to stderr through the logger thread.
pub fn line(line: &str) {
    stderr().push(line);
}

/// Wait until the lines logged so far are on stderr: before the process exits.
pub fn flush() {
    stderr().flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier, Mutex};
    use std::time::Instant;

    #[test]
    fn logging_never_waits_for_a_blocked_writer() {
        let gate = Arc::new(Barrier::new(2));
        let written = Arc::new(Mutex::new(Vec::new()));
        let queue = {
            let gate = Arc::clone(&gate);
            let written = Arc::clone(&written);
            let mut first = true;
            LogQueue::spawn(move |line| {
                if first {
                    // The writer blocks, as on a full journald socket
                    first = false;
                    gate.wait();
                }
                written.lock().unwrap().push(line.to_owned());
            })
        };
        let start = Instant::now();
        for i in 0..QUEUED_LINES * 4 {
            queue.push(&format!("line {i}"));
        }
        assert!(start.elapsed() < Duration::from_millis(500));
        gate.wait();
        queue.flush();
        let written = written.lock().unwrap();
        // The queue kept the earliest lines and dropped the rest
        assert!(written.iter().any(|l| l == "line 0"));
        assert!(
            !written
                .iter()
                .any(|l| l == &format!("line {}", QUEUED_LINES * 4 - 1))
        );
        assert!(
            written
                .iter()
                .any(|l| l.ends_with("log lines dropped while stderr was blocked"))
        );
    }

    #[test]
    fn flush_waits_for_the_queued_lines() {
        let written = Arc::new(Mutex::new(Vec::new()));
        let queue = {
            let written = Arc::clone(&written);
            LogQueue::spawn(move |line| {
                std::thread::sleep(Duration::from_millis(1));
                written.lock().unwrap().push(line.to_owned());
            })
        };
        for i in 0..20 {
            queue.push(&format!("line {i}"));
        }
        queue.flush();
        assert_eq!(written.lock().unwrap().len(), 20);
    }
}
