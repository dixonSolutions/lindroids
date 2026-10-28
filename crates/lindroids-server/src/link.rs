//! One connection to a display: the app over an abstract Unix socket or loopback TCP, or a
//! browser over a WebSocket on the same loopback port (see `web.rs`).
//!
//! Writes never block the compositor: they queue and drain when the socket is writable.

use std::io::{Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::time::Instant;

use lindroids_proto::{self as proto, Framer, ToApp, ToServer};
use serde::Serialize;

use crate::web;

pub trait Io: Read + Write + AsRawFd + Send {}
impl<T: Read + Write + AsRawFd + Send> Io for T {}

#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    /// Waiting for the app's Hello.
    Hello,
    /// Challenge sent; waiting for Auth.
    Auth { nonce: String, app_nonce: String, density: f32, version: u32, build: String, screen: Option<(u32, u32)> },
    Ready,
}

/// What the connection speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wire {
    /// Loopback TCP whose first bytes have not said yet: the link protocol or HTTP.
    Sniffing,
    /// The link protocol as is.
    Raw,
    /// A WebSocket from a browser.
    WebSocket,
    /// An HTTP answer is on its way out; then the connection closes.
    Closing,
}

pub struct Link {
    io: Box<dyn Io>,
    wire: Wire,
    /// Bytes not yet sorted out: a request head while sniffing, partial frames on a WebSocket.
    inbuf: Vec<u8>,
    port: u16,
    framer: Framer,
    out: Vec<u8>,
    out_pos: usize,
    pub phase: Phase,
    pub peer: String,
    pub since: Instant,
    pub density: f32,
    pub build: String,
    /// The app's protocol version (from its Hello).
    pub version: u32,
    /// The page's path and query, for a browser.
    pub page: Option<String>,
    pub broken: bool,
    /// Names this link among the others (windows remember which link shows them).
    pub id: u64,
    /// The run this page shows (`?run=<pid>`: a window of its own for what `lindroids run`
    /// started); None for the page that shows everything else.
    pub run: Option<u32>,
    /// The page said it is going away (`Closing`) before its link closed.
    pub closing: bool,
    /// The WebSocket close frame, if one came, and its code, if it had one (for the log: how the
    /// page went).
    pub close_frame: Option<Option<u16>>,
    /// The origin of the page (its WebSocket's Origin); empty for a display that is not a page.
    /// Signatures in the handshake cover it.
    pub origin: String,
}

impl Link {
    pub fn new(io: Box<dyn Io>, peer: String) -> Link {
        Link {
            io,
            wire: Wire::Raw,
            inbuf: Vec::new(),
            port: 0,
            framer: Framer::default(),
            out: Vec::new(),
            out_pos: 0,
            phase: Phase::Hello,
            peer,
            since: Instant::now(),
            density: 1.0,
            build: String::new(),
            version: 0,
            page: None,
            broken: false,
            id: 0,
            run: None,
            closing: false,
            close_frame: None,
            origin: String::new(),
        }
    }

    /// A loopback TCP connection to `port`: the app, or a browser.
    pub fn new_tcp(io: Box<dyn Io>, peer: String, port: u16) -> Link {
        Link { wire: Wire::Sniffing, port, ..Link::new(io, peer) }
    }

    pub fn wire(&self) -> Wire {
        self.wire
    }

    pub fn is_browser(&self) -> bool {
        self.wire == Wire::WebSocket
    }

    /// An HTTP answer went out in full: the connection is done.
    pub fn finished(&self) -> bool {
        self.wire == Wire::Closing && !self.wants_write()
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
        let bytes = proto::encode_with(msg, payload);
        if self.wire == Wire::WebSocket {
            // One message per WebSocket message: the browser needs no framer of its own.
            self.out.extend_from_slice(&web::frame(web::OP_BINARY, &bytes));
        } else {
            self.out.extend_from_slice(&bytes);
        }
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
                Ok(n) => {
                    if !self.take(&buf[..n]) {
                        return false;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return true,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return false,
            }
        }
    }

    /// Bytes from the socket, by what the connection speaks. False: close it.
    fn take(&mut self, data: &[u8]) -> bool {
        match self.wire {
            Wire::Raw => self.framer.push(data),
            Wire::Closing => {}
            Wire::Sniffing => {
                self.inbuf.extend_from_slice(data);
                match web::sniff(&self.inbuf) {
                    web::Sniff::More => {}
                    web::Sniff::Raw => {
                        self.wire = Wire::Raw;
                        let b = std::mem::take(&mut self.inbuf);
                        self.framer.push(&b);
                    }
                    web::Sniff::Bad => return false,
                    web::Sniff::Http(req, rest) => {
                        self.inbuf.clear();
                        match web::answer(&req, self.port) {
                            web::Answer::Close(bytes) => {
                                self.wire = Wire::Closing;
                                self.out.extend_from_slice(&bytes);
                            }
                            web::Answer::Upgrade(bytes) => {
                                self.origin = req.header("Origin").unwrap_or_default().to_string();
                                log::info!("browser at {} opened the link", self.peer);
                                self.wire = Wire::WebSocket;
                                self.peer = format!("browser ({})", self.peer);
                                self.out.extend_from_slice(&bytes);
                                self.flush();
                                return self.take(&rest);
                            }
                        }
                        self.flush();
                    }
                }
            }
            Wire::WebSocket => {
                self.inbuf.extend_from_slice(data);
                let u = web::unframe(&mut self.inbuf);
                self.framer.push(&u.data);
                if !u.reply.is_empty() {
                    self.out.extend_from_slice(&u.reply);
                    self.flush();
                }
                if u.closed {
                    self.close_frame = Some(u.close_code);
                    // 1001, going away: the browser closes the socket of a page it takes down (its
                    // window closed, a reload), also when the page had no time to say so itself
                    // (Chrome: no pagehide on closing a Custom Tab). A browser that dies sends
                    // nothing.
                    if u.close_code == Some(1001) {
                        self.closing = true;
                    }
                    return false;
                }
            }
        }
        true
    }

    /// The next message and its payload (FileChunk carries one).
    pub fn next(&mut self) -> Option<Result<(ToServer, Vec<u8>), String>> {
        self.framer.next::<ToServer>()
    }
}

/// Messages queued for the app: kept for completeness of the type list.
pub type Outgoing = (ToApp, Vec<u8>);
