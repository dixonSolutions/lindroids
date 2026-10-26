//! Exported bundles: Linux programs copied into the app so their shortcuts work with no terminal
//! app running.
//!
//! A bundle directory holds `manifest.json` and a file tree. It runs under the proot that ships in
//! the APK (`lib<abi>/libproot.so`), as the app's own uid, against the server that runs inside the
//! app.

use std::fs::File;
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
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
            // A root of its own, with Android's system directories bound in: binding into the
            // real /data/data/com.termux would sit under directories this app cannot read, and
            // programs that walk up their paths (mkdir -p) trip over that.
            let root = p.bundle.join("root");
            let _ = std::fs::create_dir_all(&root);
            cmd.arg("-r").arg(&root);
            for b in ["/system", "/apex", "/vendor", "/odm", "/product", "/system_ext", "/linkerconfig", "/dev", "/proc", "/sys", "/storage"] {
                if Path::new(b).exists() {
                    cmd.arg("-b").arg(b);
                }
            }
            if Path::new("/storage/emulated/0").exists() {
                cmd.arg("-b").arg("/storage/emulated/0:/sdcard");
            }
            cmd.arg("-b").arg(format!("{}:{}", p.bundle.join("usr").display(), TERMUX_PREFIX));
            cmd.arg("-b").arg(format!("{}:{}", p.bundle.join("home").display(), TERMUX_HOME));
            cmd.arg("-b").arg(p.runtime);
            cmd.arg("-w").arg(m.workdir.clone().unwrap_or_else(|| TERMUX_HOME.into()));
            common_env = vec![
                ("PREFIX".into(), TERMUX_PREFIX.into()),
                ("HOME".into(), TERMUX_HOME.into()),
                ("TMPDIR".into(), format!("{TERMUX_PREFIX}/tmp")),
                ("PATH".into(), format!("{TERMUX_PREFIX}/bin")),
                ("SHELL".into(), format!("{TERMUX_PREFIX}/bin/bash")),
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
                ("PATH".into(), {
                    // bwrap that admits it cannot work under proot (lindroids_proto::BWRAP_UNDER_PROOT).
                    let _ = lindroids_proto::write_proot_bin(&p.runtime.join("proot-bin"));
                    format!("/app/bin:{}", lindroids_proto::proot_distro_path("/run/lindroids/proot-bin"))
                }),
                ("SHELL".into(), "/bin/sh".into()),
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
    // An X11 program gets a display of its own: the sockets are reserved here, inside the
    // bundle's /tmp, and handed to satellite, which starts next to the program (X11_WRAPPER).
    let x11 = match &m.x11_satellite {
        Some(sat) => {
            // (bundle's /tmp, its sh, its Xwayland, where the runtime dir is in its view)
            let (tmp, sh, xwayland, rt_inside) = match m.kind {
                Kind::Termux => (p.bundle.join("usr/tmp"), format!("{TERMUX_PREFIX}/bin/sh"), format!("{TERMUX_PREFIX}/bin/Xwayland"), p.runtime.display().to_string()),
                Kind::Rootfs { .. } => (p.bundle.join("root/tmp"), "/bin/sh".to_string(), "/usr/bin/Xwayland".to_string(), "/run/lindroids".to_string()),
            };
            // Satellite finds Xwayland on PATH: the wrapper that adds -shm first.
            let bin = p.runtime.join(format!("x11-bin-{}", m.id));
            lindroids_server::x11::write_xwayland_wrapper(&bin, &xwayland).map_err(|e| e.to_string())?;
            let bin_inside = format!("{rt_inside}/x11-bin-{}", m.id);
            let r = lindroids_server::x11::reserve(&tmp.join(".X11-unix"), false)?;
            let mut v = vec![sh, "-c".into(), X11_WRAPPER.into(), "lindroids-x11".into(), bin_inside, sat.clone(), r.display.to_string()];
            v.extend(r.fds.iter().map(|f| f.as_raw_fd().to_string()));
            v.push("--".into());
            v.extend(argv.iter().cloned());
            Some((r, v))
        }
        None => None,
    };
    let argv: &[String] = x11.as_ref().map_or(argv, |(_, v)| v.as_slice());
    let mut cmd = proot_command(p, &m, argv, extra_env);
    cmd.stdin(Stdio::null()).stdout(log.try_clone().map_err(|e| e.to_string())?).stderr(log);
    let inherit: Vec<i32> = x11.as_ref().map(|(r, _)| r.fds.iter().map(|f| f.as_raw_fd()).collect()).unwrap_or_default();
    unsafe {
        cmd.pre_exec(move || {
            libc::setsid();
            for &fd in &inherit {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
            }
            Ok(())
        });
    }
    let child = cmd.spawn().map_err(|e| format!("starting proot: {e}"))?;
    // The child has its copies of the X sockets; ours close here.
    drop(x11);
    Ok(child.id())
}

/// Runs inside the bundle: `sh -c X11_WRAPPER lindroids-x11 <wrapper dir> <satellite> <n> <fd>...
/// -- <argv>`. Satellite serves display :n on the inherited sockets and starts Xwayland (through
/// the -shm wrapper) on the first X11 connection; it ends with the program.
const X11_WRAPPER: &str = r#"
bin=$1; sat=$2; n=$3; shift 3
l=
while [ "$1" != "--" ]; do l="$l -listenfd $1"; shift; done
shift
PATH="$bin:$PATH" "$sat" ":$n" $l &
s=$!
DISPLAY=":$n" _JAVA_AWT_WM_NONREPARENTING=1 "$@"
rc=$?
kill $s 2>/dev/null
exit $rc
"#;

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
        let m = Manifest { id: "x".into(), name: "X".into(), kind: Kind::Termux, env: vec![], workdir: None, x11_satellite: None };
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
        let m = Manifest { id: "x".into(), name: "X".into(), kind: Kind::Termux, env: vec![], workdir: None, x11_satellite: None };
        let p = SpawnPaths { native_lib_dir: Path::new("/lib"), bundle: Path::new("/b"), runtime: Path::new("/r"), log: Path::new("/l") };
        let c = proot_command(&p, &m, &["xeyes".into()], &[]);
        let args: Vec<String> = c.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(args.contains(&format!("/b/usr:{TERMUX_PREFIX}")));
        assert_eq!(args.last().unwrap(), "xeyes");
    }
}
