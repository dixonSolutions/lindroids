//! Android's own services, where the server is on Android: the CLI hands it a way to reach them
//! (the app_process helper, as the terminal app), and the server uses it for what a page cannot
//! do, such as notifications that show when no window is on screen.

use std::path::PathBuf;
use std::sync::OnceLock;

/// A notification for Android's shade.
#[derive(Debug, Clone, Default)]
pub struct Notice {
    /// Android's notification id: one per program notification, kept when it is replaced.
    pub id: i32,
    /// The program's name.
    pub app: String,
    pub title: String,
    pub body: String,
    /// A PNG of the program's icon, if there is one.
    pub icon: Option<PathBuf>,
    /// Its one-colour icon, for the status bar (else the icon's shape is used).
    pub small_icon: Option<PathBuf>,
    /// The page that shows the program's windows: tapping the notification brings it back.
    pub url: Option<String>,
}

/// What reaches Android: post (or replace) a notice, the quiet one of a running program, take one
/// away.
pub trait Android: Send + Sync {
    fn notify(&self, n: Notice);
    fn running(&self, n: Notice);
    fn cancel(&self, id: i32);
}

static ANDROID: OnceLock<Box<dyn Android>> = OnceLock::new();

/// Runs' one-colour icons (PNG, base64), by the run's pid.
static SYMBOLIC: std::sync::Mutex<Vec<(u32, String)>> = std::sync::Mutex::new(Vec::new());

/// The run `run`'s one-colour icon, from `lindroids run`.
pub fn set_symbolic(run: u32, png_b64: Option<String>) {
    let mut v = SYMBOLIC.lock().unwrap();
    v.retain(|(r, _)| *r != run && std::path::Path::new(&format!("/proc/{r}")).exists());
    if let Some(b) = png_b64 {
        v.push((run, b));
    }
}

/// The one-colour icon of the run that process `pid` belongs to, as a file for the helper.
pub fn symbolic_of(pid: u32) -> Option<PathBuf> {
    let run = crate::icons::launch_of_pid(pid)?;
    let b = SYMBOLIC.lock().unwrap().iter().find(|(r, _)| *r == run).map(|(_, b)| b.clone())?;
    icon_file(&b)
}

/// Set once, by the CLI, when the server runs on Android and can reach its services. Takes away
/// the running programs' notifications a server before this one left (it was stopped or killed).
pub fn set(a: Box<dyn Android>) {
    if ANDROID.set(a).is_ok() {
        let _ = clear_running();
    }
}

/// The running programs' notifications that are up, one id a line: they go with the server.
fn running_file() -> PathBuf {
    crate::state_dir().join("running-notifications")
}

fn note_running(id: i32, up: bool) {
    let p = running_file();
    let mut ids: Vec<String> = std::fs::read_to_string(&p).unwrap_or_default().lines().map(str::to_string).collect();
    ids.retain(|l| l != &id.to_string());
    if up {
        ids.push(id.to_string());
    }
    let _ = std::fs::write(&p, ids.join("\n"));
}

/// Takes away every running program's notification (the server starts or stops). Returns how
/// many it asked Android to take away.
pub fn clear_running() -> usize {
    let Some(a) = get() else { return 0 };
    let ids: Vec<i32> = std::fs::read_to_string(running_file()).unwrap_or_default().lines().filter_map(|l| l.parse().ok()).collect();
    for &id in &ids {
        a.cancel(id);
    }
    let _ = std::fs::remove_file(running_file());
    ids.len()
}

pub fn get() -> Option<&'static dyn Android> {
    ANDROID.get().map(|b| b.as_ref())
}

/// While the run `run` (the pid of `lindroids run`/`launch`) lives, a quiet notification stands
/// for it: its icon in the status bar, its name, a tap brings back its window. Posted once the
/// window's URL is settled (its ticket comes with the window), taken away when the run ends.
pub fn run_started(run: u32) {
    let Some(a) = get() else { return };
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(3));
        let Some((url, icon, name)) = crate::run_of_pid(run) else { return };
        let id = note_id(&format!("run-{run}"));
        note_running(id, true);
        a.running(Notice {
            id,
            app: name.clone(),
            title: name,
            body: "Running. Tap to show its window.".into(),
            icon: icon.as_deref().and_then(icon_file),
            small_icon: symbolic_of(run),
            url: Some(url),
        });
        let proc = PathBuf::from(format!("/proc/{run}"));
        while proc.exists() {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        a.cancel(id);
        note_running(id, false);
    });
}

/// A notification id from a key ("fdo-3", "portal-:1.42-x"): the same key, the same id.
pub fn note_id(key: &str) -> i32 {
    // FNV-1a, folded to a positive int; ids are only compared with each other.
    let mut h: u32 = 0x811c_9dc5;
    for b in key.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    (h & 0x7fff_ffff) as i32
}

/// A PNG (base64) kept as a file for the helper, named by its content.
pub fn icon_file(png_b64: &str) -> Option<PathBuf> {
    use base64::Engine;
    let data = base64::engine::general_purpose::STANDARD.decode(png_b64).ok()?;
    let dir = crate::state_dir().join("notify-icons");
    std::fs::create_dir_all(&dir).ok()?;
    let p = dir.join(format!("{:08x}.png", note_id(png_b64)));
    if !p.exists() {
        std::fs::write(&p, data).ok()?;
    }
    Some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_stable_and_positive() {
        assert_eq!(note_id("fdo-1"), note_id("fdo-1"));
        assert_ne!(note_id("fdo-1"), note_id("fdo-2"));
        assert!(note_id("portal-:1.42-x") >= 0);
    }
}
