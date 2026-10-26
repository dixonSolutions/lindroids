//! The Lindroids server: a Wayland compositor and a PulseAudio-protocol sound server whose output
//! is the Lindroids Android app.
//!
//! It is a plain Linux program. It runs inside the sandbox next to the Linux programs (Termux, a
//! proot distro...), as `lindroids server`, or inside the app itself for exported bundles. It never
//! calls an Android API: frames go to the app as shared memory over a Unix socket, sound as PCM,
//! and the app does everything Android-side.
//!
//! One thread runs the compositor. It polls the Wayland socket and clients, the app link, the CLI
//! socket, and a wake pipe other threads (sound, clipboard) use to post work.

pub mod applink;
pub mod audio;
mod data;
pub mod frames;
pub mod link;
pub mod icons;
pub mod keys;
mod output;
pub mod render;
mod seat;
mod shell;
pub mod shm;
mod surface;

pub use lindroids_proto::fdpass;

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::net::{Ipv4Addr, TcpListener};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use lindroids_proto::{self as proto, auth, CliReply, CliRequest, ToApp, ToServer};
use link::{Link, Phase};
use log::{info, warn};
use wayland_server::backend::{ClientData, ClientId, DisconnectReason, ObjectId};
use wayland_server::{Display, DisplayHandle, ListeningSocket};

pub use self::surface::Surface;

/// Build identifier shown by `lindroids doctor` and the app.
pub const BUILD: &str = concat!(env!("CARGO_PKG_VERSION"), "+", env!("LINDROIDS_BUILD_ID"));

/// Wrong pairing codes allowed before pairing closes until the server restarts.
const MAX_PAIRING_FAILURES: u32 = 5;

#[derive(Debug, Clone)]
pub struct Config {
    pub runtime_dir: PathBuf,
    pub sandbox: String,
    /// Output scale; None picks one from the screen density.
    pub scale: Option<u32>,
    /// Long-term secret the app proves itself with. None: load or create the per-user one.
    pub secret: Option<String>,
    /// Abstract socket for the app. None: `lindroids.server.<uid>`.
    pub socket_name: Option<String>,
}

impl Config {
    pub fn from_args(args: &[String]) -> Config {
        let mut cfg = Config {
            runtime_dir: std::env::var_os("LINDROIDS_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(default_runtime_dir),
            sandbox: "Linux".into(),
            scale: std::env::var("LINDROIDS_SCALE").ok().and_then(|s| s.parse().ok()),
            secret: None,
            socket_name: None,
        };
        let mut it = args.iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--runtime-dir" => cfg.runtime_dir = it.next().map(PathBuf::from).unwrap_or(cfg.runtime_dir),
                "--sandbox" => cfg.sandbox = it.next().cloned().unwrap_or(cfg.sandbox),
                "--scale" => cfg.scale = it.next().and_then(|s| s.parse().ok()),
                "--secret" => cfg.secret = it.next().cloned(),
                "--socket-name" => cfg.socket_name = it.next().cloned(),
                _ => {}
            }
        }
        cfg
    }
}

pub fn default_runtime_dir() -> PathBuf {
    let tmp = std::env::var_os("TMPDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"));
    tmp.join(format!("lindroids-{}", unsafe { libc::getuid() }))
}

/// Where the long-term pairing secret lives.
pub fn secret_path() -> PathBuf {
    let base = std::env::var_os("LINDROIDS_STATE_DIR").map(PathBuf::from).unwrap_or_else(|| {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"));
        home.join(".local/share/lindroids")
    });
    base.join("secret")
}

pub fn load_or_create_secret() -> String {
    let path = secret_path();
    if let Ok(s) = std::fs::read_to_string(&path) {
        let s = s.trim().to_string();
        if s.len() >= 32 {
            return s;
        }
    }
    let s = shm::random_string(43, b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_");
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, &s);
    let _ = std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600));
    s
}

// ------------------------------------------------------------------ cross-thread handoff

/// Work other threads hand to the compositor thread.
pub enum Inbox {
    ToApp(ToApp),
    /// A message with a binary payload (sound).
    WithPayload(ToApp, Vec<u8>),
    /// Wake the loop (a bundle chunk is ready).
    Wake,
}

pub struct Shared {
    inbox: Mutex<Vec<Inbox>>,
    wake_w: OwnedFd,
    wake_r: OwnedFd,
}

impl Shared {
    fn new() -> Arc<Shared> {
        let mut fds = [0; 2];
        unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) };
        Arc::new(Shared {
            inbox: Mutex::new(Vec::new()),
            wake_r: unsafe { OwnedFd::from_raw_fd(fds[0]) },
            wake_w: unsafe { OwnedFd::from_raw_fd(fds[1]) },
        })
    }

    pub fn post(&self, m: Inbox) {
        self.inbox.lock().unwrap().push(m);
        unsafe { libc::write(self.wake_w.as_raw_fd(), b"x".as_ptr() as *const _, 1) };
    }
}

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

pub fn shared() -> &'static Arc<Shared> {
    SHARED.get_or_init(Shared::new)
}

// ------------------------------------------------------------------ state

pub struct ClientState;
impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

/// A Linux toplevel shown in an Android window.
pub struct Window {
    pub id: u32,
    pub surface: ObjectId,
    pub client: ClientId,
    pub toplevel: wayland_protocols::xdg::shell::server::xdg_toplevel::XdgToplevel,
    pub xdg_surface: wayland_protocols::xdg::shell::server::xdg_surface::XdgSurface,
    pub title: String,
    pub app_id: String,
    /// Physical size of the Android surface, once known.
    pub phys: Option<(u32, u32)>,
    /// Logical size last sent in a configure.
    pub configured: (i32, i32),
    pub activated: bool,
    pub announced: bool,
    pub mapped: bool,
    pub dirty: bool,
    /// The last frame sent (what the app shows), and its size: new frames send only changes.
    pub shown: Vec<u32>,
    pub shown_size: (u32, u32),
    pub canvas: Vec<u32>,
    /// A frame is on its way to the screen; the next waits for FrameDone.
    pub in_flight: bool,
    /// Surfaces drawn in frames the app has not shown yet: their frame callbacks wait for it.
    pub waiting: Vec<ObjectId>,
    /// Popups above this window, bottom to top (their wl_surface ids).
    pub popups: Vec<ObjectId>,
    pub fullscreen: bool,
}

/// A bundle on its way from the CLI to the app.
struct BundleXfer {
    id: String,
    rx: std::sync::mpsc::Receiver<Option<Vec<u8>>>,
    started: bool,
}

struct CliConn {
    stream: UnixStream,
    buf: Vec<u8>,
    /// Waiting for the app to answer (export, import, folder picker).
    waiting: Option<Waiting>,
}

#[derive(Debug, Clone, PartialEq)]
enum Waiting {
    Export(String),
    Folder(u32),
    Import(String),
}

pub struct State {
    pub dh: DisplayHandle,
    pub cfg: Config,
    pub start: Instant,
    pub serial: u32,
    pub surfaces: HashMap<ObjectId, Surface>,
    pub windows: BTreeMap<u32, Window>,
    next_win: u32,
    pub scale: u32,
    pub link: Option<Link>,
    /// Links that have not authenticated yet.
    pending: Vec<Link>,
    bundles: Vec<BundleXfer>,
    secret: String,
    pub pairing_code: String,
    pairing_failures: u32,
    pub seat: seat::Seat,
    pub outputs: output::Outputs,
    pub data: data::DataState,
    /// Popup bookkeeping, by the popup's wl_surface id.
    pub popups: HashMap<ObjectId, shell::Popup>,
    /// Surfaces not shown anywhere whose frame callbacks are due.
    pub frame_due: Vec<ObjectId>,
    pub running: bool,
    clis: Vec<CliConn>,
    next_request: u32,
    clients: u32,
}

impl State {
    pub fn next_serial(&mut self) -> u32 {
        self.serial = self.serial.wrapping_add(1);
        self.serial
    }

    pub fn now_ms(&self) -> u32 {
        self.start.elapsed().as_millis() as u32
    }

    pub fn send_app(&mut self, msg: &ToApp) {
        self.send_app_with(msg, &[]);
    }

    pub fn send_app_with(&mut self, msg: &ToApp, payload: &[u8]) {
        let Some(link) = &mut self.link else { return };
        link.send_with(msg, payload);
        if link.broken {
            warn!("app link write failed");
            self.drop_link();
        }
    }

    fn drop_link(&mut self) {
        self.link = None;
        for w in self.windows.values_mut() {
            w.announced = false;
            w.in_flight = false;
            w.shown.clear();
            w.shown_size = (0, 0);
            w.dirty = true;
            self.frame_due.extend(w.waiting.drain(..));
        }
        for b in self.bundles.drain(..) {
            self.clis.iter_mut().filter(|c| c.waiting == Some(Waiting::Import(b.id.clone()))).for_each(|c| {
                let _ = c.stream.write_all(&proto::encode(&CliReply::Error { message: "the app went away".into() }));
                c.waiting = None;
            });
        }
    }

    /// The window that a surface (or the popup/subsurface tree it belongs to) is drawn in.
    pub fn window_of(&self, surface: &ObjectId) -> Option<u32> {
        let mut cur = surface.clone();
        for _ in 0..64 {
            let s = self.surfaces.get(&cur)?;
            match &s.role {
                surface::Role::Toplevel(win) => return Some(*win),
                surface::Role::Popup => return self.popups.get(&cur).map(|p| p.win),
                surface::Role::Subsurface => cur = s.sub.as_ref()?.parent.clone(),
                _ => return None,
            }
        }
        None
    }

    pub fn mark_dirty(&mut self, surface: &ObjectId) {
        let shown = self.window_of(surface).and_then(|win| self.windows.get_mut(&win)).map(|w| {
            w.dirty = true;
            w.announced && w.phys.is_some()
        });
        if shown != Some(true) {
            // Not on screen: let the client carry on drawing.
            self.frame_due.push(surface.clone());
        }
    }

    pub fn new_window_id(&mut self) -> u32 {
        self.next_win += 1;
        self.next_win
    }

    fn announce(&mut self, win: u32) {
        let Some(w) = self.windows.get(&win) else { return };
        if w.announced || !w.mapped || self.link.is_none() {
            return;
        }
        let title = if !w.title.is_empty() {
            w.title.clone()
        } else {
            icons::name_for_window(&self.dh, &w.client).unwrap_or_else(|| w.app_id.clone())
        };
        let msg = ToApp::WindowNew {
            win,
            title,
            app_id: w.app_id.clone(),
            icon_png_b64: icons::icon_for_window(&self.dh, &w.client, &w.app_id),
        };
        if let Some(w) = self.windows.get_mut(&win) {
            w.announced = true;
            w.dirty = true;
        }
        self.send_app(&msg);
    }

    pub fn window_mapped(&mut self, win: u32) {
        if let Some(w) = self.windows.get_mut(&win) {
            if !w.mapped {
                w.mapped = true;
                info!("window {win} mapped: {:?} ({})", w.title, w.app_id);
            }
        }
        self.announce(win);
    }

    pub fn window_gone(&mut self, win: u32) {
        if let Some(w) = self.windows.remove(&win) {
            if w.announced {
                self.send_app(&ToApp::WindowClose { win });
            }
            if self.seat.focus_win == Some(win) {
                self.seat.focus_win = None;
            }
            self.frame_due.extend(w.waiting);
        }
    }

    /// Send a configure for a window if its size or state changed.
    pub fn configure_window(&mut self, win: u32, force: bool) {
        let serial = self.next_serial();
        let scale = self.scale;
        let Some(w) = self.windows.get_mut(&win) else { return };
        let size = match w.phys {
            Some(p) => ((p.0 / scale) as i32, (p.1 / scale) as i32),
            None => (0, 0),
        };
        if !force && size == w.configured {
            return;
        }
        w.configured = size;
        shell::send_toplevel_configure(w, size, serial);
    }

    /// Handshake on a new link. Returns the link back while it is still pending.
    fn handshake(&mut self, mut link: Link, msg: ToServer) -> Option<Link> {
        match (&link.phase, msg) {
            (Phase::Hello, ToServer::Hello { version, build, density, nonce: app_nonce }) => {
                let nonce = shm::random_string(32, b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789");
                let proof = auth::server_proof(&self.secret, &app_nonce);
                link.send(&ToApp::Challenge { nonce: nonce.clone(), proof });
                link.phase = Phase::Auth { nonce, app_nonce, density, version, build };
                Some(link)
            }
            (Phase::Auth { nonce, density, version, build, .. }, ToServer::Auth { proof, code }) => {
                let (density, version, build) = (*density, *version, build.clone());
                let by_secret = proof.as_deref().map_or(false, |p| auth::same(p, &auth::app_proof(&self.secret, nonce)));
                let by_code = !by_secret
                    && self.pairing_failures < MAX_PAIRING_FAILURES
                    && code.as_deref().map(|c| c.trim()) == Some(self.pairing_code.as_str());
                if !by_secret && !by_code {
                    if code.is_some() {
                        self.pairing_failures += 1;
                    }
                    warn!("app at {} failed to authenticate", link.peer);
                    let reason = if code.is_some() { "wrong pairing code" } else { "not paired" };
                    link.send(&ToApp::AuthFailed { reason: reason.into(), pairing_open: self.pairing_failures < MAX_PAIRING_FAILURES });
                    // Let the answer drain; the link closes when the app hangs up.
                    let mut l = link;
                    l.flush();
                    return None;
                }
                if by_code {
                    link.send(&ToApp::Paired { secret: self.secret.clone() });
                    // A code works once.
                    self.pairing_code = new_pairing_code();
                    write_pairing_code(&self.cfg, &self.pairing_code);
                }
                info!("app linked ({}, protocol {version}, build {build}, density {density})", link.peer);
                if version != proto::PROTOCOL_VERSION {
                    warn!("app speaks protocol {version}, server {}", proto::PROTOCOL_VERSION);
                }
                link.density = density;
                link.phase = Phase::Ready;
                self.drop_link();
                self.link = Some(link);
                let hello = ToApp::Hello { version: proto::PROTOCOL_VERSION, build: BUILD.into(), sandbox: self.cfg.sandbox.clone() };
                self.send_app(&hello);
                if self.cfg.scale.is_none() {
                    let s = density.round().clamp(1.0, 4.0) as u32;
                    if s != self.scale {
                        self.scale = s;
                        output::scale_changed(self);
                    }
                }
                let wins: Vec<u32> = self.windows.keys().copied().collect();
                for w in wins {
                    self.announce(w);
                }
                None
            }
            (_, other) => {
                warn!("unexpected message during the handshake: {other:?}");
                None
            }
        }
    }

    fn handle_app_message(&mut self, msg: ToServer) {
        match msg {
            ToServer::Hello { .. } | ToServer::Auth { .. } | ToServer::AudioCredit { .. } => {}
            ToServer::Resize { win, width, height, .. } => {
                if let Some(w) = self.windows.get_mut(&win) {
                    w.phys = Some((width, height));
                    w.dirty = true;
                }
                output::screen_size_hint(self, width, height);
                self.configure_window(win, false);
            }
            ToServer::FrameDone { win } => {
                let mut done = Vec::new();
                if let Some(w) = self.windows.get_mut(&win) {
                    w.in_flight = false;
                    done = std::mem::take(&mut w.waiting);
                }
                let t = self.now_ms();
                let mut n = 0;
                for id in done {
                    if let Some(s) = self.surfaces.get_mut(&id) {
                        for cb in s.frame_cbs.drain(..) {
                            cb.done(t);
                            n += 1;
                        }
                    }
                }
                log::debug!("frame done win {win}: {n} callbacks");
            }
            ToServer::Close { win } => {
                if let Some(w) = self.windows.get(&win) {
                    w.toplevel.close();
                }
            }
            ToServer::Focus { win, focused } => seat::window_focus(self, win, focused),
            ToServer::Touch { win, action, id, x, y, time_ms } => seat::touch(self, win, action, id, x, y, time_ms),
            ToServer::Pointer { win, action, x, y, button, time_ms } => seat::pointer(self, win, action, x, y, button, time_ms),
            ToServer::Scroll { win, dx, dy, time_ms } => seat::scroll(self, win, dx, dy, time_ms),
            ToServer::Key { win, code, down, time_ms } => seat::key(self, win, code, down, time_ms),
            ToServer::Text { win, text } => seat::text(self, win, &text),
            ToServer::Clipboard { text } => data::android_clipboard(self, text),
            ToServer::FolderPicked { request, path } => self.reply_cli(&Waiting::Folder(request), CliReply::Folder { path }),
            ToServer::ExportResult { id, ok, message } => {
                let r = if ok { CliReply::Ok { message } } else { CliReply::Error { message } };
                self.reply_cli(&Waiting::Export(id), r);
            }
            ToServer::ImportResult { id, ok, message } => {
                let r = if ok { CliReply::Ok { message } } else { CliReply::Error { message } };
                self.reply_cli(&Waiting::Import(id), r);
            }
        }
    }

    fn reply_cli(&mut self, what: &Waiting, reply: CliReply) {
        let mut idx = self.clis.iter().position(|c| c.waiting.as_ref() == Some(what));
        // An export the app failed early on (empty id) answers the oldest waiter.
        if idx.is_none() {
            if let Waiting::Export(_) = what {
                idx = self.clis.iter().position(|c| matches!(c.waiting, Some(Waiting::Export(_))));
            }
        }
        if let Some(i) = idx {
            let mut c = self.clis.remove(i);
            c.stream.set_nonblocking(false).ok();
            let _ = c.stream.write_all(&proto::encode(&reply));
        }
    }

    fn handle_cli(&mut self, idx: usize, req: CliRequest, fds: Vec<OwnedFd>) {
        let no_app = || CliReply::Error { message: "the Lindroids app is not connected".into() };
        let reply = match req {
            CliRequest::Status => Some(CliReply::Status {
                version: proto::PROTOCOL_VERSION,
                build: BUILD.into(),
                app_connected: self.link.is_some(),
                pairing_code: (self.pairing_failures < MAX_PAIRING_FAILURES).then(|| self.pairing_code.clone()),
                windows: self
                    .windows
                    .values()
                    .map(|w| proto::WindowInfo {
                        win: w.id,
                        title: w.title.clone(),
                        app_id: w.app_id.clone(),
                        width: w.phys.map_or(0, |p| p.0),
                        height: w.phys.map_or(0, |p| p.1),
                    })
                    .collect(),
                clients: self.clients,
            }),
            CliRequest::Stop => {
                self.running = false;
                Some(CliReply::Ok { message: "stopping".into() })
            }
            CliRequest::Launch { pid, name, icon_png_b64 } => {
                icons::register_launch(pid, name, icon_png_b64);
                Some(CliReply::Ok { message: "registered".into() })
            }
            CliRequest::Export(req) => {
                if self.link.is_none() {
                    Some(no_app())
                } else {
                    let id = req.id.clone();
                    self.send_app(&ToApp::Export(req));
                    self.clis[idx].waiting = Some(Waiting::Export(id));
                    None
                }
            }
            CliRequest::ImportBundle { id } => match fds.into_iter().next() {
                _ if self.link.is_none() => Some(no_app()),
                None => Some(CliReply::Error { message: "no bundle stream came with the request".into() }),
                Some(fd) => {
                    // A thread reads the pipe; the loop forwards chunks as the link drains.
                    let (tx, rx) = std::sync::mpsc::sync_channel::<Option<Vec<u8>>>(4);
                    std::thread::spawn(move || {
                        let mut f = std::fs::File::from(fd);
                        loop {
                            let mut buf = vec![0u8; 256 * 1024];
                            match std::io::Read::read(&mut f, &mut buf) {
                                Ok(0) | Err(_) => {
                                    let _ = tx.send(None);
                                    shared().post(Inbox::Wake);
                                    break;
                                }
                                Ok(n) => {
                                    buf.truncate(n);
                                    if tx.send(Some(buf)).is_err() {
                                        break;
                                    }
                                    shared().post(Inbox::Wake);
                                }
                            }
                        }
                    });
                    self.bundles.push(BundleXfer { id: id.clone(), rx, started: false });
                    self.clis[idx].waiting = Some(Waiting::Import(id));
                    None
                }
            },
            CliRequest::PickFolder => {
                if self.link.is_none() {
                    Some(no_app())
                } else {
                    self.next_request += 1;
                    let r = self.next_request;
                    self.send_app(&ToApp::PickFolder { request: r });
                    self.clis[idx].waiting = Some(Waiting::Folder(r));
                    None
                }
            }
        };
        if let Some(r) = reply {
            let c = &mut self.clis[idx];
            c.stream.set_nonblocking(false).ok();
            let _ = c.stream.write_all(&proto::encode(&r));
            c.stream.set_nonblocking(true).ok();
        }
    }
}

fn new_pairing_code() -> String {
    shm::random_string(6, b"0123456789")
}

fn write_pairing_code(cfg: &Config, code: &str) {
    let p = cfg.runtime_dir.join("pairing-code");
    let _ = std::fs::write(&p, code);
    let _ = std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o600));
}

/// Listen on an abstract Unix socket (no file, so the app can reach it from its own sandbox).
pub fn listen_abstract(name: &str) -> std::io::Result<UnixListener> {
    unsafe {
        let fd = libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK, 0);
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut addr: libc::sockaddr_un = std::mem::zeroed();
        addr.sun_family = libc::AF_UNIX as _;
        let bytes = name.as_bytes();
        if bytes.len() + 1 > addr.sun_path.len() {
            libc::close(fd);
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "socket name too long"));
        }
        for (i, b) in bytes.iter().enumerate() {
            addr.sun_path[i + 1] = *b as _;
        }
        let len = (std::mem::size_of::<libc::sa_family_t>() + 1 + bytes.len()) as libc::socklen_t;
        if libc::bind(fd, &addr as *const _ as *const libc::sockaddr, len) != 0 || libc::listen(fd, 8) != 0 {
            let e = std::io::Error::last_os_error();
            libc::close(fd);
            return Err(e);
        }
        Ok(UnixListener::from_raw_fd(fd))
    }
}

/// Connect to an abstract Unix socket.
pub fn connect_abstract(name: &str) -> std::io::Result<UnixStream> {
    unsafe {
        let fd = libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0);
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut addr: libc::sockaddr_un = std::mem::zeroed();
        addr.sun_family = libc::AF_UNIX as _;
        let bytes = name.as_bytes();
        for (i, b) in bytes.iter().enumerate().take(addr.sun_path.len() - 1) {
            addr.sun_path[i + 1] = *b as _;
        }
        let len = (std::mem::size_of::<libc::sa_family_t>() + 1 + bytes.len()) as libc::socklen_t;
        if libc::connect(fd, &addr as *const _ as *const libc::sockaddr, len) != 0 {
            let e = std::io::Error::last_os_error();
            libc::close(fd);
            return Err(e);
        }
        Ok(UnixStream::from_raw_fd(fd))
    }
}

pub fn peer_uid(s: &UnixStream) -> u32 {
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    unsafe { libc::getsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cred as *mut _ as *mut _, &mut len) };
    cred.uid
}

// ------------------------------------------------------------------ main loop

pub fn run(cfg: Config) -> i32 {
    let shared = shared().clone();
    if let Err(e) = std::fs::create_dir_all(&cfg.runtime_dir) {
        eprintln!("lindroids: cannot create {}: {e}", cfg.runtime_dir.display());
        return 1;
    }
    let _ = std::fs::set_permissions(&cfg.runtime_dir, std::os::unix::fs::PermissionsExt::from_mode(0o700));
    std::env::set_var("LINDROIDS_RUNTIME_DIR", &cfg.runtime_dir);

    let wl_path = cfg.runtime_dir.join("wayland-0");
    let _ = std::fs::remove_file(&wl_path);
    let _ = std::fs::remove_file(cfg.runtime_dir.join("wayland-0.lock"));
    let socket = match ListeningSocket::bind_absolute(wl_path.clone()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("lindroids: cannot listen on {}: {e}", wl_path.display());
            return 1;
        }
    };
    let cli_path = cfg.runtime_dir.join("lindroids.sock");
    let _ = std::fs::remove_file(&cli_path);
    let cli_listener = match UnixListener::bind(&cli_path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("lindroids: cannot listen on {}: {e}", cli_path.display());
            return 1;
        }
    };
    cli_listener.set_nonblocking(true).ok();
    let uid = unsafe { libc::getuid() };
    let app_name = cfg.socket_name.clone().unwrap_or_else(|| proto::server_socket_name(uid));
    // The abstract socket is the fast path (same uid, or devices whose policy allows it); loopback
    // TCP works between any two apps.
    let app_listener = match listen_abstract(&app_name) {
        Ok(l) => Some(l),
        Err(e) => {
            warn!("no abstract socket @{app_name}: {e}");
            None
        }
    };
    let port = proto::server_tcp_port(uid);
    let tcp_listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
        Ok(l) => {
            l.set_nonblocking(true).ok();
            Some(l)
        }
        Err(e) => {
            warn!("no loopback port {port}: {e}");
            None
        }
    };
    if app_listener.is_none() && tcp_listener.is_none() {
        eprintln!("lindroids: the app has no way to reach this server (is another one running?)");
        return 1;
    }

    if let Err(e) = audio::start(&cfg.runtime_dir) {
        warn!("sound is off: {e}");
    }

    let mut display: Display<State> = Display::new().expect("wayland display");
    let dh = display.handle();
    let secret = cfg.secret.clone().unwrap_or_else(load_or_create_secret);
    let pairing_code = new_pairing_code();
    write_pairing_code(&cfg, &pairing_code);
    let scale = cfg.scale.unwrap_or(1);
    let mut state = State {
        dh: dh.clone(),
        cfg: cfg.clone(),
        start: Instant::now(),
        serial: 0,
        surfaces: HashMap::new(),
        windows: BTreeMap::new(),
        next_win: 0,
        scale,
        link: None,
        pending: Vec::new(),
        bundles: Vec::new(),
        secret,
        pairing_code,
        pairing_failures: 0,
        seat: seat::Seat::default(),
        outputs: output::Outputs::default(),
        data: data::DataState::default(),
        popups: HashMap::new(),
        frame_due: Vec::new(),
        running: true,
        clis: Vec::new(),
        next_request: 0,
        clients: 0,
    };
    surface::register_globals(&dh);
    shell::register_globals(&dh);
    seat::register_globals(&dh);
    output::register_globals(&dh);
    data::register_globals(&dh);

    std::fs::write(cfg.runtime_dir.join("server.pid"), std::process::id().to_string()).ok();
    info!("listening on {}, @{app_name} and 127.0.0.1:{port} (sandbox {:?}, scale {scale})", wl_path.display(), cfg.sandbox);
    // The CLI waits for this line.
    println!("lindroids-server ready {}", wl_path.display());
    let _ = std::io::stdout().flush();

    let mut last_frame_flush = Instant::now();
    while state.running {
        let mut fds: Vec<libc::pollfd> = Vec::with_capacity(8 + state.clis.len() + state.pending.len());
        let pfd = |fd: RawFd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        fds.push(pfd(socket.as_raw_fd()));
        fds.push(pfd(display.backend().poll_fd().as_raw_fd()));
        fds.push(pfd(shared.wake_r.as_raw_fd()));
        fds.push(pfd(cli_listener.as_raw_fd()));
        fds.push(pfd(app_listener.as_ref().map_or(-1, |l| l.as_raw_fd())));
        fds.push(pfd(tcp_listener.as_ref().map_or(-1, |l| l.as_raw_fd())));
        let link_idx = fds.len();
        let mut lp = pfd(state.link.as_ref().map_or(-1, |l| l.fd()));
        if state.link.as_ref().map_or(false, |l| l.wants_write()) {
            lp.events |= libc::POLLOUT;
        }
        fds.push(lp);
        let pending_base = fds.len();
        for p in &state.pending {
            fds.push(pfd(p.fd()));
        }
        let cli_base = fds.len();
        for c in &state.clis {
            fds.push(pfd(c.stream.as_raw_fd()));
        }
        let timeout = if state.frame_due.is_empty() { 1000 } else { 16 };
        unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, timeout) };

        if fds[0].revents != 0 {
            while let Ok(Some(stream)) = socket.accept() {
                match display.handle().insert_client(stream, Arc::new(ClientState)) {
                    Ok(_) => state.clients += 1,
                    Err(e) => warn!("client rejected: {e}"),
                }
            }
        }
        if fds[2].revents != 0 {
            let mut sink = [0u8; 64];
            while unsafe { libc::read(shared.wake_r.as_raw_fd(), sink.as_mut_ptr() as *mut _, 64) } > 0 {}
            let items: Vec<Inbox> = std::mem::take(&mut *shared.inbox.lock().unwrap());
            for item in items {
                match item {
                    Inbox::ToApp(m) => state.send_app(&m),
                    Inbox::WithPayload(m, p) => state.send_app_with(&m, &p),
                    Inbox::Wake => {}
                }
            }
        }
        if fds[3].revents != 0 {
            while let Ok((stream, _)) = cli_listener.accept() {
                stream.set_nonblocking(true).ok();
                state.clis.push(CliConn { stream, buf: Vec::new(), waiting: None });
            }
        }
        if fds[4].revents != 0 {
            if let Some(l) = &app_listener {
                while let Ok((stream, _)) = l.accept() {
                    stream.set_nonblocking(true).ok();
                    let peer = format!("uid {}", peer_uid(&stream));
                    state.pending.push(Link::new(Box::new(stream), peer));
                }
            }
        }
        if fds[5].revents != 0 {
            if let Some(l) = &tcp_listener {
                while let Ok((stream, addr)) = l.accept() {
                    stream.set_nonblocking(true).ok();
                    stream.set_nodelay(true).ok();
                    state.pending.push(Link::new(Box::new(stream), format!("tcp {addr}")));
                }
            }
        }
        // Links in the handshake (iterate backwards: entries get removed).
        let mut i = state.pending.len();
        while i > 0 {
            i -= 1;
            let ready = fds.get(pending_base + i).map_or(false, |p| p.revents != 0);
            if !ready {
                if state.pending[i].since.elapsed() > Duration::from_secs(15) {
                    state.pending.remove(i);
                }
                continue;
            }
            let alive = state.pending[i].read();
            match state.pending[i].next() {
                Some(Ok(m)) => {
                    let link = state.pending.remove(i);
                    if let Some(l) = state.handshake(link, m) {
                        state.pending.push(l);
                    }
                }
                Some(Err(e)) => {
                    warn!("bad handshake: {e}");
                    state.pending.remove(i);
                }
                None if !alive => {
                    state.pending.remove(i);
                }
                None => {}
            }
        }
        if fds[link_idx].revents != 0 && state.link.is_some() {
            let mut msgs = Vec::new();
            let alive = {
                let l = state.link.as_mut().unwrap();
                if fds[link_idx].revents & libc::POLLOUT != 0 {
                    l.flush();
                }
                let alive = l.read();
                while let Some(m) = l.next() {
                    msgs.push(m);
                }
                alive && !l.broken
            };
            for m in msgs {
                match m {
                    Ok(m) => state.handle_app_message(m),
                    Err(e) => warn!("bad message from app: {e}"),
                }
            }
            if !alive {
                info!("app link closed");
                state.drop_link();
            }
        }
        let mut i = state.clis.len();
        while i > 0 {
            i -= 1;
            if fds.get(cli_base + i).map_or(true, |p| p.revents == 0) {
                continue;
            }
            let mut lines = Vec::new();
            let mut fd_list = Vec::new();
            let closed = {
                let c = &mut state.clis[i];
                fdpass::read_lines_with_fds(&c.stream, &mut c.buf, &mut lines, &mut fd_list)
            };
            for line in lines {
                match proto::decode::<CliRequest>(&line) {
                    Ok(req) => state.handle_cli(i, req, std::mem::take(&mut fd_list)),
                    Err(e) => {
                        let _ = state.clis[i].stream.write_all(&proto::encode(&CliReply::Error { message: e.to_string() }));
                    }
                }
                if i >= state.clis.len() {
                    break;
                }
            }
            if closed && i < state.clis.len() && state.clis[i].waiting.is_none() {
                state.clis.remove(i);
            }
        }

        if let Err(e) = display.dispatch_clients(&mut state) {
            warn!("dispatch: {e}");
        }
        forward_bundles(&mut state);
        render_dirty(&mut state);
        if !state.frame_due.is_empty() && last_frame_flush.elapsed() >= Duration::from_millis(16) {
            last_frame_flush = Instant::now();
            let due = std::mem::take(&mut state.frame_due);
            let t = state.now_ms();
            for id in due {
                if let Some(s) = state.surfaces.get_mut(&id) {
                    for cb in s.frame_cbs.drain(..) {
                        cb.done(t);
                    }
                }
            }
        }
        let _ = display.flush_clients();
    }
    let _ = std::fs::remove_file(&wl_path);
    let _ = std::fs::remove_file(&cli_path);
    let _ = std::fs::remove_file(cfg.runtime_dir.join("server.pid"));
    0
}

/// Move bundle chunks onto the link while it keeps up.
fn forward_bundles(state: &mut State) {
    while let Some(b) = state.bundles.first_mut() {
        let Some(link) = &mut state.link else { return };
        if link.pending() > (2 << 20) {
            return;
        }
        if !b.started {
            b.started = true;
            let id = b.id.clone();
            link.send(&ToApp::BundleBegin { id });
        }
        match b.rx.try_recv() {
            Ok(Some(chunk)) => {
                let id = b.id.clone();
                link.send_with(&ToApp::BundleChunk { id, len: chunk.len() as u32 }, &chunk);
            }
            Ok(None) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                let id = b.id.clone();
                link.send(&ToApp::BundleEnd { id, ok: true });
                state.bundles.remove(0);
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
        }
    }
}

/// Compose every window that changed and send what differs from what the app shows.
fn render_dirty(state: &mut State) {
    if state.link.as_ref().map_or(true, |l| l.pending() > (8 << 20)) {
        return;
    }
    let wins: Vec<u32> = state
        .windows
        .iter()
        .filter(|(_, w)| w.dirty && w.announced && !w.in_flight && w.phys.is_some())
        .map(|(k, _)| *k)
        .collect();
    for win in wins {
        let layers = shell::layers_for_window(state, win);
        let Some(w) = state.windows.get_mut(&win) else { continue };
        let (pw, ph) = w.phys.unwrap();
        let n = (pw * ph) as usize;
        w.canvas.resize(n, 0);
        render::compose(&mut w.canvas, pw, ph, pw as usize, &layers.placed);
        w.dirty = false;
        let rects = if w.shown_size != (pw, ph) || w.shown.len() != n {
            vec![[0, 0, pw, ph]]
        } else {
            frames::diff(&w.shown, &w.canvas, pw, ph)
        };
        if rects.is_empty() {
            // Nothing visible changed: the client may draw again right away.
            state.frame_due.extend(layers.surfaces);
            continue;
        }
        let payload = frames::pack(&w.canvas, pw, &rects);
        std::mem::swap(&mut w.shown, &mut w.canvas);
        w.shown_size = (pw, ph);
        w.in_flight = true;
        w.waiting.extend(layers.surfaces);
        log::debug!("frame win {win}: {} rects, {} bytes", rects.len(), payload.len());
        let msg = ToApp::Tiles { win, width: pw, height: ph, rects, len: payload.len() as u32 };
        state.send_app_with(&msg, &payload);
    }
}
