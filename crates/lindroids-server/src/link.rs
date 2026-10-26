//! One connection to the app, over an abstract Unix socket or loopback TCP.
//!
//! Writes never block the compositor: they queue and drain when the socket is writable.

use std::io::{Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::time::Instant;

use lindroids_proto::{self as proto, Framer, ToApp, ToServer};
use serde::Serialize;

pub trait Io: Read + Write + AsRawFd + Send {}
impl<T: Read + Write + AsRawFd + Send> Io for T {}

#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    /// Waiting for the app's Hello.
    Hello,
    /// Challenge sent; waiting for Auth.
    Auth { nonce: String, app_nonce: String, density: f32, version: u32, build: String },
    Ready,
}

pub struct Link {
    io: Box<dyn Io>,
    framer: Framer,
    out: Vec<u8>,
    out_pos: usize,
    pub phase: Phase,
    pub peer: String,
    pub since: Instant,
    pub density: f32,
    pub broken: bool,
}

impl Link {
    pub fn new(io: Box<dyn Io>, peer: String) -> Link {
        Link { io, framer: Framer::default(), out: Vec::new(), out_pos: 0, phase: Phase::Hello, peer, since: Instant::now(), density: 1.0, broken: false }
    }

    pub fn fd(&self) -> RawFd {
        self.io.as_raw_fd()
    }

    pub fn send<T: Serialize>(&mut self, msg: &T) {
        self.send_with(msg, &[]);
    }

    pub fn send_with<T: Serialize>(&mut self, msg: &T, payload: &[u8]) {
        if self.broken {
            return;
        }
        self.out.extend_from_slice(&proto::encode_with(msg, payload));
        self.flush();
    }

    /// Bytes queued but not yet written.
    pub fn pending(&self) -> usize {
        self.out.len() - self.out_pos
    }

    pub fn wants_write(&self) -> bool {
        self.pending() > 0
    }

    pub fn flush(&mut self) {
        while self.out_pos < self.out.len() {
            match self.io.write(&self.out[self.out_pos..]) {
                Ok(0) => {
                    self.broken = true;
                    break;
                }
                Ok(n) => self.out_pos += n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    self.broken = true;
                    break;
                }
            }
        }
        if self.out_pos == self.out.len() {
            self.out.clear();
            self.out_pos = 0;
        } else if self.out_pos > (4 << 20) {
            self.out.drain(..self.out_pos);
            self.out_pos = 0;
        }
    }

    /// Read what is there. Returns false when the peer is gone.
    pub fn read(&mut self) -> bool {
        let mut buf = [0u8; 65536];
        loop {
            match self.io.read(&mut buf) {
                Ok(0) => return false,
                Ok(n) => self.framer.push(&buf[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return true,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return false,
            }
        }
    }

    pub fn next(&mut self) -> Option<Result<ToServer, String>> {
        self.framer.next::<ToServer>().map(|r| r.map(|(m, _)| m))
    }
}

/// Messages queued for the app: kept for completeness of the type list.
pub type Outgoing = (ToApp, Vec<u8>);
