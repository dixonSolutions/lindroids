//! Names and icons for windows.
//!
//! The CLI resolves the name and icon of what it launches (it can read .desktop files inside
//! distros and flatpaks, and render SVG) and registers them with the launched pid. A window then
//! takes the name and icon of the nearest registered ancestor of its client process. When nothing
//! was registered, the app id is looked up in the usual places on this side.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use base64::Engine;
use wayland_server::backend::ClientId;
use wayland_server::DisplayHandle;

struct Launch {
    name: String,
    icon: Option<String>,
}

static LAUNCHES: Mutex<Option<HashMap<u32, Launch>>> = Mutex::new(None);

pub fn register_launch(pid: u32, name: String, icon: Option<String>) {
    let mut g = LAUNCHES.lock().unwrap();
    let map = g.get_or_insert_with(HashMap::new);
    // Forget launches whose process is gone.
    map.retain(|p, _| std::path::Path::new(&format!("/proc/{p}")).exists());
    map.insert(pid, Launch { name, icon });
}

fn parent_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // pid (comm) state ppid ...  — comm may contain spaces and parentheses.
    let after = &stat[stat.rfind(')')? + 2..];
    after.split_whitespace().nth(1)?.parse().ok()
}

fn client_pid(dh: &DisplayHandle, client: &ClientId) -> Option<u32> {
    let c = dh.backend_handle().get_client_credentials(client.clone()).ok()?;
    Some(c.pid as u32)
}

fn with_launch<T>(dh: &DisplayHandle, client: &ClientId, f: impl Fn(&Launch) -> Option<T>) -> Option<T> {
    let mut pid = client_pid(dh, client)?;
    let g = LAUNCHES.lock().unwrap();
    let map = g.as_ref()?;
    for _ in 0..64 {
        if let Some(l) = map.get(&pid) {
            return f(l);
        }
        pid = parent_of(pid)?;
        if pid <= 1 {
            break;
        }
    }
    None
}

pub fn name_for_window(dh: &DisplayHandle, client: &ClientId) -> Option<String> {
    with_launch(dh, client, |l| Some(l.name.clone()))
}

pub fn icon_for_window(dh: &DisplayHandle, client: &ClientId, app_id: &str) -> Option<String> {
    with_launch(dh, client, |l| l.icon.clone()).or_else(|| icon_for_app_id(app_id))
}

fn data_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(h) = std::env::var_os("HOME") {
        v.push(PathBuf::from(h).join(".local/share"));
    }
    let dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_default();
    for d in dirs.split(':').filter(|d| !d.is_empty()) {
        v.push(PathBuf::from(d));
    }
    if let Some(p) = std::env::var_os("PREFIX") {
        v.push(PathBuf::from(p).join("share"));
    }
    v.push("/usr/share".into());
    v.push("/usr/local/share".into());
    v
}

/// PNG icon (base64) for a Wayland app id, from the .desktop file and hicolor theme on this side.
pub fn icon_for_app_id(app_id: &str) -> Option<String> {
    if app_id.is_empty() || app_id.contains('/') {
        return None;
    }
    let dirs = data_dirs();
    let mut icon = app_id.to_string();
    for d in &dirs {
        let desktop = d.join("applications").join(format!("{app_id}.desktop"));
        if let Ok(text) = std::fs::read_to_string(&desktop) {
            if let Some(line) = text.lines().find(|l| l.starts_with("Icon=")) {
                icon = line[5..].trim().to_string();
            }
            break;
        }
    }
    let path = if icon.starts_with('/') {
        Some(PathBuf::from(&icon)).filter(|p| p.extension().map_or(false, |e| e == "png"))
    } else {
        find_png(&dirs, &icon)
    };
    let bytes = std::fs::read(path?).ok()?;
    Some(base64::engine::general_purpose::STANDARD.encode(bytes))
}

fn find_png(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    for size in ["256x256", "192x192", "128x128", "96x96", "64x64", "48x48"] {
        for d in dirs {
            let p = d.join("icons/hicolor").join(size).join("apps").join(format!("{name}.png"));
            if p.exists() {
                return Some(p);
            }
        }
    }
    dirs.iter().map(|d| d.join("pixmaps").join(format!("{name}.png"))).find(|p| p.exists())
}
