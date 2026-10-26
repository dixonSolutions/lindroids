//! Names and icons: .desktop entries and icon themes, under any root (Termux's prefix, a proot
//! distro's rootfs, a flatpak's export dir). Icons come out as PNG, SVGs rendered with resvg.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    pub exec: Option<String>,
    pub path: PathBuf,
}

/// `share` directories to search, outermost first, for programs seen through `root`
/// ("/" for the current system).
pub fn data_dirs(root: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    let j = |p: &str| root.join(p.trim_start_matches('/'));
    if let Some(home) = std::env::var_os("HOME") {
        if root == Path::new("/") {
            v.push(PathBuf::from(home).join(".local/share"));
            v.push(PathBuf::from(std::env::var_os("HOME").unwrap()).join(".local/share/flatpak/exports/share"));
        }
    }
    if let Ok(prefix) = std::env::var("PREFIX") {
        if root == Path::new("/") {
            v.push(PathBuf::from(prefix).join("share"));
        }
    }
    for d in ["/usr/local/share", "/usr/share", "/var/lib/flatpak/exports/share", "/root/.local/share/flatpak/exports/share"] {
        v.push(j(d));
    }
    v.retain(|p| p.is_dir());
    v.dedup();
    v
}

pub fn parse(path: &Path) -> Option<Entry> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut e = Entry { path: path.to_path_buf(), ..Default::default() };
    e.id = path.file_stem()?.to_string_lossy().into_owned();
    let mut in_main = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_main = line == "[Desktop Entry]";
            continue;
        }
        if !in_main {
            continue;
        }
        if let Some(v) = line.strip_prefix("Name=") {
            e.name = v.to_string();
        } else if let Some(v) = line.strip_prefix("Icon=") {
            e.icon = Some(v.to_string());
        } else if let Some(v) = line.strip_prefix("Exec=") {
            e.exec = Some(v.to_string());
        }
    }
    (!e.name.is_empty()).then_some(e)
}

/// The desktop entry for a program: by desktop id, or by the command it runs.
pub fn find(dirs: &[PathBuf], program: &str) -> Option<Entry> {
    let base = Path::new(program).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    // Exact desktop id first (org.gnome.Calculator, foot).
    for d in dirs {
        let p = d.join("applications").join(format!("{base}.desktop"));
        if let Some(e) = parse(&p) {
            return Some(e);
        }
    }
    // Then the first entry whose Exec runs this program.
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(d.join("applications")) else { continue };
        let mut paths: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        paths.sort();
        for p in paths {
            if p.extension().map_or(true, |x| x != "desktop") {
                continue;
            }
            if let Some(e) = parse(&p) {
                let exec0 = e.exec.as_deref().unwrap_or("").split_whitespace().find(|w| !w.contains('=')).unwrap_or("");
                let exec0 = Path::new(exec0).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                if exec0 == base {
                    return Some(e);
                }
            }
        }
    }
    None
}

/// Find an icon file by theme name (or absolute path), preferring large PNGs, then SVG.
pub fn icon_file(dirs: &[PathBuf], root: &Path, icon: &str) -> Option<PathBuf> {
    if icon.starts_with('/') {
        let p = root.join(icon.trim_start_matches('/'));
        return p.exists().then_some(p);
    }
    let sizes = ["512x512", "256x256", "192x192", "128x128", "96x96", "64x64", "48x48", "32x32"];
    for d in dirs {
        for theme in ["hicolor", "Adwaita", "breeze"] {
            let p = d.join("icons").join(theme).join("scalable/apps").join(format!("{icon}.svg"));
            if p.exists() && theme == "hicolor" {
                // Scalable is the best quality, but keep looking for a large PNG first.
                let png = sizes.iter().take(3).map(|s| d.join("icons").join(theme).join(s).join("apps").join(format!("{icon}.png"))).find(|p| p.exists());
                return Some(png.unwrap_or(p));
            }
            for s in sizes {
                let p = d.join("icons").join(theme).join(s).join("apps").join(format!("{icon}.png"));
                if p.exists() {
                    return Some(p);
                }
            }
        }
        for ext in ["png", "svg"] {
            let p = d.join("pixmaps").join(format!("{icon}.{ext}"));
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

/// PNG bytes for an icon file, at most `size` pixels square (SVG rendered at `size`).
pub fn icon_png(path: &Path, size: u32) -> Option<Vec<u8>> {
    let data = std::fs::read(path).ok()?;
    let is_svg = path.extension().map_or(false, |e| e == "svg" || e == "svgz") || data.starts_with(b"<?xml") || data.starts_with(b"<svg");
    if is_svg {
        return render_svg(&data, size);
    }
    if data.starts_with(b"\x89PNG") {
        return Some(data);
    }
    None
}

pub fn render_svg(data: &[u8], size: u32) -> Option<Vec<u8>> {
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(data, &opt).ok()?;
    let mut pix = resvg::tiny_skia::Pixmap::new(size, size)?;
    let s = tree.size();
    let k = (size as f32 / s.width()).min(size as f32 / s.height());
    let tx = (size as f32 - s.width() * k) / 2.0;
    let ty = (size as f32 - s.height() * k) / 2.0;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_row(k, 0.0, 0.0, k, tx, ty), &mut pix.as_mut());
    pix.encode_png().ok()
}

/// A generic icon for programs without one: the Lindroids window, tinted by the name.
pub fn fallback_icon(name: &str, size: u32) -> Vec<u8> {
    let palette = ["#3DDC84", "#4F8EF7", "#F5A524", "#E5484D", "#8E4EC6", "#12A594", "#E54666", "#0090FF"];
    let h = name.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32);
    let color = palette[(h as usize) % palette.len()];
    let letter = name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_ascii_uppercase()).unwrap_or('?');
    let _ = letter;
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="512" height="512" viewBox="18 18 72 72">
  <rect x="18" y="18" width="72" height="72" rx="16" fill="#0F1720"/>
  <path fill="#E8EEF4" d="M30,34h48a4,4 0,0 1,4 4v32a4,4 0,0 1,-4 4h-48a4,4 0,0 1,-4 -4v-32a4,4 0,0 1,4 -4z"/>
  <path fill="#1B2430" d="M30,42h48v28h-48z"/>
  <circle cx="33" cy="38" r="1.6" fill="{color}"/>
  <path fill="{color}" d="M36,49l9,7l-9,7l-2.4,-3l5.4,-4l-5.4,-4z"/>
  <path fill="{color}" d="M48,61h14v4h-14z"/>
</svg>"##
    );
    render_svg(svg.as_bytes(), size).unwrap_or_default()
}

/// Name and PNG icon for a program run through `root`.
pub fn identify(root: &Path, program: &str, name_override: Option<&str>, icon_override: Option<&Path>) -> (String, Vec<u8>, Option<Entry>) {
    let dirs = data_dirs(root);
    let entry = find(&dirs, program);
    let name = name_override
        .map(String::from)
        .or_else(|| entry.as_ref().map(|e| e.name.clone()))
        .unwrap_or_else(|| {
            let b = Path::new(program).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| program.into());
            let mut c = b.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or(b)
        });
    let icon = icon_override
        .and_then(|p| icon_png(p, 192))
        .or_else(|| {
            let e = entry.as_ref()?;
            let file = icon_file(&dirs, root, e.icon.as_deref()?)?;
            icon_png(&file, 192)
        })
        .unwrap_or_else(|| fallback_icon(&name, 192));
    (name, icon, entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_is_a_png() {
        let png = fallback_icon("xeyes", 96);
        assert!(png.starts_with(b"\x89PNG"));
    }

    #[test]
    fn finds_entry_by_exec() {
        let dir = std::env::temp_dir().join(format!("lindroids-desktop-{}", std::process::id()));
        let apps = dir.join("share/applications");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::write(apps.join("org.example.Foo.desktop"), "[Desktop Entry]\nName=Foo Editor\nExec=/usr/bin/foo %F\nIcon=foo\n").unwrap();
        let e = find(&[dir.join("share")], "foo").unwrap();
        assert_eq!(e.name, "Foo Editor");
        assert_eq!(e.id, "org.example.Foo");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
