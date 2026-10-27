//! Running Flatpak apps without bubblewrap.
//!
//! On Android, `flatpak run` cannot work: bubblewrap needs user namespaces and a seccomp policy
//! an app sandbox does not allow. What a GUI app actually depends on from bwrap is a filesystem
//! view, and proot builds that view with ptrace alone: the runtime at `/usr`, the app at `/app`,
//! extensions at their mount points, an `/etc` from the runtime with a few host files on top, and
//! `/lib`, `/bin`, ... as symlinks into `/usr` (Flatpak's merged-usr layout).
//!
//! The installation is read straight from disk (no `flatpak` binary, no D-Bus), under any root
//! filesystem: `/`, a proot-distro rootfs, or a toolbox's `/run/host`.
//!
//! The session bus is the server's (with its desktop portal and notification service), handed
//! to the app like to every other program (`sandbox::integration_env`). Not reproduced: the D-Bus
//! proxy (the app sees the whole session bus), seccomp, `/.flatpak-info`, and `filesystems=`
//! beyond absolute paths (the home directory is always bound).

mod install;
mod keyfile;

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::run::Opts;
pub use install::{Deploy, Extension, Installation};

/// An installed Flatpak app, resolved down to the directories its sandbox is made of.
#[derive(Debug, Clone)]
pub struct App {
    pub id: String,
    /// The filesystem the installation was found under ("/", a distro rootfs, "/run/host").
    pub root: PathBuf,
    /// The app's deploy (`files/` is `/app`; `export/` has its .desktop file and icons).
    pub app: Deploy,
    /// `[Application] runtime=`, e.g. `org.gnome.Platform/x86_64/49`.
    pub runtime_ref: String,
    /// The runtime's deploy (`files/` is `/usr`); `None` when it is not installed.
    pub runtime: Option<Deploy>,
    /// Installed extensions of the runtime, then of the app.
    pub extensions: Vec<Extension>,
    /// `[Application] command=`.
    pub command: Option<String>,
    /// Installations under `root`; `Deploy::installation` and `Extension::installation` index
    /// into this.
    pub installations: Vec<Installation>,
}

/// org.gnome.Calculator, io.github.foo.Bar: at least two dots, no slashes.
pub fn looks_like_id(s: &str) -> bool {
    s.matches('.').count() >= 2 && !s.contains('/') && s.split('.').all(|p| !p.is_empty())
}

/// Find the installed app `id` under `root`. With root "/", also looks in `/run/host` (a toolbox
/// or distrobox, where the host's Flatpaks are) and in Termux's proot-distro distros (flatpak
/// does not exist for Termux itself).
pub fn find(root: &Path, id: &str) -> Option<App> {
    if !looks_like_id(id) {
        return None;
    }
    if let Some(app) = find_in(root, id) {
        return Some(app);
    }
    if root != Path::new("/") {
        return None;
    }
    let host = Path::new("/run/host");
    if host.join("var/lib/flatpak").is_dir() {
        if let Some(app) = find_in(host, id) {
            return Some(app);
        }
    }
    crate::run::distro_rootfses().iter().find_map(|d| find_in(d, id))
}

fn find_in(root: &Path, id: &str) -> Option<App> {
    let installations = install::installations(root);
    let arch = install::arch();
    let app = install::find_ref(root, &installations, None, "app", id, arch, None)?;
    let runtime_ref = app.metadata.get("Application", "runtime").unwrap_or("").to_string();
    let mut parts = runtime_ref.split('/');
    let name = parts.next().unwrap_or("");
    let rarch = parts.next().filter(|a| !a.is_empty()).unwrap_or(arch);
    let rbranch = parts.next().filter(|b| !b.is_empty());
    let runtime = (!name.is_empty()).then(|| install::find_ref(root, &installations, Some(app.installation), "runtime", name, rarch, rbranch)).flatten();
    let mut extensions = Vec::new();
    if let Some(rt) = &runtime {
        extensions.extend(install::extensions(root, &installations, &rt.metadata, "/usr", &rt.branch, rarch, rt.installation));
    }
    for e in install::extensions(root, &installations, &app.metadata, "/app", &app.branch, arch, app.installation) {
        if !extensions.iter().any(|x| x.mount == e.mount) {
            extensions.push(e);
        }
    }
    let command = app.metadata.get("Application", "command").map(String::from);
    Some(App { id: id.into(), root: root.to_path_buf(), app, runtime_ref, runtime, extensions, command, installations })
}

impl App {
    fn runtime(&self) -> Result<&Deploy, String> {
        self.runtime.as_ref().ok_or_else(|| format!("{} needs the runtime {}, which is not installed", self.id, self.runtime_ref))
    }

    /// The program to start, as a path inside the sandbox.
    fn program(&self) -> Result<String, String> {
        let cmd = self.command.as_deref().filter(|c| !c.is_empty()).ok_or_else(|| format!("{} names no command in its metadata", self.id))?;
        if cmd.starts_with('/') {
            return Ok(cmd.into());
        }
        // symlink_metadata: an /app/bin entry is often a symlink absolute to /app.
        if self.app.files().join("bin").join(cmd).symlink_metadata().is_ok() {
            return Ok(format!("/app/bin/{cmd}"));
        }
        Ok(format!("/usr/bin/{cmd}"))
    }
}

/// One piece of the sandbox's filesystem that comes from the installation.
#[derive(Debug, Clone, PartialEq)]
enum Piece {
    /// A deployed tree: the runtime, the app, an extension.
    Tree { src: PathBuf, dst: String },
    /// A directory of a mounted extension shown a second time at its point's merged directory
    /// (`merge-dirs`: Vulkan ICDs, EGL vendors, DRI drivers).
    Merge { src: PathBuf, ext: String, sub: String, dst: String },
}

/// The installation's part of the view, the same for `run` and for bundles, parents first.
fn pieces(app: &App) -> Result<Vec<Piece>, String> {
    let rt = app.runtime()?;
    let mut v = vec![
        Piece::Tree { src: rt.files(), dst: "/usr".into() },
        Piece::Tree { src: app.app.files(), dst: "/app".into() },
    ];
    let mut exts: Vec<&Extension> = app.extensions.iter().collect();
    exts.sort_by_key(|e| e.mount.matches('/').count());
    for e in &exts {
        v.push(Piece::Tree { src: e.files.clone(), dst: e.mount.clone() });
    }
    // flatpak merges these directories from all of a point's extensions; with one provider (the
    // usual case: GL.default alone) that is just the provider's directory.
    let mut merged: Vec<String> = Vec::new();
    for e in &exts {
        if e.mount == e.point {
            continue;
        }
        for d in &e.merge_dirs {
            let d = d.trim_matches('/');
            let src = e.files.join(d);
            let dst = format!("{}/{d}", e.point);
            if src.is_dir() && !merged.contains(&dst) {
                merged.push(dst.clone());
                v.push(Piece::Merge { src, ext: e.id.clone(), sub: d.into(), dst });
            }
        }
    }
    Ok(v)
}

/// `LD_LIBRARY_PATH` inside the sandbox. flatpak runs ldconfig over `/app/lib` and the
/// extensions' `add-ld-path` at every start; a search path does the same without it.
fn ld_library_path(app: &App) -> String {
    let mut v = vec!["/app/lib".to_string()];
    // App extensions first, as in flatpak's ld.so.conf.
    let (ours, theirs): (Vec<&Extension>, Vec<&Extension>) = app.extensions.iter().partition(|e| e.mount.starts_with("/app/"));
    for e in ours.into_iter().chain(theirs) {
        if let Some(p) = &e.ld_path {
            if !v.contains(p) {
                v.push(p.clone());
            }
        }
    }
    v.join(":")
}

/// The sandbox environment for home directory `home`, minus runtime-dir variables and
/// `LD_LIBRARY_PATH`. Clean, like flatpak's: nothing from a Termux shell (`LD_PRELOAD`,
/// `PREFIX`, a bionic `PATH`) means anything to the runtime's glibc programs.
fn base_env(app: &App, home: &str, user: &str) -> Vec<(String, String)> {
    let var = format!("{home}/.var/app/{}", app.id);
    let mut v: Vec<(String, String)> = vec![
        ("PATH".into(), "/app/bin:/usr/bin".into()),
        ("XDG_CONFIG_DIRS".into(), "/app/etc/xdg:/etc/xdg".into()),
        ("XDG_DATA_DIRS".into(), "/app/share:/usr/share:/usr/share/runtime/share:/run/host/user-share:/run/host/share".into()),
        ("FLATPAK_ID".into(), app.id.clone()),
        ("HOME".into(), home.into()),
        ("USER".into(), user.into()),
        ("LOGNAME".into(), user.into()),
        ("XDG_DATA_HOME".into(), format!("{var}/data")),
        ("XDG_CONFIG_HOME".into(), format!("{var}/config")),
        ("XDG_CACHE_HOME".into(), format!("{var}/cache")),
        ("XDG_STATE_HOME".into(), format!("{var}/.local/state")),
    ];
    for (k, val) in lindroids_proto::toolkit_env() {
        v.push((k.into(), val.into()));
    }
    if let Some(rt) = &app.runtime {
        v.extend(rt.metadata.environment());
    }
    v.extend(app.app.metadata.environment());
    v
}

/// Later entries win; first-seen order is kept.
fn dedup_env(env: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (k, v) in env {
        match out.iter_mut().find(|(ok, _)| *ok == k) {
            Some(slot) => slot.1 = v,
            None => out.push((k, v)),
        }
    }
    out
}

/// Top-level names flatpak links into `/usr` (merged usr), for those the runtime has.
fn usr_links(rt_files: &Path) -> Vec<&'static str> {
    ["bin", "sbin", "lib", "lib32", "lib64", "libx32"].into_iter().filter(|n| rt_files.join(n).symlink_metadata().is_ok()).collect()
}

/// Mount points of the skeleton root.
const SKELETON_DIRS: &[&str] = &["usr", "app", "etc", "proc", "dev", "sys", "tmp", "run/host", "var/tmp", "var/data", "var/config", "var/cache", "home", "root", "mnt", "media", "opt"];

/// Per-app writable dirs under `~/.var/app/<id>`.
const VAR_DIRS: &[&str] = &["data", "config", "cache/tmp", ".local/state"];

fn passwd(uid: u32, user: &str, home: &str) -> String {
    let mut s = String::from("root:x:0:0:root:/root:/bin/sh\n");
    if uid != 0 {
        s += &format!("{user}:x:{uid}:{uid}:{user}:{home}:/bin/sh\n");
    }
    s + "nobody:x:65534:65534:Nobody:/:/sbin/nologin\n"
}

fn group(gid: u32, user: &str) -> String {
    let mut s = String::from("root:x:0:\n");
    if gid != 0 {
        s += &format!("{user}:x:{gid}:\n");
    }
    s + "nobody:x:65534:\n"
}

/// `/run/host/font-dirs.xml`, which the runtime's fontconfig includes; `remaps` are
/// `<remap-dir>` lines for host font dirs.
fn font_dirs_xml(remaps: &str) -> String {
    format!("<?xml version=\"1.0\"?>\n<!DOCTYPE fontconfig SYSTEM \"urn:fontconfig:fonts.dtd\">\n<fontconfig>\n{remaps}</fontconfig>\n")
}

const RESOLV_CONF: &str = "nameserver 8.8.8.8\nnameserver 1.1.1.1\n";
const HOSTS: &str = "127.0.0.1 localhost\n::1 localhost\n";

fn random_hex(n: usize) -> String {
    let mut buf = vec![0u8; n / 2];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut buf)).is_err() {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let mut x = (t as u64) ^ ((std::process::id() as u64) << 32) | 1;
        for b in buf.iter_mut() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *b = x as u8;
        }
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// The first of `candidates` that exists, symlinks resolved (a bind of a symlink whose target
/// is not in the sandbox would show nothing).
fn host_file(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().filter_map(|p| std::fs::canonicalize(p).ok()).find(|p| p.is_file())
}

fn on_android() -> bool {
    Path::new("/system/build.prop").exists() || Path::new("/system/bin/app_process").exists()
}

/// Android has no /etc/localtime; its zone is a system property.
fn android_timezone() -> Option<String> {
    let out = Command::new("/system/bin/getprop").arg("persist.sys.timezone").output().ok()?;
    let tz = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!tz.is_empty()).then_some(tz)
}

/// Where generated per-app state lives (skeleton roots, /etc files).
fn state_dir() -> PathBuf {
    lindroids_server::state_dir().join("flatpak")
}

fn io_err(p: &Path, e: std::io::Error) -> String {
    format!("{}: {e}", p.display())
}

fn mkdirs(p: &Path) -> Result<(), String> {
    std::fs::create_dir_all(p).map_err(|e| io_err(p, e))
}

/// A symlink at `link` pointing at `target`, replacing an empty directory or a wrong link.
fn ensure_symlink(target: &str, link: &Path) -> Result<(), String> {
    if std::fs::read_link(link).map_or(false, |t| t == Path::new(target)) {
        return Ok(());
    }
    let _ = std::fs::remove_dir(link);
    let _ = std::fs::remove_file(link);
    std::os::unix::fs::symlink(target, link).map_err(|e| io_err(link, e))
}

/// The `proot` command line that runs `app` like `flatpak run` would, with its windows going to
/// the server whose runtime dir is `rt`.
pub fn command(app: &App, args: &[String], o: &Opts, rt: &Path) -> Result<Command, String> {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        if let Some(pid) = running_instance(&Path::new(&dir).join(".flatpak"), &app.id) {
            // Its single-instance check (a socket in its /tmp) cannot see this one, whose /tmp is
            // private: both would write the same ~/.var/app data at once.
            return Err(format!("{} is already running under flatpak (pid {pid}); close it first: two instances would share its data", app.id));
        }
    }
    command_in(app, args, o, rt, &state_dir())
}

/// A live `flatpak run` instance of `id` (desktop Linux, a toolbox): Flatpak keeps one record per
/// instance in `$XDG_RUNTIME_DIR/.flatpak/<instance>/` with its `info` and the sandbox's `pid`.
fn running_instance(records: &Path, id: &str) -> Option<u32> {
    std::fs::read_dir(records).ok()?.flatten().find_map(|e| {
        let info = std::fs::read_to_string(e.path().join("info")).ok()?;
        let name = info.lines().skip_while(|l| l.trim() != "[Application]").find_map(|l| l.trim().strip_prefix("name="))?;
        if name != id {
            return None;
        }
        let pid: u32 = std::fs::read_to_string(e.path().join("pid")).ok()?.trim().parse().ok()?;
        Path::new(&format!("/proc/{pid}")).exists().then_some(pid)
    })
}

fn command_in(app: &App, args: &[String], o: &Opts, rt: &Path, state: &Path) -> Result<Command, String> {
    let (x11_env, integrated_x11) = crate::sandbox::x11_env(rt);
    if o.x11 && !integrated_x11 {
        return Err("--x11 for a Flatpak needs the integrated X display (xwayland-satellite and Xwayland; `lindroids doctor`)".into());
    }
    let rtd = app.runtime()?;
    let program = app.program()?;
    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };
    let root = &app.root;

    // Home: the user's own, or in a distro its /root (what `proot-distro login` gives).
    let (home_host, home, user) = match o.distro.is_some() || root.starts_with(PathBuf::from(lindroids_proto::TERMUX_PREFIX).join("var/lib/proot-distro")) {
        true => (install::real(root, &root.join("root")), "/root".to_string(), "root".to_string()),
        false => {
            let h = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
            let u = std::env::var("USER").ok().filter(|u| !u.is_empty() && !u.contains(':')).unwrap_or_else(|| "user".into());
            (PathBuf::from(&h), h, u)
        }
    };
    let var_host = home_host.join(".var/app").join(&app.id);
    for d in VAR_DIRS {
        mkdirs(&var_host.join(d))?;
    }

    // The skeleton root everything is bound into.
    let base = state.join(&app.id);
    let skel = base.join("root");
    let run_user = format!("run/user/{uid}");
    for d in SKELETON_DIRS.iter().copied().chain([run_user.as_str(), home.trim_start_matches('/')]) {
        mkdirs(&skel.join(d))?;
    }
    for n in usr_links(&rtd.files()) {
        ensure_symlink(&format!("usr/{n}"), &skel.join(n))?;
    }
    ensure_symlink("../run", &skel.join("var/run"))?;

    // /etc is the runtime's, with the files flatpak takes from the host (or makes) on top.
    let etc = base.join("etc");
    mkdirs(&etc)?;
    std::fs::write(etc.join("passwd"), passwd(uid, &user, &home)).map_err(|e| io_err(&etc, e))?;
    std::fs::write(etc.join("group"), group(gid, &user)).map_err(|e| io_err(&etc, e))?;
    let prefix = std::env::var("PREFIX").ok().filter(|p| !p.is_empty());
    let host_etc = |name: &str| -> Vec<PathBuf> {
        let mut v = Vec::new();
        if let Some(p) = &prefix {
            v.push(Path::new(p).join("etc").join(name));
        }
        v.push(Path::new("/etc").join(name));
        if root != Path::new("/") {
            v.push(install::real(root, &root.join("etc").join(name)));
        }
        v
    };
    let or_generated = |found: Option<PathBuf>, name: &str, body: &str| -> Result<PathBuf, String> {
        if let Some(p) = found {
            return Ok(p);
        }
        let p = etc.join(name);
        std::fs::write(&p, body).map_err(|e| io_err(&p, e))?;
        Ok(p)
    };
    let machine_id = match host_file(&host_etc("machine-id")).filter(|p| std::fs::read_to_string(p).map_or(false, |s| s.trim().len() == 32)) {
        Some(p) => p,
        None => {
            // One generated id for all apps, kept, like a real machine's.
            let p = state.join("machine-id");
            if std::fs::read_to_string(&p).map_or(true, |s| s.trim().len() != 32) {
                std::fs::write(&p, random_hex(32) + "\n").map_err(|e| io_err(&p, e))?;
            }
            p
        }
    };
    let resolv = or_generated(host_file(&host_etc("resolv.conf")), "resolv.conf", RESOLV_CONF)?;
    let hosts = or_generated(host_file(&host_etc("hosts")), "hosts", HOSTS)?;
    let localtime = host_file(&host_etc("localtime"));

    // A private /tmp per app, next to the server's sockets.
    let tmp = rt.join("flatpak").join(&app.id).join("tmp");
    mkdirs(&tmp)?;

    let mut binds: Vec<(PathBuf, String)> = Vec::new();
    for p in pieces(app)? {
        match p {
            Piece::Tree { src, dst } | Piece::Merge { src, dst, .. } => binds.push((src, dst)),
        }
    }
    binds.push((rtd.files().join("etc"), "/etc".into()));
    binds.push((etc.join("passwd"), "/etc/passwd".into()));
    binds.push((etc.join("group"), "/etc/group".into()));
    binds.push((machine_id, "/etc/machine-id".into()));
    binds.push((resolv, "/etc/resolv.conf".into()));
    binds.push((hosts, "/etc/hosts".into()));
    if let Some(l) = localtime {
        binds.push((l, "/etc/localtime".into()));
    }
    for d in ["/dev", "/proc", "/sys"] {
        if Path::new(d).exists() {
            binds.push((d.into(), d.into()));
        }
    }
    // Android has no /dev/shm; glibc's shm_open (PulseAudio's client, Chromium) needs one.
    if !Path::new("/dev/shm").is_dir() {
        let shm = rt.join("flatpak").join(&app.id).join("shm");
        mkdirs(&shm)?;
        binds.push((shm, "/dev/shm".into()));
    }
    binds.push((tmp, "/tmp".into()));
    // The runtime dir also at the path the server knows it by: files the portal hands out
    // (picked documents) are named by that path.
    binds.push((rt.to_path_buf(), rt.display().to_string()));
    // The integrated X display: its abstract socket is reachable from here anyway; the socket
    // directory makes the path form work too.
    let x_dir = lindroids_server::x11::socket_dir();
    if integrated_x11 && x_dir.is_dir() {
        binds.push((x_dir, "/tmp/.X11-unix".into()));
    }
    binds.push((var_host.join("cache/tmp"), "/var/tmp".into()));
    binds.push((rt.to_path_buf(), format!("/run/user/{uid}")));
    binds.push((home_host.clone(), home.clone()));
    for d in ["data", "config", "cache"] {
        binds.push((var_host.join(d), format!("/var/{d}")));
    }
    // Host fonts where the runtime's fontconfig (conf.d/50-flatpak.conf) looks for them.
    let fonts = prefix.as_ref().map(|p| Path::new(p).join("share/fonts")).filter(|p| p.is_dir()).unwrap_or_else(|| PathBuf::from("/usr/share/fonts"));
    let mut remaps = String::new();
    for (src, dst) in [
        (fonts, "/run/host/fonts"),
        (PathBuf::from("/usr/local/share/fonts"), "/run/host/local-fonts"),
        (home_host.join(".local/share/fonts"), "/run/host/user-fonts"),
    ] {
        if src.is_dir() {
            remaps += &format!("  <remap-dir as-path=\"{}\">{dst}</remap-dir>\n", src.display());
            binds.push((src, dst.into()));
        }
    }
    // The runtime's fontconfig includes this; flatpak writes it to map host font paths.
    let font_dirs = etc.join("font-dirs.xml");
    std::fs::write(&font_dirs, font_dirs_xml(&remaps)).map_err(|e| io_err(&font_dirs, e))?;
    binds.push((font_dirs, "/run/host/font-dirs.xml".into()));
    // Android shared storage.
    if Path::new("/storage").is_dir() {
        binds.push(("/storage".into(), "/storage".into()));
    }
    if Path::new("/storage/emulated/0").is_dir() {
        binds.push(("/storage/emulated/0".into(), "/sdcard".into()));
    } else if Path::new("/sdcard").is_dir() {
        binds.push(("/sdcard".into(), "/sdcard".into()));
    }
    // A Flatpak installed inside a proot-distro distro: proot-distro emulates the hard links
    // ostree makes with symlinks into <rootfs>/.l2s, by their path on this side. Make that path
    // resolve here too.
    let l2s = app.root.join(".l2s");
    if app.root != Path::new("/") && l2s.is_dir() {
        let p = l2s.display().to_string();
        binds.push((l2s, p));
    }
    // Absolute paths from `filesystems=` (home and the xdg-* dirs are inside the bound home).
    let reserved = ["/usr", "/app", "/etc", "/proc", "/dev", "/sys", "/tmp", "/run", "/lib", "/lib64", "/bin", "/sbin", "/var"];
    for fs in app.app.metadata.list("Context", "filesystems") {
        let path = fs.split(':').next().unwrap_or("");
        let taken = reserved.iter().any(|r| path == *r || path.starts_with(&format!("{r}/")));
        if path.len() > 1 && path.starts_with('/') && !taken && Path::new(path).exists() {
            binds.push((path.into(), path.into()));
        }
    }
    // Inside a distro rootfs, deploys may hold symlinks absolute to the distro's own paths (hard
    // links that proot's link2symlink turned into symlinks, for one): show the installations at
    // the paths the distro has them.
    if root != Path::new("/") {
        let mut used: Vec<usize> = vec![app.app.installation, rtd.installation];
        used.extend(app.extensions.iter().map(|e| e.installation));
        used.sort();
        used.dedup();
        for i in used {
            let inst = &app.installations[i];
            binds.push((inst.host.clone(), inst.guest.clone()));
        }
    }

    let mut env = base_env(app, &home, &user);
    env.push(("LD_LIBRARY_PATH".into(), ld_library_path(app)));
    env.push(("XDG_RUNTIME_DIR".into(), format!("/run/user/{uid}")));
    env.push(("WAYLAND_DISPLAY".into(), "wayland-0".into()));
    env.extend(crate::sandbox::integration_env(rt, Path::new(&format!("/run/user/{uid}"))));
    if let Some((_, d)) = env.iter_mut().find(|(k, _)| k == "XDG_DATA_DIRS") {
        if let Some(with) = crate::sandbox::data_dirs_with_runtime(rt, Path::new(&format!("/run/user/{uid}")), d) {
            *d = with;
        }
    }
    env.extend(x11_env);
    if o.x11 {
        env.extend([("GDK_BACKEND", "x11"), ("QT_QPA_PLATFORM", "xcb"), ("SDL_VIDEODRIVER", "x11")].map(|(k, v)| (k.to_string(), v.to_string())));
    }
    env.push(("PULSE_SERVER".into(), format!("unix:/run/user/{uid}/pulse/native")));
    env.push(("LANG".into(), std::env::var("LANG").ok().filter(|l| !l.is_empty()).unwrap_or_else(|| "C.UTF-8".into())));
    for (k, v) in std::env::vars() {
        if k.starts_with("LC_") || matches!(k.as_str(), "TERM" | "COLORTERM" | "TZ" | "LANGUAGE") {
            env.push((k, v));
        }
    }
    if std::env::var_os("TZ").is_none() && !binds.iter().any(|(_, d)| d == "/etc/localtime") {
        if let Some(tz) = on_android().then(android_timezone).flatten() {
            env.push(("TZ".into(), tz));
        }
    }
    env.extend(o.env.iter().cloned());
    let env = dedup_env(env);

    let mut c = Command::new("proot");
    // proot is a host program and keeps the host environment, minus what would reach the first
    // program in the sandbox before `env -i` clears it (Termux's LD_PRELOAD is a bionic library).
    c.env_remove("LD_PRELOAD").env_remove("LD_LIBRARY_PATH");
    if std::env::var_os("PROOT_TMP_DIR").is_none() {
        let pt = rt.join("proot-tmp");
        mkdirs(&pt)?;
        c.env("PROOT_TMP_DIR", pt);
    }
    c.arg("--kill-on-exit");
    if on_android() {
        // Android forbids hard links in app data, and its kernels have no SysV IPC.
        c.args(["--link2symlink", "--sysvipc"]);
    } else if std::env::var_os("PROOT_NO_SECCOMP").is_none() {
        // Desktop proot builds (Debian's and Ubuntu's 5.4.0 on Linux 7.x) kill every runtime
        // program with SIGBUS in its loader when seccomp acceleration is on.
        c.env("PROOT_NO_SECCOMP", "1");
    }
    c.arg("-r").arg(&skel);
    for (src, dst) in &binds {
        c.arg("-b").arg(format!("{}:{dst}", src.display()));
    }
    c.arg("-w").arg(&home);
    if rtd.files().join("bin/env").exists() {
        c.args(["/usr/bin/env", "-i"]);
        c.args(env.iter().map(|(k, v)| format!("{k}={v}")));
    } else {
        c.env_clear().envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    }
    c.arg(&program);
    c.args(args);
    if std::env::var("LINDROIDS_LOG").as_deref() == Ok("debug") {
        let line: Vec<String> = std::iter::once(c.get_program()).chain(c.get_args()).map(|a| a.to_string_lossy().into_owned()).collect();
        eprintln!("lindroids: {}", line.join(" "));
    }
    Ok(c)
}

/// Name and PNG icon, from the app's exported .desktop file and icons.
pub fn identify(app: &App, o: &Opts) -> (String, Vec<u8>) {
    let dirs = [app.app.dir.join("export/share"), app.app.files().join("share")];
    let desktop = format!("{}.desktop", app.id);
    let entry = dirs.iter().find_map(|d| crate::desktop::parse(&d.join("applications").join(&desktop)));
    let name = o.name.clone().or_else(|| entry.as_ref().map(|e| e.name.clone())).unwrap_or_else(|| app.id.rsplit('.').next().unwrap_or(&app.id).to_string());
    let icon = o
        .icon
        .as_deref()
        .and_then(|p| crate::desktop::icon_png(p, 192))
        .or_else(|| {
            let icon = entry.as_ref().and_then(|e| e.icon.clone()).unwrap_or_else(|| app.id.clone());
            // An absolute Icon= is a path inside the sandbox, where /app is the app's files.
            let file = match icon.strip_prefix("/app/") {
                Some(rel) => Some(app.app.files().join(rel)).filter(|p| p.exists()),
                None => crate::desktop::icon_file(&dirs, &app.root, &icon),
            }?;
            crate::desktop::icon_png(&file, 192)
        })
        .unwrap_or_else(|| crate::desktop::fallback_icon(&name, 192));
    (name, icon)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn write(p: &Path, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    /// A deploy laid out like flatpak's: <kind>/<id>/<arch>/<branch>/<commit>, `active` → the
    /// commit.
    fn deploy(inst: &Path, kind: &str, id: &str, branch: &str, metadata: &str) -> PathBuf {
        let b = inst.join(kind).join(id).join(install::arch()).join(branch);
        let commit = b.join("0123abcd");
        std::fs::create_dir_all(commit.join("files")).unwrap();
        write(&commit.join("metadata"), metadata);
        symlink("0123abcd", b.join("active")).unwrap();
        commit
    }

    fn fake_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("lindroids-flatpak-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let arch = install::arch();
        let m = install::multiarch(arch);
        let sys = root.join("var/lib/flatpak");
        let app = deploy(&sys, "app", "org.example.App", "stable", &format!(
            "[Application]\nname=org.example.App\nruntime=org.example.Platform/{arch}/24\ncommand=example\n\n\
             [Context]\nfilesystems=home;\n\n[Environment]\nEXAMPLE=1\n\n\
             [Extension org.example.App.Locale]\ndirectory=share/runtime/locale\n\n\
             [Extension org.example.App.Debug]\ndirectory=lib/debug\n"
        ));
        write(&app.join("files/bin/example"), "#!/bin/sh\n");
        write(&app.join("export/share/applications/org.example.App.desktop"), "[Desktop Entry]\nName=Example\nName[de]=Beispiel\nIcon=org.example.App\n");
        symlink(format!("{arch}/stable"), sys.join("app/org.example.App/current")).unwrap();
        deploy(&sys, "runtime", "org.example.App.Locale", "stable", "[Runtime]\nname=org.example.App.Locale\n");
        let rt = deploy(&sys, "runtime", "org.example.Platform", "24", &format!(
            "[Runtime]\nname = org.example.Platform\n\n[Environment]\nGI_TYPELIB_PATH = /app/lib/girepository-1.0\n\n\
             [Extension org.example.Platform.GL]\nversions = 24;1.4\ndirectory = lib/{m}/GL\nsubdirectories = true\n\
             add-ld-path = lib\nmerge-dirs = glvnd/egl_vendor.d;vulkan/icd.d\nenable-if = active-gl-driver\n\n\
             [Extension org.example.Platform.GL.Debug]\ndirectory = lib/debug/GL\nsubdirectories = true\n\n\
             [Extension org.example.Platform.codecs]\ndirectory = lib/{m}/codecs\nversion = 24-extra\nadd-ld-path = lib\n"
        ));
        std::fs::create_dir_all(rt.join("files/lib")).unwrap();
        write(&rt.join("files/bin/env"), "");
        symlink("bin", rt.join("files/sbin")).unwrap();
        write(&rt.join("files/etc/passwd"), "");
        write(&rt.join("files/etc/os-release"), "ID=example\n");
        symlink("/run/host/monitor/resolv.conf", rt.join("files/etc/resolv.conf")).unwrap();
        // The GL driver lives in a per-user installation; codecs in the system one.
        let user = root.join("home/alice/.local/share/flatpak");
        let gl = deploy(&user, "runtime", "org.example.Platform.GL.default", "1.4", "[Runtime]\n");
        std::fs::create_dir_all(gl.join("files/glvnd/egl_vendor.d")).unwrap();
        deploy(&user, "runtime", "org.example.Platform.GL.nvidia-550", "24", "[Runtime]\n");
        deploy(&sys, "runtime", "org.example.Platform.codecs", "24-extra", "[Runtime]\n");
        root
    }

    #[test]
    fn ids() {
        assert!(looks_like_id("org.gnome.Calculator"));
        assert!(!looks_like_id("gnome-calculator"));
        assert!(!looks_like_id("./a.b.c"));
        assert!(!looks_like_id("a..b.c"));
    }

    #[test]
    fn finds_app_runtime_and_extensions() {
        let root = fake_root("find");
        let app = find(&root, "org.example.App").expect("found");
        let m = install::multiarch(install::arch());
        assert_eq!(app.app.branch, "stable");
        assert_eq!(app.command.as_deref(), Some("example"));
        let rt = app.runtime.as_ref().expect("runtime");
        assert_eq!((rt.id.as_str(), rt.branch.as_str()), ("org.example.Platform", "24"));
        assert!(rt.dir.ends_with("0123abcd"));
        let gl = format!("/usr/lib/{m}/GL/default");
        let codecs = format!("/usr/lib/{m}/codecs");
        let mounts: Vec<(&str, &str)> = app.extensions.iter().map(|e| (e.id.as_str(), e.mount.as_str())).collect();
        assert_eq!(mounts, vec![
            ("org.example.Platform.GL.default", gl.as_str()),
            ("org.example.Platform.codecs", codecs.as_str()),
            ("org.example.App.Locale", "/app/share/runtime/locale"),
        ]);
        // nvidia-550 is not an active GL driver here; Debug extensions are skipped.
        let gl_ext = &app.extensions[0];
        assert_eq!(gl_ext.branch, "1.4");
        assert_eq!(gl_ext.point, format!("/usr/lib/{m}/GL"));
        assert_eq!(gl_ext.ld_path.as_deref(), Some(format!("{gl}/lib").as_str()));
        assert!(app.installations[gl_ext.installation].host.ends_with("home/alice/.local/share/flatpak"));
        assert_eq!(ld_library_path(&app), format!("/app/lib:{gl}/lib:{codecs}/lib"));
        assert!(find(&root, "org.example.Missing").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_runtime_is_reported() {
        let root = fake_root("nort");
        std::fs::remove_dir_all(root.join("var/lib/flatpak/runtime/org.example.Platform")).unwrap();
        let app = find(&root, "org.example.App").expect("the app is still found");
        let err = command_in(&app, &[], &Opts::default(), &root, &root.join("state")).unwrap_err();
        assert!(err.contains("org.example.Platform"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn absolute_symlinks_stay_inside_the_root() {
        let root = fake_root("links");
        // /var/lib/flatpak as an absolute link, the way a distro might have it.
        std::fs::rename(root.join("var/lib/flatpak"), root.join("srv-flatpak")).unwrap();
        symlink("/srv-flatpak", root.join("var/lib/flatpak")).unwrap();
        let app = find(&root, "org.example.App").expect("found through the link");
        assert!(app.app.dir.starts_with(root.join("srv-flatpak")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn command_builds_the_view() {
        let root = fake_root("cmd");
        let app = find(&root, "org.example.App").unwrap();
        let state = root.join("state");
        let rt = root.join("rt");
        std::fs::create_dir_all(&rt).unwrap();
        let o = Opts { distro: Some("test".into()), env: vec![("EXAMPLE".into(), "2".into())], ..Default::default() };
        let c = command_in(&app, &["--flag".into()], &o, &rt, &state).unwrap();
        let args: Vec<String> = c.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        let has = |s: &str| args.iter().any(|a| a == s);
        let rtf = app.runtime.as_ref().unwrap().files();
        let uid = unsafe { libc::getuid() };
        let m = install::multiarch(install::arch());
        assert!(has(&format!("{}:/usr", rtf.display())));
        assert!(has(&format!("{}:/app", app.app.files().display())));
        assert!(has(&format!("{}:/etc", rtf.join("etc").display())));
        assert!(has(&format!("{}:/run/user/{uid}", rt.display())));
        assert!(has(&format!("{}:/usr/lib/{m}/GL/glvnd/egl_vendor.d", app.extensions[0].files.join("glvnd/egl_vendor.d").display())));
        assert!(args.iter().any(|a| a.ends_with(":/etc/passwd")));
        // Installations are visible at their own paths inside a distro.
        assert!(has(&format!("{}:/var/lib/flatpak", root.join("var/lib/flatpak").display())));
        assert!(has(&format!("XDG_RUNTIME_DIR=/run/user/{uid}")));
        assert!(has("EXAMPLE=2") && !has("EXAMPLE=1"));
        assert!(has("GI_TYPELIB_PATH=/app/lib/girepository-1.0"));
        assert!(has("HOME=/root"));
        assert_eq!(&args[args.len() - 2..], ["/app/bin/example", "--flag"]);
        let skel = state.join("org.example.App/root");
        assert_eq!(std::fs::read_link(skel.join("lib")).unwrap(), Path::new("usr/lib"));
        assert_eq!(std::fs::read_link(skel.join("sbin")).unwrap(), Path::new("usr/sbin"));
        assert!(skel.join("lib64").symlink_metadata().is_err());
        assert!(root.join("root/.var/app/org.example.App/config").is_dir());
        let passwd = std::fs::read_to_string(state.join("org.example.App/etc/passwd")).unwrap();
        assert!(passwd.starts_with("root:x:0:0:"));
        // Building it twice (every launch) works.
        command_in(&app, &[], &o, &rt, &state).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Against the Flatpaks installed on this machine:
    /// `LINDROIDS_FLATPAK_IDS=org.gnome.Calculator,... LINDROIDS_FLATPAK_ICONS=<dir> cargo test
    /// -p lindroids -- --ignored real_apps --nocapture`.
    #[test]
    #[ignore]
    fn real_apps() {
        let ids = std::env::var("LINDROIDS_FLATPAK_IDS").unwrap_or_else(|_| "org.gnome.Calculator".into());
        let icons = std::env::var_os("LINDROIDS_FLATPAK_ICONS").map(PathBuf::from);
        for id in ids.split(',') {
            let app = find(Path::new("/"), id).unwrap_or_else(|| panic!("{id} is not installed"));
            let (name, icon) = identify(&app, &Opts::default());
            assert!(icon.starts_with(b"\x89PNG"));
            let exts: Vec<&str> = app.extensions.iter().map(|e| e.id.as_str()).collect();
            eprintln!("{id}: {name:?}, runtime {}, extensions {exts:?}", app.runtime_ref);
            if let Some(d) = &icons {
                std::fs::create_dir_all(d).unwrap();
                std::fs::write(d.join(format!("{id}.png")), &icon).unwrap();
            }
        }
    }

    #[test]
    fn finds_a_running_instance() {
        let root = std::env::temp_dir().join(format!("lindroids-flatpak-inst-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write(&root.join("123/info"), "[Application]\nname=org.example.App\n\n[Instance]\ninstance-id=123\n");
        std::fs::write(root.join("123/pid"), std::process::id().to_string()).unwrap();
        write(&root.join("456/info"), "[Application]\nname=org.example.Other\n");
        std::fs::write(root.join("456/pid"), std::process::id().to_string()).unwrap();
        assert_eq!(running_instance(&root, "org.example.App"), Some(std::process::id()));
        // A record whose process is gone is not an instance.
        std::fs::write(root.join("123/pid"), "999999999").unwrap();
        assert_eq!(running_instance(&root, "org.example.App"), None);
        assert_eq!(running_instance(&root.join("missing"), "org.example.App"), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn identifies_from_export() {
        let root = fake_root("ident");
        let app = find(&root, "org.example.App").unwrap();
        let (name, icon) = identify(&app, &Opts::default());
        assert_eq!(name, "Example");
        assert!(icon.starts_with(b"\x89PNG"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
