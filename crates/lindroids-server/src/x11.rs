//! Integrated X11: every Lindroids session has an X display, and X11 programs' windows become
//! Android windows like any Wayland window.
//!
//! The server reserves a display (`:N`) at startup by listening on its sockets, the way niri does,
//! and starts xwayland-satellite (github.com/Supreeeme/xwayland-satellite, MPL-2.0) on the first
//! connection, handing it the listening sockets (`-listenfd`). Satellite runs Xwayland rootless
//! and turns each X11 toplevel into an xdg_toplevel of ours, so an X11 program's windows get their
//! own Android tasks. Nothing runs until an X11 program connects; satellite exits with the server.
//!
//! The display is published in `<runtime>/x11-display`; the CLI sets DISPLAY from it for
//! everything it starts. Off with LINDROIDS_X11=off, or when Xwayland or satellite is missing.

use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use log::{info, warn};
use std::sync::atomic::{AtomicU32, Ordering};

/// The running xwayland-satellite (its pid) and the X display it serves, for `run_for_window`.
static SATELLITE: AtomicU32 = AtomicU32::new(0);
static DISPLAY: AtomicU32 = AtomicU32::new(u32::MAX);

/// The run an X11 window belongs to. Satellite shows every X11 window as one Wayland client of
/// its own, so the Wayland side cannot tell whose it is; the X server can. Find the X toplevel
/// with this title and class, ask the X-Resource extension which process owns it, and go up from
/// that process to what `lindroids run` started. Only for satellite's windows (`client_pid`), so
/// asking never starts X11 by itself.
pub fn run_for_window(client_pid: Option<u32>, title: &str, app_id: &str) -> Option<u32> {
    let sat = SATELLITE.load(Ordering::SeqCst);
    if sat == 0 || client_pid != Some(sat) {
        return None;
    }
    // On a thread, with a deadline: this runs in the compositor, and Xwayland reaches the
    // compositor through satellite, so a stuck X server must not stop it.
    let (title, app_id) = (title.to_string(), app_id.to_string());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(owner_of_x_window(&title, &app_id));
    });
    rx.recv_timeout(Duration::from_millis(500)).ok().flatten()
}

fn owner_of_x_window(title: &str, app_id: &str) -> Option<u32> {
    use x11rb::connection::Connection;
    use x11rb::protocol::res::{ClientIdMask, ClientIdSpec, ConnectionExt as _};
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};

    let display = DISPLAY.load(Ordering::SeqCst);
    if display == u32::MAX {
        return None;
    }
    let (conn, screen) = x11rb::rust_connection::RustConnection::connect(Some(&format!(":{display}"))).ok()?;
    let root = conn.setup().roots.get(screen)?.root;
    let atom = |name: &[u8]| conn.intern_atom(false, name).ok()?.reply().ok().map(|r| r.atom);
    let (net_name, utf8) = (atom(b"_NET_WM_NAME")?, atom(b"UTF8_STRING")?);
    let text = |w: u32, prop: u32, ty: u32| -> Option<Vec<u8>> {
        let r = conn.get_property(false, w, prop, ty, 0, 1024).ok()?.reply().ok()?;
        (r.value_len > 0).then_some(r.value)
    };
    let mut best: Option<(u32, u32)> = None; // (score, window)
    for w in conn.query_tree(root).ok()?.reply().ok()?.children {
        let Some(class) = text(w, AtomEnum::WM_CLASS.into(), AtomEnum::STRING.into()) else { continue };
        let class_ok = class.split(|b| *b == 0).any(|c| !c.is_empty() && c == app_id.as_bytes());
        let name = text(w, net_name, utf8).or_else(|| text(w, AtomEnum::WM_NAME.into(), AtomEnum::STRING.into()));
        let title_ok = !title.is_empty() && name.as_deref() == Some(title.as_bytes());
        let score = class_ok as u32 * 2 + title_ok as u32;
        if score > 0 && best.map_or(true, |(s, _)| score > s) {
            best = Some((score, w));
        }
    }
    let (_, window) = best?;
    let spec = ClientIdSpec { client: window, mask: ClientIdMask::LOCAL_CLIENT_PID };
    let ids = conn.res_query_client_ids(&[spec]).ok()?.reply().ok()?;
    let pid = ids.ids.iter().find(|id| id.spec.mask.contains(ClientIdMask::LOCAL_CLIENT_PID)).and_then(|id| id.value.first().copied())?;
    crate::icons::launch_of_pid(pid)
}

/// Where satellite comes from, in order: LINDROIDS_XWAYLAND_SATELLITE, the copy Lindroids ships
/// next to its own binary (`<prefix>/libexec/lindroids/`), then PATH (a distro's own package).
/// The first one that runs here: the shipped copy is a glibc build outside Termux, which a musl
/// distro (Alpine) cannot run, while its own package can.
pub fn find_satellite() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("LINDROIDS_XWAYLAND_SATELLITE") {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let mut candidates = Vec::new();
    if let Some(bin_dir) = lindroids_proto::self_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        candidates.push(bin_dir.join("../libexec/lindroids/xwayland-satellite"));
        // The glibc CLI for Cortex lives in that directory itself.
        candidates.push(bin_dir.join("xwayland-satellite"));
    }
    candidates.extend(which("xwayland-satellite"));
    candidates.into_iter().filter(|p| p.is_file()).find(|p| {
        Command::new(p).arg("--test-listenfd-support").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().map_or(false, |s| s.success())
    })
}

pub fn which(cmd: &str) -> Option<PathBuf> {
    std::env::var("PATH").ok()?.split(':').filter(|d| !d.is_empty()).map(|d| Path::new(d).join(cmd)).find(|p| p.is_file())
}

/// The directory X clients look in for display sockets: Termux's libraries are patched to use
/// `$PREFIX/tmp`; everything else uses `/tmp`. A proot-distro distro started with --shared-tmp
/// sees Termux's tmp as its /tmp, so both find the same socket.
pub fn socket_dir() -> PathBuf {
    let termux_tmp = Path::new(lindroids_proto::TERMUX_PREFIX).join("tmp");
    let in_termux = std::env::var("PREFIX").map_or(false, |p| p.contains("/com.termux")) && termux_tmp.is_dir();
    if in_termux {
        termux_tmp.join(".X11-unix")
    } else {
        PathBuf::from("/tmp/.X11-unix")
    }
}

enum Phase {
    /// Listening; satellite starts on the first connection.
    Waiting,
    Running { child: Child, since: Instant },
    /// Satellite keeps dying at once: wait before trying again (a waiting client stays queued).
    Backoff { until: Instant },
    /// Gave up: connections are refused so X clients fail instead of hanging.
    Failed,
}

pub struct X11 {
    pub display: u32,
    satellite: PathBuf,
    /// Filesystem socket, then abstract ones.
    listeners: Vec<OwnedFd>,
    lock: PathBuf,
    sock_path: PathBuf,
    published: PathBuf,
    log: PathBuf,
    wayland: PathBuf,
    runtime: PathBuf,
    phase: Phase,
    quick_deaths: u32,
    /// Readable when a child of the server exited (SIGCHLD): satellite is looked at then,
    /// instead of every half second.
    child_exited: Option<OwnedFd>,
}

/// The write end of the pipe SIGCHLD writes to (-1: none).
static CHILD_PIPE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);
/// The SIGCHLD handler there was before (a library's, such as async-process's), and its flags:
/// it is called too.
static PREV_CHILD: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(libc::SIG_DFL);
static PREV_CHILD_FLAGS: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

extern "C" fn on_child(sig: libc::c_int, info: *mut libc::siginfo_t, ctx: *mut libc::c_void) {
    let fd = CHILD_PIPE.load(Ordering::Relaxed);
    if fd >= 0 {
        // The interrupted code may be about to read errno: leave it as it was.
        #[cfg(target_os = "android")]
        let errno = unsafe { libc::__errno() };
        #[cfg(not(target_os = "android"))]
        let errno = unsafe { libc::__errno_location() };
        let saved = unsafe { *errno };
        unsafe { libc::write(fd, b"c".as_ptr() as *const _, 1) };
        unsafe { *errno = saved };
    }
    let prev = PREV_CHILD.load(Ordering::Relaxed);
    if prev != libc::SIG_DFL && prev != libc::SIG_IGN {
        unsafe {
            if PREV_CHILD_FLAGS.load(Ordering::Relaxed) & libc::SA_SIGINFO != 0 {
                let f: extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut libc::c_void) = std::mem::transmute(prev);
                f(sig, info, ctx);
            } else {
                let f: extern "C" fn(libc::c_int) = std::mem::transmute(prev);
                f(sig);
            }
        }
    }
}

/// A pipe that becomes readable when any child of the server exits. None if it cannot be set up
/// (then satellite is looked at on a timer).
fn child_exit_pipe() -> Option<OwnedFd> {
    let mut fds = [0; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
        return None;
    }
    let (r, w) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    let mut old: libc::sigaction = unsafe { std::mem::zeroed() };
    if unsafe { libc::sigaction(libc::SIGCHLD, std::ptr::null(), &mut old) } != 0 || old.sa_sigaction == libc::SIG_IGN {
        // Ignored children are reaped by the kernel: no exit to wait for, and nothing to change.
        return None;
    }
    PREV_CHILD.store(old.sa_sigaction, Ordering::Relaxed);
    PREV_CHILD_FLAGS.store(old.sa_flags, Ordering::Relaxed);
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = on_child as *const () as usize;
    // Restarted: other threads' reads and writes go on; only polls wake up.
    action.sa_flags = libc::SA_SIGINFO | libc::SA_RESTART | libc::SA_NOCLDSTOP;
    unsafe { libc::sigemptyset(&mut action.sa_mask) };
    CHILD_PIPE.store(w.as_raw_fd(), Ordering::Relaxed);
    if unsafe { libc::sigaction(libc::SIGCHLD, &action, std::ptr::null_mut()) } != 0 {
        CHILD_PIPE.store(-1, Ordering::Relaxed);
        return None;
    }
    // The write end lives as long as the server: the handler may run at any time.
    std::mem::forget(w);
    Some(r)
}

impl X11 {
    /// Reserve a display and listen on it, or say why X11 is off.
    pub fn start(runtime: &Path, wayland: &Path) -> Result<X11, String> {
        if std::env::var("LINDROIDS_X11").as_deref() == Ok("off") {
            return Err("off (LINDROIDS_X11=off)".into());
        }
        let satellite = find_satellite().ok_or("xwayland-satellite not found")?;
        let xwayland = which("Xwayland").ok_or("Xwayland not installed (`lindroids setup x11`)")?;
        let bin = runtime.join("x11-bin");
        write_xwayland_wrapper(&bin, &xwayland.display().to_string()).map_err(|e| format!("{}: {e}", bin.display()))?;
        if !Command::new(&satellite).arg("--test-listenfd-support").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().map_or(false, |s| s.success()) {
            return Err(format!("{} is too old (no -listenfd; 0.6 or newer is needed)", satellite.display()));
        }
        let r = reserve(&socket_dir(), true)?;
        let published = runtime.join("x11-display");
        std::fs::write(&published, format!(":{}\n", r.display)).map_err(|e| format!("{}: {e}", published.display()))?;
        info!("X11 on :{} ({}), satellite {}", r.display, r.sock_path.display(), satellite.display());
        Ok(X11 {
            display: r.display,
            satellite,
            listeners: r.fds,
            lock: r.lock,
            sock_path: r.sock_path,
            published,
            log: runtime.join("x11.log"),
            wayland: wayland.to_path_buf(),
            runtime: runtime.to_path_buf(),
            phase: Phase::Waiting,
            quick_deaths: 0,
            child_exited: None,
        })
    }

    /// The sockets to poll for a first connection, while satellite is not running; while it
    /// runs, the pipe that says a child exited.
    pub fn poll_fds(&self) -> Vec<RawFd> {
        match self.phase {
            Phase::Waiting | Phase::Failed => self.listeners.iter().map(|l| l.as_raw_fd()).collect(),
            Phase::Running { .. } => self.child_exited.iter().map(|p| p.as_raw_fd()).collect(),
            Phase::Backoff { .. } => Vec::new(),
        }
    }

    /// A poll timeout this needs: to end a backoff, or to notice satellite exiting where no
    /// signal says so.
    pub fn wants_timeout(&self) -> Option<Duration> {
        match self.phase {
            Phase::Running { .. } if self.child_exited.is_some() => None,
            Phase::Running { .. } => Some(Duration::from_millis(500)),
            Phase::Backoff { until } => Some(until.saturating_duration_since(Instant::now())),
            _ => None,
        }
    }

    /// A polled socket is readable: an X client is connecting.
    pub fn on_connection(&mut self) {
        match self.phase {
            Phase::Waiting => self.spawn(),
            Phase::Failed => {
                for l in &self.listeners {
                    loop {
                        let fd = unsafe { libc::accept4(l.as_raw_fd(), std::ptr::null_mut(), std::ptr::null_mut(), libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK) };
                        if fd < 0 {
                            break;
                        }
                        unsafe { libc::close(fd) };
                    }
                }
            }
            _ => {}
        }
    }

    /// Notice satellite exiting, and end a backoff.
    pub fn tick(&mut self) {
        if let Some(p) = &self.child_exited {
            let mut sink = [0u8; 64];
            while unsafe { libc::read(p.as_raw_fd(), sink.as_mut_ptr() as *mut _, sink.len()) } > 0 {}
        }
        match &mut self.phase {
            Phase::Running { child, since } => {
                let Ok(Some(status)) = child.try_wait() else { return };
                let quick = since.elapsed() < Duration::from_secs(5);
                warn!("X11: xwayland-satellite exited ({status}); see {}", self.log.display());
                self.quick_deaths = if quick { self.quick_deaths + 1 } else { 0 };
                self.phase = if self.quick_deaths >= 3 {
                    warn!("X11: xwayland-satellite keeps failing; X11 programs are refused until the server restarts");
                    Phase::Failed
                } else if quick {
                    Phase::Backoff { until: Instant::now() + Duration::from_secs(2) }
                } else {
                    Phase::Waiting
                };
            }
            Phase::Backoff { until } if Instant::now() >= *until => self.phase = Phase::Waiting,
            _ => {}
        }
    }

    fn spawn(&mut self) {
        let log = std::fs::OpenOptions::new().create(true).append(true).open(&self.log).ok();
        let (out, err) = match log.as_ref().and_then(|f| Some((f.try_clone().ok()?, f.try_clone().ok()?))) {
            Some((a, b)) => (Stdio::from(a), Stdio::from(b)),
            None => (Stdio::null(), Stdio::null()),
        };
        let mut cmd = Command::new(&self.satellite);
        cmd.arg(format!(":{}", self.display));
        for l in &self.listeners {
            cmd.args(["-listenfd", &l.as_raw_fd().to_string()]);
        }
        // Satellite runs `Xwayland` from PATH: ours first (see write_xwayland_wrapper).
        let path = format!("{}:{}", self.runtime.join("x11-bin").display(), std::env::var("PATH").unwrap_or_default());
        cmd.env("PATH", path);
        cmd.env("WAYLAND_DISPLAY", &self.wayland)
            .env("XDG_RUNTIME_DIR", &self.runtime)
            .env_remove("DISPLAY")
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err);
        let fds: Vec<RawFd> = self.listeners.iter().map(|l| l.as_raw_fd()).collect();
        unsafe {
            cmd.pre_exec(move || {
                // The listening sockets are inherited (they are close-on-exec here), and
                // satellite, with its Xwayland, ends when the server does.
                for &fd in &fds {
                    let flags = libc::fcntl(fd, libc::F_GETFD);
                    libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
                }
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
        // Before it starts, so that an exit at once is not missed.
        if self.child_exited.is_none() {
            self.child_exited = child_exit_pipe();
        }
        match cmd.spawn() {
            Ok(child) => {
                info!("X11: started xwayland-satellite for :{} (pid {})", self.display, child.id());
                SATELLITE.store(child.id(), Ordering::SeqCst);
                DISPLAY.store(self.display, Ordering::SeqCst);
                self.phase = Phase::Running { child, since: Instant::now() };
            }
            Err(e) => {
                warn!("X11: cannot start {}: {e}", self.satellite.display());
                self.phase = Phase::Failed;
                self.on_connection();
            }
        }
    }
}

impl Drop for X11 {
    fn drop(&mut self) {
        if let Phase::Running { child, .. } = &mut self.phase {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_file(&self.sock_path);
        if !self.lock.as_os_str().is_empty() {
            let _ = std::fs::remove_file(&self.lock);
        }
        let _ = std::fs::remove_file(&self.published);
    }
}

/// Xwayland lets in only clients of its own uid, which it takes from getuid(): under a fake root
/// (Cortex's preloaded library, a server started inside proot -0) that is 0 while every client's
/// socket carries the real uid, so no X client would get in. There Xwayland runs with `-ac` and
/// listens only on its socket file, in the app's private storage where no other app can reach it
/// (no abstract sockets). The same mix-up keeps the portal off D-Bus's EXTERNAL auth (portal.rs).
pub(crate) fn fake_root() -> bool {
    use std::sync::OnceLock;
    static FAKE: OnceLock<bool> = OnceLock::new();
    *FAKE.get_or_init(|| crate::host::detect().fake_root)
}

/// `<dir>/Xwayland`, which runs `real` with `-shm`. Satellite cannot pass Xwayland that flag, and
/// Lindroids only takes CPU buffers (wl_shm), so glamor (GPU rendering) could never show anything
/// here; worse, probing for it aborts Xwayland inside libepoxy where there is no libEGL (Debian's
/// xwayland does not depend on it).
pub fn write_xwayland_wrapper(dir: &Path, real: &str) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir)?;
    let p = dir.join("Xwayland");
    let ac = if fake_root() { " -ac" } else { "" };
    // Under Termux's system linker exec (Termux from Google Play), proot cannot start the helper
    // its SysV shared memory goes through (it re-runs /proc/self/exe, the linker: "error: expected
    // absolute path: "--shm-helper""), so an X program's MIT-SHM only prints that and fails. Without
    // the extension, X programs send their images over the socket from the start.
    let shm = if lindroids_proto::linker_exec() { " -extension MIT-SHM" } else { "" };
    std::fs::write(&p, format!("#!/bin/sh\n# Lindroids: CPU buffers only (see x11.rs).\nexec {real} -shm{ac}{shm} \"$@\"\n"))?;
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755))
}

/// A display number with its listening sockets: the file `<dir>/X<n>`, then the abstract
/// `/tmp/.X11-unix/X<n>` (where Linux X clients look first; the only one a client with a private
/// /tmp can reach) and the abstract form of the file's own path when that differs.
pub struct Reservation {
    pub display: u32,
    pub fds: Vec<OwnedFd>,
    /// Empty when no lock was taken.
    pub lock: PathBuf,
    pub sock_path: PathBuf,
}

/// Find a free display in `dir` (`.../.X11-unix`) and listen on it. With `lock`, take
/// `<dir>/../.X<n>-lock` for this process as an X server does; without, a display counts as free
/// when its socket file is stale and its abstract names are unused.
pub fn reserve(dir: &Path, lock: bool) -> Result<Reservation, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let _ = std::fs::set_permissions(dir, std::os::unix::fs::PermissionsExt::from_mode(0o1777));
    let lock_dir = dir.parent().unwrap_or(Path::new("/tmp")).to_path_buf();
    for n in 1..64u32 {
        let lock_path = if lock { lock_dir.join(format!(".X{n}-lock")) } else { PathBuf::new() };
        if lock && !take_lock(&lock_path) {
            continue;
        }
        let release = |sock: Option<&Path>| {
            if let Some(s) = sock {
                let _ = std::fs::remove_file(s);
            }
            if lock {
                let _ = std::fs::remove_file(&lock_path);
            }
        };
        let sock_path = dir.join(format!("X{n}"));
        if sock_path.exists() && !stale_socket(&sock_path) {
            release(None);
            continue;
        }
        let _ = std::fs::remove_file(&sock_path);
        let mut fds = match listen_path(&sock_path) {
            Ok(fd) => vec![fd],
            Err(e) => {
                release(None);
                return Err(format!("{}: {e}", sock_path.display()));
            }
        };
        let mut names = vec![format!("/tmp/.X11-unix/X{n}")];
        let own = sock_path.display().to_string();
        if !names.contains(&own) {
            names.push(own);
        }
        if fake_root() {
            names.clear();
        }
        let mut taken = false;
        for name in &names {
            match listen_abstract(name) {
                Ok(fd) => fds.push(fd),
                Err(e) if e.raw_os_error() == Some(libc::EADDRINUSE) => taken = true,
                Err(e) => warn!("X11: no abstract socket @{name}: {e}"),
            }
        }
        if taken {
            // Someone else's display :n, known only by its abstract socket.
            release(Some(&sock_path));
            continue;
        }
        return Ok(Reservation { display: n, fds, lock: lock_path, sock_path });
    }
    Err("no free X display".into())
}

/// Create `/tmp/.X<n>-lock` as an X server does (pid, 10 columns), replacing a dead owner's.
fn take_lock(lock: &Path) -> bool {
    for _ in 0..2 {
        match std::fs::OpenOptions::new().write(true).create_new(true).open(lock) {
            Ok(mut f) => {
                let _ = write!(f, "{:>10}\n", std::process::id());
                return true;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let pid: i32 = std::fs::read_to_string(lock).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
                let alive = pid > 0 && (unsafe { libc::kill(pid, 0) } == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM));
                if alive || std::fs::remove_file(lock).is_err() {
                    return false;
                }
            }
            Err(_) => return false,
        }
    }
    false
}

/// A socket file nobody listens on any more.
fn stale_socket(path: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_err()
}

fn sockaddr(name: &[u8], abstract_: bool) -> std::io::Result<(libc::sockaddr_un, libc::socklen_t)> {
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    let off = usize::from(abstract_);
    if name.len() + off >= addr.sun_path.len() {
        return Err(std::io::Error::from_raw_os_error(libc::ENAMETOOLONG));
    }
    for (i, b) in name.iter().enumerate() {
        addr.sun_path[i + off] = *b as libc::c_char;
    }
    let base = std::mem::size_of::<libc::sa_family_t>();
    let len = if abstract_ { base + 1 + name.len() } else { base + name.len() + 1 };
    Ok((addr, len as libc::socklen_t))
}

fn listen_on(name: &[u8], abstract_: bool) -> std::io::Result<OwnedFd> {
    let (addr, len) = sockaddr(name, abstract_)?;
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    if unsafe { libc::bind(fd.as_raw_fd(), &addr as *const _ as *const libc::sockaddr, len) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    if unsafe { libc::listen(fd.as_raw_fd(), 64) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(fd)
}

fn listen_path(path: &Path) -> std::io::Result<OwnedFd> {
    use std::os::unix::ffi::OsStrExt;
    listen_on(path.as_os_str().as_bytes(), false)
}

fn listen_abstract(name: &str) -> std::io::Result<OwnedFd> {
    listen_on(name.as_bytes(), true)
}

#[cfg(test)]
mod child_tests {
    use super::*;

    #[test]
    fn a_child_exiting_wakes_the_pipe() {
        let pipe = child_exit_pipe().expect("SIGCHLD pipe");
        let mut child = Command::new("true").spawn().unwrap();
        child.wait().unwrap();
        let mut p = libc::pollfd { fd: pipe.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        assert_eq!(unsafe { libc::poll(&mut p, 1, 2000) }, 1, "the exit is seen without a timer");
    }
}
