//! Where Flatpak keeps things: installations, deployed refs, and the extensions a runtime or app
//! asks for. Everything is read from disk; the `flatpak` binary (and its D-Bus helpers) need not
//! be installed or runnable, which on Android they are not.

use std::path::{Component, Path, PathBuf};

use super::keyfile::KeyFile;

/// Flatpak's name for the architecture this binary runs on.
pub fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86" => "i386",
        "arm" => "arm",
        a => a,
    }
}

/// The Debian-style library directory runtimes use (`/usr/lib/<triplet>`).
#[cfg(test)]
pub fn multiarch(arch: &str) -> String {
    match arch {
        "arm" => "arm-linux-gnueabihf".into(),
        a => format!("{a}-linux-gnu"),
    }
}

/// A Flatpak installation (a directory with `app/`, `runtime/`, `repo/`).
#[derive(Debug, Clone, PartialEq)]
pub struct Installation {
    /// As this process sees it.
    pub host: PathBuf,
    /// As programs inside `root` see it.
    pub guest: String,
}

/// Installations under `root`, the ones a user would pick first first: per-user ones, then the
/// system one.
pub fn installations(root: &Path) -> Vec<Installation> {
    let mut v = Vec::new();
    let mut add = |guest: String| {
        let host = real(root, &root.join(guest.trim_start_matches('/')));
        if host.join("app").is_dir() || host.join("runtime").is_dir() {
            if !v.iter().any(|i: &Installation| i.host == host) {
                v.push(Installation { host, guest });
            }
        }
    };
    if root == Path::new("/") {
        if let Some(d) = std::env::var_os("FLATPAK_USER_DIR") {
            add(PathBuf::from(d).display().to_string());
        }
        if let Some(home) = std::env::var_os("HOME") {
            add(PathBuf::from(home).join(".local/share/flatpak").display().to_string());
        }
    }
    add("/root/.local/share/flatpak".into());
    if let Ok(rd) = std::fs::read_dir(real(root, &root.join("home"))) {
        let mut users: Vec<String> = rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        users.sort();
        for u in users {
            add(format!("/home/{u}/.local/share/flatpak"));
        }
    }
    if root == Path::new("/") {
        if let Some(d) = std::env::var_os("FLATPAK_SYSTEM_DIR") {
            add(PathBuf::from(d).display().to_string());
        }
    }
    add("/var/lib/flatpak".into());
    v
}

/// `p` (a path under `root`) with symlinks resolved the way a program inside `root` would see
/// them: an absolute link target is relative to `root`, not to this process's `/`. A proot
/// distro's rootfs is full of those.
pub fn real(root: &Path, p: &Path) -> PathBuf {
    if root == Path::new("/") {
        return std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    }
    let Ok(rel) = p.strip_prefix(root) else { return p.to_path_buf() };
    let mut todo: Vec<std::ffi::OsString> = rel.components().rev().filter_map(|c| match c {
        Component::Normal(s) => Some(s.to_os_string()),
        Component::ParentDir => Some("..".into()),
        _ => None,
    }).collect();
    let mut cur = root.to_path_buf();
    let mut hops = 0;
    while let Some(c) = todo.pop() {
        if c == ".." {
            if cur != root {
                cur.pop();
            }
            continue;
        }
        let next = cur.join(&c);
        match std::fs::read_link(&next) {
            Ok(target) if hops < 40 => {
                hops += 1;
                if target.is_absolute() {
                    cur = root.to_path_buf();
                }
                for t in target.components().rev() {
                    match t {
                        Component::Normal(s) => todo.push(s.to_os_string()),
                        Component::ParentDir => todo.push("..".into()),
                        _ => {}
                    }
                }
            }
            _ => cur = next,
        }
    }
    cur
}

/// A deployed ref: `<installation>/<kind>/<id>/<arch>/<branch>/active`.
#[derive(Debug, Clone, PartialEq)]
pub struct Deploy {
    pub id: String,
    pub arch: String,
    pub branch: String,
    /// The deploy directory (`active` resolved), holding `files/`, `metadata` and, for apps,
    /// `export/`.
    pub dir: PathBuf,
    pub metadata: KeyFile,
    /// Index into the installation list this was found in.
    pub installation: usize,
}

impl Deploy {
    pub fn files(&self) -> PathBuf {
        self.dir.join("files")
    }
}

fn deploy_at(root: &Path, insts: &[Installation], i: usize, kind: &str, id: &str, arch: &str, branch: &str) -> Option<Deploy> {
    let active = real(root, &insts[i].host.join(kind).join(id).join(arch).join(branch).join("active"));
    if !active.join("files").is_dir() {
        return None;
    }
    let metadata = KeyFile::read(&active.join("metadata")).unwrap_or_default();
    Some(Deploy { id: id.into(), arch: arch.into(), branch: branch.into(), dir: active, metadata, installation: i })
}

/// Look up `kind` ("app" or "runtime") `id`. With no `branch`, the one `current` points to, then
/// stable, then master, then any. `prefer` is searched first.
pub fn find_ref(root: &Path, insts: &[Installation], prefer: Option<usize>, kind: &str, id: &str, arch: &str, branch: Option<&str>) -> Option<Deploy> {
    let order: Vec<usize> = prefer.into_iter().chain((0..insts.len()).filter(|i| Some(*i) != prefer)).collect();
    for i in order {
        let base = insts[i].host.join(kind).join(id);
        if !base.is_dir() {
            continue;
        }
        if let Some(b) = branch {
            if let Some(d) = deploy_at(root, insts, i, kind, id, arch, b) {
                return Some(d);
            }
            continue;
        }
        let mut branches: Vec<String> = Vec::new();
        if let Ok(cur) = std::fs::read_link(base.join("current")) {
            let cur = cur.to_string_lossy().into_owned();
            if let Some((a, b)) = cur.split_once('/') {
                if a == arch {
                    branches.push(b.to_string());
                }
            }
        }
        branches.push("stable".into());
        branches.push("master".into());
        if let Ok(rd) = std::fs::read_dir(base.join(arch)) {
            let mut rest: Vec<String> = rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
            rest.sort();
            branches.extend(rest);
        }
        for b in &branches {
            if let Some(d) = deploy_at(root, insts, i, kind, id, arch, b) {
                return Some(d);
            }
        }
    }
    None
}

/// An extension mounted into the app's view.
#[derive(Debug, Clone, PartialEq)]
pub struct Extension {
    /// The extension's ref id, e.g. `org.freedesktop.Platform.GL.default`.
    pub id: String,
    pub branch: String,
    /// Its `files/` directory, as this process sees it.
    pub files: PathBuf,
    /// Where it appears inside the sandbox, e.g. `/usr/lib/x86_64-linux-gnu/GL/default`.
    pub mount: String,
    /// The extension point's directory inside the sandbox (`/usr/lib/x86_64-linux-gnu/GL`);
    /// equals `mount` unless the point has `subdirectories=true`.
    pub point: String,
    /// `add-ld-path`, inside the sandbox.
    pub ld_path: Option<String>,
    /// `merge-dirs`: subdirectories that flatpak merges from all of a point's extensions into
    /// `<point>/<dir>` (Vulkan ICDs, EGL vendors, DRI drivers).
    pub merge_dirs: Vec<String>,
    pub installation: usize,
}

/// GL drivers flatpak would enable: `$FLATPAK_GL_DRIVERS`, else the Mesa build ("default") and
/// a host-provided one.
fn gl_drivers() -> Vec<String> {
    match std::env::var("FLATPAK_GL_DRIVERS") {
        Ok(v) if !v.is_empty() => v.split(':').map(String::from).collect(),
        _ => vec!["default".into(), "host".into()],
    }
}

/// flatpak's `enable-if` conditions that can be judged without flatpak.
fn enabled(cond: Option<&str>, suffix: Option<&str>) -> bool {
    match cond {
        None => true,
        Some("active-gl-driver") => suffix.map_or(true, |s| gl_drivers().iter().any(|d| d == s)),
        Some("have-intel-gpu") => Path::new("/sys/module/i915").exists(),
        Some(c) => match c.strip_prefix("have-kernel-module-") {
            Some(m) => Path::new("/sys/module").join(m).exists(),
            None => true,
        },
    }
}

/// Installed extensions for the `[Extension ...]` points in `meta`, mounted under `base`
/// ("/usr" for a runtime's points, "/app" for an app's). `default_branch` is used when a point
/// names no version (the ref's own branch).
pub fn extensions(root: &Path, insts: &[Installation], meta: &KeyFile, base: &str, default_branch: &str, arch: &str, prefer: usize) -> Vec<Extension> {
    let mut out = Vec::new();
    for (name, g) in meta.extension_points() {
        // Debug symbols and sources are only for `flatpak run --devel`.
        if name.ends_with(".Debug") || name.ends_with(".Sources") {
            continue;
        }
        let Some(dir) = g.get("directory") else { continue };
        let point = format!("{base}/{}", dir.trim_matches('/'));
        let mut versions = g.list("versions");
        if versions.is_empty() {
            versions = g.get("version").map(|v| vec![v.to_string()]).unwrap_or_else(|| vec![default_branch.to_string()]);
        }
        let ld = g.get("add-ld-path").map(|p| p.trim_matches('/').to_string());
        let merge_dirs = g.list("merge-dirs");
        let cond = g.get("enable-if");
        let mut add = |id: &str, mount: String, suffix: Option<&str>| {
            if !enabled(cond, suffix) || out.iter().any(|e: &Extension| e.id == id || e.mount == mount) {
                return;
            }
            let Some(d) = versions.iter().find_map(|v| find_ref(root, insts, Some(prefer), "runtime", id, arch, Some(v))) else { return };
            let ld_path = ld.as_ref().map(|l| if l.is_empty() { mount.clone() } else { format!("{mount}/{l}") });
            out.push(Extension {
                id: id.into(),
                branch: d.branch.clone(),
                files: d.files(),
                mount,
                point: point.clone(),
                ld_path,
                merge_dirs: merge_dirs.clone(),
                installation: d.installation,
            });
        };
        if g.bool("subdirectories") {
            let mut ids: Vec<String> = Vec::new();
            for inst in insts {
                let Ok(rd) = std::fs::read_dir(inst.host.join("runtime")) else { continue };
                for e in rd.filter_map(|e| e.ok()) {
                    let n = e.file_name().to_string_lossy().into_owned();
                    let Some(suffix) = n.strip_prefix(name).and_then(|s| s.strip_prefix('.')) else { continue };
                    // GL.Debug is its own extension point, not a driver called "Debug".
                    if suffix.is_empty() || suffix == "Debug" || suffix.ends_with(".Debug") || ids.contains(&n) {
                        continue;
                    }
                    ids.push(n);
                }
            }
            ids.sort();
            let sub_suffix = g.get("subdirectory-suffix");
            for id in ids {
                let suffix = &id[name.len() + 1..];
                let mut mount = format!("{point}/{suffix}");
                if let Some(s) = sub_suffix {
                    mount = format!("{mount}/{}", s.trim_matches('/'));
                }
                add(&id, mount, Some(suffix));
            }
        } else {
            add(name, point.clone(), None);
        }
    }
    out
}
