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
    /// It has a window of its own (a page for this run), not a place in the shared one.
    window: bool,
}

static LAUNCHES: Mutex<Option<HashMap<u32, Launch>>> = Mutex::new(None);

pub fn register_launch(pid: u32, name: String, icon: Option<String>, window: bool) {
    let mut g = LAUNCHES.lock().unwrap();
    let map = g.get_or_insert_with(HashMap::new);
    // Forget launches whose process is gone.
    map.retain(|p, _| std::path::Path::new(&format!("/proc/{p}")).exists());
    map.insert(pid, Launch { name, icon, window });
}

/// The name and icon `lindroids run` gave a launch.
pub fn launch_info(pid: u32) -> Option<(String, Option<String>)> {
    let g = LAUNCHES.lock().unwrap();
    g.as_ref()?.get(&pid).map(|l| (l.name.clone(), l.icon.clone()))
}

/// Whether this launch shows its windows in a window of its own.
pub fn wants_own_window(pid: u32) -> bool {
    let g = LAUNCHES.lock().unwrap();
    g.as_ref().and_then(|m| m.get(&pid)).map_or(false, |l| l.window)
}

/// The launch (its pid) a process descends from, if any.
pub fn launch_of_pid(pid: u32) -> Option<u32> {
    let g = LAUNCHES.lock().unwrap();
    let map = g.as_ref()?;
    let mut pid = pid;
    for _ in 0..64 {
        if map.contains_key(&pid) {
            return Some(pid);
        }
        pid = parent_of(pid)?;
        if pid <= 1 {
            break;
        }
    }
    None
}

/// The launch a Wayland client belongs to.
pub fn launch_for_window(dh: &DisplayHandle, client: &ClientId) -> Option<u32> {
    launch_of_pid(client_pid(dh, client)?)
}

/// `pid` and every process below it, children first. A program under proot is proot's tracee:
/// signalling proot alone (SIGKILL above all) leaves it running.
pub fn process_tree(pid: u32) -> Vec<u32> {
    let mut parent: HashMap<u32, u32> = HashMap::new();
    if let Ok(rd) = std::fs::read_dir("/proc") {
        for e in rd.flatten() {
            if let Some(p) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) {
                if let Some(pp) = parent_of(p) {
                    parent.insert(p, pp);
                }
            }
        }
    }
    let mut tree = vec![pid];
    let mut i = 0;
    while i < tree.len() {
        let p = tree[i];
        tree.extend(parent.iter().filter(|(_, pp)| **pp == p).map(|(c, _)| *c));
        i += 1;
    }
    tree.reverse();
    tree
}

/// The processes `lindroids run` and `launch` started that are still running.
pub fn launched_pids() -> Vec<u32> {
    let g = LAUNCHES.lock().unwrap();
    g.as_ref()
        .map(|m| m.keys().copied().filter(|p| std::path::Path::new(&format!("/proc/{p}")).exists()).collect())
        .unwrap_or_default()
}

fn parent_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // pid (comm) state ppid ...  — comm may contain spaces and parentheses.
    let after = &stat[stat.rfind(')')? + 2..];
    after.split_whitespace().nth(1)?.parse().ok()
}

pub fn client_pid(dh: &DisplayHandle, client: &ClientId) -> Option<u32> {
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

#[cfg(test)]
mod tests {
    #[test]
    fn process_trees_list_children_before_their_parent() {
        let mut child = std::process::Command::new("sh").args(["-c", "sleep 30 & sleep 30; wait"]).spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let tree = super::process_tree(child.id());
        let _ = child.kill();
        for p in &tree {
            unsafe { libc::kill(*p as i32, libc::SIGKILL) };
        }
        let _ = child.wait();
        assert!(tree.len() >= 3, "{tree:?}");
        assert_eq!(tree.last(), Some(&child.id()));
    }
}
