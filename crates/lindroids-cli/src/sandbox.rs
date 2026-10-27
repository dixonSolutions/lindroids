//! Where the CLI is running (Termux, a glibc distro under proot, plain Linux), and the environment
//! Linux programs need to find the server.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use lindroids_proto::{self as proto, TERMUX_PREFIX};

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// Termux (or a fork using the same prefix).
    Termux,
    /// A glibc/musl distro under proot on Android (UserLAnd, Andronix, AnLinux, proot-distro login).
    Proot,
    /// Not on Android: a desktop, for testing the server against real clients.
    Desktop,
}

#[derive(Debug, Clone)]
pub struct Sandbox {
    pub kind: Kind,
    pub name: String,
    pub uid: u32,
}

pub fn detect() -> Sandbox {
    let uid = lindroids_proto::real_uid();
    let prefix = std::env::var("PREFIX").unwrap_or_default();
    let on_android = Path::new("/system/build.prop").exists() || Path::new("/system/bin/app_process").exists();
    if prefix.contains("/com.termux") || (on_android && Path::new(TERMUX_PREFIX).exists() && std::env::var_os("TERMUX_VERSION").is_some()) {
        return Sandbox { kind: Kind::Termux, name: "Termux".into(), uid };
    }
    let pretty = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|t| t.lines().find(|l| l.starts_with("PRETTY_NAME=")).map(|l| l[12..].trim_matches('"').to_string()))
        .unwrap_or_else(|| "Linux".into());
    if on_android {
        Sandbox { kind: Kind::Proot, name: pretty, uid }
    } else {
        Sandbox { kind: Kind::Desktop, name: pretty, uid }
    }
}

/// The server's runtime dir: LINDROIDS_RUNTIME_DIR, which Lindroids gives the programs it starts
/// (in a proot distro the uid is a faked 0, so the default would name another directory), else
/// the default for this uid.
pub fn runtime_dir() -> PathBuf {
    std::env::var_os("LINDROIDS_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_dir()).unwrap_or_else(lindroids_server::default_runtime_dir)
}

/// Environment for a program running in this sandbox (runtime dir as seen by the program).
pub fn client_env(runtime: &Path) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = vec![
        ("XDG_RUNTIME_DIR".into(), runtime.display().to_string()),
        ("WAYLAND_DISPLAY".into(), "wayland-0".into()),
        ("PULSE_SERVER".into(), format!("unix:{}/pulse/native", runtime.display())),
    ];
    // Minimal distros start with LANG unset or "C"; terminals and toolkits want UTF-8.
    if std::env::var("LANG").map_or(true, |l| l.is_empty() || l == "C" || l == "POSIX") {
        v.push(("LANG".into(), "C.UTF-8".into()));
    }
    for (k, val) in proto::toolkit_env() {
        if std::env::var_os(k).is_none() {
            v.push((k.into(), val.into()));
        }
    }
    v
}

/// Desktop integration for a program: the server's session bus (where the Android-backed portal
/// and notification service are), toolkits pointed at the portal, `xdg-open` that hands links and
/// files to Android, and the way back to the server. `runtime` is the runtime dir as this
/// process sees it, `inside` as the program will.
pub fn integration_env(runtime: &Path, inside: &Path) -> Vec<(String, String)> {
    let mut v = vec![("LINDROIDS_RUNTIME_DIR".to_string(), inside.display().to_string())];
    if runtime.join("bus").exists() {
        v.push(("DBUS_SESSION_BUS_ADDRESS".into(), format!("unix:path={}", inside.join("bus").display())));
        // GLib and GTK 3 use the portal (open, file chooser, settings) only when asked to outside
        // a Flatpak; GTK 4 likewise with GDK_DEBUG=portals; Qt through its portal theme.
        v.push(("GTK_USE_PORTAL".into(), "1".into()));
        if std::env::var_os("GDK_DEBUG").is_none() {
            v.push(("GDK_DEBUG".into(), "portals".into()));
        }
        if std::env::var_os("QT_QPA_PLATFORMTHEME").is_none() {
            v.push(("QT_QPA_PLATFORMTHEME".into(), "xdgdesktopportal".into()));
        }
    }
    v
}

/// XDG_DATA_DIRS with `<runtime>/share` first (the portal's GTK 3 dark theme), as the program
/// sees it, before `base`: the data dirs it would have had.
pub fn data_dirs_with_runtime(runtime: &Path, inside: &Path, base: &str) -> Option<String> {
    runtime.join("share/themes").is_dir().then(|| format!("{}:{base}", inside.join("share").display()))
}

/// The data dirs a program here has when XDG_DATA_DIRS is not set.
pub fn default_data_dirs() -> String {
    match std::env::var("XDG_DATA_DIRS") {
        Ok(d) if !d.is_empty() => d,
        _ if detect().kind == Kind::Termux => format!("{p}/local/share:{p}/share", p = lindroids_proto::TERMUX_PREFIX),
        _ => "/usr/local/share:/usr/share".into(),
    }
}

/// `<runtime>/bin`: `xdg-open` (and `sensible-browser`, `x-www-browser`) that hand links and
/// files to Android through `lindroids open`, with a copy of this binary for them to run.
/// Returns the directory as the program sees it, to put first on its PATH.
pub fn write_open_helpers(runtime: &Path, inside: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let bin = runtime.join("bin");
    std::fs::create_dir_all(&bin).ok()?;
    let me = std::env::current_exe().ok()?;
    let copy = bin.join("lindroids");
    let fresh = std::fs::metadata(&copy).ok().and_then(|m| m.modified().ok()) >= std::fs::metadata(&me).ok().and_then(|m| m.modified().ok());
    if !copy.exists() || !fresh {
        let tmp = bin.join(".lindroids.new");
        if std::fs::hard_link(&me, &tmp).is_err() {
            std::fs::copy(&me, &tmp).ok()?;
        }
        std::fs::rename(&tmp, &copy).ok()?;
    }
    let script = "#!/bin/sh\n# Lindroids: links and files open with Android.\nexec \"$(dirname \"$0\")/lindroids\" open \"$@\"\n";
    for name in ["xdg-open", "sensible-browser", "x-www-browser"] {
        let p = bin.join(name);
        if std::fs::read_to_string(&p).ok().as_deref() != Some(script) {
            std::fs::write(&p, script).ok()?;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).ok()?;
        }
    }
    Some(inside.join("bin"))
}

/// The server's integrated X display (`:N`), if it has one; `runtime` as this process sees it.
pub fn x11_display(runtime: &Path) -> Option<String> {
    let d = std::fs::read_to_string(runtime.join("x11-display")).ok()?;
    let d = d.trim();
    d.starts_with(':').then(|| d.to_string())
}

/// DISPLAY for programs started here: the integrated display, so X11-only programs (and X11
/// helpers of Wayland ones) get Android windows too. Without one, DISPLAY is removed rather than
/// left pointing at the terminal's own X server or nowhere.
pub fn x11_env(runtime: &Path) -> (Vec<(String, String)>, bool) {
    match x11_display(runtime) {
        Some(d) => (
            vec![
                ("DISPLAY".into(), d),
                // Java's AWT draws nothing under a non-reparenting window manager unless told.
                ("_JAVA_AWT_WM_NONREPARENTING".into(), "1".into()),
            ],
            true,
        ),
        None => (Vec::new(), false),
    }
}

pub fn which(cmd: &str) -> Option<PathBuf> {
    if cmd.contains('/') {
        return Path::new(cmd).exists().then(|| PathBuf::from(cmd));
    }
    std::env::var("PATH").ok()?.split(':').map(|d| Path::new(d).join(cmd)).find(|p| p.is_file())
}

pub fn have(cmd: &str) -> bool {
    which(cmd).is_some()
}

fn run_quiet(cmd: &str, args: &[&str], timeout: Duration) -> bool {
    let Some(path) = which(cmd) else { return false };
    let Ok(mut child) = Command::new(path).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else {
        return false;
    };
    let t0 = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) => return st.success(),
            Ok(None) if t0.elapsed() < timeout => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// Whether this runs on Android (Termux, or a proot distro inside it), as opposed to a desktop.
pub fn on_android() -> bool {
    Path::new("/system/build.prop").exists() || Path::new("/system/bin/app_process").exists()
}

/// Open `url` in the web browser. Returns how, if a way worked.
///
/// On Android this is an ordinary VIEW intent from the terminal app, which needs no permission
/// and no setting. Chrome reuses the tab an app opened before when the intent names that app, so
/// every `lindroids run` brings the same Lindroids tab to the front instead of opening another.
pub fn open_browser(url: &str) -> Option<&'static str> {
    let termux_am = format!("{}/bin/termux-am", proto::TERMUX_PREFIX);
    let intent: Vec<&str> = vec![
        "start", "-a", "android.intent.action.VIEW", "-d", url, "--es", "com.android.browser.application_id", "lindroids",
    ];
    // Termux's am socket server (fast, and the one that works from a proot distro too), Termux's
    // app_process am, Android's own am; then the desktop ways.
    let mut tries: Vec<(&'static str, String, Vec<&str>)> = vec![
        ("termux-am", "termux-am".into(), intent.clone()),
        ("termux-am", termux_am, intent.clone()),
        ("am", "am".into(), intent.clone()),
        ("/system/bin/am", "/system/bin/am".into(), intent.clone()),
    ];
    if !on_android() {
        tries.clear();
    }
    tries.push(("xdg-open", "xdg-open".into(), vec![url]));
    // Termux's `am` runs app_process, which on x86 images with an ARM translation layer loads the
    // native bridge and is killed by seccomp (a tombstone each time): skip it there.
    let bridged = native_bridge();
    for (label, cmd, args) in tries {
        if bridged && cmd == "am" {
            continue;
        }
        if run_quiet(&cmd, &args, Duration::from_secs(8)) {
            return Some(label);
        }
    }
    None
}

/// A page's URL in `origin` (the server's, `http://<label>.localhost:<port>`): for the run `run`
/// (what `lindroids run` started, by pid) or the shared page; `window`: a window of its own
/// (Android), which goes full screen on the first tap; `ticket`: lets a browser that has not
/// paired yet pair. The fragment never leaves the browser, and the page drops it at once. The
/// same run gets the same URL (its ticket lasts while it runs), so opening it again brings back
/// the same window.
pub fn page_url(origin: &str, run: Option<u32>, window: bool, ticket: Option<&str>) -> String {
    let mut query = Vec::new();
    if window {
        query.push("window".to_string());
    }
    if let Some(pid) = run {
        query.push(format!("run={pid}"));
    }
    // Frame rate and drawing time in a corner of the window (LINDROIDS_STATS=1).
    if std::env::var("LINDROIDS_STATS").is_ok_and(|v| v == "1") {
        query.push("stats".to_string());
    }
    let mut url = format!("{origin}/");
    if !query.is_empty() {
        url.push('?');
        url.push_str(&query.join("&"));
    }
    if let Some(t) = ticket {
        url.push_str("#t=");
        url.push_str(t);
    }
    url
}

/// liblindroids-compat.so for this CPU, empty where none is needed (see build.rs).
const COMPAT_SO: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/liblindroids-compat.so"));

/// On x86 Android, glibc programs under proot need the alarm() of liblindroids-compat.so
/// (third_party/compat). Writes it into `dir` and returns the file name, if this is such a system.
pub fn write_compat_preload(dir: &Path) -> Option<&'static str> {
    let on_android = Path::new("/system/build.prop").exists() || Path::new("/system/bin/app_process").exists();
    if COMPAT_SO.is_empty() || !on_android {
        return None;
    }
    std::fs::create_dir_all(dir).ok()?;
    let p = dir.join("liblindroids-compat.so");
    if std::fs::read(&p).ok().as_deref() != Some(COMPAT_SO) {
        std::fs::write(&p, COMPAT_SO).ok()?;
    }
    Some("liblindroids-compat.so")
}

/// LD_PRELOAD with `lib` added to what the program would have had.
pub fn preload_with(lib: &str, existing: Option<&str>) -> String {
    match existing.filter(|e| !e.trim().is_empty()) {
        Some(e) if e.split([' ', ':']).any(|x| x == lib) => e.to_string(),
        Some(e) => format!("{lib} {e}"),
        None => lib.to_string(),
    }
}

/// An x86 Android image with an ARM translation layer (houdini, libndk_translation): app_process
/// started from a terminal app is killed by seccomp there, so `am` and `termux-open` cannot work.
pub fn native_bridge() -> bool {
    Command::new("/system/bin/getprop")
        .arg("ro.dalvik.vm.native.bridge")
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .map_or(false, |v| !v.is_empty() && v != "0")
}
