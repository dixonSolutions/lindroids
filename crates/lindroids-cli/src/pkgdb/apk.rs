//! apk (Alpine): `lib/apk/db/installed` holds every installed package as a paragraph of one-letter
//! fields, its files included: `F:` a directory, the `R:` lines after it the files in it.
//! Dependencies are mostly virtual (`so:libfoo.so.1`, `cmd:foo`, `pc:foo`), resolved through the
//! other packages' `p:` provides.

use std::collections::HashMap;
use std::path::Path;

use super::{unversioned, Db, Files, Format};

/// What every Alpine root has: the directory layout, /bin/sh and its applets, the C library.
const BASE: [&str; 5] = ["alpine-baselayout", "alpine-baselayout-data", "busybox", "busybox-binsh", "musl"];

/// A dependency as a name to resolve: `!foo` (a conflict) is none, `foo@testing` (a pinned
/// repository) is `foo`.
fn dep_name(token: &str) -> Option<String> {
    if token.starts_with('!') {
        return None;
    }
    let name = unversioned(token).split('@').next().unwrap_or("");
    (!name.is_empty()).then(|| name.to_string())
}

pub(super) fn open(installed: &Path) -> Result<Db, String> {
    let text = std::fs::read_to_string(installed).map_err(|e| format!("{}: {e}", installed.display()))?;
    let mut db = Db::new(Format::Apk, Files::Loaded(HashMap::new()));
    let mut files = HashMap::new();
    for para in text.split("\n\n") {
        let mut name = String::new();
        let mut deps = Vec::new();
        let mut provides = Vec::new();
        let mut install_if = Vec::new();
        let mut paths = Vec::new();
        let mut dir = String::new();
        for line in para.lines() {
            let Some((k, v)) = line.split_once(':') else { continue };
            match k {
                "P" => name = v.to_string(),
                "D" => deps.extend(v.split_whitespace().filter_map(dep_name).map(|d| vec![d])),
                "i" => {
                    install_if = v.split_whitespace().map(|c| (c.starts_with('!'), unversioned(c.trim_start_matches('!')).to_string())).collect();
                }
                "p" => provides.extend(v.split_whitespace().map(|p| unversioned(p).to_string()).filter(|p| !p.is_empty())),
                "F" => {
                    dir = format!("/{}", v.trim_matches('/'));
                    paths.push(dir.clone());
                }
                "R" => paths.push(format!("{}/{v}", dir.trim_end_matches('/'))),
                _ => {}
            }
        }
        if name.is_empty() {
            continue;
        }
        files.insert(name.clone(), paths);
        if !install_if.is_empty() {
            db.install_if.push((name.clone(), install_if));
        }
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

    const INSTALLED: &str = "\
C:Q1abc=
P:musl
V:1.2.5-r10
A:x86_64
F:lib
R:ld-musl-x86_64.so.1
a:0:0:755
Z:Q1x=
R:libc.musl-x86_64.so.1
p:so:libc.musl-x86_64.so.1=1

P:busybox
V:1.37.0-r31
D:so:libc.musl-x86_64.so.1
p:cmd:busybox=1.37.0-r31 cmd:sh=1.37.0-r31
F:bin
R:busybox

P:libxcb
V:1.17.0-r0
D:so:libc.musl-x86_64.so.1 so:libXau.so.6
p:so:libxcb.so.1=1.1.0 pc:xcb=1.17.0
F:usr/lib
R:libxcb.so.1
R:libxcb.so.1.1.0

P:libxau
V:1.0.12-r0
D:so:libc.musl-x86_64.so.1
p:so:libXau.so.6=6.0.0
F:usr/lib
R:libXau.so.6

P:xterm
V:410-r0
D:ncurses-terminfo-base>=6 !xterm-old so:libc.musl-x86_64.so.1 so:libxcb.so.1 pc:xcb cmd:busybox@main
p:cmd:xterm=410-r0
F:usr
F:usr/bin
R:xterm
R:uxterm
F:usr/share/applications
R:xterm.desktop

P:ncurses-terminfo-base
V:6.6-r0
F:etc/terminfo/x
R:xterm

P:libglycin
V:2.1.5-r0
D:so:libc.musl-x86_64.so.1
p:so:libglycin-2.so.0=0
F:usr/lib
R:libglycin-2.so.0

P:glycin-svg
V:2.1.5-r0
D:so:libc.musl-x86_64.so.1
i:libglycin~2.1.5
F:usr/libexec/glycin-loaders/2+
R:glycin-svg

P:gdk-pixbuf
V:2.44.7-r0
D:so:libglycin-2.so.0
F:usr/lib
R:libgdk_pixbuf-2.0.so.0

P:gdk-pixbuf-lang
V:2.44.7-r0
i:gdk-pixbuf=2.44.7-r0 lang
F:usr/share/locale

P:xterm-doc
V:410-r0
i:docs xterm=410-r0 !busybox
";

    #[test]
    fn closure_resolves_so_cmd_and_pc_provides() {
        let dir = scratch("apk");
        std::fs::write(dir.join("installed"), INSTALLED).unwrap();
        let db = open(&dir.join("installed")).unwrap();
        assert_eq!(db.format, Format::Apk);
        let c = db.closure(&["xterm".into()], false);
        assert_eq!(c.into_iter().collect::<Vec<_>>(), vec!["busybox", "libxau", "libxcb", "musl", "ncurses-terminfo-base", "xterm"]);
        // The base packages that are installed; the others are left out quietly.
        let with_base = db.closure(&["libxau".into()], true);
        assert!(with_base.contains("busybox") && with_base.contains("musl") && !with_base.contains("xterm"));
        // install_if: the loader comes with libglycin; the lang pack wants `lang`, which is not
        // installed; the doc package wants `docs` and no busybox.
        let pixbuf = db.closure(&["gdk-pixbuf".into()], false);
        assert_eq!(pixbuf.into_iter().collect::<Vec<_>>(), vec!["gdk-pixbuf", "glycin-svg", "libglycin", "musl"]);
        assert!(!db.closure(&["xterm".into()], false).contains("xterm-doc"));
        assert_eq!(db.owner("/usr/bin/xterm").as_deref(), Some("xterm"));
        assert_eq!(db.owner("/etc/terminfo/x/xterm").as_deref(), Some("ncurses-terminfo-base"));
        assert_eq!(db.owner("/lib/libc.musl-x86_64.so.1").as_deref(), Some("musl"));
        let files = db.files(&["xterm".to_string()].into_iter().collect());
        let want = ["/usr", "/usr/bin", "/usr/bin/uxterm", "/usr/bin/xterm", "/usr/share/applications", "/usr/share/applications/xterm.desktop"];
        assert_eq!(files.iter().map(String::as_str).collect::<Vec<_>>(), want);
        assert!(db.all_owned().contains("/usr/lib/libxcb.so.1.1.0"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
