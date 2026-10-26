//! pacman (Arch): `var/lib/pacman/local/<name>-<version>/desc` holds a package's `%NAME%`,
//! `%DEPENDS%` and `%PROVIDES%` sections, `files` its `%FILES%`, relative to the root and with a
//! trailing slash on directories.

use std::collections::HashMap;
use std::path::Path;

use super::{unversioned, Db, Files, Format};

/// Arch has no Essential field, and its `base` pulls in systemd: the directory layout and
/// symlinks, the C library, /bin/sh, and the tools shell scripts take for granted.
const BASE: [&str; 7] = ["filesystem", "glibc", "bash", "coreutils", "sed", "grep", "findutils"];

/// The `%SECTION%` blocks of a pacman database file, by section name.
fn sections(text: &str) -> HashMap<&str, Vec<&str>> {
    let mut out: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut cur = None;
    for line in text.lines() {
        if line.len() > 2 && line.starts_with('%') && line.ends_with('%') {
            cur = Some(&line[1..line.len() - 1]);
            out.entry(cur.unwrap()).or_default();
        } else if line.is_empty() {
            cur = None;
        } else if let Some(s) = cur {
            out.entry(s).or_default().push(line);
        }
    }
    out
}

pub(super) fn open(local: &Path) -> Result<Db, String> {
    let rd = std::fs::read_dir(local).map_err(|e| format!("{}: {e}", local.display()))?;
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.join("desc").is_file()).collect();
    // Directory order is the filesystem's; sort so that the first provider of a name is stable.
    entries.sort();
    let mut db = Db::new(Format::Pacman, Files::Loaded(HashMap::new()));
    let mut files = HashMap::new();
    for e in entries {
        let Ok(desc) = std::fs::read_to_string(e.join("desc")) else { continue };
        let s = sections(&desc);
        let Some(name) = s.get("NAME").and_then(|v| v.first()).map(|n| n.to_string()) else { continue };
        let names = |k: &str| -> Vec<String> { s.get(k).into_iter().flatten().map(|d| unversioned(d).to_string()).filter(|d| !d.is_empty()).collect() };
        let deps = names("DEPENDS").into_iter().map(|d| vec![d]).collect();
        let provides = names("PROVIDES");
        let list = std::fs::read_to_string(e.join("files")).unwrap_or_default();
        let paths = sections(&list).remove("FILES").unwrap_or_default().iter().map(|p| format!("/{}", p.trim_end_matches('/'))).collect();
        files.insert(name.clone(), paths);
        db.add(name, deps, provides);
    }
    db.files = Files::Loaded(files);
    db.set_base(&BASE);
    Ok(db)
}

#[cfg(test)]
mod tests {
    use super::super::tests::scratch;
    use super::*;

    fn package(local: &Path, dir: &str, desc: &str, files: &str) {
        let d = local.join(dir);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("desc"), desc).unwrap();
        std::fs::write(d.join("files"), files).unwrap();
    }

    #[test]
    fn closure_resolves_provides_without_versions() {
        let local = scratch("pacman");
        std::fs::write(local.join("ALPM_DB_VERSION"), "9\n").unwrap();
        package(&local, "glibc-2.42-1", "%NAME%\nglibc\n\n%VERSION%\n2.42-1\n\n%PROVIDES%\nlibc.so=6-64\n\n", "%FILES%\nusr/\nusr/lib/\nusr/lib/libc.so.6\n\n");
        package(&local, "filesystem-2025.10.12-1", "%NAME%\nfilesystem\n\n", "%FILES%\nbin\nusr/\nusr/bin/\n\n%BACKUP%\netc/passwd\tabc\n\n");
        package(
            &local,
            "xterm-411-1",
            "%NAME%\nxterm\n\n%DEPENDS%\nglibc>=2.38\nlibxft\nlibc.so=6-64\n\n%OPTDEPENDS%\nluit: locale converter\n\n",
            "%FILES%\nusr/\nusr/bin/\nusr/bin/xterm\n\n",
        );
        package(&local, "libxft-2.3.9-1", "%NAME%\nlibxft\n\n%DEPENDS%\nlibx11\n\n", "%FILES%\nusr/lib/libXft.so.2\n");
        package(&local, "libx11-1.8.12-1", "%NAME%\nlibx11\n\n%PROVIDES%\nlibx11-xcb=1.8\nlibx11\n\n", "%FILES%\nusr/lib/libX11.so.6\n");
        package(&local, "luit-20240910-1", "%NAME%\nluit\n\n", "%FILES%\nusr/bin/luit\n");
        let db = open(&local).unwrap();
        assert_eq!(db.format, Format::Pacman);
        assert_eq!(db.closure(&["xterm".into()], false).into_iter().collect::<Vec<_>>(), vec!["glibc", "libx11", "libxft", "xterm"]);
        assert!(db.closure(&["libx11-xcb".into()], true).contains("filesystem"));
        assert_eq!(db.owner("/usr/bin/xterm").as_deref(), Some("xterm"));
        assert_eq!(db.owner("/bin").as_deref(), Some("filesystem"));
        let files = db.files(&["filesystem".to_string()].into_iter().collect());
        assert_eq!(files.into_iter().collect::<Vec<_>>(), vec!["/bin", "/usr", "/usr/bin"]);
        let _ = std::fs::remove_dir_all(&local);
    }
}
