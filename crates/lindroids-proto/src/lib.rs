//! Wire types shared by the `lindroids` CLI, the server that runs inside a Linux sandbox, and the
//! Lindroids Android app.
//!
//! # The link (server ⇄ app)
//!
//! A byte stream: an abstract Unix socket when the app may reach the sandbox's sockets, otherwise
//! TCP on 127.0.0.1 (Android lets any two apps talk over loopback, but SELinux keeps apps out of
//! each other's Unix sockets). Nothing on the link depends on passing file descriptors.
//!
//! Each message is one line of JSON. A message with a `len` field is followed by exactly `len`
//! bytes of binary payload (frame tiles, PCM, bundle data).
//!
//! # The control socket (CLI ⇄ server)
//!
//! A Unix socket in the runtime dir, same uid, JSON lines; fds may travel with requests.

use serde::{Deserialize, Serialize};

pub mod auth;
pub mod fdpass;

/// Bumped whenever a message changes shape. The app and the server refuse to talk across
/// versions, and the CLI says which side needs updating.
pub const PROTOCOL_VERSION: u32 = 2;

/// Android package of the app.
pub const APP_PACKAGE: &str = "io.github.dixonsolutions.lindroids";

/// Broadcast the CLI sends to wake the app and tell it where the server is.
pub const ACTION_SERVER_HELLO: &str = "io.github.dixonsolutions.lindroids.SERVER_HELLO";

/// The real uid of this process. Under proot's fake root (proot-distro, UserLAnd) getuid() says
/// 0, but the app looks for the sandbox app's own uid; the kernel's view in /proc is not faked.
pub fn real_uid() -> u32 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("Uid:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse().ok()))
        .unwrap_or_else(|| unsafe { libc::getuid() })
}

/// Abstract socket a server listens on. One server per uid (per sandbox app).
pub fn server_socket_name(uid: u32) -> String {
    format!("lindroids.server.{uid}")
}

/// Loopback TCP port a server listens on: fixed per uid, so the app can find it.
pub fn server_tcp_port(uid: u32) -> u16 {
    let app_id = uid % 100_000;
    47_000 + (app_id.saturating_sub(10_000) % 10_000) as u16
}

/// Termux's prefix and home, which Termux packages have compiled in.
pub const TERMUX_PREFIX: &str = "/data/data/com.termux/files/usr";
pub const TERMUX_HOME: &str = "/data/data/com.termux/files/home";

/// Environment that steers toolkits onto Wayland and software rendering.
pub fn toolkit_env() -> Vec<(&'static str, &'static str)> {
    vec![
        ("GDK_BACKEND", "wayland"),
        ("QT_QPA_PLATFORM", "wayland"),
        ("QT_WAYLAND_DISABLE_WINDOWDECORATION", "1"),
        ("SDL_VIDEODRIVER", "wayland"),
        ("MOZ_ENABLE_WAYLAND", "1"),
        ("ELECTRON_OZONE_PLATFORM_HINT", "wayland"),
        ("CLUTTER_BACKEND", "wayland"),
        ("GSK_RENDERER", "cairo"),
        ("LIBGL_ALWAYS_SOFTWARE", "1"),
        ("NO_AT_BRIDGE", "1"),
        ("XDG_SESSION_TYPE", "wayland"),
    ]
}

// ------------------------------------------------------------------ server -> app

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ToApp {
    /// Answer to the app's Hello: proves the server knows the secret (if the app has one), and
    /// gives the app a nonce to prove itself with.
    Challenge { nonce: String, proof: String },
    /// The app is authenticated; windows follow.
    Hello {
        version: u32,
        build: String,
        /// Human-readable name of the sandbox, e.g. "Termux" or "Debian (proot)".
        sandbox: String,
    },
    /// Pairing with a code worked: keep this secret for next time.
    Paired { secret: String },
    /// Authentication failed. The server closes the link after this.
    AuthFailed { reason: String, pairing_open: bool },
    /// A Linux toplevel appeared and needs an Android window.
    WindowNew {
        win: u32,
        title: String,
        app_id: String,
        /// PNG bytes, base64, if the server could resolve the app's icon.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        icon_png_b64: Option<String>,
    },
    WindowTitle { win: u32, title: String },
    WindowClose { win: u32 },
    /// Part of a new frame: `rects` changed (x, y, w, h); the payload is their RGBA pixels, row
    /// by row, one rect after another, LZ4-compressed (lz4_flex block format with prepended
    /// size). A frame of a new size always covers the whole window. The app answers with
    /// FrameDone; until then no further frame for this window is sent.
    Tiles { win: u32, width: u32, height: u32, rects: Vec<[u32; 4]>, len: u32 },
    /// Show or hide the soft keyboard for a window (text-input focus).
    Keyboard { win: u32, show: bool },
    /// The Linux side set the clipboard.
    Clipboard { text: String },
    /// Ask the app to pin a home screen shortcut for an exported app.
    Export(ExportRequest),
    /// Ask the app to open its folder picker and report the chosen path.
    PickFolder { request: u32 },
    /// Remove an exported program (its shortcut and bundle); answered with ExportResult.
    Unexport { id: String },
    /// Sound: a stream opens, gets PCM payloads, is paused/resumed, and closes.
    AudioOpen { stream: u32, rate: u32, channels: u32, format: AudioFormat },
    AudioData { stream: u32, len: u32 },
    AudioCork { stream: u32, corked: bool },
    AudioClose { stream: u32 },
    /// A bundle (an uncompressed tar stream) for the app to unpack, in chunks.
    BundleBegin { id: String },
    BundleChunk { id: String, len: u32 },
    BundleEnd { id: String, ok: bool },
    Log { level: String, msg: String },
}

// ------------------------------------------------------------------ app -> server

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ToServer {
    /// First message on a link.
    Hello {
        version: u32,
        build: String,
        density: f32,
        nonce: String,
        /// The screen's size in physical pixels, so windows can be sized before they open.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        screen: Option<(u32, u32)>,
    },
    /// Second message: `proof` from an earlier pairing's secret, or `code`, the pairing code the
    /// user read off the terminal.
    Auth {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        proof: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
    },
    /// The frame sent last for this window is on screen (or dropped); send the next one.
    FrameDone { win: u32 },
    /// The window's surface changed size, in physical pixels.
    Resize { win: u32, width: u32, height: u32, density: f32 },
    Touch { win: u32, action: TouchAction, id: i32, x: f32, y: f32, time_ms: u32 },
    Pointer { win: u32, action: PointerAction, x: f32, y: f32, button: u32, time_ms: u32 },
    Scroll { win: u32, dx: f32, dy: f32, time_ms: u32 },
    /// A key, as an Android KeyEvent keycode (the server maps it to evdev).
    Key { win: u32, code: u32, down: bool, time_ms: u32 },
    /// Committed text from the soft keyboard / IME.
    Text { win: u32, text: String },
    Focus { win: u32, focused: bool },
    /// The user closed the Android window (back gesture / recents swipe).
    Close { win: u32 },
    Clipboard { text: String },
    FolderPicked { request: u32, path: Option<String> },
    ExportResult { id: String, ok: bool, message: String },
    ImportResult { id: String, ok: bool, message: String },
    /// Sound flow control: the app has room for this many more bytes of a stream.
    AudioCredit { stream: u32, bytes: u32 },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AudioFormat {
    S16le,
    F32le,
}

impl AudioFormat {
    pub fn bytes(self) -> u32 {
        match self {
            AudioFormat::S16le => 2,
            AudioFormat::F32le => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TouchAction {
    Down,
    Move,
    Up,
    Cancel,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PointerAction {
    Enter,
    Leave,
    Motion,
    Press,
    Release,
}

/// What `lindroids export` hands to the app.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExportRequest {
    /// Stable id, e.g. "org.gnome.Calculator" or "termux.xeyes".
    pub id: String,
    pub name: String,
    /// PNG bytes, base64.
    pub icon_png_b64: String,
    pub launch: LaunchSpec,
}

/// How the app starts an exported program when its shortcut is tapped.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum LaunchSpec {
    /// Ask Termux to run `lindroids run ...` (Termux's RUN_COMMAND service).
    /// Light, but needs Termux installed; Termux is started if it is closed.
    Termux { argv: Vec<String> },
    /// The program was copied into the app as a bundle and runs inside the app under its
    /// bundled proot, with no sandbox app involved.
    Bundle { bundle: String, argv: Vec<String>, env: Vec<(String, String)> },
}

/// `manifest.json` at the top of an exported bundle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BundleManifest {
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub kind: BundleKind,
    /// Extra environment for every launch.
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// Working directory inside the bundle's view of the filesystem.
    #[serde(default)]
    pub workdir: Option<String>,
    /// An X11 program: the bundle carries Xwayland and xwayland-satellite (this path, in the
    /// bundle's view), and every launch gets an X display of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x11_satellite: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BundleKind {
    /// Termux packages: `usr/` is mounted at Termux's prefix, `home/` at its home.
    Termux,
    /// A glibc root filesystem (`root/`), from a proot distro or a flatpak.
    Rootfs {
        /// Bind mounts inside the rootfs: (path relative to the bundle, path inside).
        #[serde(default)]
        binds: Vec<(String, String)>,
    },
}

// ------------------------------------------------------------------ CLI <-> server

/// Requests from the CLI to a running server (local Unix socket).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum CliRequest {
    Status,
    Stop,
    Export(ExportRequest),
    /// Hand the app a bundle: the read end of a pipe carrying an uncompressed tar stream travels
    /// with this line as SCM_RIGHTS.
    ImportBundle { id: String },
    PickFolder,
    /// Remove an exported program from the app.
    Unexport { id: String },
    /// The CLI started a program: windows from this process (or its children) get this name and
    /// icon.
    Launch {
        pid: u32,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        icon_png_b64: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum CliReply {
    Status {
        version: u32,
        build: String,
        app_connected: bool,
        /// Build of the connected app, to tell when it needs updating.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app_build: Option<String>,
        /// Shown in the terminal so the user can pair the app.
        pairing_code: Option<String>,
        windows: Vec<WindowInfo>,
        clients: u32,
    },
    Ok { message: String },
    Folder { path: Option<String> },
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WindowInfo {
    pub win: u32,
    pub title: String,
    pub app_id: String,
    pub width: u32,
    pub height: u32,
}

// ------------------------------------------------------------------ framing

/// Messages that may carry a binary payload say how long it is.
pub trait Payload {
    fn payload_len(&self) -> usize {
        0
    }
}

impl Payload for ToApp {
    fn payload_len(&self) -> usize {
        match self {
            ToApp::Tiles { len, .. } | ToApp::AudioData { len, .. } | ToApp::BundleChunk { len, .. } => *len as usize,
            _ => 0,
        }
    }
}

impl Payload for ToServer {}
impl Payload for CliRequest {}
impl Payload for CliReply {}

pub fn encode<T: Serialize>(msg: &T) -> Vec<u8> {
    let mut v = serde_json::to_vec(msg).expect("protocol types always serialize");
    v.push(b'\n');
    v
}

/// A message followed by its payload. The message's `len` must equal `payload.len()`.
pub fn encode_with<T: Serialize>(msg: &T, payload: &[u8]) -> Vec<u8> {
    let mut v = encode(msg);
    v.extend_from_slice(payload);
    v
}

pub fn decode<'a, T: Deserialize<'a>>(line: &'a [u8]) -> Result<T, serde_json::Error> {
    serde_json::from_slice(line)
}

/// Splits a byte stream into messages and their payloads.
#[derive(Default)]
pub struct Framer {
    buf: Vec<u8>,
    start: usize,
}

impl Framer {
    pub fn push(&mut self, data: &[u8]) {
        if self.start > 0 && self.start == self.buf.len() {
            self.buf.clear();
            self.start = 0;
        } else if self.start > (1 << 20) {
            self.buf.drain(..self.start);
            self.start = 0;
        }
        self.buf.extend_from_slice(data);
    }

    pub fn buffered(&self) -> usize {
        self.buf.len() - self.start
    }

    /// The next complete message, if one is buffered.
    pub fn next<T: for<'a> Deserialize<'a> + Payload>(&mut self) -> Option<Result<(T, Vec<u8>), String>> {
        let rest = &self.buf[self.start..];
        let nl = rest.iter().position(|&b| b == b'\n')?;
        let msg: T = match decode(&rest[..nl]) {
            Ok(m) => m,
            Err(e) => {
                let line = String::from_utf8_lossy(&rest[..nl]).into_owned();
                self.start += nl + 1;
                return Some(Err(format!("{e}: {line}")));
            }
        };
        let n = msg.payload_len();
        if rest.len() < nl + 1 + n {
            return None;
        }
        let payload = rest[nl + 1..nl + 1 + n].to_vec();
        self.start += nl + 1 + n;
        Some(Ok((msg, payload)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_touch() {
        let m = ToServer::Touch { win: 3, action: TouchAction::Down, id: 0, x: 1.5, y: 2.0, time_ms: 9 };
        let line = encode(&m);
        assert_eq!(*line.last().unwrap(), b'\n');
        let back: ToServer = decode(&line[..line.len() - 1]).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn tagged_shape_is_stable() {
        let line = encode(&ToApp::WindowClose { win: 7 });
        assert_eq!(std::str::from_utf8(&line).unwrap(), "{\"t\":\"window_close\",\"win\":7}\n");
    }

    #[test]
    fn framer_splits_payloads_across_reads() {
        let a = encode_with(&ToApp::AudioData { stream: 1, len: 5 }, b"hello");
        let b = encode(&ToApp::WindowClose { win: 2 });
        let all: Vec<u8> = a.iter().chain(b.iter()).copied().collect();
        let mut f = Framer::default();
        let mut got = Vec::new();
        for chunk in all.chunks(3) {
            f.push(chunk);
            while let Some(m) = f.next::<ToApp>() {
                got.push(m.unwrap());
            }
        }
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].1, b"hello");
        assert_eq!(got[1].0, ToApp::WindowClose { win: 2 });
    }

    #[test]
    fn ports_are_stable_and_in_range() {
        assert_eq!(server_tcp_port(10282), 47282);
        assert_eq!(server_tcp_port(1010282), 47282);
        assert!(server_tcp_port(19999) < 57000);
    }
}
