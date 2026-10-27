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
    /// The page that shows the program's windows: tapping the notification brings it back.
    pub url: Option<String>,
}

/// What reaches Android: post (or replace) a notice, take one away.
pub trait Android: Send + Sync {
    fn notify(&self, n: Notice);
    fn cancel(&self, id: i32);
}

static ANDROID: OnceLock<Box<dyn Android>> = OnceLock::new();

/// Set once, by the CLI, when the server runs on Android and can reach its services.
pub fn set(a: Box<dyn Android>) {
    let _ = ANDROID.set(a);
}

pub fn get() -> Option<&'static dyn Android> {
    ANDROID.get().map(|b| b.as_ref())
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
