//! Exported bundles: Linux programs copied into the app so their shortcuts work with no terminal
//! app running.
//!
//! A bundle directory holds `manifest.json` and a file tree. It runs under the proot that ships in
//! the APK (`lib<abi>/libproot.so`), as the app's own uid, against the server that runs inside the
//! app.

use std::fs::File;
use std::io::Read;
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};


pub use lindroids_proto::{BundleKind as Kind, BundleManifest as Manifest, TERMUX_HOME, TERMUX_PREFIX};

/// Unpack a tar stream into `dest` (replacing what was there). Returns the manifest.
pub fn import(reader: impl Read, dest: &Path) -> Result<Manifest, String> {
    let tmp = dest.with_extension("partial");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let mut ar = tar::Archive::new(reader);
    ar.set_preserve_permissions(true);
    ar.set_overwrite(true);
    ar.set_unpack_xattrs(false);
    ar.unpack(&tmp).map_err(|e| format!("unpacking: {e}"))?;
    let text = std::fs::read_to_string(tmp.join("manifest.json")).map_err(|_| "the bundle has no manifest.json".to_string())?;
    let manifest: Manifest = serde_json::from_str(&text).map_err(|e| format!("manifest.json: {e}"))?;
    if dest.exists() {
        let old = dest.with_extension("old");
        let _ = std::fs::remove_dir_all(&old);
        std::fs::rename(dest, &old).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_dir_all(&old);
    }
    std::fs::rename(&tmp, dest).map_err(|e| e.to_string())?;
    Ok(manifest)
}

/// Import from a pipe fd (the app side of `lindroids export --bundle`).
pub fn import_fd(fd: OwnedFd, dest: &Path) -> Result<Manifest, String> {
    import(std::io::BufReader::with_capacity(1 << 20, File::from(fd)), dest)
}

pub struct SpawnPaths<'a> {
    /// Where libproot.so and its loaders are (the app's nativeLibraryDir).
    pub native_lib_dir: &'a Path,
    pub bundle: &'a Path,
    /// The in-app server's runtime dir (Wayland and Pulse sockets).
    pub runtime: &'a Path,
    pub log: &'a Path,
}

/// The proot command line for a bundle. Separate from `spawn` so it can be tested anywhere.
pub fn proot_command(p: &SpawnPaths, m: &Manifest, argv: &[String], extra_env: &[(String, String)]) -> Command {
    let proot = p.native_lib_dir.join("libproot.so");
    let mut cmd = Command::new(&proot);
    let tmp = p.runtime.join("proot-tmp");
    let _ = std::fs::create_dir_all(&tmp);
    cmd.env_clear();
    cmd.env("PROOT_TMP_DIR", &tmp);
    cmd.env("PROOT_LOADER", p.native_lib_dir.join("libproot-loader.so"));
    let l32 = p.native_lib_dir.join("libproot-loader32.so");
    if l32.exists() {
        cmd.env("PROOT_LOADER_32", l32);
    }
    cmd.env("LD_LIBRARY_PATH", p.native_lib_dir);
    for k in ["ANDROID_ROOT", "ANDROID_DATA", "ANDROID_ART_ROOT", "ANDROID_I18N_ROOT", "ANDROID_TZDATA_ROOT", "BOOTCLASSPATH", "EXTERNAL_STORAGE"] {
        if let Some(v) = std::env::var_os(k) {
            cmd.env(k, v);
        }
    }
    cmd.arg("--kill-on-exit");
    let rt = p.runtime.display().to_string();
    let common_env: Vec<(String, String)>;
    match &m.kind {
        Kind::Termux => {
            cmd.arg("-b").arg(format!("{}:{}", p.bundle.join("usr").display(), TERMUX_PREFIX));
            cmd.arg("-b").arg(format!("{}:{}", p.bundle.join("home").display(), TERMUX_HOME));
            cmd.arg("-w").arg(m.workdir.clone().unwrap_or_else(|| TERMUX_HOME.into()));
            common_env = vec![
                ("PREFIX".into(), TERMUX_PREFIX.into()),
                ("HOME".into(), TERMUX_HOME.into()),
                ("TMPDIR".into(), format!("{TERMUX_PREFIX}/tmp")),
                ("PATH".into(), format!("{TERMUX_PREFIX}/bin")),
                ("XDG_RUNTIME_DIR".into(), rt.clone()),
                ("WAYLAND_DISPLAY".into(), format!("{rt}/wayland-0")),
                ("PULSE_SERVER".into(), format!("unix:{rt}/pulse/native")),
            ];
        }
        Kind::Rootfs { binds } => {
            cmd.arg("-r").arg(p.bundle.join("root"));
            cmd.arg("--link2symlink");
            for b in ["/dev", "/proc", "/sys", "/storage", "/system", "/apex", "/vendor"] {
                if Path::new(b).exists() {
                    cmd.arg("-b").arg(b);
                }
            }
            cmd.arg("-b").arg("/storage/emulated/0:/sdcard");
            cmd.arg("-b").arg(format!("{rt}:/run/lindroids"));
            for (src, dst) in binds {
                cmd.arg("-b").arg(format!("{}:{}", p.bundle.join(src).display(), dst));
            }
            cmd.arg("-w").arg(m.workdir.clone().unwrap_or_else(|| "/root".into()));
            common_env = vec![
                ("HOME".into(), "/root".into()),
                ("USER".into(), "root".into()),
                ("TMPDIR".into(), "/tmp".into()),
                ("PATH".into(), "/app/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into()),
                ("XDG_RUNTIME_DIR".into(), "/run/lindroids".into()),
                ("WAYLAND_DISPLAY".into(), "wayland-0".into()),
                ("PULSE_SERVER".into(), "unix:/run/lindroids/pulse/native".into()),
            ];
        }
    }
    for (k, v) in lindroids_proto::toolkit_env() {
        cmd.env(k, v);
    }
    cmd.env("LANG", "C.UTF-8");
    for (k, v) in common_env.iter().chain(m.env.iter()).chain(extra_env.iter()) {
        cmd.env(k, v);
    }
    cmd.args(argv);
    cmd
}

/// Start a bundled program in the background. Waits (briefly) for the server's socket first.
pub fn spawn(p: &SpawnPaths, argv: &[String], extra_env: &[(String, String)]) -> Result<u32, String> {
    let text = std::fs::read_to_string(p.bundle.join("manifest.json")).map_err(|e| format!("{}: {e}", p.bundle.display()))?;
    let m: Manifest = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let sock = p.runtime.join("wayland-0");
    let t0 = Instant::now();
    while !sock.exists() && t0.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let log = File::create(p.log).map_err(|e| e.to_string())?;
    let mut cmd = proot_command(p, &m, argv, extra_env);
    cmd.stdin(Stdio::null()).stdout(log.try_clone().map_err(|e| e.to_string())?).stderr(log);
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let child = cmd.spawn().map_err(|e| format!("starting proot: {e}"))?;
    Ok(child.id())
}

/// Turn a raw fd from Java into an owned one.
///
/// # Safety
/// `fd` must be an open descriptor nobody else owns.
pub unsafe fn owned(fd: i32) -> OwnedFd {
    OwnedFd::from_raw_fd(fd)
}

pub fn bundle_dir(root: &Path, id: &str) -> PathBuf {
    let safe: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '_' }).collect();
    root.join(safe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_a_tar_stream() {
        let dir = std::env::temp_dir().join(format!("lindroids-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut b = tar::Builder::new(Vec::new());
        let m = Manifest { id: "x".into(), name: "X".into(), kind: Kind::Termux, env: vec![], workdir: None };
        let text = serde_json::to_vec(&m).unwrap();
        let mut h = tar::Header::new_gnu();
        h.set_size(text.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        b.append_data(&mut h, "manifest.json", &text[..]).unwrap();
        let mut h = tar::Header::new_gnu();
        h.set_size(3);
        h.set_mode(0o755);
        h.set_cksum();
        b.append_data(&mut h, "usr/bin/hi", &b"#!x"[..]).unwrap();
        let data = b.into_inner().unwrap();
        let dest = dir.join("x");
        let got = import(&data[..], &dest).unwrap();
        assert_eq!(got, m);
        assert!(dest.join("usr/bin/hi").exists());
        // A second import replaces the first.
        import(&data[..], &dest).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn termux_bundle_command() {
        let m = Manifest { id: "x".into(), name: "X".into(), kind: Kind::Termux, env: vec![], workdir: None };
        let p = SpawnPaths { native_lib_dir: Path::new("/lib"), bundle: Path::new("/b"), runtime: Path::new("/r"), log: Path::new("/l") };
        let c = proot_command(&p, &m, &["xeyes".into()], &[]);
        let args: Vec<String> = c.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(args.contains(&format!("/b/usr:{TERMUX_PREFIX}")));
        assert_eq!(args.last().unwrap(), "xeyes");
    }
}
