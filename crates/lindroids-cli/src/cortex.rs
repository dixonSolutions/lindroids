//! Cortex Terminal (github.com/ssfuisu/cortex): Ubuntu on Android without proot. Its paths (/usr,
//! /etc, /tmp ...) are mapped into the app's files by libcortex-hook.so, an LD_PRELOAD library
//! that only dynamically linked glibc programs load. The static CLI would see Android's own file
//! system there (no programs, no Xwayland, no /etc/os-release), so it hands over to the glibc
//! build the arm .debs and tarballs carry (`libexec/lindroids/lindroids-glibc`,
//! scripts/build-glibc-cli.sh), which runs as any Ubuntu program does there.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Cortex's root (`CORTEX_ROOT`), when this runs in a Cortex session with its hook (normal mode;
/// root mode is a real chroot, where nothing needs mapping).
pub fn root() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("CORTEX_ROOT")?);
    let hooked = std::env::var("LD_PRELOAD").map_or(false, |p| p.contains("libcortex-hook"));
    (hooked && root.is_dir()).then_some(root)
}

/// In Cortex, the static CLI execs the glibc one through Ubuntu's loader, as the hook itself
/// starts Ubuntu's programs (the kernel cannot find the loader at its Linux path). Returns only
/// when this is not needed or not possible, after saying why.
pub fn hand_over() {
    if !cfg!(target_env = "musl") {
        return;
    }
    let Some(root) = root() else { return };
    let Some(glibc) = glibc_cli(&root) else {
        eprintln!("lindroids: in Cortex this needs lindroids-glibc, which this install lacks: reinstall with the install script (apt install --reinstall lindroids)");
        return;
    };
    let Some((loader, libs)) = loader(&root) else {
        eprintln!("lindroids: Ubuntu's dynamic loader is not in {}", root.display());
        return;
    };
    let err = Command::new(&loader)
        .arg("--library-path")
        .arg(&libs)
        .arg("--argv0")
        .arg(std::env::args_os().next().unwrap_or_else(|| "lindroids".into()))
        .arg(&glibc)
        .args(std::env::args_os().skip(1))
        .env("CORTEX_REAL_EXE", &glibc)
        .exec();
    eprintln!("lindroids: cannot start {}: {err}", glibc.display());
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

/// Ubuntu's loader for this CPU and its library directories, under `root`.
fn loader(root: &Path) -> Option<(PathBuf, String)> {
    let (multiarch, ld) = match std::env::consts::ARCH {
        "aarch64" => ("aarch64-linux-gnu", "ld-linux-aarch64.so.1"),
        "arm" => ("arm-linux-gnueabihf", "ld-linux-armhf.so.3"),
        "x86_64" => ("x86_64-linux-gnu", "ld-linux-x86-64.so.2"),
        _ => return None,
    };
    let dirs = [format!("usr/lib/{multiarch}"), format!("lib/{multiarch}"), "usr/lib".into(), "lib".into()];
    let ld = dirs.iter().map(|d| root.join(d).join(ld)).find(|p| p.exists())?;
    let libs = dirs.iter().map(|d| root.join(d)).filter(|p| p.is_dir()).map(|p| p.display().to_string()).collect::<Vec<_>>().join(":");
    Some((ld, libs))
}

/// Ubuntu's locales as glibc must be told about them in Cortex, whose hook misses glibc's own
/// opens of `/usr/lib/locale`: without LOCPATH, setlocale() fails even for C.UTF-8 (foot then
/// refuses to start its shell). Not when locales were compiled into locale-archive, which
/// LOCPATH would hide.
pub fn locale_dir() -> Option<PathBuf> {
    let dir = root()?.join("usr/lib/locale");
    (dir.join("C.utf8").is_dir() && !dir.join("locale-archive").exists()).then_some(dir)
}

/// Open `url` with Android through Cortex's own bridge (`cortex-api open`; it works while Cortex
/// is on screen, as it is when `lindroids run` was typed there).
pub fn open(url: &str) -> bool {
    Command::new("cortex-api")
        .args(["open", url])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_or(false, |s| s.success())
}
