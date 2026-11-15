// SPDX-License-Identifier: MIT
//! A connection to the command server: TCP, or a command that relays the protocol on its
//! standard input and output.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::process::{Child, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crate::command::Command;
use crate::parser::{Event, Item, Parser};

/// Where the command server is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Endpoint {
    Tcp {
        host: String,
        port: u16,
    },
    /// A program, run with these arguments, that relays the protocol on stdin and stdout.
    Command {
        program: String,
        args: Vec<String>,
    },
}

/// The items of one packet. A packet ends at a CR after a NUL, or, failing that, when no byte
/// has arrived for [`Connection::quiet`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    pub items: Vec<Item>,
    /// When the last of its bytes arrived.
    pub received: SystemTime,
}

enum Chunk {
    Data(Vec<u8>),
    End(io::Result<()>),
}

/// An open connection.
pub struct Connection {
    writer: Box<dyn Write + Send>,
    chunks: Receiver<Chunk>,
    parser: Parser,
    items: Vec<Item>,
    last_byte: Instant,
    ready: VecDeque<Packet>,
    unsolicited: Vec<Packet>,
    closed: Option<io::Error>,
    tcp: Option<TcpStream>,
    child: Option<Child>,
    /// How long a request waits for its reply.
    pub timeout: Duration,
    /// The silence that ends a packet without its CR.
    pub quiet: Duration,
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

fn read_into(mut source: impl Read + Send + 'static) -> Receiver<Chunk> {
    let (sender, chunks) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        loop {
            let end = match source.read(&mut buffer) {
                Ok(0) => Ok(()),
                Ok(n) => {
                    if sender.send(Chunk::Data(buffer[..n].to_vec())).is_err() {
                        return;
                    }
                    continue;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => Err(e),
            };
            let _ = sender.send(Chunk::End(end));
            return;
        }
    });
    chunks
}

impl Connection {
    pub fn open(endpoint: &Endpoint) -> io::Result<Connection> {
        match endpoint {
            Endpoint::Tcp { host, port } => {
                let mut last = io::Error::new(io::ErrorKind::NotFound, "no address");
                for address in (host.as_str(), *port).to_socket_addrs()? {
                    match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
                        Ok(stream) => {
                            stream.set_nodelay(true)?;
                            let chunks = read_into(stream.try_clone()?);
                            let writer = Box::new(stream.try_clone()?);
                            return Ok(Connection::new(writer, chunks, Some(stream), None));
                        }
                        Err(e) => last = e,
                    }
                }
                Err(last)
            }
            Endpoint::Command { program, args } => {
                let mut child = std::process::Command::new(program)
                    .args(args)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .spawn()?;
                let writer = Box::new(child.stdin.take().unwrap());
                let chunks = read_into(child.stdout.take().unwrap());
                Ok(Connection::new(writer, chunks, None, Some(child)))
            }
        }
    }

    fn new(
        writer: Box<dyn Write + Send>,
        chunks: Receiver<Chunk>,
        tcp: Option<TcpStream>,
        child: Option<Child>,
    ) -> Connection {
        Connection {
            writer,
            chunks,
            parser: Parser::new(),
            items: Vec::new(),
            last_byte: Instant::now(),
            ready: VecDeque::new(),
            unsolicited: Vec::new(),
            closed: None,
            tcp,
            child,
            timeout: Duration::from_secs(2),
            quiet: Duration::from_millis(200),
        }
    }

    pub fn send(&mut self, command: Command) -> io::Result<()> {
        self.writer.write_all(&command.encode())?;
        self.writer.flush()
    }

    fn finish_packet(&mut self) {
        if !self.items.is_empty() {
            self.ready.push_back(Packet {
                items: std::mem::take(&mut self.items),
                received: SystemTime::now(),
            });
        }
    }

    /// The next packet, or None when none completes within `timeout`. Once the server has
    /// closed the connection and every packet is out, this returns the error
    /// `UnexpectedEof`, or the error the read failed with.
    pub fn next_packet(&mut self, timeout: Duration) -> io::Result<Option<Packet>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(packet) = self.ready.pop_front() {
                return Ok(Some(packet));
            }
            if let Some(error) = &self.closed {
                return Err(io::Error::new(error.kind(), error.to_string()));
            }
            let now = Instant::now();
            let mut wait = deadline.saturating_duration_since(now);
            if !self.items.is_empty() {
                wait = wait.min((self.last_byte + self.quiet).saturating_duration_since(now));
            }
            match self.chunks.recv_timeout(wait) {
                Ok(Chunk::Data(bytes)) => {
                    self.last_byte = Instant::now();
                    for event in self.parser.feed(&bytes) {
                        match event {
                            Event::Item(item) => self.items.push(item),
                            Event::PacketEnd => self.finish_packet(),
                        }
                    }
                }
                Ok(Chunk::End(result)) => {
                    self.finish_packet();
                    self.closed = Some(result.err().unwrap_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "the A16 closed the connection",
                        )
                    }));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    self.closed = Some(io::Error::other("the reader stopped"));
                }
                Err(RecvTimeoutError::Timeout) => {
                    if !self.items.is_empty() && self.last_byte.elapsed() >= self.quiet {
                        self.finish_packet();
                    } else if Instant::now() >= deadline {
                        return Ok(None);
                    }
                }
            }
        }
    }

    /// Sends a command and returns the packet that answers it, or None when none arrives
    /// within [`Connection::timeout`]. Other packets that arrive meanwhile are kept for
    /// [`Connection::take_unsolicited`]. For a [`Command::Raw`], the first packet answers.
    pub fn request(&mut self, command: Command) -> io::Result<Option<Packet>> {
        self.unsolicited.extend(self.ready.drain(..));
        self.send(command)?;
        let deadline = Instant::now() + self.timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let Some(packet) = self.next_packet(left)? else {
                return Ok(None);
            };
            let raw = matches!(command, Command::Raw(_));
            if raw || packet.items.iter().any(|item| command.answered_by(item)) {
                return Ok(Some(packet));
            }
            self.unsolicited.push(packet);
        }
    }

    /// The packets that arrived during requests without answering them.
    pub fn take_unsolicited(&mut self) -> Vec<Packet> {
        std::mem::take(&mut self.unsolicited)
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        if let Some(stream) = &self.tcp {
            let _ = stream.shutdown(Shutdown::Both);
        }
        if let Some(child) = &mut self.child {
            self.writer = Box::new(io::sink());
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
