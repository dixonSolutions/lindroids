//! Reading a dpkg database (Termux's or a Debian/Ubuntu rootfs's) to find what a program needs:
//! the package that owns it, every package it depends on, and their files.
//!
//! Exported bundles carry exactly that closure instead of a whole distribution.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

pub struct Db {
    /// Where the database lives: `<root>/var/lib/dpkg` (Termux: `$PREFIX/var/lib/dpkg`).
    dir: PathBuf,
    /// Installed packages by name: their dependency groups (alternatives within a group).
    deps: HashMap<String, Vec<Vec<String>>>,
    /// Virtual package -> real installed packages providing it.
    provides: HashMap<String, Vec<String>>,
    /// Package -> its `info/<name>[:arch].list` file name.
    lists: HashMap<String, String>,
    essential: Vec<String>,
}

fn strip(dep: &str) -> String {
    let d = dep.trim();
    let d = d.split_whitespace().next().unwrap_or("");
    d.split(':').next().unwrap_or("").to_string()
}

fn parse_deps(v: &str) -> Vec<Vec<String>> {
    v.split(',').map(|g| g.split('|').map(strip).filter(|s| !s.is_empty()).collect()).filter(|g: &Vec<String>| !g.is_empty()).collect()
}

impl Db {
    pub fn open(dpkg_dir: &Path) -> Result<Db, String> {
        let status = std::fs::read_to_string(dpkg_dir.join("status")).map_err(|e| format!("{}/status: {e}", dpkg_dir.display()))?;
        let mut db = Db { dir: dpkg_dir.to_path_buf(), deps: HashMap::new(), provides: HashMap::new(), lists: HashMap::new(), essential: Vec::new() };
        for para in status.split("\n\n") {
            let mut name = String::new();
            let mut arch = String::new();
            let mut installed = false;
            let mut deps = Vec::new();
            let mut provides = Vec::new();
            let mut essential = false;
            // Fold continuation lines into their field.
            let mut fields: Vec<(String, String)> = Vec::new();
            for line in para.lines() {
                if line.starts_with(' ') || line.starts_with('\t') {
                    if let Some(last) = fields.last_mut() {
                        last.1.push(' ');
                        last.1.push_str(line.trim());
                    }
                } else if let Some((k, v)) = line.split_once(':') {
                    fields.push((k.to_string(), v.trim().to_string()));
                }
            }
            for (k, v) in fields {
                match k.as_str() {
                    "Package" => name = v,
                    "Architecture" => arch = v,
                    "Status" => installed = v.ends_with(" installed"),
                    "Depends" | "Pre-Depends" => deps.extend(parse_deps(&v)),
                    "Provides" => provides = v.split(',').map(strip).filter(|s| !s.is_empty()).collect(),
                    "Essential" => essential = v == "yes",
                    _ => {}
                }
            }
            if name.is_empty() || !installed {
                continue;
            }
            for p in provides {
                db.provides.entry(p).or_default().push(name.clone());
            }
            let with_arch = format!("{name}:{arch}");
            let list = if dpkg_dir.join("info").join(format!("{with_arch}.list")).exists() { with_arch } else { name.clone() };
            db.lists.insert(name.clone(), list);
            if essential {
                db.essential.push(name.clone());
            }
            db.deps.insert(name, deps);
        }
        Ok(db)
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

    /// The package that owns `path` (as dpkg recorded it, e.g. /usr/bin/foo).
    pub fn owner(&self, path: &str) -> Option<String> {
        for (pkg, list) in &self.lists {
            if let Ok(text) = std::fs::read_to_string(self.dir.join("info").join(format!("{list}.list"))) {
                if text.lines().any(|l| l == path) {
                    return Some(pkg.clone());
                }
            }
        }
        None
    }

    /// `roots`, everything they depend on, and (with `essential`) the Essential: yes packages.
    pub fn closure(&self, roots: &[String], essential: bool) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut todo: Vec<String> = roots.iter().filter_map(|r| self.resolve(r)).collect();
        if essential {
            todo.extend(self.essential.iter().cloned());
        }
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
        out
    }

    /// Every path the packages own, as recorded (absolute, in the package's own view).
    pub fn files(&self, pkgs: &BTreeSet<String>) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for p in pkgs {
            let Some(list) = self.lists.get(p) else { continue };
            if let Ok(text) = std::fs::read_to_string(self.dir.join("info").join(format!("{list}.list"))) {
                out.extend(text.lines().filter(|l| !l.is_empty() && *l != "/.").map(String::from));
            }
        }
        out
    }

    /// Paths owned by any installed package (to tell generated files from packaged ones).
    pub fn all_owned(&self) -> HashSet<String> {
        let all: BTreeSet<String> = self.lists.keys().cloned().collect();
        self.files(&all).into_iter().collect()
    }
}

/// Files that package scripts generate in the packages' own directories (gschemas.compiled,
/// loaders.cache, icon-theme.cache...): not in any .list, yet toolkits fail without them.
/// `to_host` maps a recorded path to where it is on this filesystem.
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

    fn fake_db() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lindroids-dpkg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("info")).unwrap();
        std::fs::write(
            dir.join("status"),
            "Package: app\nStatus: install ok installed\nArchitecture: amd64\nDepends: libfoo (>= 1), libbar | libbaz, gone\n\n\
             Package: libfoo\nStatus: install ok installed\nArchitecture: amd64\nPre-Depends: libc6\n\n\
             Package: libbaz\nStatus: install ok installed\nArchitecture: amd64\nProvides: libbar\n\n\
             Package: libc6\nStatus: install ok installed\nArchitecture: amd64\n\n\
             Package: old\nStatus: deinstall ok config-files\nArchitecture: amd64\n\n\
             Package: dash\nStatus: install ok installed\nArchitecture: amd64\nEssential: yes\n",
        )
        .unwrap();
        std::fs::write(dir.join("info/app.list"), "/.\n/usr\n/usr/bin\n/usr/bin/app\n").unwrap();
        std::fs::write(dir.join("info/libfoo:amd64.list"), "/usr/lib/libfoo.so.1\n").unwrap();
        dir
    }

    #[test]
    fn closure_follows_alternatives_and_provides() {
        let dir = fake_db();
        let db = Db::open(&dir).unwrap();
        let c = db.closure(&["app".into()], false);
        assert_eq!(c.into_iter().collect::<Vec<_>>(), vec!["app", "libbaz", "libc6", "libfoo"]);
        assert!(db.closure(&["app".into()], true).contains("dash"));
        assert_eq!(db.owner("/usr/bin/app").as_deref(), Some("app"));
        let files = db.files(&["app".to_string(), "libfoo".to_string()].into_iter().collect());
        assert!(files.contains("/usr/lib/libfoo.so.1"));
        assert!(!files.contains("/."));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
