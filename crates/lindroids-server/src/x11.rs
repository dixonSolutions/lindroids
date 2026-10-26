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

/// Where satellite comes from, in order: LINDROIDS_XWAYLAND_SATELLITE, the copy Lindroids ships
/// next to its own binary (`<prefix>/libexec/lindroids/`), then PATH (a distro's own package).
pub fn find_satellite() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("LINDROIDS_XWAYLAND_SATELLITE") {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    if let Some(bin_dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        let p = bin_dir.join("../libexec/lindroids/xwayland-satellite");
        if p.is_file() {
            return Some(p);
        }
    }
    which("xwayland-satellite")
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
}

impl X11 {
    /// Reserve a display and listen on it, or say why X11 is off.
    pub fn start(runtime: &Path, wayland: &Path) -> Result<X11, String> {
        if std::env::var("LINDROIDS_X11").as_deref() == Ok("off") {
            return Err("off (LINDROIDS_X11=off)".into());
        }
        let satellite = find_satellite().ok_or("xwayland-satellite not found")?;
        which("Xwayland").ok_or("Xwayland not installed (`lindroids setup x11`)")?;
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
        })
    }

    /// The sockets to poll for a first connection, while satellite is not running.
    pub fn poll_fds(&self) -> Vec<RawFd> {
        match self.phase {
            Phase::Waiting | Phase::Failed => self.listeners.iter().map(|l| l.as_raw_fd()).collect(),
            _ => Vec::new(),
        }
    }

    /// A poll timeout this needs (to notice satellite exiting, or to end a backoff).
    pub fn wants_timeout(&self) -> Option<Duration> {
        match self.phase {
            Phase::Running { .. } | Phase::Backoff { .. } => Some(Duration::from_millis(500)),
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
        match cmd.spawn() {
            Ok(child) => {
                info!("X11: started xwayland-satellite for :{} (pid {})", self.display, child.id());
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
