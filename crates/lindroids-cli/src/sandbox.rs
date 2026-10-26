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
    let uid = unsafe { libc::getuid() };
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
    for (k, val) in proto::toolkit_env() {
        if std::env::var_os(k).is_none() {
            v.push((k.into(), val.into()));
        }
    }
    // The program runs with the server's runtime dir; a stale DISPLAY would send X11 apps to
    // nowhere.
    v
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
    for (label, cmd, pre) in tries {
        let mut a: Vec<&str> = pre.to_vec();
        a.extend(args.iter().copied());
        if run_quiet(cmd, &a, Duration::from_secs(8)) {
            return Some(label);
        }
    }
    None
}
