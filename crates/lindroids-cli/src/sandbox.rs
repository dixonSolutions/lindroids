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

pub fn runtime_dir() -> PathBuf {
    lindroids_server::default_runtime_dir()
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

fn which(cmd: &str) -> Option<PathBuf> {
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

/// Wake the app and tell it where the server is. Tries every way Android offers a terminal app to
/// send an intent; returns which one worked, if any.
pub fn wake_app(sb: &Sandbox, secret: &str) -> Option<&'static str> {
    let socket = proto::server_socket_name(sb.uid);
    let uid = sb.uid.to_string();
    let component = format!("{}/.HelloReceiver", proto::APP_PACKAGE);
    let args: Vec<&str> = vec![
        "broadcast", "-n", &component, "-a", proto::ACTION_SERVER_HELLO,
        "--es", "socket", &socket, "--es", "uid", &uid, "--es", "sandbox", &sb.name, "--es", "secret", secret,
    ];
    // Termux's am socket server (fast), Termux's app_process am, Android's own am, then cmd.
    let tries: [(&str, &str, &[&str]); 4] = [
        ("termux-am", "termux-am", &[]),
        ("am", "am", &[]),
        ("/system/bin/am", "/system/bin/am", &[]),
        ("cmd", "/system/bin/cmd", &["activity"]),
    ];
    // Termux's `am` runs app_process, which on x86 images with an ARM translation layer loads the
    // native bridge and is killed by seccomp (a tombstone each time): skip it there.
    let bridged = Command::new("/system/bin/getprop")
        .arg("ro.dalvik.vm.native.bridge")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .map_or(false, |v| !v.is_empty() && v != "0");
    for (label, cmd, pre) in tries {
        if bridged && label == "am" {
            continue;
        }
        let mut a: Vec<&str> = pre.to_vec();
        a.extend(args.iter().copied());
        if run_quiet(cmd, &a, Duration::from_secs(8)) {
            return Some(label);
        }
    }
    None
}
