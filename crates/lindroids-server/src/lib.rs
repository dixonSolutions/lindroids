//! The Lindroids server: a Wayland compositor and a PulseAudio-protocol sound server whose output
//! is a display on the link: the phone's web browser (`web.rs`), or the older Lindroids app.
//!
//! It is a plain Linux program. It runs inside the sandbox next to the Linux programs (Termux, a
//! proot distro...), as `lindroids server`. It never calls an Android API: changed tiles of each
//! window and PCM go over a loopback link, and the display does the rest.
//!
//! One thread runs the compositor. It polls the Wayland socket and clients, the display link, the
//! CLI socket, and a wake pipe other threads (sound, clipboard) use to post work.

pub mod android;
pub mod applink;
pub mod audio;
mod data;
mod extras;
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
pub mod portal;
pub mod web;
pub mod x11;

pub use lindroids_proto::fdpass;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::net::{Ipv4Addr, TcpListener};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use lindroids_proto::{self as proto, auth, CliReply, CliRequest, ToApp, ToServer};
use link::{Link, Phase};
use log::{info, warn};
use wayland_server::backend::{ClientData, ClientId, DisconnectReason, ObjectId};
use wayland_server::{Display, DisplayHandle, ListeningSocket};

pub use self::surface::Surface;

/// Build identifier shown by `lindroids doctor` and the app.
pub const BUILD: &str = concat!(env!("LINDROIDS_VERSION"), "+", env!("LINDROIDS_BUILD_ID"));

/// Wrong pairing codes allowed before pairing closes until the server restarts.
const MAX_PAIRING_FAILURES: u32 = 5;

#[derive(Debug, Clone)]
pub struct Config {
    pub runtime_dir: PathBuf,
    pub sandbox: String,
    /// Output scale; None picks one from the screen density.
    pub scale: Option<u32>,
    /// Abstract socket for the app. None: `lindroids.server.<uid>`.
    pub socket_name: Option<String>,
}

impl Config {
    pub fn from_args(args: &[String]) -> Config {
        let mut cfg = Config {
            runtime_dir: std::env::var_os("LINDROIDS_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(default_runtime_dir),
            sandbox: "Linux".into(),
            scale: std::env::var("LINDROIDS_SCALE").ok().and_then(|s| s.parse().ok()),
            socket_name: None,
        };
        let mut it = args.iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--runtime-dir" => cfg.runtime_dir = it.next().map(PathBuf::from).unwrap_or(cfg.runtime_dir),
                "--sandbox" => cfg.sandbox = it.next().cloned().unwrap_or(cfg.sandbox),
                "--scale" => cfg.scale = it.next().and_then(|s| s.parse().ok()),
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

/// Where lindroids keeps what lasts: the relay's key, paired displays, exported programs.
pub fn state_dir() -> PathBuf {
    std::env::var_os("LINDROIDS_STATE_DIR").map(PathBuf::from).unwrap_or_else(|| {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"));
        home.join(".local/share/lindroids")
    })
}


/// An exported program's record (`lindroids export`), if `id` is one.
pub fn exported_path(id: &str) -> Option<PathBuf> {
    proto::valid_export_id(id).then(|| state_dir().join("apps").join(format!("{id}.json")))
}

pub fn load_exported(id: &str) -> Option<proto::Exported> {
    let p = exported_path(id)?;
    std::fs::read(p).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

/// Writes a file only this user can read, creating its directory.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("new");
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
    f.write_all(bytes)?;
    drop(f);
    std::fs::rename(&tmp, path)
}

/// The label of the pages' origin, `http://<label>.localhost:<port>`: 128 random bits, made
/// once. An app that takes the port while the server is down can serve pages, but not in this
/// origin, so not with what pages keep (see docs/DISPLAY.md, "The origin").
pub fn origin_label() -> String {
    let path = state_dir().join("origin");
    if let Ok(s) = std::fs::read_to_string(&path) {
        let s = s.trim();
        if s.len() >= 26 && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()) {
            return s.to_string();
        }
    }
    // Base32 (lowercase, so it survives DNS case folding): 26 characters, 130 bits.
    let mut bytes = [0u8; 26];
    let _ = proto::auth::random_bytes(&mut bytes);
    let label: String = bytes.iter().map(|b| b"abcdefghijklmnopqrstuvwxyz234567"[(b & 31) as usize] as char).collect();
    let _ = write_private(&path, label.as_bytes());
    // Whoever wrote first wins (two processes starting at once).
    std::fs::read_to_string(&path).map(|s| s.trim().to_string()).unwrap_or(label)
}

/// The pages' origin.
pub fn page_origin(port: u16) -> String {
    format!("http://{}.localhost:{port}", origin_label())
}

/// The origin this server serves pages in.
fn served_origin() -> String {
    format!("http://{}.localhost:{}", web::origin_label(), proto::server_tcp_port(proto::real_uid()))
}

/// Tickets handed to the CLI for page URLs: (ticket, the run it is for, good until).
static TICKETS: Mutex<Vec<(String, Option<u32>, Instant)>> = Mutex::new(Vec::new());

/// The tickets still good (their run still running).
fn live_tickets() -> std::sync::MutexGuard<'static, Vec<(String, Option<u32>, Instant)>> {
    let mut t = TICKETS.lock().unwrap();
    t.retain(|(_, r, until)| *until > Instant::now() && r.map_or(true, |p| Path::new(&format!("/proc/{p}")).exists()));
    t
}

/// The run a process belongs to, for what it shows outside its window (an Android notification):
/// the URL of the run's window (with the run's ticket, as `lindroids run` opened it), and the
/// icon and name `lindroids run` gave it.
pub(crate) fn run_of_pid(pid: u32) -> Option<(String, Option<String>, String)> {
    let run = icons::launch_of_pid(pid)?;
    let (name, icon) = icons::launch_info(run)?;
    let mut url = format!("{}/?window&run={run}", served_origin());
    if let Some((t, ..)) = live_tickets().iter().find(|(_, r, _)| *r == Some(run)) {
        url.push_str("#t=");
        url.push_str(t);
    }
    Some((url, icon, name))
}

/// The relay's own key pair (`<state>/relay.key`, 0600), made once.
pub fn load_or_create_relay_key() -> proto::auth::KeyPair {
    let path = state_dir().join("relay.key");
    if let Some(k) = std::fs::read(&path).ok().and_then(|b| proto::auth::KeyPair::from_bytes(&b)) {
        return k;
    }
    let k = proto::auth::KeyPair::generate().expect("randomness for the relay's key");
    if let Err(e) = write_private(&path, &k.to_bytes()) {
        warn!("cannot keep the relay's key in {}: {e} (displays will have to pair again)", path.display());
    }
    k
}

fn displays_dir() -> PathBuf {
    state_dir().join("displays")
}

/// The displays that paired with this relay.
pub fn load_displays() -> Vec<proto::DisplayInfo> {
    let Ok(rd) = std::fs::read_dir(displays_dir()) else { return Vec::new() };
    let mut v: Vec<proto::DisplayInfo> = rd
        .flatten()
        .filter(|e| e.path().extension().map_or(false, |x| x == "json"))
        .filter_map(|e| std::fs::read(e.path()).ok().and_then(|b| serde_json::from_slice(&b).ok()))
        .collect();
    v.sort_by_key(|d| d.paired);
    v
}

fn find_display(id: &str) -> Option<proto::DisplayInfo> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return None;
    }
    std::fs::read(displays_dir().join(format!("{id}.json"))).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

fn save_display(d: &proto::DisplayInfo) -> std::io::Result<()> {
    write_private(&displays_dir().join(format!("{}.json", d.id)), &serde_json::to_vec_pretty(d).unwrap_or_default())
}

/// Forget a display: it has to pair again.
pub fn forget_display(id: &str) -> bool {
    find_display(id).is_some() && std::fs::remove_file(displays_dir().join(format!("{id}.json"))).is_ok()
}

// ------------------------------------------------------------------ cross-thread handoff

/// Work other threads hand to the compositor thread.
pub enum Inbox {
    ToApp(ToApp),
    /// A message with a binary payload.
    WithPayload(ToApp, Vec<u8>),
    /// From a Linux process (its pid, when known: sound, the portal): to the page showing its run.
    From(Option<u32>, ToApp, Vec<u8>),
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

static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static WAKE_FD: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

extern "C" fn on_stop_signal(_: libc::c_int) {
    STOP.store(true, std::sync::atomic::Ordering::SeqCst);
    let fd = WAKE_FD.load(std::sync::atomic::Ordering::SeqCst);
    if fd >= 0 {
        unsafe { libc::write(fd, b"s".as_ptr() as *const _, 1) };
    }
}

/// SIGTERM, SIGINT and SIGHUP end the main loop the way `lindroids stop` does, so the X display's
/// socket and lock and the runtime files are removed. Only for the standalone server: the app
/// that hosts one keeps Android's signal handling.
pub fn stop_on_signals() {
    WAKE_FD.store(shared().wake_w.as_raw_fd(), std::sync::atomic::Ordering::SeqCst);
    for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        unsafe { libc::signal(sig, on_stop_signal as *const () as libc::sighandler_t) };
    }
}

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
    /// Below 1 when the client insists on being larger than the Android window: the frame is
    /// scaled down to fit instead of cut off (and input scaled back up).
    pub fit: f32,
    /// Where the scaled-down content starts (it is centred), in physical pixels.
    pub fit_offset: (i32, i32),
    /// xdg-decoration object, and the mode told to the client (None: not told yet).
    pub decoration: Option<wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1>,
    pub decoration_mode: Option<wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode>,
    /// The run (the pid `lindroids run` started) this window belongs to, once known.
    pub run: Option<u32>,
    /// The link (page) the window is announced on.
    pub link: Option<u64>,
}

struct CliConn {
    stream: UnixStream,
    buf: Vec<u8>,
    /// Waiting for the app to answer (export, import, folder picker).
    waiting: Option<Waiting>,
}

#[derive(Debug, Clone, PartialEq)]
enum Waiting {
    Folder(u32),
    /// A worker thread answers this CLI (`lindroids open`).
    Detached,
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
    /// Linked pages: one per run that has a window of its own, and the page showing the rest.
    pub links: Vec<Link>,
    next_link: u64,
    /// The page last used (linked, or touched): what is not about one window goes there.
    primary: Option<u64>,
    /// Links that have not authenticated yet.
    pending: Vec<Link>,
    relay_key: auth::KeyPair,
    pub pairing_code: String,
    pairing_failures: u32,
    pub seat: seat::Seat,
    pub outputs: output::Outputs,
    pub data: data::DataState,
    pub extras: extras::ExtrasState,
    /// Popup bookkeeping, by the popup's wl_surface id.
    pub popups: HashMap<ObjectId, shell::Popup>,
    /// Surfaces not shown anywhere whose frame callbacks are due.
    pub frame_due: Vec<ObjectId>,
    pub running: bool,
    /// Pages that went away, by the run they showed (None: the page showing everything else):
    /// what was started into them ends when the grace is over, unless a page for it links again.
    unlinked: Vec<(Option<u32>, Instant, Duration)>,
    clis: Vec<CliConn>,
    next_request: u32,
    clients: u32,
    /// Sound streams: the pid that plays each, and the page it is open on.
    sound: HashMap<u32, (Option<u32>, Option<u64>)>,
    /// The streams in `sound` that record (the page captures the microphone for them).
    mic: HashSet<u32>,
    /// The page each of the portal's requests went to: what follows it goes there too.
    asked: HashMap<u32, u64>,
}

impl State {
    pub fn next_serial(&mut self) -> u32 {
        self.serial = self.serial.wrapping_add(1);
        self.serial
    }

    pub fn now_ms(&self) -> u32 {
        self.start.elapsed().as_millis() as u32
    }

    /// A ticket for a page URL: the run's (good while it runs), or the shared page's (good for
    /// 10 minutes). The same one again while it is good, so the URL stays the same and opening it
    /// again brings back the same window.
    fn ticket(&mut self, run: Option<u32>) -> String {
        let mut tickets = live_tickets();
        if let Some((t, ..)) = tickets.iter().find(|(_, r, _)| *r == run) {
            return t.clone();
        }
        let t = auth::random_token(16);
        let life = if run.is_some() { Duration::from_secs(24 * 3600) } else { Duration::from_secs(600) };
        tickets.push((t.clone(), run, Instant::now() + life));
        t
    }

    /// Whether `t` is a ticket handed out and still good; for `run`, that run's.
    fn ticket_valid(&mut self, t: &str, run: Option<u32>) -> bool {
        live_tickets().iter().any(|(v, r, _)| auth::same(v, t) && (run.is_none() || *r == run))
    }

    /// The origin pages are served in.
    fn origin(&self) -> String {
        served_origin()
    }

    /// Whether any page is linked.
    pub fn linked(&self) -> bool {
        !self.links.is_empty()
    }

    fn primary_link(&self) -> Option<&Link> {
        self.primary.and_then(|id| self.links.iter().find(|l| l.id == id)).or_else(|| self.links.last())
    }

    /// What is not about one window nor from one program (the clipboard): to the page last used.
    pub fn send_app(&mut self, msg: &ToApp) {
        self.send_app_with(msg, &[]);
    }

    pub fn send_app_with(&mut self, msg: &ToApp, payload: &[u8]) {
        if let Some(id) = self.primary_link().map(|l| l.id) {
            self.send_to(id, msg, payload);
        }
    }

    /// About one window: to the page that shows it.
    pub fn send_win(&mut self, win: u32, msg: &ToApp) {
        self.send_win_with(win, msg, &[]);
    }

    pub fn send_win_with(&mut self, win: u32, msg: &ToApp, payload: &[u8]) {
        if let Some(id) = self.windows.get(&win).and_then(|w| w.link) {
            self.send_to(id, msg, payload);
        }
    }

    fn send_to(&mut self, id: u64, msg: &ToApp, payload: &[u8]) {
        let Some(link) = self.links.iter_mut().find(|l| l.id == id) else { return };
        link.send_with(msg, payload);
        if link.broken {
            warn!("app link write failed");
            self.drop_link(id);
        }
    }

    /// What a Linux process sent (sound, the portal's requests, notifications): to the page
    /// showing its run. A sound stream stays on the page it was opened on, and what follows a
    /// request (a picked file's contents, the rest of a save) goes where the request went.
    pub fn send_from(&mut self, pid: Option<u32>, msg: &ToApp, payload: &[u8]) {
        let to = match msg {
            ToApp::AudioOpen { stream, .. } | ToApp::RecordOpen { stream, .. } => {
                let to = self.page_for(pid);
                self.sound.insert(*stream, (pid, to));
                if matches!(msg, ToApp::RecordOpen { .. }) {
                    self.mic.insert(*stream);
                }
                to
            }
            ToApp::AudioData { stream, .. } | ToApp::AudioCork { stream, .. } => self.sound.get(stream).and_then(|s| s.1),
            ToApp::AudioClose { stream } | ToApp::RecordClose { stream } => {
                self.mic.remove(stream);
                self.sound.remove(stream).and_then(|s| s.1)
            }
            _ => match portal_request(msg) {
                Some(r) => {
                    let follow_up = matches!(msg, ToApp::FetchFile { .. } | ToApp::WriteFile { offset: 1.., .. });
                    let to = self.asked.get(&r).copied().or_else(|| if follow_up { None } else { self.page_for(pid) });
                    match to {
                        Some(id) => {
                            self.asked.insert(r, id);
                        }
                        // No page to answer it: the asker gives up now rather than at its timeout.
                        None => portal::forget(r),
                    }
                    to
                }
                None => self.page_for(pid),
            },
        };
        if let Some(id) = to {
            self.send_to(id, msg, payload);
        }
    }

    /// The page for what a process asks: its run's, else the page last used.
    fn page_for(&self, pid: Option<u32>) -> Option<u64> {
        let run = pid.and_then(icons::launch_of_pid);
        pick_page(&self.pages(), self.primary, run, run.map_or(false, icons::wants_own_window), run.is_some())
    }

    /// The linked pages, oldest first: their link and the run each shows.
    fn pages(&self) -> Vec<(u64, Option<u32>)> {
        self.links.iter().map(|l| (l.id, l.run)).collect()
    }

    /// The page for a run's windows: its own window, else (for what did not ask for one) the page
    /// showing everything else, else the page last used. None: not shown (yet).
    fn route(&self, run: Option<u32>) -> Option<u64> {
        pick_page(&self.pages(), self.primary, run, run.map_or(false, icons::wants_own_window), true)
    }

    /// Sound streams whose page is not the one that should play them now (a page linked or went,
    /// or the page last used changed) start over there: closed on the old page, opened again on
    /// the new one by the sound thread.
    fn reroute_sound(&mut self) {
        let mut moved = Vec::new();
        let mut closes = Vec::new();
        let streams: Vec<(u32, Option<u32>, Option<u64>)> = self.sound.iter().map(|(s, (p, l))| (*s, *p, *l)).collect();
        for (stream, pid, old) in streams {
            let to = self.page_for(pid);
            if to == old {
                continue;
            }
            self.sound.insert(stream, (pid, to));
            if let Some(l) = old {
                closes.push((l, stream));
            }
            moved.push(stream);
        }
        for (l, stream) in closes {
            let close = if self.mic.contains(&stream) { ToApp::RecordClose { stream } } else { ToApp::AudioClose { stream } };
            self.send_to(l, &close, &[]);
        }
        if !moved.is_empty() {
            audio::reopen(moved);
        }
    }

    /// A window starts over on another page (or none): what the old one showed is forgotten.
    fn unshow(&mut self, win: u32) {
        if let Some(w) = self.windows.get_mut(&win) {
            w.announced = false;
            w.link = None;
            w.in_flight = false;
            w.shown.clear();
            w.shown_size = (0, 0);
            w.dirty = true;
            self.frame_due.extend(w.waiting.drain(..));
        }
    }

    /// The run a window belongs to: what `lindroids run` started its client (a Wayland client
    /// descending from it, or an X11 window owned by one), if anything.
    fn run_of(&self, win: u32) -> Option<u32> {
        let w = self.windows.get(&win)?;
        w.run
            .or_else(|| icons::launch_for_window(&self.dh, &w.client))
            .or_else(|| x11::run_for_window(icons::client_pid(&self.dh, &w.client), &w.title, &w.app_id))
    }

    /// Show every window on the page it belongs on now that pages (or launches) came or went.
    fn reroute(&mut self) {
        let wins: Vec<u32> = self.windows.keys().copied().collect();
        for win in wins {
            let Some(w) = self.windows.get(&win) else { continue };
            if !w.mapped {
                continue;
            }
            let run = self.run_of(win);
            let to = self.route(run);
            if w.link == to && w.announced {
                continue;
            }
            if w.announced && w.link.is_some() {
                self.send_win(win, &ToApp::WindowClose { win });
            }
            self.unshow(win);
            self.announce(win);
        }
        self.reroute_sound();
    }

    fn drop_link(&mut self, id: u64) {
        self.links.retain(|l| l.id != id);
        if self.primary == Some(id) {
            self.primary = self.links.last().map(|l| l.id);
        }
        portal::app_changed(self.primary_link().map_or(0, |l| l.version));
        // What the page was asked will not be answered (a file picker it showed, a file to open).
        let unanswered: Vec<u32> = self.asked.iter().filter(|(_, l)| **l == id).map(|(r, _)| *r).collect();
        for r in unanswered {
            self.asked.remove(&r);
            portal::forget(r);
        }
        let wins: Vec<u32> = self.windows.iter().filter(|(_, w)| w.link == Some(id)).map(|(k, _)| *k).collect();
        for win in wins {
            self.unshow(win);
        }
        self.reroute();
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
        if w.announced || !w.mapped {
            return;
        }
        let run = self.run_of(win);
        let Some(to) = self.route(run) else {
            if let Some(w) = self.windows.get_mut(&win) {
                w.run = run;
            }
            return;
        };
        let Some(w) = self.windows.get(&win) else { return };
        let launched = icons::name_for_window(&self.dh, &w.client);
        // A rootful Xwayland window is the X11 program the user ran: name it after that.
        let title = if w.app_id == "org.freedesktop.Xwayland" && launched.is_some() {
            launched.unwrap()
        } else if !w.title.is_empty() {
            w.title.clone()
        } else {
            launched.unwrap_or_else(|| w.app_id.clone())
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
            w.run = run;
            w.link = Some(to);
        }
        self.send_win(win, &msg);
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
                if let Some(id) = w.link {
                    self.send_to(id, &ToApp::WindowClose { win }, &[]);
                }
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
        // Before the Android window exists, a fullscreen client still needs a real size (SDL
        // sizes its fullscreen windows from it): the screen, or a plausible one.
        let guess = if w.fullscreen { Some(self.outputs.screen.unwrap_or((1280, 800))) } else { None };
        let size = match w.phys.or(guess) {
            Some(p) => ((p.0 / scale) as i32, (p.1 / scale) as i32),
            None => (0, 0),
        };
        if !force && size == w.configured {
            return;
        }
        w.configured = size;
        log::debug!("configure window {win}: {size:?}");
        shell::send_toplevel_configure(w, size, serial);
    }

    /// Handshake on a new link. Returns the link back while it is still pending.
    fn handshake(&mut self, mut link: Link, msg: ToServer) -> Option<Link> {
        match (&link.phase, msg) {
            (Phase::Hello, ToServer::Hello { version, build, density, nonce: app_nonce, screen, page }) => {
                link.page = page.filter(|p| p.starts_with('/') && p.len() < 256);
                link.run = link.page.as_deref().and_then(run_of_page);
                let nonce = auth::random_token(24);
                let proof = self.relay_key.sign(&auth::relay_transcript(&link.origin, &app_nonce, &nonce));
                link.send(&ToApp::Challenge { nonce: nonce.clone(), proof, relay_key: self.relay_key.public() });
                link.phase = Phase::Auth { nonce, app_nonce, density, version, build, screen };
                Some(link)
            }
            (Phase::Auth { nonce, app_nonce, density, version, build, screen }, ToServer::Auth { display, proof, public_key, ticket, code, name }) => {
                let (density, version, build, screen) = (*density, *version, build.clone(), *screen);
                let signed = auth::display_transcript(&link.origin, nonce, app_nonce);
                let proof = proof.unwrap_or_default();
                // A paired display signs with the key it paired with.
                let known = display.as_deref().and_then(find_display);
                let by_key = known.as_ref().map_or(false, |d| auth::verify(&d.public_key, &signed, &proof));
                // A new one shows it holds its new key, and has a ticket or the code.
                let new_key = public_key.filter(|k| !by_key && auth::valid_public(k) && auth::verify(k, &signed, &proof));
                // The code first: typed by the user, it is also their say-so for a new relay key.
                let by_code = new_key.is_some()
                    && self.pairing_failures < MAX_PAIRING_FAILURES
                    && code.as_deref().map(|c| c.trim()) == Some(self.pairing_code.as_str());
                let by_ticket = new_key.is_some() && !by_code && ticket.as_deref().map_or(false, |t| self.ticket_valid(t, None));
                // A run's window also needs that run's ticket, which only the CLI hands out (in the
                // URL it opens): a page in our origin that the CLI did not open (the key works there
                // too: it cannot be read out, but it can be used) attaches to nothing.
                let run_ok = link.run.map_or(true, |r| ticket.as_deref().map_or(false, |t| self.ticket_valid(t, Some(r))));
                if (!by_key && !by_ticket && !by_code) || !run_ok {
                    if code.is_some() && !by_code {
                        self.pairing_failures += 1;
                    }
                    warn!("app at {} failed to authenticate", link.peer);
                    let reason = if !run_ok && (by_key || by_ticket || by_code) {
                        "this window was not opened by lindroids run"
                    } else if code.is_some() {
                        "wrong pairing code"
                    } else if display.is_some() {
                        "this display is not paired with this Lindroids any more"
                    } else {
                        "not paired"
                    };
                    link.send(&ToApp::AuthFailed { reason: reason.into(), pairing_open: self.pairing_failures < MAX_PAIRING_FAILURES });
                    // Let the answer drain; the link closes when the app hangs up.
                    let mut l = link;
                    l.flush();
                    return None;
                }
                if let Some(public_key) = new_key.filter(|_| by_ticket || by_code) {
                    let paired = proto::DisplayInfo {
                        id: auth::random_token(9),
                        name: name.unwrap_or_default().chars().take(80).collect(),
                        public_key,
                        paired: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()),
                    };
                    if let Err(e) = save_display(&paired) {
                        warn!("cannot keep the new display: {e}");
                    }
                    info!("display {} ({}) paired by {}", paired.id, paired.name, if by_ticket { "ticket" } else { "code" });
                    link.send(&ToApp::Paired { display: paired.id });
                    if by_code {
                        // A code works once.
                        self.pairing_code = new_pairing_code();
                        write_pairing_code(&self.cfg, &self.pairing_code);
                    }
                }
                info!("app linked ({}, protocol {version}, build {build}, density {density})", link.peer);
                if version != proto::PROTOCOL_VERSION {
                    warn!("app speaks protocol {version}, server {}", proto::PROTOCOL_VERSION);
                }
                link.density = density;
                link.build = build.clone();
                link.version = version;
                link.phase = Phase::Ready;
                // One page per run (and one for the rest). A run's window that is already linked
                // stays: bringing it back sends its intent again, and Firefox then raises the
                // window *and* loads a second copy nobody sees, which is told to stand down.
                let run = link.run;
                if run.is_some() && self.links.iter().any(|l| l.run == run) {
                    info!("{} already has its window: the new page stands down", run.map_or(String::new(), |r| format!("run {r}")));
                    link.send(&ToApp::Bye { reason: "this program's window is open already".into() });
                    let mut l = link;
                    l.flush();
                    return None;
                }
                // For the rest, the newest wins, and the one before is told why it lost the link
                // (a browser tab then waits instead of taking it back).
                let old: Vec<u64> = self.links.iter().filter(|l| l.run == run).map(|l| l.id).collect();
                for id in old {
                    if let Some(l) = self.links.iter_mut().find(|l| l.id == id) {
                        l.send(&ToApp::Bye { reason: "another display took over".into() });
                        l.flush();
                    }
                    self.drop_link(id);
                }
                self.next_link += 1;
                let id = self.next_link;
                link.id = id;
                self.links.push(link);
                self.primary = Some(id);
                let before = self.unlinked.len();
                self.unlinked.retain(|(r, ..)| *r != run);
                if self.unlinked.len() != before {
                    info!("a page linked again: what was started keeps running");
                }
                portal::app_changed(version);
                let stats = std::env::var("LINDROIDS_STATS").is_ok_and(|v| v == "1");
                let hello = ToApp::Hello { version: proto::PROTOCOL_VERSION, build: web::page_build().into(), sandbox: self.cfg.sandbox.clone(), stats };
                self.send_to(id, &hello, &[]);
                // A run's window shows the program's name and icon until its first window does.
                if let Some((name, icon_png_b64)) = run.and_then(icons::launch_info) {
                    self.send_to(id, &ToApp::Run { name, icon_png_b64 }, &[]);
                }
                if let Some((w, h)) = screen {
                    output::screen_size_hint(self, w, h);
                }
                if self.cfg.scale.is_none() {
                    let s = scale_for_density(density);
                    if s != self.scale {
                        self.scale = s;
                        output::scale_changed(self);
                    }
                }
                self.reroute();
                None
            }
            (_, other) => {
                warn!("unexpected message during the handshake: {other:?}");
                None
            }
        }
    }

    fn handle_app_message(&mut self, from: u64, msg: ToServer, payload: Vec<u8>) {
        if matches!(msg, ToServer::Touch { .. } | ToServer::Pointer { .. } | ToServer::Key { .. } | ToServer::Text { .. } | ToServer::Focus { focused: true, .. })
            && self.primary != Some(from)
        {
            self.primary = Some(from);
            // Sound no run claims plays on the page last used.
            self.reroute_sound();
        }
        if portal::wants(&msg) {
            return portal::from_app(msg, payload);
        }
        match msg {
            // Answers to requests nobody is waiting for any more.
            ToServer::System(_)
            | ToServer::MemoryWarning { .. }
            | ToServer::OpenResult { .. }
            | ToServer::NotificationAction { .. }
            | ToServer::NotificationClosed { .. }
            | ToServer::FilesPicked { .. }
            | ToServer::FileChunk { .. }
            | ToServer::WriteResult { .. } => {}
            ToServer::Hello { .. } | ToServer::Auth { .. } => {}
            ToServer::Log { msg } => info!("page {from}: {}", msg.chars().take(500).collect::<String>()),
            ToServer::AudioCredit { stream, bytes } => audio::credit(stream, bytes),
            // Only from the page the stream records on: one it moved away from may still send.
            ToServer::RecordData { stream, .. } => {
                if self.mic.contains(&stream) && self.sound.get(&stream).and_then(|s| s.1) == Some(from) {
                    audio::record_data(stream, payload);
                }
            }
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
            ToServer::Closing => {
                if let Some(l) = self.links.iter_mut().find(|l| l.id == from) {
                    l.closing = true;
                }
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
        }
    }

    fn reply_cli(&mut self, what: &Waiting, reply: CliReply) {
        if let Some(i) = self.clis.iter().position(|c| c.waiting.as_ref() == Some(what)) {
            let mut c = self.clis.remove(i);
            c.stream.set_nonblocking(false).ok();
            let _ = c.stream.write_all(&proto::encode(&reply));
        }
    }

    fn handle_cli(&mut self, idx: usize, req: CliRequest, fds: Vec<OwnedFd>) {
        let no_app = || CliReply::Error { message: "no display is linked: open the windows page in the browser (`lindroids pair`)".into() };
        let reply = match req {
            CliRequest::Status => Some(CliReply::Status {
                version: proto::PROTOCOL_VERSION,
                build: BUILD.into(),
                app_connected: self.linked(),
                app_build: self.primary_link().map(|l| l.build.clone()),
                display_page: self.primary_link().and_then(|l| l.page.clone()),
                shown_runs: self.links.iter().filter_map(|l| l.run).collect(),
                origin: self.origin(),
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
            CliRequest::Ticket { run } => Some(CliReply::Ticket { ticket: self.ticket(run), origin: self.origin() }),
            CliRequest::Displays => Some(CliReply::Displays { displays: load_displays() }),
            CliRequest::ForgetDisplay { id } => Some(if forget_display(&id) {
                CliReply::Ok { message: format!("forgot {id}: it has to pair again") }
            } else {
                CliReply::Error { message: format!("no display {id:?} (lindroids displays)") }
            }),
            CliRequest::Stop => {
                self.running = false;
                Some(CliReply::Ok { message: "stopping".into() })
            }
            CliRequest::Launch { pid, name, icon_png_b64, symbolic_png_b64, window } => {
                icons::register_launch(pid, name, icon_png_b64, window);
                android::set_symbolic(pid, symbolic_png_b64);
                if window {
                    android::run_started(pid);
                }
                // Windows it opened before it was registered move to where they belong.
                self.reroute();
                Some(CliReply::Ok { message: "registered".into() })
            }
            CliRequest::RunEnded { pid, code, output } => {
                // Its page shows the end even when the program never showed a window, and why.
                self.send_from(Some(pid), &ToApp::RunEnded { code, output }, &[]);
                Some(CliReply::Ok { message: "noted".into() })
            }
            CliRequest::Info => Some(CliReply::Info { system: portal::system() }),
            CliRequest::Notify { title, body } => {
                if !self.linked() {
                    Some(no_app())
                } else {
                    portal::notify(peer_pid(&self.clis[idx].stream), title, body);
                    Some(CliReply::Ok { message: "shown".into() })
                }
            }
            CliRequest::Open { target } => match self.clis[idx].stream.try_clone() {
                Err(e) => Some(CliReply::Error { message: e.to_string() }),
                Ok(mut stream) => {
                    // A file travels as a descriptor (the CLI may see another filesystem than
                    // this server, as in a proot distro); anything else is a URI. Either goes to
                    // the page of the program that ran `lindroids open` (xdg-open).
                    let file = fds.into_iter().next();
                    let pid = peer_pid(&stream);
                    self.clis[idx].waiting = Some(Waiting::Detached);
                    std::thread::spawn(move || {
                        let r = match file {
                            Some(fd) => {
                                let name = Path::new(&target).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| portal::fd_name(&fd));
                                portal::open_file(pid, std::fs::File::from(fd), name)
                            }
                            None => portal::open_uri(pid, target),
                        };
                        let reply = match r {
                            Ok(()) => CliReply::Ok { message: "opened".into() },
                            Err(message) => CliReply::Error { message },
                        };
                        stream.set_nonblocking(false).ok();
                        let _ = stream.write_all(&proto::encode(&reply));
                    });
                    None
                }
            },
            // On the page of the program that asked.
            CliRequest::PickFolder => match self.page_for(peer_pid(&self.clis[idx].stream)) {
                None => Some(no_app()),
                Some(page) => {
                    self.next_request += 1;
                    let r = self.next_request;
                    self.send_to(page, &ToApp::PickFolder { request: r }, &[]);
                    self.clis[idx].waiting = Some(Waiting::Folder(r));
                    None
                }
            },
        };
        if let Some(r) = reply {
            let c = &mut self.clis[idx];
            c.stream.set_nonblocking(false).ok();
            let _ = c.stream.write_all(&proto::encode(&r));
            c.stream.set_nonblocking(true).ok();
        }
    }
}

/// Integer output scale for a screen density. A bit below the density: desktop apps are laid out
/// for ~800px-wide windows, and 540 logical pixels across a 440 dpi phone fit far more of them
/// than 360 would, while text stays readable.
pub fn scale_for_density(density: f32) -> u32 {
    (density * 0.75).round().clamp(1.0, 4.0) as u32
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

fn peer_cred(s: &UnixStream) -> libc::ucred {
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    unsafe { libc::getsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cred as *mut _ as *mut _, &mut len) };
    cred
}

pub fn peer_uid(s: &UnixStream) -> u32 {
    peer_cred(s).uid
}

/// The process at the other end of a Unix socket (as it was when it connected), if known.
pub fn peer_pid(s: &UnixStream) -> Option<u32> {
    let pid = peer_cred(s).pid;
    (pid > 0).then_some(pid as u32)
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
    let uid = proto::real_uid();
    let app_name = cfg.socket_name.clone().unwrap_or_else(|| proto::server_socket_name(uid));
    // The abstract socket is the fast path (same uid, or devices whose policy allows it); loopback
    // TCP works between any two apps.
    // LINDROIDS_TRANSPORT=tcp: loopback only, as on devices whose SELinux keeps apps out of each
    // other's sockets (for testing that path on devices that would allow the other one).
    let tcp_only = std::env::var("LINDROIDS_TRANSPORT").as_deref() == Ok("tcp");
    let app_listener = if tcp_only {
        None
    } else {
        match listen_abstract(&app_name) {
            Ok(l) => Some(l),
            Err(e) => {
                warn!("no abstract socket @{app_name}: {e}");
                None
            }
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
        eprintln!("lindroids: no port for the browser to reach this server (is another one running?)");
        return 1;
    }

    if let Err(e) = audio::start(&cfg.runtime_dir) {
        warn!("sound is off: {e}");
    }

    let mut display: Display<State> = Display::new().expect("wayland display");
    let dh = display.handle();
    let relay_key = load_or_create_relay_key();
    web::set_origin_label(origin_label());
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
        links: Vec::new(),
        next_link: 0,
        primary: None,
        pending: Vec::new(),
        relay_key,
        pairing_code,
        pairing_failures: 0,
        seat: seat::Seat::default(),
        outputs: output::Outputs::default(),
        data: data::DataState::default(),
        extras: extras::ExtrasState::default(),
        popups: HashMap::new(),
        frame_due: Vec::new(),
        unlinked: Vec::new(),
        running: true,
        clis: Vec::new(),
        next_request: 0,
        clients: 0,
        sound: HashMap::new(),
        mic: HashSet::new(),
        asked: HashMap::new(),
    };
    surface::register_globals(&dh);
    shell::register_globals(&dh);
    seat::register_globals(&dh);
    output::register_globals(&dh);
    data::register_globals(&dh);
    extras::register_globals(&dh);

    let _ = std::fs::remove_file(cfg.runtime_dir.join("x11-display"));
    let mut x11 = match x11::X11::start(&cfg.runtime_dir, &wl_path) {
        Ok(x) => Some(x),
        Err(e) => {
            info!("X11 is off: {e}");
            None
        }
    };

    let _portal = match portal::start(&cfg.runtime_dir) {
        Ok(p) => Some(p),
        Err(e) => {
            info!("desktop integration (portal) is off: {e}");
            None
        }
    };

    std::fs::write(cfg.runtime_dir.join("server.pid"), std::process::id().to_string()).ok();
    info!("listening on {}, @{app_name} and 127.0.0.1:{port} (sandbox {:?}, scale {scale})", wl_path.display(), cfg.sandbox);
    // The CLI waits for this line.
    println!("lindroids-server ready {}", wl_path.display());
    let _ = std::io::stdout().flush();

    let mut last_frame_flush = Instant::now();
    let mut last_socket_check = Instant::now();
    while state.running {
        let mut fds: Vec<libc::pollfd> = Vec::with_capacity(8 + state.clis.len() + state.pending.len());
        let pfd = |fd: RawFd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        fds.push(pfd(socket.as_raw_fd()));
        fds.push(pfd(display.backend().poll_fd().as_raw_fd()));
        fds.push(pfd(shared.wake_r.as_raw_fd()));
        fds.push(pfd(cli_listener.as_raw_fd()));
        fds.push(pfd(app_listener.as_ref().map_or(-1, |l| l.as_raw_fd())));
        fds.push(pfd(tcp_listener.as_ref().map_or(-1, |l| l.as_raw_fd())));
        let link_base = fds.len();
        for l in &state.links {
            let mut lp = pfd(l.fd());
            if l.wants_write() {
                lp.events |= libc::POLLOUT;
            }
            fds.push(lp);
        }
        let pending_base = fds.len();
        for p in &state.pending {
            let mut f = pfd(p.fd());
            if p.wants_write() {
                f.events |= libc::POLLOUT;
            }
            fds.push(f);
        }
        let cli_base = fds.len();
        for c in &state.clis {
            fds.push(pfd(c.stream.as_raw_fd()));
        }
        let x11_base = fds.len();
        for fd in x11.as_ref().map(|x| x.poll_fds()).unwrap_or_default() {
            fds.push(pfd(fd));
        }
        let due: Vec<Option<u32>> = state.unlinked.iter().filter(|(_, since, grace)| since.elapsed() >= *grace).map(|(r, ..)| *r).collect();
        if !due.is_empty() {
            state.unlinked.retain(|(_, since, grace)| since.elapsed() < *grace);
            for run in due {
                end_launches(run);
            }
        }
        let mut timeout = if state.frame_due.is_empty() { 1000 } else { 16 };
        if let Some(t) = x11.as_ref().and_then(|x| x.wants_timeout()) {
            timeout = timeout.min(t.as_millis() as i32);
        }
        unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, timeout) };

        if STOP.load(std::sync::atomic::Ordering::SeqCst) {
            info!("stopping on a signal");
            state.running = false;
        }
        // Termux empties its tmp dir when it starts again: a server whose control socket went
        // with it can no longer be reached or stopped, and would hold the port for good.
        if last_socket_check.elapsed() >= Duration::from_secs(2) {
            last_socket_check = Instant::now();
            if !cli_path.exists() {
                warn!("{} is gone (the runtime dir was emptied): stopping", cli_path.display());
                state.running = false;
            }
        }
        if let Some(x) = x11.as_mut() {
            if fds[x11_base..].iter().any(|p| p.revents != 0) {
                x.on_connection();
            }
            x.tick();
        }
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
                    Inbox::From(pid, m, p) => state.send_from(pid, &m, &p),
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
                    state.pending.push(Link::new_tcp(Box::new(stream), format!("tcp {addr}"), port));
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
            if fds[pending_base + i].revents & libc::POLLOUT != 0 {
                state.pending[i].flush();
            }
            let alive = state.pending[i].read();
            if state.pending[i].finished() || state.pending[i].broken {
                state.pending.remove(i);
                continue;
            }
            match state.pending[i].next() {
                Some(Ok((m, _))) => {
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
        let polled: Vec<(u64, i16)> = state.links.iter().enumerate().map(|(k, l)| (l.id, fds.get(link_base + k).map_or(0, |p| p.revents))).collect();
        for (id, revents) in polled {
            if revents == 0 {
                continue;
            }
            let mut msgs = Vec::new();
            let alive = {
                let Some(l) = state.links.iter_mut().find(|l| l.id == id) else { continue };
                if revents & libc::POLLOUT != 0 {
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
                    Ok((m, payload)) => state.handle_app_message(id, m, payload),
                    Err(e) => warn!("bad message from app: {e}"),
                }
            }
            if !alive {
                // Closed on purpose (the page said so: its window was closed, or it reloads), or
                // lost (the browser died: swiped away with its last window, or killed for memory,
                // and then it reloads the page when the user comes back). Either way a page that
                // links again in time keeps the programs; otherwise they end with their window.
                let Some(l) = state.links.iter().find(|l| l.id == id) else { continue };
                let (run, grace) = (l.run, if l.closing { Duration::from_secs(5) } else { linger() });
                info!("page for {} closed; what was started into it ends in {}s unless it links again", run.map_or("the rest".into(), |r| format!("run {r}")), grace.as_secs());
                state.drop_link(id);
                if !state.links.iter().any(|l| l.run == run) {
                    state.unlinked.push((run, Instant::now(), grace));
                }
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
    // The programs went with the server; so do their notifications (the helper takes a moment
    // for each).
    let n = android::clear_running();
    if n > 0 {
        std::thread::sleep(Duration::from_secs(n.min(5) as u64));
    }
    0
}

/// Which page something goes to, among the linked ones (`pages`: link and the run each shows,
/// oldest first): its run's own page; nothing (yet) for a run that wants one and has none; else
/// the page showing everything else if `shared_first`, else the page last used (`primary`).
fn pick_page(pages: &[(u64, Option<u32>)], primary: Option<u64>, run: Option<u32>, own_window: bool, shared_first: bool) -> Option<u64> {
    if let Some(r) = run {
        if let Some((id, _)) = pages.iter().rev().find(|(_, p)| *p == Some(r)) {
            return Some(*id);
        }
        if own_window {
            return None;
        }
    }
    let last_used = primary.filter(|p| pages.iter().any(|(id, _)| id == p)).or_else(|| pages.last().map(|(id, _)| *id));
    let shared = pages.iter().rev().find(|(_, p)| p.is_none()).map(|(id, _)| *id);
    if shared_first {
        shared.or(last_used)
    } else {
        last_used
    }
}

/// The portal's request a message is part of.
fn portal_request(msg: &ToApp) -> Option<u32> {
    let r = match msg {
        ToApp::OpenUri { request, .. } | ToApp::OpenFile { request, .. } | ToApp::FetchFile { request, .. } | ToApp::WriteFile { request, .. } => *request,
        ToApp::PickFiles(p) => p.request,
        _ => return None,
    };
    (r >= portal::FIRST_REQUEST).then_some(r)
}

/// The run a page shows: `?run=<pid>` in its URL.
fn run_of_page(page: &str) -> Option<u32> {
    let query = page.split_once('?')?.1;
    query.split('&').find_map(|kv| kv.strip_prefix("run=")).and_then(|v| v.parse().ok())
}

/// How long programs outlive a page that vanished without saying so (`LINDROIDS_LINGER`, seconds).
fn linger() -> Duration {
    Duration::from_secs(std::env::var("LINDROIDS_LINGER").ok().and_then(|v| v.parse().ok()).unwrap_or(60))
}

/// The window is gone, so are the programs `lindroids run` and `launch` started into it (`run`:
/// that run; None: those without a window of their own): SIGTERM, and SIGKILL for whatever is
/// still there 5 s later. (proot's --kill-on-exit takes its tracees.)
fn end_launches(run: Option<u32>) {
    let pids: Vec<u32> = match run {
        Some(r) => icons::launched_pids().into_iter().filter(|p| *p == r).collect(),
        None => icons::launched_pids().into_iter().filter(|p| !icons::wants_own_window(*p)).collect(),
    };
    if pids.is_empty() {
        return;
    }
    info!("the window is gone: ending {pids:?}");
    // The whole tree under each, children first: a program under proot is proot's tracee, and
    // outlives proot otherwise.
    let tree: Vec<u32> = pids.iter().flat_map(|p| icons::process_tree(*p)).collect();
    for &p in &tree {
        unsafe { libc::kill(p as i32, libc::SIGTERM) };
    }
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(5));
        for p in pids.iter().flat_map(|p| icons::process_tree(*p)) {
            unsafe { libc::kill(p as i32, libc::SIGKILL) };
        }
    });
}

/// Compose every window that changed and send what differs from what the app shows.
fn render_dirty(state: &mut State) {
    // Pages with a lot still to send get no new frames until it has gone out.
    let busy: Vec<u64> = state.links.iter().filter(|l| l.pending() > (8 << 20)).map(|l| l.id).collect();
    let wins: Vec<u32> = state
        .windows
        .iter()
        .filter(|(_, w)| w.dirty && w.announced && !w.in_flight && w.phys.is_some())
        .filter(|(_, w)| w.link.map_or(false, |id| !busy.contains(&id)))
        .map(|(k, _)| *k)
        .collect();
    for win in wins {
        let mut layers = shell::layers_for_window(state, win);
        let (cw, ch) = shell::content_size(state, win);
        let scale = state.scale as f32;
        let Some(w) = state.windows.get_mut(&win) else { continue };
        let (pw, ph) = w.phys.unwrap();
        let fit = if cw > 0 && ch > 0 { (pw as f32 / (cw as f32 * scale)).min(ph as f32 / (ch as f32 * scale)).min(1.0) } else { 1.0 };
        w.fit = fit;
        // Content smaller than the window (a dialog with a size of its own, a scaled-down client)
        // sits in the middle rather than in a corner.
        w.fit_offset = if cw > 0 && ch > 0 {
            (((pw as f32 - cw as f32 * scale * fit) / 2.0).max(0.0) as i32, ((ph as f32 - ch as f32 * scale * fit) / 2.0).max(0.0) as i32)
        } else {
            (0, 0)
        };
        if fit < 1.0 || w.fit_offset != (0, 0) {
            let (ox, oy) = w.fit_offset;
            for p in &mut layers.placed {
                p.x = (p.x as f32 * fit).round() as i32 + ox;
                p.y = (p.y as f32 * fit).round() as i32 + oy;
                p.w = ((p.w as f32 * fit).round() as u32).max(1);
                p.h = ((p.h as f32 * fit).round() as u32).max(1);
            }
        }
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
        state.send_win_with(win, &msg, &payload);
    }
}

#[cfg(test)]
mod tests {
    use super::{pick_page, run_of_page};

    #[test]
    fn pages_name_their_run() {
        assert_eq!(run_of_page("/?window&run=4242"), Some(4242));
        assert_eq!(run_of_page("/?run=7&window"), Some(7));
        assert_eq!(run_of_page("/?window"), None);
        assert_eq!(run_of_page("/"), None);
        assert_eq!(run_of_page("/?run=x"), None);
    }

    #[test]
    fn requests_go_to_the_page_of_the_asking_run() {
        // Link 1 shows everything else, 2 shows run 100, 3 shows run 200; 3 was used last.
        let pages = [(1, None), (2, Some(100)), (3, Some(200))];
        let last = Some(3);
        // A run's own page, whichever page was used last.
        assert_eq!(pick_page(&pages, last, Some(100), true, true), Some(2));
        assert_eq!(pick_page(&pages, Some(1), Some(200), true, true), Some(3));
        // A run that wants a page of its own and has none yet: nowhere until it links.
        assert_eq!(pick_page(&pages, last, Some(300), true, true), None);
        // A run started into the shared page: that page.
        assert_eq!(pick_page(&pages, last, Some(400), false, true), Some(1));
        // What no run claims: the page last used, or the newest if that one went.
        assert_eq!(pick_page(&pages, last, None, false, false), Some(3));
        assert_eq!(pick_page(&pages, Some(9), None, false, false), Some(3));
        // Windows no run claims go to the shared page.
        assert_eq!(pick_page(&pages, last, None, false, true), Some(1));
        // No shared page: the page last used.
        assert_eq!(pick_page(&pages[1..], Some(2), Some(400), false, true), Some(2));
        assert_eq!(pick_page(&[], None, Some(100), false, true), None);
    }
}
