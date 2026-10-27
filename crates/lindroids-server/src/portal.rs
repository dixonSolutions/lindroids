//! Android integration for Linux programs, through the interfaces they already use on a Linux
//! desktop: the XDG desktop portal (`org.freedesktop.portal.Desktop`) and the notification
//! service (`org.freedesktop.Notifications`), on a session bus of the server's own.
//!
//! What lives in Linux stays there: the bus is the environment's own `dbus-daemon`, programs talk
//! D-Bus as anywhere else, and only the answers come from Android, over the app link:
//!
//! | interface                        | Android                                               |
//! | -------------------------------- | ----------------------------------------------------- |
//! | Settings (appearance)            | night mode, Material You accent, contrast, animations |
//! | OpenURI                          | ACTION_VIEW: browser, mail, maps, any app             |
//! | FileChooser                      | Android's document picker                             |
//! | Notification, fdo Notifications  | Android notifications, with their buttons             |
//! | MemoryMonitor                    | onTrimMemory                                          |
//! | NetworkMonitor                   | the default network: available, metered, validated    |
//! | PowerProfileMonitor              | battery saver                                         |
//!
//! Toolkits are pointed at it by the environment the CLI gives programs (DBUS_SESSION_BUS_ADDRESS,
//! GTK_USE_PORTAL, GDK_DEBUG=portals, QT_QPA_PLATFORMTHEME). Off with LINDROIDS_PORTAL=off, or
//! when there is no dbus-daemon (`lindroids setup x11` installs it).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use lindroids_proto::{Notification, PickFiles, PickedFile, SystemState, ToApp, ToServer};
use log::{info, warn};
use zbus::message::Header;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

const DESKTOP: &str = "/org/freedesktop/portal/desktop";
const NOTIFICATIONS: &str = "/org/freedesktop/Notifications";
/// Requests the portal makes of the app are numbered from here; lower numbers are the CLI's.
pub const FIRST_REQUEST: u32 = 1 << 30;
/// Largest file handed to Android in one piece (OpenFile).
const MAX_OPEN: u64 = 64 << 20;

#[derive(Default)]
struct Shared {
    runtime: OnceLock<PathBuf>,
    conn: OnceLock<zbus::blocking::Connection>,
    system: Mutex<Option<SystemState>>,
    /// Requests waiting for the app's answer.
    waiting: Mutex<HashMap<u32, mpsc::Sender<Answer>>>,
    /// Who asked for each notification, by the id the app knows it by.
    notes: Mutex<HashMap<String, NoteOwner>>,
    /// The connected app's protocol version (0: none).
    app_version: AtomicU32,
}

fn shared() -> &'static Shared {
    static S: OnceLock<Shared> = OnceLock::new();
    S.get_or_init(Shared::default)
}

static NEXT_REQUEST: AtomicU32 = AtomicU32::new(FIRST_REQUEST);
static NEXT_NOTE: AtomicU32 = AtomicU32::new(1);

enum Answer {
    Open(bool),
    Picked(Vec<PickedFile>),
    Chunk { last: bool, error: Option<String>, data: Vec<u8> },
    Write(bool, Option<String>),
}

#[derive(Clone)]
enum NoteOwner {
    Fdo(u32),
    Portal { sender: String, id: String },
}

fn to_app(msg: ToApp, payload: Vec<u8>) {
    let inbox = if payload.is_empty() { crate::Inbox::ToApp(msg) } else { crate::Inbox::WithPayload(msg, payload) };
    crate::shared().post(inbox);
}

/// Send a request and return the channel its answers arrive on; None without a capable app.
fn ask(make: impl FnOnce(u32) -> (ToApp, Vec<u8>)) -> Option<(u32, mpsc::Receiver<Answer>)> {
    if shared().app_version.load(Ordering::SeqCst) < 3 {
        return None;
    }
    let r = NEXT_REQUEST.fetch_add(1, Ordering::SeqCst);
    let (tx, rx) = mpsc::channel();
    shared().waiting.lock().unwrap().insert(r, tx);
    let (m, p) = make(r);
    to_app(m, p);
    Some((r, rx))
}

/// Answers to an existing request number (FetchFile's chunks come back under the number of the
/// pick they belong to).
fn listen(r: u32) -> mpsc::Receiver<Answer> {
    let (tx, rx) = mpsc::channel();
    shared().waiting.lock().unwrap().insert(r, tx);
    rx
}

fn forget(r: u32) {
    shared().waiting.lock().unwrap().remove(&r);
}

fn ask_once(make: impl FnOnce(u32) -> (ToApp, Vec<u8>), timeout: Duration) -> Option<Answer> {
    let (r, rx) = ask(make)?;
    let a = rx.recv_timeout(timeout).ok();
    forget(r);
    a
}

// ------------------------------------------------------------------ the main loop's side

/// Whether a message from the app belongs here (an answer to one of the portal's requests, or
/// Android's state).
pub fn wants(msg: &ToServer) -> bool {
    match msg {
        ToServer::System(_) | ToServer::MemoryWarning { .. } | ToServer::NotificationAction { .. } | ToServer::NotificationClosed { .. } => true,
        ToServer::OpenResult { request, .. }
        | ToServer::FilesPicked { request, .. }
        | ToServer::FileChunk { request, .. }
        | ToServer::WriteResult { request, .. } => *request >= FIRST_REQUEST,
        _ => false,
    }
}

/// A message `wants` accepted.
pub fn from_app(msg: ToServer, payload: Vec<u8>) {
    let answer = |r: u32, a: Answer, done: bool| {
        let mut w = shared().waiting.lock().unwrap();
        if let Some(tx) = w.get(&r) {
            let _ = tx.send(a);
        }
        if done {
            w.remove(&r);
        }
    };
    match msg {
        ToServer::System(s) => system_changed(s),
        ToServer::MemoryWarning { level } => {
            info!("Android asks to save memory (level {level})");
            emit(None, DESKTOP, "org.freedesktop.portal.MemoryMonitor", "LowMemoryWarning", &(level,));
        }
        ToServer::NotificationAction { id, action } => note_action(&id, &action),
        ToServer::NotificationClosed { id, reason } => note_closed(&id, reason),
        ToServer::OpenResult { request, ok, .. } => answer(request, Answer::Open(ok), true),
        ToServer::FilesPicked { request, files } => answer(request, Answer::Picked(files), true),
        ToServer::FileChunk { request, last, error, .. } => answer(request, Answer::Chunk { last, error, data: payload }, false),
        ToServer::WriteResult { request, ok, message } => answer(request, Answer::Write(ok, message), true),
        _ => {}
    }
}

/// The app linked (with its protocol version) or went away (0).
pub fn app_changed(version: u32) {
    shared().app_version.store(version, Ordering::SeqCst);
    if version == 0 {
        // Nobody will answer what is waiting.
        shared().waiting.lock().unwrap().clear();
    }
}

/// What Android last reported (`lindroids info`).
pub fn system() -> Option<SystemState> {
    shared().system.lock().unwrap().clone()
}

// ------------------------------------------------------------------ the bus

/// The session bus and the services on it; dropping it stops the bus.
pub struct Portal {
    daemon: Child,
    socket: PathBuf,
}

impl Drop for Portal {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// Start the session bus in `runtime` (`<runtime>/bus`) and the services on it.
pub fn start(runtime: &Path) -> Result<Portal, String> {
    if std::env::var("LINDROIDS_PORTAL").as_deref() == Ok("off") {
        return Err("off (LINDROIDS_PORTAL=off)".into());
    }
    let daemon = crate::x11::which("dbus-daemon").ok_or("dbus-daemon is not installed (`lindroids setup x11`)")?;
    let socket = runtime.join("bus");
    let _ = std::fs::remove_file(&socket);
    let conf = runtime.join("dbus-session.conf");
    std::fs::write(&conf, bus_config(&socket)).map_err(|e| format!("{}: {e}", conf.display()))?;
    let log = std::fs::File::create(runtime.join("dbus.log")).ok();
    let mut cmd = Command::new(&daemon);
    cmd.arg(format!("--config-file={}", conf.display())).args(["--nofork", "--nopidfile"]).stdin(Stdio::null());
    match log {
        Some(f) => {
            cmd.stdout(f.try_clone().map(Stdio::from).unwrap_or(Stdio::null())).stderr(Stdio::from(f));
        }
        None => {
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    let child = cmd.spawn().map_err(|e| format!("{}: {e}", daemon.display()))?;
    let portal = Portal { daemon: child, socket: socket.clone() };
    let t0 = Instant::now();
    while !socket.exists() {
        if t0.elapsed() > Duration::from_secs(5) {
            return Err(format!("dbus-daemon did not start (see {})", runtime.join("dbus.log").display()));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = shared().runtime.set(runtime.to_path_buf());
    write_gtk3_dark_theme(runtime);
    let conn = zbus::blocking::connection::Builder::address(format!("unix:path={}", socket.display()).as_str())
        .and_then(|b| b.serve_at(DESKTOP, Settings))
        .and_then(|b| b.serve_at(DESKTOP, OpenUri))
        .and_then(|b| b.serve_at(DESKTOP, FileChooser))
        .and_then(|b| b.serve_at(DESKTOP, PortalNotification))
        .and_then(|b| b.serve_at(DESKTOP, MemoryMonitor))
        .and_then(|b| b.serve_at(DESKTOP, NetworkMonitor))
        .and_then(|b| b.serve_at(DESKTOP, PowerProfileMonitor))
        .and_then(|b| b.serve_at(NOTIFICATIONS, Notifications))
        .and_then(|b| b.name("org.freedesktop.portal.Desktop"))
        .and_then(|b| b.name("org.freedesktop.Notifications"))
        .and_then(|b| b.build())
        .map_err(|e| format!("portal: {e}"))?;
    let _ = shared().conn.set(conn);
    info!("desktop portal and notifications on {}", socket.display());
    Ok(portal)
}

/// A session bus for one sandbox's programs. Programs under proot run as a faked root, so
/// EXTERNAL authentication (the claimed uid must be the socket peer's) fails for them;
/// ANONYMOUS is safe here because the socket is in the runtime directory, which only this uid
/// can enter.
fn bus_config(socket: &Path) -> String {
    format!(
        r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path={}</listen>
  <auth>EXTERNAL</auth>
  <auth>ANONYMOUS</auth>
  <allow_anonymous/>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
        socket.display()
    )
}

/// GTK 3 has no color-scheme: through the portal it follows GNOME's `gtk-theme`, which is
/// "Adwaita-dark" in night mode, and looks for a theme by that name on disk. Its dark variant only
/// exists inside GTK, so `<runtime>/share/themes/Adwaita-dark` (first on the programs'
/// XDG_DATA_DIRS) names it. GTK 3 then switches live with Android's night mode.
fn write_gtk3_dark_theme(runtime: &Path) {
    let dir = runtime.join("share/themes/Adwaita-dark/gtk-3.0");
    let css = "@import url(\"resource:///org/gtk/libgtk/theme/Adwaita/gtk-contained-dark.css\");\n";
    if std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join("gtk.css"), css)).is_err() {
        warn!("cannot write the GTK 3 dark theme in {}", dir.display());
    }
}

fn conn() -> Option<&'static zbus::blocking::Connection> {
    shared().conn.get()
}

fn emit<B: serde::Serialize + zbus::zvariant::DynamicType>(dest: Option<&str>, path: &str, iface: &str, name: &str, body: &B) {
    if let Some(c) = conn() {
        if let Err(e) = c.emit_signal(dest, path, iface, name, body) {
            warn!("portal: {iface}.{name}: {e}");
        }
    }
}

// ------------------------------------------------------------------ requests and responses

/// The Request object path a portal call answers on: the caller derives it from its unique name
/// and the `handle_token` it chose, and listens there before calling.
fn request_path(hdr: &Header<'_>, options: &HashMap<String, OwnedValue>) -> (OwnedObjectPath, String) {
    let sender = hdr.sender().map(|s| s.to_string()).unwrap_or_default();
    let escaped = sender.trim_start_matches(':').replace('.', "_");
    let token = opt_str(options, "handle_token").unwrap_or_else(|| format!("lindroids{}", NEXT_REQUEST.fetch_add(1, Ordering::SeqCst)));
    let path = format!("{DESKTOP}/request/{escaped}/{token}");
    let path = ObjectPath::try_from(path).map(OwnedObjectPath::from).unwrap_or_else(|_| OwnedObjectPath::try_from(DESKTOP).unwrap());
    (path, sender)
}

/// Response(response, results): 0 success, 1 cancelled, 2 failed.
fn respond(sender: &str, handle: &OwnedObjectPath, response: u32, results: HashMap<&str, Value<'_>>) {
    emit(Some(sender), handle.as_str(), "org.freedesktop.portal.Request", "Response", &(response, results));
}

fn opt_str(o: &HashMap<String, OwnedValue>, k: &str) -> Option<String> {
    match o.get(k).map(|v| &**v) {
        Some(Value::Str(s)) => Some(s.to_string()),
        _ => None,
    }
}

fn opt_bool(o: &HashMap<String, OwnedValue>, k: &str) -> bool {
    matches!(o.get(k).map(|v| &**v), Some(Value::Bool(true)))
}

// ------------------------------------------------------------------ Settings

/// The settings Android's state stands for.
fn settings(s: &SystemState) -> Vec<(&'static str, &'static str, Value<'static>)> {
    let mut v: Vec<(&str, &str, Value)> = vec![
        // 1 prefer dark, 2 prefer light.
        ("org.freedesktop.appearance", "color-scheme", Value::from(if s.dark { 1u32 } else { 2u32 })),
        ("org.freedesktop.appearance", "contrast", Value::from(u32::from(s.high_contrast))),
        ("org.freedesktop.appearance", "reduced-motion", Value::from(u32::from(s.reduced_motion))),
        // GNOME's keys, which GTK 3 reads through the portal (GTK_USE_PORTAL), and older
        // libadwaita.
        ("org.gnome.desktop.interface", "color-scheme", Value::from(if s.dark { "prefer-dark" } else { "prefer-light" })),
        ("org.gnome.desktop.interface", "gtk-theme", Value::from(if s.dark { "Adwaita-dark" } else { "Adwaita" })),
        ("org.gnome.desktop.interface", "text-scaling-factor", Value::from(f64::from(s.font_scale.max(0.5)))),
        ("org.gnome.desktop.interface", "enable-animations", Value::from(!s.reduced_motion)),
        ("org.gnome.desktop.a11y.interface", "high-contrast", Value::from(s.high_contrast)),
    ];
    if let Some([r, g, b]) = s.accent {
        v.push(("org.freedesktop.appearance", "accent-color", Value::from((f64::from(r), f64::from(g), f64::from(b)))));
    }
    v
}

fn lookup(namespace: &str, key: &str) -> Option<Value<'static>> {
    let s = system()?;
    settings(&s).into_iter().find(|(n, k, _)| *n == namespace && *k == key).map(|(_, _, v)| v)
}

fn owned(v: Value<'_>) -> OwnedValue {
    v.try_to_owned().expect("settings values hold no file descriptors")
}

fn system_changed(new: SystemState) {
    let old = shared().system.lock().unwrap().replace(new.clone());
    let before: HashMap<(&str, &str), Value> = old.as_ref().map(|o| settings(o).into_iter().map(|(n, k, v)| ((n, k), v)).collect()).unwrap_or_default();
    for (n, k, v) in settings(&new) {
        if before.get(&(n, k)) != Some(&v) {
            emit(None, DESKTOP, "org.freedesktop.portal.Settings", "SettingChanged", &(n, k, v));
        }
    }
    if old.as_ref().map(|o| o.power_saver) != Some(new.power_saver) {
        let changed: HashMap<&str, Value> = [("power-saver-enabled", Value::from(new.power_saver))].into_iter().collect();
        emit(None, DESKTOP, "org.freedesktop.DBus.Properties", "PropertiesChanged", &("org.freedesktop.portal.PowerProfileMonitor", changed, Vec::<String>::new()));
    }
    if old.as_ref().map(|o| &o.network) != Some(&new.network) {
        emit(None, DESKTOP, "org.freedesktop.portal.NetworkMonitor", "changed", &());
    }
}

struct Settings;

#[zbus::interface(name = "org.freedesktop.portal.Settings")]
impl Settings {
    fn read_all(&self, namespaces: Vec<String>) -> HashMap<String, HashMap<String, OwnedValue>> {
        let wanted = |n: &str| {
            namespaces.is_empty()
                || namespaces.iter().any(|p| p.is_empty() || p == n || p.strip_suffix('*').is_some_and(|pre| n.starts_with(pre)))
        };
        let mut out: HashMap<String, HashMap<String, OwnedValue>> = HashMap::new();
        if let Some(s) = system() {
            for (n, k, v) in settings(&s) {
                if wanted(n) {
                    out.entry(n.to_string()).or_default().insert(k.to_string(), owned(v));
                }
            }
        }
        out
    }

    /// Deprecated: the value wrapped in one more variant.
    fn read(&self, namespace: String, key: String) -> zbus::fdo::Result<OwnedValue> {
        let v = lookup(&namespace, &key).ok_or_else(|| zbus::fdo::Error::Failed("Requested setting not found".into()))?;
        Ok(owned(Value::Value(Box::new(v))))
    }

    fn read_one(&self, namespace: String, key: String) -> zbus::fdo::Result<OwnedValue> {
        lookup(&namespace, &key).map(owned).ok_or_else(|| zbus::fdo::Error::Failed("Requested setting not found".into()))
    }

    #[zbus(signal)]
    async fn setting_changed(emitter: &SignalEmitter<'_>, namespace: &str, key: &str, value: Value<'_>) -> zbus::Result<()>;

    #[zbus(property)]
    fn version(&self) -> u32 {
        2
    }
}

// ------------------------------------------------------------------ OpenURI

/// Hand a file to Android to open with an app: Lindroids' side of `xdg-open file`.
pub fn open_file(mut f: std::fs::File, name: String) -> Result<(), String> {
    let mut data = Vec::new();
    Read::by_ref(&mut f).take(MAX_OPEN + 1).read_to_end(&mut data).map_err(|e| e.to_string())?;
    if data.len() as u64 > MAX_OPEN {
        return Err(format!("{name} is too large to hand to Android ({} MB at most)", MAX_OPEN >> 20));
    }
    let len = data.len() as u32;
    match ask_once(|r| (ToApp::OpenFile { request: r, name, mime: None, len }, data), Duration::from_secs(120)) {
        Some(Answer::Open(true)) => Ok(()),
        Some(_) => Err("Android has no app to open it".into()),
        None => Err("no browser page is linked (`lindroids pair`)".into()),
    }
}

/// Lindroids' side of `xdg-open URI`.
pub fn open_uri(uri: String) -> Result<(), String> {
    match ask_once(|r| (ToApp::OpenUri { request: r, uri }, Vec::new()), Duration::from_secs(120)) {
        Some(Answer::Open(true)) => Ok(()),
        Some(_) => Err("Android has no app for it".into()),
        None => Err("no browser page is linked (`lindroids pair`)".into()),
    }
}

struct OpenUri;

#[zbus::interface(name = "org.freedesktop.portal.OpenURI")]
impl OpenUri {
    #[zbus(name = "OpenURI")]
    fn open_uri(&self, #[zbus(header)] hdr: Header<'_>, _parent_window: String, uri: String, options: HashMap<String, OwnedValue>) -> OwnedObjectPath {
        let (handle, sender) = request_path(&hdr, &options);
        let h = handle.clone();
        std::thread::spawn(move || {
            let code = match open_uri(uri) {
                Ok(()) => 0,
                Err(e) => {
                    warn!("OpenURI: {e}");
                    2
                }
            };
            respond(&sender, &h, code, HashMap::new());
        });
        handle
    }

    fn open_file(&self, #[zbus(header)] hdr: Header<'_>, _parent_window: String, fd: zbus::zvariant::OwnedFd, options: HashMap<String, OwnedValue>) -> OwnedObjectPath {
        let (handle, sender) = request_path(&hdr, &options);
        let h = handle.clone();
        let fd: std::os::fd::OwnedFd = fd.into();
        std::thread::spawn(move || {
            let name = fd_name(&fd);
            let code = match open_file(std::fs::File::from(fd), name) {
                Ok(()) => 0,
                Err(e) => {
                    warn!("OpenFile: {e}");
                    2
                }
            };
            respond(&sender, &h, code, HashMap::new());
        });
        handle
    }

    /// Android has no "show this folder" for a Linux folder.
    fn open_directory(&self, #[zbus(header)] hdr: Header<'_>, _parent_window: String, _fd: zbus::zvariant::OwnedFd, options: HashMap<String, OwnedValue>) -> OwnedObjectPath {
        let (handle, sender) = request_path(&hdr, &options);
        let h = handle.clone();
        std::thread::spawn(move || respond(&sender, &h, 2, HashMap::new()));
        handle
    }

    #[zbus(property)]
    fn version(&self) -> u32 {
        4
    }
}

/// A file's name from its descriptor.
pub fn fd_name(fd: &impl std::os::fd::AsRawFd) -> String {
    std::fs::read_link(format!("/proc/self/fd/{}", fd.as_raw_fd()))
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "file".into())
}

// ------------------------------------------------------------------ FileChooser

/// `file://` URI for a path, percent-encoded.
pub fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut s = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

/// MIME types for a portal filter list (a(sa(us)): name, [(0 glob | 1 mime, pattern)]).
fn filter_mimes(options: &HashMap<String, OwnedValue>) -> Vec<String> {
    let Some(v) = options.get("filters").and_then(|v| v.try_clone().ok()) else { return Vec::new() };
    let Ok(filters) = <Vec<(String, Vec<(u32, String)>)>>::try_from(v) else { return Vec::new() };
    let mut out: Vec<String> = Vec::new();
    for (_, rules) in filters {
        for (kind, pattern) in rules {
            let m = if kind == 1 { Some(pattern) } else { mime_for_glob(&pattern).map(String::from) };
            if let Some(m) = m {
                if !out.contains(&m) {
                    out.push(m);
                }
            }
        }
    }
    // A glob nothing matched means "anything" rather than nothing.
    if out.is_empty() {
        out
    } else if out.iter().any(|m| m == "*/*") {
        Vec::new()
    } else {
        out
    }
}

fn mime_for_glob(glob: &str) -> Option<&'static str> {
    let ext = glob.rsplit('.').next()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "*" => "*/*",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "txt" | "text" => "text/plain",
        "md" => "text/markdown",
        "html" | "htm" => "text/html",
        "csv" => "text/csv",
        "json" => "application/json",
        "zip" => "application/zip",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/x-wav",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        _ => return None,
    })
}

fn documents_dir(request: u32) -> PathBuf {
    shared().runtime.get().cloned().unwrap_or_else(|| PathBuf::from("/tmp")).join("documents").join(request.to_string())
}

/// A file name that cannot climb out of its directory.
fn safe_name(name: &str) -> String {
    let n: String = name.chars().map(|c| if c == '/' || c == '\0' { '_' } else { c }).collect();
    if n.is_empty() || n == "." || n == ".." {
        "file".into()
    } else {
        n
    }
}

/// Where a picked document is readable on the Linux side: its shared-storage path when this side
/// can read it, else a copy fetched from the app.
fn readable_path(request: u32, index: u32, f: &PickedFile) -> Result<PathBuf, String> {
    if let Some(p) = &f.path {
        if std::fs::File::open(p).is_ok() {
            return Ok(PathBuf::from(p));
        }
    }
    let dir = documents_dir(request);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dest = dir.join(safe_name(&f.name));
    let mut out = std::fs::File::create(&dest).map_err(|e| e.to_string())?;
    if shared().app_version.load(Ordering::SeqCst) < 3 {
        return Err("no browser page is linked (`lindroids pair`)".into());
    }
    let rx = listen(request);
    to_app(ToApp::FetchFile { request, index }, Vec::new());
    let result = loop {
        match rx.recv_timeout(Duration::from_secs(60)) {
            Ok(Answer::Chunk { error: Some(e), .. }) => break Err(e),
            Ok(Answer::Chunk { last, data, .. }) => {
                if let Err(e) = out.write_all(&data) {
                    break Err(e.to_string());
                }
                if last {
                    break Ok(dest);
                }
            }
            _ => break Err(format!("{}: the page stopped sending it", f.name)),
        }
    };
    forget(request);
    result
}

/// After a save into a copy, write it back into the picked document whenever it changes.
fn write_back(staging: PathBuf, uri: String) {
    std::thread::spawn(move || {
        let stamp = |p: &Path| std::fs::metadata(p).ok().map(|m| (m.modified().ok(), m.len()));
        let mut last = stamp(&staging);
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(4 * 3600) {
            std::thread::sleep(Duration::from_secs(1));
            let now = stamp(&staging);
            if now == last || now.is_none() {
                continue;
            }
            // Let the writer finish.
            std::thread::sleep(Duration::from_millis(700));
            if stamp(&staging) != now {
                continue;
            }
            last = now;
            let Ok(data) = std::fs::read(&staging) else { continue };
            let chunks: Vec<&[u8]> = if data.is_empty() { vec![&[][..]] } else { data.chunks(1 << 20).collect() };
            let n = chunks.len();
            let mut offset = 0u64;
            let (r, rx) = match ask(|r| {
                let c = chunks[0];
                (ToApp::WriteFile { request: r, uri: uri.clone(), offset: 0, last: n == 1, len: c.len() as u32 }, c.to_vec())
            }) {
                Some(x) => x,
                None => continue,
            };
            offset += chunks[0].len() as u64;
            for (i, c) in chunks.iter().enumerate().skip(1) {
                to_app(ToApp::WriteFile { request: r, uri: uri.clone(), offset, last: i + 1 == n, len: c.len() as u32 }, c.to_vec());
                offset += c.len() as u64;
            }
            match rx.recv_timeout(Duration::from_secs(120)) {
                Ok(Answer::Write(true, _)) => info!("saved {} into {uri}", staging.display()),
                Ok(Answer::Write(false, m)) => warn!("saving into {uri}: {}", m.unwrap_or_default()),
                _ => warn!("saving into {uri}: no answer"),
            }
            forget(r);
        }
    });
}

struct FileChooser;

impl FileChooser {
    fn pick(hdr: &Header<'_>, title: String, options: HashMap<String, OwnedValue>, save: bool) -> OwnedObjectPath {
        let (handle, sender) = request_path(hdr, &options);
        let h = handle.clone();
        let req = PickFiles {
            request: 0,
            title,
            save,
            multiple: !save && opt_bool(&options, "multiple"),
            directory: !save && opt_bool(&options, "directory"),
            mime_types: filter_mimes(&options),
            name: opt_str(&options, "current_name"),
        };
        std::thread::spawn(move || {
            let Some((request, rx)) = ask(|r| (ToApp::PickFiles(PickFiles { request: r, ..req }), Vec::new())) else {
                return respond(&sender, &h, 2, HashMap::new());
            };
            let answer = rx.recv_timeout(Duration::from_secs(3600));
            forget(request);
            let files = match answer {
                Ok(Answer::Picked(files)) => files,
                _ => return respond(&sender, &h, 2, HashMap::new()),
            };
            if files.is_empty() {
                return respond(&sender, &h, 1, HashMap::new());
            }
            let mut uris = Vec::new();
            if save {
                let f = &files[0];
                let direct = f.path.as_ref().map(PathBuf::from).filter(|p| {
                    p.parent().is_some_and(|d| {
                        use std::ffi::CString;
                        CString::new(d.as_os_str().as_encoded_bytes()).is_ok_and(|c| unsafe { libc::access(c.as_ptr(), libc::W_OK) } == 0)
                    })
                });
                let path = match direct {
                    Some(p) => p,
                    None => {
                        let dir = documents_dir(request);
                        let p = dir.join(safe_name(&f.name));
                        if std::fs::create_dir_all(&dir).is_err() || std::fs::File::create(&p).is_err() {
                            return respond(&sender, &h, 2, HashMap::new());
                        }
                        write_back(p.clone(), f.uri.clone());
                        p
                    }
                };
                uris.push(file_uri(&path));
            } else {
                for (i, f) in files.iter().enumerate() {
                    match readable_path(request, i as u32, f) {
                        Ok(p) => uris.push(file_uri(&p)),
                        Err(e) => warn!("FileChooser: {e}"),
                    }
                }
            }
            if uris.is_empty() {
                return respond(&sender, &h, 2, HashMap::new());
            }
            let results: HashMap<&str, Value> = [("uris", Value::from(uris))].into_iter().collect();
            respond(&sender, &h, 0, results);
        });
        handle
    }
}

#[zbus::interface(name = "org.freedesktop.portal.FileChooser")]
impl FileChooser {
    fn open_file(&self, #[zbus(header)] hdr: Header<'_>, _parent_window: String, title: String, options: HashMap<String, OwnedValue>) -> OwnedObjectPath {
        Self::pick(&hdr, title, options, false)
    }

    fn save_file(&self, #[zbus(header)] hdr: Header<'_>, _parent_window: String, title: String, options: HashMap<String, OwnedValue>) -> OwnedObjectPath {
        Self::pick(&hdr, title, options, true)
    }

    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }
}

// ------------------------------------------------------------------ notifications

fn icon_b64(path: &str) -> Option<String> {
    use base64::Engine;
    let p = path.strip_prefix("file://").unwrap_or(path);
    if !p.starts_with('/') || !p.to_ascii_lowercase().ends_with(".png") {
        return None;
    }
    let data = std::fs::read(p).ok().filter(|d| d.len() < 1 << 20)?;
    Some(base64::engine::general_purpose::STANDARD.encode(data))
}

fn note_action(id: &str, action: &str) {
    let owner = shared().notes.lock().unwrap().get(id).cloned();
    match owner {
        Some(NoteOwner::Fdo(n)) => emit(None, NOTIFICATIONS, "org.freedesktop.Notifications", "ActionInvoked", &(n, action)),
        Some(NoteOwner::Portal { sender, id }) => {
            emit(Some(&sender), DESKTOP, "org.freedesktop.portal.Notification", "ActionInvoked", &(id.as_str(), action, Vec::<Value>::new()))
        }
        None => {}
    }
}

fn note_closed(id: &str, reason: u32) {
    let owner = shared().notes.lock().unwrap().remove(id);
    if let Some(NoteOwner::Fdo(n)) = owner {
        emit(None, NOTIFICATIONS, "org.freedesktop.Notifications", "NotificationClosed", &(n, reason));
    }
}

/// `lindroids notify`.
pub fn notify(title: String, body: String) {
    let n = NEXT_NOTE.fetch_add(1, Ordering::SeqCst);
    to_app(ToApp::Notify(Notification { id: format!("cli-{n}"), app: "Terminal".into(), title, body, urgency: 1, ..Default::default() }), Vec::new());
}

struct Notifications;

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        _expire_timeout: i32,
    ) -> u32 {
        let n = if replaces_id != 0 { replaces_id } else { NEXT_NOTE.fetch_add(1, Ordering::SeqCst) };
        let id = format!("fdo-{n}");
        let urgency = match hints.get("urgency").map(|v| &**v) {
            Some(Value::U8(u)) => (*u).min(2),
            _ => 1,
        };
        let icon = icon_b64(&app_icon).or_else(|| opt_str(&hints, "image-path").and_then(|p| icon_b64(&p)));
        // Pairs of (key, label); "default" is the notification's own tap.
        let buttons = actions.chunks(2).filter(|p| p.len() == 2 && p[0] != "default").map(|p| (p[0].clone(), p[1].clone())).collect();
        shared().notes.lock().unwrap().insert(id.clone(), NoteOwner::Fdo(n));
        to_app(ToApp::Notify(Notification { id, app: app_name, title: summary, body: strip_markup(&body), icon_png_b64: icon, actions: buttons, urgency }), Vec::new());
        n
    }

    fn close_notification(&self, id: u32) {
        to_app(ToApp::CloseNotification { id: format!("fdo-{id}") }, Vec::new());
    }

    fn get_capabilities(&self) -> Vec<String> {
        ["body", "actions", "icon-static", "persistence"].map(String::from).to_vec()
    }

    fn get_server_information(&self) -> (String, String, String, String) {
        ("Lindroids".into(), "dixonSolutions".into(), crate::BUILD.into(), "1.2".into())
    }

    #[zbus(signal)]
    async fn notification_closed(emitter: &SignalEmitter<'_>, id: u32, reason: u32) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn action_invoked(emitter: &SignalEmitter<'_>, id: u32, action_key: &str) -> zbus::Result<()>;
}

/// Notification bodies may carry a little markup; Android shows plain text.
fn strip_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'")
}

struct PortalNotification;

#[zbus::interface(name = "org.freedesktop.portal.Notification")]
impl PortalNotification {
    fn add_notification(&self, #[zbus(header)] hdr: Header<'_>, id: String, notification: HashMap<String, OwnedValue>) {
        let sender = hdr.sender().map(|s| s.to_string()).unwrap_or_default();
        let key = format!("portal-{sender}-{id}");
        let urgency = match opt_str(&notification, "priority").as_deref() {
            Some("low") => 0,
            Some("high") | Some("urgent") => 2,
            _ => 1,
        };
        let buttons = notification
            .get("buttons")
            .and_then(|v| v.try_clone().ok())
            .and_then(|v| <Vec<HashMap<String, OwnedValue>>>::try_from(v).ok())
            .unwrap_or_default()
            .iter()
            .filter_map(|b| Some((opt_str(b, "action")?, opt_str(b, "label")?)))
            .collect();
        shared().notes.lock().unwrap().insert(key.clone(), NoteOwner::Portal { sender, id });
        to_app(
            ToApp::Notify(Notification {
                id: key,
                app: String::new(),
                title: opt_str(&notification, "title").unwrap_or_default(),
                body: opt_str(&notification, "body").unwrap_or_default(),
                icon_png_b64: None,
                actions: buttons,
                urgency,
            }),
            Vec::new(),
        );
    }

    fn remove_notification(&self, #[zbus(header)] hdr: Header<'_>, id: String) {
        let sender = hdr.sender().map(|s| s.to_string()).unwrap_or_default();
        to_app(ToApp::CloseNotification { id: format!("portal-{sender}-{id}") }, Vec::new());
    }

    #[zbus(signal)]
    async fn action_invoked(emitter: &SignalEmitter<'_>, id: &str, action: &str, parameter: Vec<Value<'_>>) -> zbus::Result<()>;

    #[zbus(property)]
    fn version(&self) -> u32 {
        1
    }
}

// ------------------------------------------------------------------ monitors

struct MemoryMonitor;

#[zbus::interface(name = "org.freedesktop.portal.MemoryMonitor")]
impl MemoryMonitor {
    #[zbus(signal)]
    async fn low_memory_warning(emitter: &SignalEmitter<'_>, level: u8) -> zbus::Result<()>;

    #[zbus(property)]
    fn version(&self) -> u32 {
        1
    }
}

struct NetworkMonitor;

#[zbus::interface(name = "org.freedesktop.portal.NetworkMonitor")]
impl NetworkMonitor {
    fn get_available(&self) -> bool {
        system().map_or(true, |s| s.network.available)
    }

    fn get_metered(&self) -> bool {
        system().is_some_and(|s| s.network.metered)
    }

    fn get_connectivity(&self) -> u32 {
        system().map_or(4, |s| u32::from(s.network.connectivity.clamp(1, 4)))
    }

    fn get_status(&self) -> HashMap<String, OwnedValue> {
        let s = system().unwrap_or_default();
        [
            ("available".to_string(), owned(Value::from(s.network.available))),
            ("metered".to_string(), owned(Value::from(s.network.metered))),
            ("connectivity".to_string(), owned(Value::from(u32::from(s.network.connectivity.clamp(1, 4))))),
        ]
        .into_iter()
        .collect()
    }

    fn can_reach(&self, hostname: String, port: u32) -> bool {
        use std::net::ToSocketAddrs;
        (hostname.as_str(), port as u16)
            .to_socket_addrs()
            .ok()
            .and_then(|mut a| a.next())
            .is_some_and(|a| std::net::TcpStream::connect_timeout(&a, Duration::from_secs(3)).is_ok())
    }

    #[zbus(signal)]
    async fn changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }
}

struct PowerProfileMonitor;

#[zbus::interface(name = "org.freedesktop.portal.PowerProfileMonitor")]
impl PowerProfileMonitor {
    #[zbus(property, name = "power-saver-enabled")]
    fn power_saver_enabled(&self) -> bool {
        system().is_some_and(|s| s.power_saver)
    }

    #[zbus(property)]
    fn version(&self) -> u32 {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_and_names() {
        assert_eq!(file_uri(Path::new("/sdcard/My Files/a#1.txt")), "file:///sdcard/My%20Files/a%231.txt");
        assert_eq!(safe_name("../x/y"), ".._x_y");
        assert_eq!(safe_name(".."), "file");
        assert_eq!(strip_markup("<b>Hi</b> &amp; bye"), "Hi & bye");
    }

    #[test]
    fn appearance_follows_android() {
        let s = SystemState { dark: true, accent: Some([0.2, 0.4, 0.6]), font_scale: 1.3, ..Default::default() };
        let v = settings(&s);
        let get = |n: &str, k: &str| v.iter().find(|(a, b, _)| *a == n && *b == k).map(|(_, _, v)| v.clone());
        assert_eq!(get("org.freedesktop.appearance", "color-scheme"), Some(Value::from(1u32)));
        assert_eq!(get("org.gnome.desktop.interface", "gtk-theme"), Some(Value::from("Adwaita-dark")));
        assert!(get("org.freedesktop.appearance", "accent-color").is_some());
        let light = settings(&SystemState::default());
        assert!(light.iter().any(|(n, k, v)| *n == "org.freedesktop.appearance" && *k == "color-scheme" && *v == Value::from(2u32)));
    }

    #[test]
    fn globs_become_mime_types() {
        assert_eq!(mime_for_glob("*.PNG"), Some("image/png"));
        assert_eq!(mime_for_glob("*.unknownext"), None);
    }
}
