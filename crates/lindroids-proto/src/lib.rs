//! Wire types shared by the `lindroids` CLI, the server that runs inside a Linux sandbox, and the
//! display: the web page the server serves to the phone's browser (`data/web/lindroids.js` in the
//! server crate), or `lindroids debug-app` in tests. "App" in the names below is that display.
//!
//! # The link (server ⇄ display)
//!
//! A WebSocket on 127.0.0.1 from the browser, each server message one WebSocket message; or, for
//! tests, the same bytes over loopback TCP or an abstract Unix socket. Nothing on the link depends
//! on passing file descriptors.
//!
//! Each message is one line of JSON. A message with a `len` field is followed by exactly `len`
//! bytes of binary payload (frame tiles, PCM, file data).
//!
//! # The control socket (CLI ⇄ server)
//!
//! A Unix socket in the runtime dir, same uid, JSON lines; fds may travel with requests.

use serde::{Deserialize, Serialize};

pub mod auth;
pub mod fdpass;

/// Bumped whenever a message changes shape. The app and the server refuse to talk across
/// versions, and the CLI says which side needs updating.
pub const PROTOCOL_VERSION: u32 = 6;

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

/// Loopback TCP port a server listens on (and serves its web page on): fixed per uid, so a page
/// added to the home screen finds it again.
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
        // No accessibility bus here (Android's screen reader cannot read the window's canvas):
        // GTK 3 and GTK 4 would each warn about it at start.
        ("NO_AT_BRIDGE", "1"),
        ("GTK_A11Y", "none"),
        ("XDG_SESSION_TYPE", "wayland"),
    ]
}

/// `bwrap` for programs under proot. proot fakes the namespace calls bubblewrap makes and then
/// cannot do the mounts, so a real bwrap "succeeds" and leaves the sandboxed program without a
/// filesystem. Programs that try bubblewrap and fall back when namespaces are unavailable (glycin,
/// which loads GTK's images since gdk-pixbuf 2.44) then fail outright. This one says what is true
/// there, in the words glycin recognises.
pub const BWRAP_UNDER_PROOT: &str = "#!/bin/sh\n\
# Lindroids: under proot there are no namespaces, so bubblewrap cannot work here.\n\
echo \"bwrap: No permissions to create a new namespace (proot)\" >&2\n\
exit 1\n";

/// Write [`BWRAP_UNDER_PROOT`] into `dir` (created if needed).
pub fn write_proot_bin(dir: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir)?;
    let p = dir.join("bwrap");
    std::fs::write(&p, BWRAP_UNDER_PROOT)?;
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755))
}

/// PATH for a glibc/musl distro under proot, with `bin_dir` (holding the bwrap above) first.
pub fn proot_distro_path(bin_dir: &str) -> String {
    format!("{bin_dir}:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin")
}

// ------------------------------------------------------------------ server -> app

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ToApp {
    /// Answer to the app's Hello: the relay's public key and its signature over the app's nonce
    /// (`auth::relay_transcript`), and a nonce for the app to sign in turn.
    Challenge {
        nonce: String,
        proof: String,
        #[serde(default)]
        relay_key: String,
    },
    /// The app is authenticated; windows follow.
    Hello {
        version: u32,
        build: String,
        /// Human-readable name of the sandbox, e.g. "Termux" or "Debian (proot)".
        sandbox: String,
        /// Show the frame rate in a corner (the server runs with LINDROIDS_STATS=1).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        stats: bool,
    },
    /// What a run's own window shows until the program's first window does (and after the
    /// last one goes): the program's name and icon (PNG, base64).
    Run {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        icon_png_b64: Option<String>,
    },
    /// Pairing worked: this display's id, which it names when it signs from now on.
    Paired { display: String },
    /// Authentication failed. The server closes the link after this.
    AuthFailed { reason: String, pairing_open: bool },
    /// A Linux toplevel appeared and needs a window (a tab of the page).
    WindowNew {
        win: u32,
        title: String,
        app_id: String,
        /// PNG bytes, base64, if the server could resolve the app's icon.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        icon_png_b64: Option<String>,
    },
    WindowTitle { win: u32, title: String },
    /// A program asked to bring this window forward (xdg-activation): show it.
    WindowActivate { win: u32 },
    /// The pointer's shape over Linux windows, as a CSS cursor name ("none": hidden).
    Cursor { css: String },
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
    /// Ask the display for a folder and report the chosen path (a browser has none to give).
    PickFolder { request: u32 },
    /// Sound: a stream opens, gets PCM payloads, is paused/resumed, and closes.
    AudioOpen { stream: u32, rate: u32, channels: u32, format: AudioFormat },
    AudioData { stream: u32, len: u32 },
    AudioCork { stream: u32, corked: bool },
    AudioClose { stream: u32 },
    /// Microphone: capture for a PulseAudio record stream, `channels` (1 or 2) at `rate` Hz.
    /// The app answers with RecordData until RecordClose (which also comes when the stream is
    /// corked; it opens again with RecordOpen).
    RecordOpen { stream: u32, rate: u32, channels: u32 },
    RecordClose { stream: u32 },
    Log { level: String, msg: String },
    /// The server is closing this link: another display linked, or the server stops.
    Bye { reason: String },
    /// An exported program's own page (`/app/<id>/`, installed as an app) asked to start it
    /// (`Launch`): its run's window, which the page goes to.
    Navigate { url: String },

    // --- Android integration (the portal; see lindroids-server/src/portal.rs) ---
    /// Open a URI with Android (ACTION_VIEW: browser, mail, maps, dialer...). Answered with
    /// OpenResult.
    OpenUri { request: u32, uri: String },
    /// Open a Linux file with an Android app: the payload is its content (the app shares it
    /// through its FileProvider). Answered with OpenResult.
    OpenFile { request: u32, name: String, #[serde(default, skip_serializing_if = "Option::is_none")] mime: Option<String>, len: u32 },
    /// Show (or replace, same `id`) an Android notification. Taps and buttons come back as
    /// NotificationAction, dismissals as NotificationClosed.
    Notify(Notification),
    CloseNotification { id: String },
    /// Keep the screen on (a program inhibits idle: a video, a presentation), or no longer.
    WakeLock { on: bool },
    /// The run this page shows ended (its program exited with `code`), windows or not; for an
    /// error, the last lines of its error output.
    RunEnded {
        code: i32,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        output: Vec<String>,
    },
    /// Open Android's file picker (ACTION_OPEN_DOCUMENT / OPEN_DOCUMENT_TREE / CREATE_DOCUMENT).
    /// Answered with FilesPicked.
    PickFiles(PickFiles),
    /// Send the content of a picked document (FilesPicked's `files[index]`) as FileChunk
    /// payloads: the Linux side cannot read it in place.
    FetchFile { request: u32, index: u32 },
    /// Write the payload into a document picked for saving, a `writable` picked document, or a
    /// new file in a picked folder (the folder's uri + "/" + its path inside it). Answered with
    /// WriteResult. `offset` 0 truncates first; later chunks of the same write follow with
    /// growing offsets, the last one with `last`.
    WriteFile { request: u32, uri: String, offset: u64, last: bool, len: u32 },
}

/// An Android notification asked for by a Linux program (org.freedesktop.Notifications, the
/// Notification portal, `lindroids notify`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Notification {
    /// The server's key: the same id replaces the notification.
    pub id: String,
    /// Who sent it, shown as the notification's sub text ("Thunderbird").
    #[serde(default)]
    pub app: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_png_b64: Option<String>,
    /// Buttons: (action key, label). Tapping the notification itself is the action "default".
    #[serde(default)]
    pub actions: Vec<(String, String)>,
    /// 0 low, 1 normal, 2 urgent (heads-up).
    #[serde(default = "one")]
    pub urgency: u8,
}

fn one() -> u8 {
    1
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PickFiles {
    pub request: u32,
    #[serde(default)]
    pub title: String,
    /// Pick a place to save (CREATE_DOCUMENT) instead of files to open.
    #[serde(default)]
    pub save: bool,
    #[serde(default)]
    pub multiple: bool,
    /// Pick a folder (OPEN_DOCUMENT_TREE).
    #[serde(default)]
    pub directory: bool,
    /// MIME types to offer ("image/png", "text/*"); empty: anything.
    #[serde(default)]
    pub mime_types: Vec<String>,
    /// Suggested file name when saving.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// One document the user picked. For a folder, the folder itself comes first (mime
/// `inode/directory`, `rel` ""), then everything in it, folders included.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PickedFile {
    /// The document's content URI (kept by the app for FetchFile/WriteFile).
    pub uri: String,
    pub name: String,
    /// Where the same document is in Android's shared storage as a path
    /// ("/storage/emulated/0/Download/a.pdf"), when it is there: the Linux side uses it directly
    /// if it can.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default)]
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    /// The app can write changes back into it (WriteFile), not only send its content.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub writable: bool,
    /// In a picked folder: where it is inside it ("notes/a.txt").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rel: Option<String>,
}

/// What Android says about itself, for Linux programs (the Settings, NetworkMonitor,
/// PowerProfileMonitor portals; `lindroids info`). Sent after the handshake and on each change.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SystemState {
    /// Night mode (the system's dark theme).
    #[serde(default)]
    pub dark: bool,
    /// The system accent colour (Material You's primary on Android 12+), sRGB 0..1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<[f32; 3]>,
    #[serde(default)]
    pub high_contrast: bool,
    /// Animations turned off (animator duration scale 0, or "remove animations").
    #[serde(default)]
    pub reduced_motion: bool,
    /// The system font scale (1.0 is normal).
    #[serde(default = "one_f")]
    pub font_scale: f32,
    #[serde(default)]
    pub power_saver: bool,
    #[serde(default)]
    pub network: NetworkState,
    #[serde(default)]
    pub memory: MemoryState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery: Option<BatteryState>,
    /// BCP 47 language tag of the system locale ("en-AU").
    #[serde(default)]
    pub locale: String,
    /// The system time zone ("Australia/Sydney").
    #[serde(default)]
    pub timezone: String,
}

fn one_f() -> f32 {
    1.0
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct NetworkState {
    pub available: bool,
    pub metered: bool,
    /// As the NetworkMonitor portal counts: 1 local only, 2 limited, 3 captive portal, 4 full.
    pub connectivity: u8,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MemoryState {
    /// Bytes, from ActivityManager.MemoryInfo.
    pub total: u64,
    pub available: u64,
    pub low: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct BatteryState {
    /// Percent.
    pub level: u8,
    pub charging: bool,
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
        /// The page's path ("/", "/app/<id>"): the CLI brings the browser back to exactly that
        /// URL, which Firefox answers by switching to the tab instead of opening another.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        page: Option<String>,
    },
    /// Second message. A paired display: its id (`display`) and its signature over the relay's
    /// nonce (`proof`, `auth::display_transcript`). Pairing: a new public key (`public_key`), the
    /// signature made with it (`proof`: it holds the private key), and what allows pairing: a
    /// `ticket` from the page's URL, or the `code` the user read off the terminal.
    Auth {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        proof: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        public_key: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ticket: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        /// What the display calls itself (`lindroids displays`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
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
    /// The user closed the window (its tab).
    Close { win: u32 },
    /// The page is going away: its window was closed, or it is reloading. What `lindroids run`
    /// started ends unless a page links again within moments.
    Closing,
    /// The page came on screen or left it (visibilitychange). A page lost while out of sight was
    /// dropped by its browser (Android killed its process in the background), not closed.
    Shown { shown: bool },
    /// From an exported program's own page (`/app/<id>/`): start the program, or bring back the
    /// one running; the server answers with its window's URL (`Navigate`).
    Launch { id: String },
    /// From an exported program's install page: the browser installed it as an app of its own
    /// (`appinstalled`). Android may hide the new package from queries; this says it is there.
    Installed { id: String },
    Clipboard { text: String },
    FolderPicked { request: u32, path: Option<String> },
    /// Sound flow control: the app has room for this many more bytes of a stream.
    AudioCredit { stream: u32, bytes: u32 },
    /// Microphone samples for a stream RecordOpen opened: the payload is interleaved s16le PCM
    /// at the rate and channel count it asked for, whole frames, sent as captured (~20-40 ms).
    RecordData { stream: u32, len: u32 },

    // --- Android integration ---
    /// Android's state (theme, accent, network, memory, battery...): after Auth, and on changes.
    System(SystemState),
    /// Android asked the app to trim memory (onTrimMemory), as the MemoryMonitor portal's level:
    /// 50 moderate, 100 low, 255 critical.
    MemoryWarning { level: u8 },
    OpenResult { request: u32, ok: bool, #[serde(default, skip_serializing_if = "Option::is_none")] message: Option<String> },
    /// A notification was tapped ("default") or one of its buttons pressed.
    NotificationAction { id: String, action: String },
    /// A notification went away: 1 expired, 2 dismissed by the user, 3 closed by a request.
    NotificationClosed { id: String, reason: u32 },
    /// The picker's answer (empty: cancelled). For a save, the one document created.
    FilesPicked { request: u32, files: Vec<PickedFile> },
    /// Content of a picked document, answering FetchFile; the payload follows. `last` ends it
    /// (an empty last chunk with `error` set if it could not be read).
    FileChunk { request: u32, index: u32, last: bool, #[serde(default, skip_serializing_if = "Option::is_none")] error: Option<String>, len: u32 },
    WriteResult { request: u32, ok: bool, #[serde(default, skip_serializing_if = "Option::is_none")] message: Option<String> },
    /// What the page wants in the server's log: its errors, and what the browser refused.
    Log { msg: String },
    /// The user asked this window's page for another window of its program ("New window"):
    /// the server starts another instance, as `lindroids launch --new` would.
    NewInstance,
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

/// A program `lindroids export` made a home screen shortcut for; the shortcut runs
/// `lindroids launch <id>`. Kept as `<state dir>/apps/<id>.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Exported {
    /// Stable id, e.g. "org.gnome.Calculator" or "termux.xeyes": letters, digits, `.`, `-`, `_`.
    pub id: String,
    pub name: String,
    /// PNG bytes, base64.
    pub icon_png_b64: String,
    /// Arguments to `lindroids run`.
    pub run: Vec<String>,
    /// Exported with `--webapk`: the package of the app the browser installed for it (empty until
    /// one was found). Its windows open in that app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webapk: Option<String>,
    /// The export's browser tag (`--browser`): "default", "webapk", or a browser's package.
    /// Overrides the browser setting (`lindroids config browser`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser: Option<String>,
}

/// Whether `id` is safe as an exported program's id (a file name, a URL path segment, a shell word).
pub fn valid_export_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && !id.starts_with('.') && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

// ------------------------------------------------------------------ CLI <-> server

/// Requests from the CLI to a running server (local Unix socket).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum CliRequest {
    Status,
    Stop,
    /// A ticket that lets a browser pair: for a page showing the run `run` (good while it runs),
    /// or the shared page (good for 10 minutes). It travels in the page URL's fragment.
    Ticket {
        #[serde(default)]
        run: Option<u32>,
    },
    /// The paired displays (`lindroids displays`).
    Displays,
    /// Forget a display: it has to pair again (`lindroids displays forget <id>`).
    ForgetDisplay { id: String },
    PickFolder,
    /// `lindroids open`: a URI, or an absolute path whose file Android should open.
    Open { target: String },
    /// `lindroids notify`.
    Notify { title: String, body: String },
    /// The window `lindroids launch <id>` opens, when an exported program's own page asked for
    /// the launch: the server sends that page there (`Navigate`) rather than the CLI opening it.
    AppWindow { id: String, url: String },
    /// `lindroids info`: what Android reports.
    Info,
    /// The program the CLI started (`Launch`) ended, with this exit code (and, for an error, the
    /// last lines it wrote to stderr).
    RunEnded {
        pid: u32,
        code: i32,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        output: Vec<String>,
    },
    /// The CLI started a program: windows from this process (or its children) get this name and
    /// icon.
    Launch {
        pid: u32,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        icon_png_b64: Option<String>,
        /// Its one-colour icon, for Android's status bar.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        symbolic_png_b64: Option<String>,
        /// It gets a window of its own (a page opened for this run) rather than a place in the
        /// shared one.
        #[serde(default)]
        window: bool,
        /// What it is an instance of ("com.vscodium.codium", "termux.foot"): instances of the
        /// same app are numbered (`VSCodium`, `VSCodium 2`) and found again by `Instances`.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        app_key: String,
        /// Arguments to `lindroids` that start one more instance ("launch --new <id>"), for the
        /// page's "New window".
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        again: Vec<String>,
        /// The process of `lindroids run` that watches it (it sends RunEnded).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        supervisor: Option<u32>,
    },
    /// The running instances of an app (`Launch`'s `app_key`), most recently used first.
    Instances { app_key: String },
    /// The run `pid` ended at once and cleanly, as a program that hands its work to an instance
    /// already running does (VSCodium, GApplications): the next new window of that app's other
    /// instances is this run's (it shows in this run's own window). Answered when one appeared
    /// (Claimed) or none did within moments (Focused: the instance to bring forward instead).
    Claim { pid: u32 },
}

/// One running instance of an app.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Instance {
    /// Its run (the pid `lindroids run` started; for a claimed instance, the run that claimed).
    pub pid: u32,
    /// 1 for the first instance, 2 for the next... the lowest free number.
    pub ordinal: u32,
    /// Its name with the number from 2 on ("VSCodium 2").
    pub name: String,
    /// Its window (page) is linked.
    #[serde(default)]
    pub shown: bool,
    /// When it was last used (focused, or its page came on screen), in milliseconds since the
    /// server started; 0: never.
    #[serde(default)]
    pub last_focus: u64,
    /// It shows windows of another instance's process (a handed-over window), not a process
    /// of its own.
    #[serde(default)]
    pub claimed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum CliReply {
    /// A ticket, and the origin pages are served in (`http://<label>.localhost:<port>`).
    Ticket { ticket: String, origin: String },
    Displays { displays: Vec<DisplayInfo> },
    Status {
        version: u32,
        build: String,
        app_connected: bool,
        /// Build of the connected app, to tell when it needs updating.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app_build: Option<String>,
        /// Path of the linked page, if it is one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_page: Option<String>,
        /// Shown in the terminal so the user can pair the app.
        pairing_code: Option<String>,
        windows: Vec<WindowInfo>,
        clients: u32,
        /// Runs whose own window (page) is linked.
        #[serde(default)]
        shown_runs: Vec<u32>,
        /// The origin pages are served in.
        #[serde(default)]
        origin: String,
    },
    Ok { message: String },
    Folder { path: Option<String> },
    Info { system: Option<SystemState> },
    Error { message: String },
    /// Answer to Launch: the instance's number among its app's.
    Launched { ordinal: u32 },
    /// Answer to Instances: the app's, most recently used first, and how many runs (processes)
    /// are running in all.
    Instances {
        instances: Vec<Instance>,
        #[serde(default)]
        runs: u32,
    },
    /// Answer to Claim: these windows (ids) are the run's now.
    Claimed { windows: Vec<u32> },
    /// Answer to Claim: no new window appeared; `pid` is the instance to bring forward.
    Focused { pid: Option<u32> },
}

/// A paired display, as the relay keeps it (`<state>/displays/<id>.json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DisplayInfo {
    pub id: String,
    pub name: String,
    /// The display's public key (`auth`).
    pub public_key: String,
    /// When it paired, in seconds since the epoch.
    pub paired: u64,
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
            ToApp::Tiles { len, .. }
            | ToApp::AudioData { len, .. }
            | ToApp::OpenFile { len, .. }
            | ToApp::WriteFile { len, .. } => *len as usize,
            _ => 0,
        }
    }
}

impl Payload for ToServer {
    fn payload_len(&self) -> usize {
        match self {
            ToServer::FileChunk { len, .. } | ToServer::RecordData { len, .. } => *len as usize,
            _ => 0,
        }
    }
}
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
    fn record_messages_have_their_shape() {
        let line = encode(&ToApp::RecordOpen { stream: 3, rate: 48000, channels: 1 });
        assert_eq!(std::str::from_utf8(&line).unwrap(), "{\"t\":\"record_open\",\"stream\":3,\"rate\":48000,\"channels\":1}\n");
        let mut f = Framer::default();
        f.push(b"{\"t\":\"record_data\",\"stream\":3,\"len\":4}\n\x01\x00\xff\x7f{\"t\":\"closing\"}\n");
        assert_eq!(f.next::<ToServer>().unwrap().unwrap(), (ToServer::RecordData { stream: 3, len: 4 }, vec![1, 0, 0xff, 0x7f]));
        assert_eq!(f.next::<ToServer>().unwrap().unwrap().0, ToServer::Closing);
    }

    #[test]
    fn launch_from_protocol_5_still_decodes() {
        // A CLI before instances: no app_key, again, supervisor.
        let old = br#"{"t":"launch","pid":42,"name":"VSCodium","window":true}"#;
        let m: CliRequest = decode(old).unwrap();
        assert_eq!(
            m,
            CliRequest::Launch { pid: 42, name: "VSCodium".into(), icon_png_b64: None, symbolic_png_b64: None, window: true, app_key: String::new(), again: vec![], supervisor: None }
        );
        // And a new one round-trips.
        let new = CliRequest::Launch {
            pid: 7,
            name: "Mousepad".into(),
            icon_png_b64: None,
            symbolic_png_b64: None,
            window: true,
            app_key: "termux.mousepad".into(),
            again: vec!["launch".into(), "--new".into(), "termux.mousepad".into()],
            supervisor: Some(6),
        };
        let line = encode(&new);
        assert_eq!(decode::<CliRequest>(&line[..line.len() - 1]).unwrap(), new);
    }

    #[test]
    fn instance_messages_round_trip() {
        for m in [CliRequest::Instances { app_key: "org.x.Y".into() }, CliRequest::Claim { pid: 9 }] {
            let line = encode(&m);
            assert_eq!(decode::<CliRequest>(&line[..line.len() - 1]).unwrap(), m);
        }
        let i = Instance { pid: 3, ordinal: 2, name: "X 2".into(), shown: true, last_focus: 99, claimed: true };
        for r in [
            CliReply::Instances { instances: vec![i.clone()], runs: 4 },
            CliReply::Claimed { windows: vec![5, 6] },
            CliReply::Focused { pid: Some(3) },
            CliReply::Focused { pid: None },
            CliReply::Launched { ordinal: 2 },
        ] {
            let line = encode(&r);
            assert_eq!(decode::<CliReply>(&line[..line.len() - 1]).unwrap(), r);
        }
        // The page's "New window".
        assert_eq!(decode::<ToServer>(br#"{"t":"new_instance"}"#).unwrap(), ToServer::NewInstance);
        // An Instance without the optional fields.
        let j: Instance = serde_json::from_str(r#"{"pid":1,"ordinal":1,"name":"X"}"#).unwrap();
        assert!(!j.shown && !j.claimed && j.last_focus == 0);
    }

    #[test]
    fn ports_are_stable_and_in_range() {
        assert_eq!(server_tcp_port(10282), 47282);
        assert_eq!(server_tcp_port(1010282), 47282);
        assert!(server_tcp_port(19999) < 57000);
    }
}
