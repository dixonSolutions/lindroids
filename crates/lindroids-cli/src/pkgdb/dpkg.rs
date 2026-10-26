//! dpkg: `status` holds every package's paragraph, `info/<name>[:arch].list` its files.

use std::collections::HashMap;
use std::path::Path;

use super::{Db, Files, Format};

fn strip(dep: &str) -> String {
    let d = dep.trim();
    let d = d.split_whitespace().next().unwrap_or("");
    d.split(':').next().unwrap_or("").to_string()
}

fn parse_deps(v: &str) -> Vec<Vec<String>> {
    v.split(',').map(|g| g.split('|').map(strip).filter(|s| !s.is_empty()).collect()).filter(|g: &Vec<String>| !g.is_empty()).collect()
}

pub(super) fn open(dpkg_dir: &Path) -> Result<Db, String> {
    let status = std::fs::read_to_string(dpkg_dir.join("status")).map_err(|e| format!("{}/status: {e}", dpkg_dir.display()))?;
    let info = dpkg_dir.join("info");
    let mut lists = HashMap::new();
    let mut db = Db::new(Format::Dpkg, Files::Lists { info: info.clone(), lists: HashMap::new() });
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
        let with_arch = format!("{name}:{arch}");
        let list = if info.join(format!("{with_arch}.list")).exists() { with_arch } else { name.clone() };
        lists.insert(name.clone(), list);
        if essential {
            db.essential.push(name.clone());
        }
        db.add(name, deps, provides);
    }
    db.files = Files::Lists { info, lists };
    Ok(db)
}

#[cfg(test)]
mod tests {
    use super::super::tests::scratch;
    use super::*;

    fn fake_db() -> std::path::PathBuf {
        let dir = scratch("dpkg");
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
        assert!(!db.installed("old"));
        assert_eq!(db.owner("/usr/bin/app").as_deref(), Some("app"));
        let files = db.files(&["app".to_string(), "libfoo".to_string()].into_iter().collect());
        assert!(files.contains("/usr/lib/libfoo.so.1"));
        assert!(!files.contains("/."));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
