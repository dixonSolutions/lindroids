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
        /// Set when pairing by code worked: the display id given, and the relay's key to pin.
        paired: Option<(String, String)>,
        sandbox: String,
        /// Messages that arrived with the server's Hello.
        backlog: Vec<(ToApp, Vec<u8>)>,
    },
    /// Not paired, or wrong code. `open`: pairing still accepts codes.
    Denied { open: bool, reason: String },
    /// The server could not prove it holds the relay key we pinned: not the real server.
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

/// What the app tells a server about itself.
#[derive(Debug, Clone, Copy)]
pub struct Display {
    pub density: f32,
    /// Physical pixels.
    pub screen: Option<(u32, u32)>,
}

/// Who the app is to the relay: its key pair and, once paired, its display id and the relay key
/// it pinned. Kept as a file (`load`/`save`) only this user can read.
pub struct Identity {
    pub key: auth::KeyPair,
    pub display: Option<String>,
    pub relay_key: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct IdentityFile {
    key: String,
    display: Option<String>,
    relay_key: Option<String>,
}

impl Identity {
    /// The identity in `path`, or a new unpaired one.
    pub fn load(path: &std::path::Path) -> Identity {
        use base64::Engine;
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let file: Option<IdentityFile> = std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok());
        if let Some(f) = file {
            if let Some(key) = b64.decode(&f.key).ok().and_then(|b| auth::KeyPair::from_bytes(&b)) {
                return Identity { key, display: f.display, relay_key: f.relay_key };
            }
        }
        Identity { key: auth::KeyPair::generate().expect("randomness"), display: None, relay_key: None }
    }

    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        use base64::Engine;
        use std::os::unix::fs::OpenOptionsExt;
        let f = IdentityFile {
            key: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(self.key.to_bytes()),
            display: self.display.clone(),
            relay_key: self.relay_key.clone(),
        };
        let _ = std::fs::remove_file(path);
        let mut out = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
        out.write_all(&serde_json::to_vec(&f).unwrap_or_default())
    }
}

pub fn connect(uid: u32, socket_name: Option<&str>, id: &Identity, code: Option<&str>, display: Display, build: &str) -> Outcome {
    let mut conn = match reach(uid, socket_name) {
        Ok(c) => c,
        Err(e) => return Outcome::Absent(e),
    };
    let mut framer = Framer::default();
    let deadline = Instant::now() + Duration::from_secs(8);
    let nonce = crate::shm::random_string(32, b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789");
    let hello = ToServer::Hello {
        version: proto::PROTOCOL_VERSION,
        build: build.into(),
        density: display.density,
        nonce: nonce.clone(),
        screen: display.screen,
        page: None,
    };
    if let Err(e) = conn.write_all(&proto::encode(&hello)) {
        return Outcome::Absent(e.to_string());
    }
    let (server_nonce, proof, relay_key) = match read_one(&mut conn, &mut framer, deadline) {
        Ok((ToApp::Challenge { nonce, proof, relay_key }, _)) => (nonce, proof, relay_key),
        Ok((other, _)) => return Outcome::Absent(format!("unexpected answer {other:?}")),
        Err(e) => return Outcome::Absent(e),
    };
    // Not a web page: no origin in what is signed.
    let relay_signed = auth::relay_transcript("", &nonce, &server_nonce);
    if id.relay_key.as_deref().map_or(false, |k| k != relay_key) || !auth::verify(&relay_key, &relay_signed, &proof) {
        return Outcome::WrongServer;
    }
    let my_proof = Some(id.key.sign(&auth::display_transcript("", &server_nonce, &nonce)));
    let auth_msg = match (&id.display, code) {
        (Some(d), _) if id.relay_key.is_some() => {
            ToServer::Auth { display: Some(d.clone()), proof: my_proof, public_key: None, ticket: None, code: None, name: None }
        }
        (_, Some(c)) => ToServer::Auth {
            display: None,
            proof: my_proof,
            public_key: Some(id.key.public()),
            ticket: None,
            code: Some(c.into()),
            name: Some(build.into()),
        },
        _ => ToServer::Auth { display: None, proof: None, public_key: None, ticket: None, code: None, name: None },
    };
    if let Err(e) = conn.write_all(&proto::encode(&auth_msg)) {
        return Outcome::Absent(e.to_string());
    }
    let mut paired = None;
    loop {
        match read_one(&mut conn, &mut framer, deadline) {
            Ok((ToApp::Paired { display }, _)) => paired = Some((display, relay_key.clone())),
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
