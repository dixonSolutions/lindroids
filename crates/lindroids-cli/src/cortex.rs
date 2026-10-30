//! Linux roots mapped by a preloaded library (lindroids_server::host::Layer::Preload): Cortex
//! Terminal (github.com/ssfuisu/cortex), or another app that does the same. `/usr`, `/etc`,
//! `/tmp` ... are mapped into the app's files by an LD_PRELOAD library that only dynamically
//! linked glibc programs load. The static CLI would see Android's own file system there (no
//! programs, no Xwayland, no /etc/os-release), so it hands over to the glibc build the arm .debs
//! and tarballs carry (`libexec/lindroids/lindroids-glibc`, scripts/build-glibc-cli.sh), which
//! runs as any Ubuntu program does there. What such a root gets wrong for GUI programs is put
//! right here too (`prepare`, `locale_dir`), and Cortex's own bridge to Android is used.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use lindroids_server::host::{self, Host};

/// The mapped root (real path), when this runs in one.
pub fn root() -> Option<PathBuf> {
    host::detect().mapped_root().map(Path::to_path_buf)
}

/// In a mapped root, the static CLI execs the glibc one through the root's loader, as the
/// mapping library itself starts the root's programs (the kernel cannot find the loader at its
/// Linux path). Returns only when this is not needed or not possible, after saying why.
pub fn hand_over() {
    if !cfg!(target_env = "musl") || std::env::var_os("LINDROIDS_NO_HANDOVER").is_some() {
        return;
    }
    let host = host::detect();
    let Some(root) = host.mapped_root() else { return };
    let Some(glibc) = glibc_cli(root) else {
        eprintln!("lindroids: {} needs lindroids-glibc, which this install lacks: reinstall with the install script (apt install --reinstall lindroids)", place(&host));
        return;
    };
    let Some((loader, libs)) = loader(root) else {
        eprintln!("lindroids: no glibc loader for this CPU in {}", root.display());
        return;
    };
    let err = Command::new(&loader)
        .arg("--library-path")
        .arg(&libs)
        .arg("--argv0")
        .arg(std::env::args_os().next().unwrap_or_else(|| "lindroids".into()))
        .arg(&glibc)
        .args(std::env::args_os().skip(1))
        // Cortex's library answers /proc/self/exe with this.
        .env("CORTEX_REAL_EXE", &glibc)
        .exec();
    eprintln!("lindroids: cannot start {}: {err}", glibc.display());
}

fn place(host: &Host) -> String {
    host.app_label().unwrap_or_else(|| "this Linux".into())
}

/// lindroids-glibc: next to this binary's prefix (`<bin>/../libexec/lindroids`), else where the
/// .deb puts it. Real paths, as the kernel sees them.
fn glibc_cli(root: &Path) -> Option<PathBuf> {
    let rel = "libexec/lindroids/lindroids-glibc";
    let mut candidates = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        candidates.push(dir.join("..").join(rel));
    }
    candidates.push(root.join("usr").join(rel));
    candidates.push(root.join("usr/local").join(rel));
    candidates.into_iter().find(|p| p.is_file())
}

/// The root's loader for this CPU and its library directories.
fn loader(root: &Path) -> Option<(PathBuf, String)> {
    let (multiarch, ld) = match std::env::consts::ARCH {
        "aarch64" => ("aarch64-linux-gnu", "ld-linux-aarch64.so.1"),
        "arm" => ("arm-linux-gnueabihf", "ld-linux-armhf.so.3"),
        "x86_64" => ("x86_64-linux-gnu", "ld-linux-x86-64.so.2"),
        "x86" => ("i386-linux-gnu", "ld-linux.so.2"),
        _ => return None,
    };
    let dirs = [format!("usr/lib/{multiarch}"), format!("lib/{multiarch}"), "usr/lib".into(), "lib".into()];
    let ld = dirs.iter().map(|d| root.join(d).join(ld)).find(|p| p.exists())?;
    let libs = dirs.iter().map(|d| root.join(d)).filter(|p| p.is_dir()).map(|p| p.display().to_string()).collect::<Vec<_>>().join(":");
    Some((ld, libs))
}

/// The root's locales as glibc must be told about them: the mapping misses glibc's own opens of
/// `/usr/lib/locale`, so without LOCPATH setlocale() fails even for C.UTF-8 (foot then refuses to
/// start its shell). Not when locales were compiled into locale-archive, which LOCPATH would hide.
pub fn locale_dir() -> Option<PathBuf> {
    let dir = root()?.join("usr/lib/locale");
    (dir.join("C.utf8").is_dir() && !dir.join("locale-archive").exists()).then_some(dir)
}

/// Before a program starts: build what such a root skipped. Cortex replaces update-mime-database
/// with a no-op (dpkg-divert, for faster installs), so `/usr/share/mime/mime.cache` never exists,
/// GIO cannot tell an SVG from anything else, and GTK aborts on its first SVG icon. Runs the real
/// one (`.distrib`) when the cache is missing or older than its sources.
pub fn prepare() {
    if root().is_none() {
        return;
    }
    let mime = Path::new("/usr/share/mime");
    let newest_source = std::fs::read_dir(mime.join("packages"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .max();
    let cache = std::fs::metadata(mime.join("mime.cache")).ok().and_then(|m| m.modified().ok());
    let stale = match (newest_source, cache) {
        (Some(_), None) => true,
        (Some(src), Some(c)) => src > c,
        _ => false,
    };
    if !stale {
        return;
    }
    let tool = ["/usr/bin/update-mime-database.distrib", "/usr/bin/update-mime-database"].into_iter().find(|t| Path::new(t).is_file());
    if let Some(tool) = tool {
        let ok = Command::new(tool).arg(mime).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().map_or(false, |s| s.success());
        if !ok || !mime.join("mime.cache").exists() {
            eprintln!("lindroids: no MIME database (GTK programs may fail on SVG icons): run {tool} {}", mime.display());
        }
    }
}

/// Open `url` with Android through Cortex's own bridge (`cortex-api open`; it works while Cortex
/// is on screen, as it is when `lindroids run` was typed there).
pub fn open(url: &str) -> bool {
    Command::new("cortex-api")
        .args(["open", url])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_or(false, |s| s.success())
}
