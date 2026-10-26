//! The app's end of a link: find a server, check it is the real one, authenticate.
//!
//! Used by the Android app and by `lindroids debug-app` (which stands in for it in tests).

use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use lindroids_proto::{self as proto, auth, Framer, ToApp, ToServer};

pub enum Conn {
    Unix(UnixStream),
    Tcp(TcpStream),
}

impl Conn {
    pub fn try_clone(&self) -> std::io::Result<Conn> {
        Ok(match self {
            Conn::Unix(s) => Conn::Unix(s.try_clone()?),
            Conn::Tcp(s) => Conn::Tcp(s.try_clone()?),
        })
    }

    pub fn shutdown(&self) {
        let _ = match self {
            Conn::Unix(s) => s.shutdown(std::net::Shutdown::Both),
            Conn::Tcp(s) => s.shutdown(std::net::Shutdown::Both),
        };
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Conn::Unix(_) => "unix",
            Conn::Tcp(_) => "tcp",
        }
    }

    fn set_timeout(&self, t: Option<Duration>) {
        let _ = match self {
            Conn::Unix(s) => s.set_read_timeout(t),
            Conn::Tcp(s) => s.set_read_timeout(t),
        };
    }
}

impl Read for Conn {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Conn::Unix(s) => s.read(b),
            Conn::Tcp(s) => s.read(b),
        }
    }
}

impl Write for Conn {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        match self {
            Conn::Unix(s) => s.write(b),
            Conn::Tcp(s) => s.write(b),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl AsRawFd for Conn {
    fn as_raw_fd(&self) -> RawFd {
        match self {
            Conn::Unix(s) => s.as_raw_fd(),
            Conn::Tcp(s) => s.as_raw_fd(),
        }
    }
}

pub enum Outcome {
    Linked {
        conn: Conn,
        framer: Framer,
        /// Set when pairing by code worked: the secret to keep.
        paired: Option<String>,
        sandbox: String,
        /// Messages that arrived with the server's Hello.
        backlog: Vec<(ToApp, Vec<u8>)>,
    },
    /// Not paired, or wrong code. `open`: pairing still accepts codes.
    Denied { open: bool, reason: String },
    /// The server could not prove it knows our secret: a stale pairing, or not the real server.
    WrongServer,
    /// Nothing there, or it hung up.
    Absent(String),
}

/// Reach the server of sandbox `uid`: its abstract socket if we may, else loopback TCP.
pub fn reach(uid: u32, socket_name: Option<&str>) -> Result<Conn, String> {
    let name = socket_name.map(String::from).unwrap_or_else(|| proto::server_socket_name(uid));
    if let Ok(s) = crate::connect_abstract(&name) {
        // The kernel names who listens there; a socket name alone proves nothing.
        if crate::peer_uid(&s) == uid {
            return Ok(Conn::Unix(s));
        }
    }
    let port = proto::server_tcp_port(uid);
    let s = TcpStream::connect_timeout(&(Ipv4Addr::LOCALHOST, port).into(), Duration::from_secs(2)).map_err(|e| e.to_string())?;
    s.set_nodelay(true).ok();
    Ok(Conn::Tcp(s))
}

fn read_one(conn: &mut Conn, framer: &mut Framer, deadline: Instant) -> Result<(ToApp, Vec<u8>), String> {
    loop {
        if let Some(m) = framer.next::<ToApp>() {
            return m;
        }
        if Instant::now() > deadline {
            return Err("the server did not answer".into());
        }
        conn.set_timeout(Some(Duration::from_millis(500)));
        let mut buf = [0u8; 65536];
        match conn.read(&mut buf) {
            Ok(0) => return Err("the server hung up".into()),
            Ok(n) => framer.push(&buf[..n]),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(e) => return Err(e.to_string()),
        }
    }
}

pub fn connect(uid: u32, socket_name: Option<&str>, secret: Option<&str>, code: Option<&str>, density: f32, build: &str) -> Outcome {
    let mut conn = match reach(uid, socket_name) {
        Ok(c) => c,
        Err(e) => return Outcome::Absent(e),
    };
    let mut framer = Framer::default();
    let deadline = Instant::now() + Duration::from_secs(8);
    let nonce = crate::shm::random_string(32, b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789");
    let hello = ToServer::Hello { version: proto::PROTOCOL_VERSION, build: build.into(), density, nonce: nonce.clone() };
    if let Err(e) = conn.write_all(&proto::encode(&hello)) {
        return Outcome::Absent(e.to_string());
    }
    let (server_nonce, proof) = match read_one(&mut conn, &mut framer, deadline) {
        Ok((ToApp::Challenge { nonce, proof }, _)) => (nonce, proof),
        Ok((other, _)) => return Outcome::Absent(format!("unexpected answer {other:?}")),
        Err(e) => return Outcome::Absent(e),
    };
    let auth_msg = match (secret, code) {
        (_, Some(c)) => ToServer::Auth { proof: None, code: Some(c.into()) },
        (Some(s), None) => {
            if !auth::same(&proof, &auth::server_proof(s, &nonce)) {
                return Outcome::WrongServer;
            }
            ToServer::Auth { proof: Some(auth::app_proof(s, &server_nonce)), code: None }
        }
        (None, None) => ToServer::Auth { proof: None, code: None },
    };
    if let Err(e) = conn.write_all(&proto::encode(&auth_msg)) {
        return Outcome::Absent(e.to_string());
    }
    let mut paired = None;
    loop {
        match read_one(&mut conn, &mut framer, deadline) {
            Ok((ToApp::Paired { secret }, _)) => paired = Some(secret),
            Ok((ToApp::AuthFailed { reason, pairing_open }, _)) => return Outcome::Denied { open: pairing_open, reason },
            Ok((ToApp::Hello { sandbox, .. }, _)) => {
                conn.set_timeout(None);
                let mut backlog = Vec::new();
                while let Some(Ok(m)) = framer.next::<ToApp>() {
                    backlog.push(m);
                }
                return Outcome::Linked { conn, framer, paired, sandbox, backlog };
            }
            Ok(_) => {}
            Err(e) => return Outcome::Absent(e),
        }
    }
}
