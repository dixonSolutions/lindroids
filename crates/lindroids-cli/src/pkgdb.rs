//! Reading a package database to find what a program needs: the package that owns it, every
//! package it depends on, and their files. Termux and Debian/Ubuntu keep a dpkg database, Alpine
//! an apk one and Arch a pacman one; each is read into the same shape, so the closure, the plugin
//! providers and the generated files are worked out the same way for all of them.
//!
//! Exported bundles carry exactly that closure instead of a whole distribution.

mod apk;
mod dpkg;
mod pacman;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Which package manager wrote the database, and so which distribution family the root is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// Termux, Debian, Ubuntu: `var/lib/dpkg`.
    Dpkg,
    /// Alpine: `lib/apk/db/installed`.
    Apk,
    /// Arch: `var/lib/pacman/local`.
    Pacman,
}

pub struct Db {
    pub format: Format,
    /// Installed packages by name: their dependency groups (alternatives within a group).
    deps: HashMap<String, Vec<Vec<String>>>,
    /// Virtual name -> real installed packages providing it, in database order. Versions are
    /// stripped: dpkg's `foo (= 1)`, apk's `so:libfoo.so.1=1.2`, pacman's `foo=1-2`.
    provides: HashMap<String, Vec<String>>,
    /// What every root needs to be a system at all: dpkg's `Essential: yes`, a fixed base for
    /// formats that have no such field.
    essential: Vec<String>,
    /// apk's `install_if`: packages that come by themselves once all their conditions are
    /// installed (`!name`: once it is not). Alpine's glycin image loaders arrive this way, and
    /// GTK draws no PNG or SVG without them.
    install_if: Vec<(String, Vec<(bool, String)>)>,
    files: Files,
}

enum Files {
    /// dpkg keeps each package's list in `info/<name>[:arch].list`: read when asked.
    Lists { info: PathBuf, lists: HashMap<String, String> },
    /// apk (whose lists are in the same file as everything else) and pacman: read at open, as
    /// absolute paths without a trailing slash.
    Loaded(HashMap<String, Vec<String>>),
}

impl Db {
    /// A dpkg database directory (Termux: `$PREFIX/var/lib/dpkg`).
    pub fn open(dpkg_dir: &Path) -> Result<Db, String> {
        dpkg::open(dpkg_dir)
    }

    /// The database of the root filesystem `rootfs`, whichever package manager it has.
    pub fn open_rootfs(rootfs: &Path) -> Result<Db, String> {
        if rootfs.join("var/lib/dpkg/status").is_file() {
            return dpkg::open(&rootfs.join("var/lib/dpkg"));
        }
        if rootfs.join("lib/apk/db/installed").is_file() {
            return apk::open(&rootfs.join("lib/apk/db/installed"));
        }
        if rootfs.join("var/lib/pacman/local").is_dir() {
            return pacman::open(&rootfs.join("var/lib/pacman/local"));
        }
        if ["usr/lib/sysimage/rpm", "var/lib/rpm"].iter().any(|d| rootfs.join(d).is_dir()) {
            return Err("it uses rpm (Fedora, openSUSE...), whose SQLite database Lindroids does not read yet".into());
        }
        Err(format!("no dpkg, apk or pacman database in {}", rootfs.display()))
    }

    fn new(format: Format, files: Files) -> Db {
        Db { format, deps: HashMap::new(), provides: HashMap::new(), essential: Vec::new(), install_if: Vec::new(), files }
    }

    /// Record an installed package; `provides` are names already stripped of versions.
    fn add(&mut self, name: String, deps: Vec<Vec<String>>, provides: Vec<String>) {
        for p in provides {
            if p != name {
                self.provides.entry(p).or_default().push(name.clone());
            }
        }
        self.deps.insert(name, deps);
    }

    /// For formats without an Essential field: `base`, as far as it is installed.
    fn set_base(&mut self, base: &[&str]) {
        self.essential = base.iter().filter(|p| self.installed(p)).map(|p| p.to_string()).collect();
    }

    pub fn installed(&self, pkg: &str) -> bool {
        self.deps.contains_key(pkg)
    }

    fn resolve(&self, name: &str) -> Option<String> {
        if self.installed(name) {
            return Some(name.to_string());
        }
        self.provides.get(name).and_then(|v| v.first().cloned())
    }

    /// A package's files, as recorded (absolute, in the package's own view).
    fn files_of(&self, pkg: &str) -> Vec<String> {
        match &self.files {
            Files::Lists { info, lists } => {
                let Some(list) = lists.get(pkg) else { return Vec::new() };
                match std::fs::read_to_string(info.join(format!("{list}.list"))) {
                    Ok(text) => text.lines().filter(|l| !l.is_empty() && *l != "/.").map(String::from).collect(),
                    Err(_) => Vec::new(),
                }
            }
            Files::Loaded(m) => m.get(pkg).cloned().unwrap_or_default(),
        }
    }

    fn packages(&self) -> impl Iterator<Item = &String> {
        self.deps.keys()
    }

    /// The package that owns `path` (as the database recorded it, e.g. /usr/bin/foo).
    pub fn owner(&self, path: &str) -> Option<String> {
        if let Files::Loaded(m) = &self.files {
            return m.iter().find(|(_, fs)| fs.iter().any(|f| f == path)).map(|(p, _)| p.clone());
        }
        self.packages().find(|p| self.files_of(p).iter().any(|l| l == path)).cloned()
    }

    /// `roots`, everything they depend on, and (with `essential`) the essential packages.
    pub fn closure(&self, roots: &[String], essential: bool) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut todo: Vec<String> = roots.iter().filter_map(|r| self.resolve(r)).collect();
        if essential {
            todo.extend(self.essential.iter().cloned());
        }
        loop {
            while let Some(p) = todo.pop() {
                if !out.insert(p.clone()) {
                    continue;
                }
                for group in self.deps.get(&p).into_iter().flatten() {
                    // The first alternative that is installed (or provided).
                    if let Some(dep) = group.iter().find_map(|d| self.resolve(d)) {
                        if !out.contains(&dep) {
                            todo.push(dep);
                        }
                    }
                }
            }
            let met = |(negated, c): &(bool, String)| self.resolve(c).is_some_and(|r| out.contains(&r)) != *negated;
            todo.extend(self.install_if.iter().filter(|(p, conds)| !out.contains(p) && conds.iter().all(met)).map(|(p, _)| p.clone()));
            if todo.is_empty() {
                return out;
            }
        }
    }

    /// Every path the packages own, as recorded (absolute, in the package's own view).
    pub fn files(&self, pkgs: &BTreeSet<String>) -> BTreeSet<String> {
        pkgs.iter().flat_map(|p| self.files_of(p)).collect()
    }

    /// Every installed package's files.
    pub fn all_files(&self) -> HashMap<String, Vec<String>> {
        self.packages().map(|p| (p.clone(), self.files_of(p))).collect()
    }

    /// `closure`, plus the installed packages that drop plugins into plugin directories the closure
    /// owns (pixbuf loaders, GIO/GTK modules, GStreamer plugins...), with their own dependencies.
    /// Packages do not depend on their plugins, yet toolkits fail without them: GTK cannot draw
    /// its SVG icons without librsvg's pixbuf loader.
    pub fn with_plugins(&self, closure: BTreeSet<String>, files: &HashMap<String, Vec<String>>) -> BTreeSet<String> {
        let mut set = closure;
        for _ in 0..4 {
            let plugin_dirs: Vec<&str> = set
                .iter()
                .filter_map(|p| files.get(p))
                .flatten()
                .map(String::as_str)
                .filter(|d| is_plugin_dir(d))
                .collect();
            let mut add = Vec::new();
            for (pkg, fs) in files {
                if set.contains(pkg) {
                    continue;
                }
                let provides = fs.iter().any(|f| {
                    let parent = f.rsplit_once('/').map_or("", |(d, _)| d);
                    plugin_dirs.iter().any(|d| parent == *d)
                });
                if provides {
                    add.push(pkg.clone());
                }
            }
            if add.is_empty() {
                break;
            }
            let more = self.closure(&add, false);
            set.extend(more);
        }
        set
    }

    /// Paths owned by any installed package (to tell generated files from packaged ones).
    pub fn all_owned(&self) -> HashSet<String> {
        self.packages().flat_map(|p| self.files_of(p)).collect()
    }
}

/// A dependency or provide without its version constraint: `foo>=1`, `so:libfoo.so.1=1.2`,
/// `foo<2` -> `foo`. (dpkg's `foo (>= 1)` is split on whitespace by its own parser.)
fn unversioned(s: &str) -> &str {
    s.split(['<', '>', '=', '~']).next().unwrap_or("").trim()
}

fn is_plugin_dir(d: &str) -> bool {
    const MARKS: [&str; 9] = [
        "/gdk-pixbuf-2.0/2.10.0/loaders",
        "/gio/modules",
        "/gtk-3.0/3.0.0/immodules",
        "/gtk-3.0/modules",
        "/gtk-4.0/4.0.0/media",
        "/gtk-4.0/4.0.0/printbackends",
        "/gstreamer-1.0",
        "/qt6/plugins/platforms",
        "/qt6/plugins/wayland-shell-integration",
    ];
    MARKS.iter().any(|m| d.ends_with(m))
}

/// Files that package scripts generate in the packages' own directories (gschemas.compiled,
/// loaders.cache, icon-theme.cache...): not in any package's file list, yet toolkits fail without
/// them. `to_host` maps a recorded path to where it is on this filesystem.
pub fn generated_files(owned_dirs: &BTreeSet<String>, all_owned: &HashSet<String>, to_host: impl Fn(&str) -> PathBuf) -> Vec<String> {
    let mut out = Vec::new();
    for d in owned_dirs {
        // Big shared directories hold user-installed files, not caches; leave them out.
        let base = d.rsplit('/').next().unwrap_or("");
        let shared = matches!(
            base,
            "" | "bin" | "sbin" | "lib" | "lib32" | "lib64" | "libexec" | "share" | "usr" | "include" | "doc" | "man" | "info" | "locale"
                | "applications" | "pixmaps" | "var" | "opt" | "home" | "root" | "tmp"
        ) || base.contains("-linux-");
        if shared {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(to_host(d)) else { continue };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if !ft.is_file() {
                continue;
            }
            let p = format!("{}/{}", d.trim_end_matches('/'), e.file_name().to_string_lossy());
            if !all_owned.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory for a fixture, removed first.
    pub(super) fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lindroids-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn detects_the_database_of_a_rootfs() {
        let root = scratch("rootfs-detect");
        assert!(Db::open_rootfs(&root).err().unwrap().contains("no dpkg, apk or pacman"));
        std::fs::create_dir_all(root.join("usr/lib/sysimage/rpm")).unwrap();
        assert!(Db::open_rootfs(&root).err().unwrap().contains("rpm"));
        std::fs::create_dir_all(root.join("var/lib/pacman/local")).unwrap();
        assert_eq!(Db::open_rootfs(&root).unwrap().format, Format::Pacman);
        std::fs::create_dir_all(root.join("lib/apk/db")).unwrap();
        std::fs::write(root.join("lib/apk/db/installed"), "").unwrap();
        assert_eq!(Db::open_rootfs(&root).unwrap().format, Format::Apk);
        std::fs::create_dir_all(root.join("var/lib/dpkg/info")).unwrap();
        std::fs::write(root.join("var/lib/dpkg/status"), "").unwrap();
        assert_eq!(Db::open_rootfs(&root).unwrap().format, Format::Dpkg);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn versions_come_off() {
        assert_eq!(unversioned("musl>=1.2.3_git20230424"), "musl");
        assert_eq!(unversioned("so:libc.musl-x86_64.so.1=1"), "so:libc.musl-x86_64.so.1");
        assert_eq!(unversioned("libacl.so=1-64"), "libacl.so");
        assert_eq!(unversioned("atk<=2.38.0-2"), "atk");
        assert_eq!(unversioned("cmd:sh"), "cmd:sh");
    }

    #[test]
    fn plugin_providers_join_the_closure() {
        let mut files = HashMap::new();
        files.insert("gtk".to_string(), vec!["/usr/lib/gdk-pixbuf-2.0/2.10.0/loaders".to_string()]);
        files.insert("rsvg".to_string(), vec!["/usr/lib/gdk-pixbuf-2.0/2.10.0/loaders/libpixbufloader-svg.so".to_string()]);
        files.insert("other".to_string(), vec!["/usr/bin/other".to_string()]);
        let mut db = Db::new(Format::Pacman, Files::Loaded(files.clone()));
        db.add("gtk".into(), vec![], vec![]);
        db.add("rsvg".into(), vec![vec!["libxml".into()]], vec![]);
        db.add("libxml2".into(), vec![], vec!["libxml".into()]);
        db.add("other".into(), vec![], vec![]);
        let set = db.with_plugins(db.closure(&["gtk".into()], false), &db.all_files());
        assert_eq!(set.into_iter().collect::<Vec<_>>(), vec!["gtk", "libxml2", "rsvg"]);
    }

    #[test]
    fn generated_files_are_the_unowned_ones() {
        let root = scratch("generated");
        std::fs::create_dir_all(root.join("usr/share/glib-2.0/schemas")).unwrap();
        std::fs::create_dir_all(root.join("usr/bin")).unwrap();
        for f in ["usr/share/glib-2.0/schemas/a.xml", "usr/share/glib-2.0/schemas/gschemas.compiled", "usr/bin/stray"] {
            std::fs::write(root.join(f), "").unwrap();
        }
        let dirs: BTreeSet<String> = ["/usr/share/glib-2.0/schemas", "/usr/bin"].iter().map(|s| s.to_string()).collect();
        let owned: HashSet<String> = ["/usr/share/glib-2.0/schemas/a.xml".to_string()].into_iter().collect();
        let g = generated_files(&dirs, &owned, |p| root.join(p.trim_start_matches('/')));
        assert_eq!(g, vec!["/usr/share/glib-2.0/schemas/gschemas.compiled"]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
