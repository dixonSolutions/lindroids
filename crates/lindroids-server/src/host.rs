//! What Lindroids runs in, and how that differs from a plain Linux system. One detection for the
//! CLI and the server, from what the system shows rather than from a list of app names, so that
//! hosts nobody listed still get the right behaviour:
//!
//! - **layer**: how this Linux userland reaches the kernel: Termux's own prefix; proot (ptrace,
//!   found by its tracer process: proot-distro, UserLAnd, Andronix, AnLinux...); a preloaded
//!   library that maps the root (Cortex Terminal, or any app doing the same); a chroot (rooted
//!   phones: Cortex's root mode, Linux Deploy); a container (docker, podman, LXC, distrobox,
//!   toolbox, systemd-nspawn); or the system itself.
//! - **fake root**: getuid() says 0 but the kernel knows another uid (proot -0, Cortex). Sockets
//!   carry the real one, so peers that compare the two (Xwayland, D-Bus EXTERNAL) need care.
//! - **bridges**: the ways to reach Android from here, best first: Termux's am, Cortex's
//!   `cortex-api`, Android's own am, then xdg-open.
//! - **app**: the Android package whose files this runs in, when it shows.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub enum Layer {
    /// Termux's prefix (or a fork with the same layout).
    Termux,
    /// proot: `via` is the tracer's name.
    Proot { via: String },
    /// A preloaded library maps `/usr`, `/etc`... into `root` (real path): only dynamically linked
    /// glibc programs see the mapped files. `name`: "Cortex", else the library's name.
    Preload { name: String, root: PathBuf, lib: PathBuf },
    /// A chroot as real root (no ptrace, no preload).
    Chroot,
    /// A container: docker, podman, lxc, distrobox, toolbox, systemd-nspawn...
    Container { engine: String },
    /// The system itself.
    Native,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Bridge {
    /// Termux's am (its socket server, else its app_process am).
    TermuxAm,
    /// Cortex Terminal's `cortex-api` (open, notification, clipboard...).
    CortexApi,
    /// Android's own /system/bin/am (works for root and shell, and from Termux).
    Am,
    /// The desktop's xdg-open.
    XdgOpen,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Host {
    pub android: bool,
    pub layer: Layer,
    pub fake_root: bool,
    /// Android package whose private files this runs in (com.termux, org.cortex.terminal...).
    pub app: Option<String>,
    pub bridges: Vec<Bridge>,
}

/// What detection looks at, gathered once (`probe`) so `classify` stays a pure function.
#[derive(Debug, Clone, Default)]
pub struct Probe {
    pub android: bool,
    pub termux: bool,
    /// The name (comm, else exe file name) of the process tracing this one, if any.
    pub tracer: Option<String>,
    pub ld_preload: Option<String>,
    pub cortex_root: Option<PathBuf>,
    pub getuid: u32,
    pub real_uid: u32,
    /// `/` is not init's root (readable only as real root).
    pub chrooted: bool,
    pub container: Option<String>,
    /// This binary is static (it never loads a preloaded library).
    pub static_build: bool,
    /// `/usr/bin` is there as this process sees it.
    pub usr_visible: bool,
    /// Paths that may name the Android app's data dir: $PREFIX, $HOME, the tracer's exe, ours.
    pub paths: Vec<String>,
    pub have_cortex_api: bool,
    pub have_termux_am: bool,
    pub have_system_am: bool,
}

pub fn detect() -> Host {
    classify(&probe())
}

pub fn classify(p: &Probe) -> Host {
    let layer = if p.termux {
        Layer::Termux
    } else if let Some(t) = p.tracer.as_ref().filter(|t| t.contains("proot")) {
        Layer::Proot { via: t.clone() }
    } else if let Some(pre) = preload_layer(p) {
        pre
    } else if let Some(engine) = &p.container {
        Layer::Container { engine: engine.clone() }
    } else if p.chrooted {
        Layer::Chroot
    } else {
        Layer::Native
    };
    let mut bridges = Vec::new();
    if p.android {
        if p.have_termux_am || matches!(layer, Layer::Termux | Layer::Proot { .. }) {
            bridges.push(Bridge::TermuxAm);
        }
        if p.have_cortex_api || matches!(&layer, Layer::Preload { name, .. } if name == "Cortex") {
            bridges.push(Bridge::CortexApi);
        }
        // An app's own uid cannot use Android's am (shell commands are for shell and root), and
        // Cortex's am is not one; Termux's runs am in app_process, which works.
        if p.have_system_am && (p.real_uid == 0 || p.real_uid == 2000 || bridges.contains(&Bridge::TermuxAm)) {
            bridges.push(Bridge::Am);
        }
    }
    bridges.push(Bridge::XdgOpen);
    let app = p.paths.iter().find_map(|s| android_package(s));
    Host { android: p.android, layer, fake_root: p.getuid != p.real_uid, app, bridges }
}

/// A preloaded library that maps the root: Cortex by name; any other when a static binary does
/// not see `/usr` on Android (only the mapping could show it to the shell that started us).
fn preload_layer(p: &Probe) -> Option<Layer> {
    let libs: Vec<PathBuf> = p.ld_preload.as_deref()?.split([' ', ':']).filter(|s| !s.is_empty()).map(PathBuf::from).collect();
    if let Some(lib) = libs.iter().find(|l| l.to_string_lossy().contains("libcortex-hook")) {
        let root = p.cortex_root.clone().or_else(|| root_of(lib))?;
        return Some(Layer::Preload { name: "Cortex".into(), root, lib: lib.clone() });
    }
    if !(p.android && p.static_build && !p.usr_visible) {
        return None;
    }
    libs.iter().find_map(|lib| {
        let root = root_of(lib)?;
        let name = lib.file_stem()?.to_string_lossy().trim_start_matches("lib").to_string();
        Some(Layer::Preload { name, root, lib: lib.clone() })
    })
}

/// The root a mapping library belongs to: the nearest ancestor with an `etc/os-release`.
fn root_of(lib: &Path) -> Option<PathBuf> {
    lib.ancestors().skip(1).find(|d| d.join("etc/os-release").is_file()).map(Path::to_path_buf)
}

/// `/data/data/<pkg>/...` or `/data/user/<n>/<pkg>/...` -> `<pkg>`.
pub fn android_package(path: &str) -> Option<String> {
    let rest = path.strip_prefix("/data/data/").or_else(|| {
        let r = path.strip_prefix("/data/user/")?;
        r.split_once('/').map(|(_, r)| r)
    })?;
    let pkg = rest.split('/').next()?;
    (pkg.contains('.') && !pkg.is_empty()).then(|| pkg.to_string())
}

fn probe() -> Probe {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let field = |k: &str| status.lines().find(|l| l.starts_with(k)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<u32>().ok());
    let getuid = unsafe { libc::getuid() };
    let real_uid = field("Uid:").unwrap_or(getuid);
    let tracer_pid = field("TracerPid:").filter(|&p| p != 0);
    let tracer_exe = tracer_pid.and_then(|p| std::fs::read_link(format!("/proc/{p}/exe")).ok());
    let tracer = tracer_pid.and_then(|p| {
        let comm = std::fs::read_to_string(format!("/proc/{p}/comm")).ok().map(|c| c.trim().to_string());
        comm.filter(|c| !c.is_empty()).or_else(|| tracer_exe.as_ref().and_then(|e| e.file_name()).map(|n| n.to_string_lossy().into_owned()))
    });
    // proot hides itself from TracerPid in some builds; its loader variables stay.
    let tracer = tracer.or_else(|| ["PROOT_TMP_DIR", "PROOT_LOADER", "PROOT_L2S_DIR"].iter().any(|v| std::env::var_os(v).is_some()).then(|| "proot".to_string()));
    let prefix = std::env::var("PREFIX").unwrap_or_default();
    let android = ["/system/build.prop", "/system/bin/app_process", "/dev/binder", "/dev/binderfs", "/dev/hwbinder"].iter().any(|p| Path::new(p).exists())
        || std::fs::read_to_string("/proc/version").map_or(false, |v| v.to_lowercase().contains("android"));
    let termux = prefix.contains("/com.termux")
        || (android && std::env::var_os("TERMUX_VERSION").is_some() && Path::new(lindroids_proto::TERMUX_PREFIX).exists());
    let chrooted = real_uid == 0
        && match (std::fs::metadata("/"), std::fs::metadata("/proc/1/root/")) {
            (Ok(a), Ok(b)) => {
                use std::os::unix::fs::MetadataExt;
                (a.dev(), a.ino()) != (b.dev(), b.ino())
            }
            _ => false,
        };
    let mut paths: Vec<String> = ["PREFIX", "HOME", "CORTEX_ROOT"].iter().filter_map(|v| std::env::var(v).ok()).collect();
    paths.extend(tracer_exe.map(|e| e.display().to_string()));
    paths.extend(std::env::current_exe().ok().map(|e| e.display().to_string()));
    let on_path = |cmd: &str| std::env::var_os("PATH").map_or(false, |p| std::env::split_paths(&p).any(|d| d.join(cmd).is_file()));
    Probe {
        android,
        termux,
        tracer,
        ld_preload: std::env::var("LD_PRELOAD").ok().filter(|s| !s.trim().is_empty()),
        cortex_root: std::env::var_os("CORTEX_ROOT").map(PathBuf::from).filter(|p| p.is_dir()),
        getuid,
        real_uid,
        chrooted,
        container: container(),
        static_build: cfg!(target_env = "musl"),
        usr_visible: Path::new("/usr/bin").is_dir(),
        paths,
        have_cortex_api: on_path("cortex-api"),
        have_termux_am: on_path("termux-am") || Path::new(lindroids_proto::TERMUX_PREFIX).join("bin/termux-am").exists(),
        have_system_am: Path::new("/system/bin/am").exists(),
    }
}

/// The container engine this runs in, from the marks engines leave.
fn container() -> Option<String> {
    if std::env::var_os("CONTAINER_ID").is_some() || std::env::var_os("DISTROBOX_ENTER_PATH").is_some() {
        return Some("distrobox".into());
    }
    if std::env::var_os("TOOLBOX_PATH").is_some() || Path::new("/run/.toolboxenv").exists() {
        return Some("toolbox".into());
    }
    if let Ok(t) = std::fs::read_to_string("/run/.containerenv") {
        let engine = t.lines().find_map(|l| l.strip_prefix("engine=")).map(|e| e.trim_matches('"').split('-').next().unwrap_or("podman").to_string());
        return Some(engine.unwrap_or_else(|| "podman".into()));
    }
    if Path::new("/.dockerenv").exists() {
        return Some("docker".into());
    }
    if let Some(c) = std::env::var("container").ok().filter(|c| !c.is_empty()) {
        return Some(c);
    }
    let cgroup = std::fs::read_to_string("/proc/1/cgroup").unwrap_or_default();
    ["docker", "lxc", "kubepods", "libpod"].iter().find(|k| cgroup.contains(*k)).map(|k| if *k == "libpod" { "podman".into() } else { k.to_string() })
}

impl Host {
    /// The root a preloaded mapping library shows Linux programs, when there is one.
    pub fn mapped_root(&self) -> Option<&Path> {
        match &self.layer {
            Layer::Preload { root, .. } => Some(root),
            _ => None,
        }
    }

    pub fn is_cortex(&self) -> bool {
        matches!(&self.layer, Layer::Preload { name, .. } if name == "Cortex")
    }

    /// The terminal app to name to people ("In Termux, run ..."), when known.
    pub fn app_label(&self) -> Option<String> {
        match (&self.layer, self.app.as_deref()) {
            (Layer::Termux, _) | (_, Some("com.termux")) => Some("Termux".into()),
            (_, Some("org.cortex.terminal")) => Some("Cortex".into()),
            (_, Some("tech.ula")) => Some("UserLAnd".into()),
            (_, Some(p)) if p.contains("andronix") => Some("Andronix".into()),
            (Layer::Preload { name, .. }, _) => Some(name.clone()),
            _ => None,
        }
    }

    /// One line for `lindroids doctor`.
    pub fn describe(&self) -> String {
        let layer = match &self.layer {
            Layer::Termux => "Termux prefix".to_string(),
            Layer::Proot { via } => format!("proot ({via})"),
            Layer::Preload { name, root, .. } => format!("{name}: preloaded path mapping onto {}", root.display()),
            Layer::Chroot => "chroot".into(),
            Layer::Container { engine } => format!("container ({engine})"),
            Layer::Native => "native".into(),
        };
        let bridges: Vec<&str> = self
            .bridges
            .iter()
            .map(|b| match b {
                Bridge::TermuxAm => "termux-am",
                Bridge::CortexApi => "cortex-api",
                Bridge::Am => "am",
                Bridge::XdgOpen => "xdg-open",
            })
            .collect();
        format!(
            "{}{layer}{}{}; Android via {}",
            if self.android { "Android, " } else { "" },
            if self.fake_root { ", fake root" } else { "" },
            self.app.as_deref().map(|a| format!(", app {a}")).unwrap_or_default(),
            bridges.join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn android() -> Probe {
        Probe { android: true, getuid: 10325, real_uid: 10325, usr_visible: true, have_system_am: true, ..Default::default() }
    }

    #[test]
    fn termux() {
        let h = classify(&Probe { termux: true, have_termux_am: true, paths: vec!["/data/data/com.termux/files/usr".into()], ..android() });
        assert_eq!(h.layer, Layer::Termux);
        assert_eq!(h.bridges, vec![Bridge::TermuxAm, Bridge::Am, Bridge::XdgOpen]);
        assert_eq!(h.app.as_deref(), Some("com.termux"));
        assert_eq!(h.app_label().as_deref(), Some("Termux"));
        assert!(!h.fake_root);
    }

    #[test]
    fn proot_distro_fakes_root() {
        let h = classify(&Probe { tracer: Some("proot".into()), getuid: 0, paths: vec!["/data/data/com.termux/files/usr/bin/proot".into()], ..android() });
        assert_eq!(h.layer, Layer::Proot { via: "proot".into() });
        assert!(h.fake_root);
        assert_eq!(h.bridges[0], Bridge::TermuxAm);
        assert_eq!(h.app_label().as_deref(), Some("Termux"));
    }

    #[test]
    fn userland() {
        let h = classify(&Probe { tracer: Some("proot".into()), getuid: 0, paths: vec!["/data/user/0/tech.ula/files/support/proot".into()], ..android() });
        assert_eq!(h.app.as_deref(), Some("tech.ula"));
        assert_eq!(h.app_label().as_deref(), Some("UserLAnd"));
    }

    #[test]
    fn cortex() {
        let root = std::env::temp_dir().join(format!("lindroids-host-test-{}", std::process::id()));
        std::fs::create_dir_all(root.join("etc")).unwrap();
        std::fs::write(root.join("etc/os-release"), "ID=ubuntu\n").unwrap();
        let lib = root.join("usr/lib/libcortex-hook.so");
        let p = Probe {
            ld_preload: Some(lib.display().to_string()),
            getuid: 0,
            static_build: true,
            usr_visible: false,
            have_cortex_api: true,
            paths: vec!["/data/user/0/org.cortex.terminal/files/cortex".into()],
            ..android()
        };
        let h = classify(&p);
        assert!(h.is_cortex());
        assert_eq!(h.mapped_root(), Some(root.as_path()));
        assert!(h.fake_root);
        // An app uid: no system am.
        assert_eq!(h.bridges, vec![Bridge::CortexApi, Bridge::XdgOpen]);
        assert_eq!(h.app_label().as_deref(), Some("Cortex"));
        // Another app doing the same, by its effect: a static binary sees no /usr.
        let other = classify(&Probe { ld_preload: Some(root.join("usr/lib/libmapper.so").display().to_string()), have_cortex_api: false, ..p.clone() });
        assert_eq!(other.layer, Layer::Preload { name: "mapper".into(), root: root.clone(), lib: root.join("usr/lib/libmapper.so") });
        // An ordinary preload (no mapping): /usr is there.
        let plain = classify(&Probe { ld_preload: Some("/usr/lib/libfoo.so".into()), usr_visible: true, ..android() });
        assert_eq!(plain.layer, Layer::Native);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn chroot_and_containers() {
        let h = classify(&Probe { chrooted: true, getuid: 0, real_uid: 0, ..android() });
        assert_eq!(h.layer, Layer::Chroot);
        assert_eq!(h.bridges, vec![Bridge::Am, Bridge::XdgOpen]);
        let d = classify(&Probe { container: Some("docker".into()), usr_visible: true, ..Default::default() });
        assert_eq!(d.layer, Layer::Container { engine: "docker".into() });
        assert_eq!(d.bridges, vec![Bridge::XdgOpen]);
        assert!(!d.android);
    }

    #[test]
    fn packages() {
        assert_eq!(android_package("/data/data/com.termux/files/home").as_deref(), Some("com.termux"));
        assert_eq!(android_package("/data/user/10/org.cortex.terminal/files").as_deref(), Some("org.cortex.terminal"));
        assert_eq!(android_package("/home/me"), None);
    }
}
