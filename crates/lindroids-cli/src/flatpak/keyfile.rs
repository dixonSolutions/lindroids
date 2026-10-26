//! Flatpak's `metadata` files are GLib key files: `[Group]` headers and `key=value` lines.
//! Runtimes write `key = value` (spaces around `=`), apps usually do not; both are read the same.

#[derive(Debug, Clone, Default, PartialEq)]
pub struct KeyFile {
    pub groups: Vec<Group>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Group {
    pub name: String,
    pub entries: Vec<(String, String)>,
}

impl Group {
    pub fn get(&self, key: &str) -> Option<&str> {
        // A repeated key: the last one wins, as in GLib.
        self.entries.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// A `;`-separated list (`shared=ipc;network;`), without empty items.
    pub fn list(&self, key: &str) -> Vec<String> {
        self.get(key).map(split_list).unwrap_or_default()
    }

    pub fn bool(&self, key: &str) -> bool {
        matches!(self.get(key), Some("true") | Some("1"))
    }
}

pub fn split_list(v: &str) -> Vec<String> {
    v.split(';').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

impl KeyFile {
    pub fn parse(text: &str) -> KeyFile {
        let mut kf = KeyFile::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                kf.groups.push(Group { name: name.to_string(), entries: Vec::new() });
                continue;
            }
            let (Some(g), Some((k, v))) = (kf.groups.last_mut(), line.split_once('=')) else { continue };
            let k = k.trim();
            // Translations (`Name[de]=`) are not needed for running anything.
            if k.is_empty() || k.contains('[') {
                continue;
            }
            g.entries.push((k.to_string(), unescape(v.trim())));
        }
        kf
    }

    pub fn read(path: &std::path::Path) -> Option<KeyFile> {
        std::fs::read_to_string(path).ok().map(|t| KeyFile::parse(&t))
    }

    pub fn group(&self, name: &str) -> Option<&Group> {
        self.groups.iter().find(|g| g.name == name)
    }

    pub fn get(&self, group: &str, key: &str) -> Option<&str> {
        self.group(group)?.get(key)
    }

    pub fn list(&self, group: &str, key: &str) -> Vec<String> {
        self.group(group).map(|g| g.list(key)).unwrap_or_default()
    }

    /// `[Extension <name>]` groups, in file order.
    pub fn extension_points(&self) -> impl Iterator<Item = (&str, &Group)> {
        self.groups.iter().filter_map(|g| g.name.strip_prefix("Extension ").map(|n| (n.trim(), g)))
    }

    /// The entries of `[Environment]`, in file order.
    pub fn environment(&self) -> Vec<(String, String)> {
        self.group("Environment").map(|g| g.entries.clone()).unwrap_or_default()
    }
}

/// GLib key file escapes: `\s`, `\n`, `\t`, `\r`, `\\`.
fn unescape(v: &str) -> String {
    if !v.contains('\\') {
        return v.to_string();
    }
    let mut out = String::with_capacity(v.len());
    let mut it = v.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(o) => {
                out.push('\\');
                out.push(o);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP: &str = "\
[Application]
name=org.gnome.Calculator
runtime=org.gnome.Platform/x86_64/51
sdk=org.gnome.Sdk/x86_64/51
command=gnome-calculator

[Context]
shared=ipc;network;
sockets=fallback-x11;wayland;
filesystems=xdg-download;/media:ro;

# a comment
[Extension org.gnome.Calculator.Locale]
directory=share/runtime/locale
locale-subset=true
";

    #[test]
    fn reads_app_metadata() {
        let kf = KeyFile::parse(APP);
        assert_eq!(kf.get("Application", "command"), Some("gnome-calculator"));
        assert_eq!(kf.get("Application", "runtime"), Some("org.gnome.Platform/x86_64/51"));
        assert_eq!(kf.list("Context", "shared"), vec!["ipc", "network"]);
        assert_eq!(kf.list("Context", "filesystems"), vec!["xdg-download", "/media:ro"]);
        let exts: Vec<&str> = kf.extension_points().map(|(n, _)| n).collect();
        assert_eq!(exts, vec!["org.gnome.Calculator.Locale"]);
        assert!(kf.group("Extension org.gnome.Calculator.Locale").unwrap().bool("locale-subset"));
    }

    #[test]
    fn reads_runtime_style_spacing_and_escapes() {
        let kf = KeyFile::parse("[Environment]\nXDG_DATA_DIRS = /app/share:/usr/share\nGREETING=a\\sb\nName[de]=x\n[Runtime]\nname = org.freedesktop.Platform\n");
        assert_eq!(kf.environment(), vec![("XDG_DATA_DIRS".into(), "/app/share:/usr/share".into()), ("GREETING".into(), "a b".into())]);
        assert_eq!(kf.get("Runtime", "name"), Some("org.freedesktop.Platform"));
    }

    #[test]
    fn last_key_wins_and_junk_is_ignored() {
        let kf = KeyFile::parse("orphan=1\n[A]\nk=1\nk=2\nnot a pair\n");
        assert_eq!(kf.get("A", "k"), Some("2"));
        assert_eq!(kf.groups.len(), 1);
    }
}
