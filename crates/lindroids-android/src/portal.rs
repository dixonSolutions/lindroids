//! Android integration for Linux programs, the parts that do not need Android: what to send and
//! when, how Android's ids and paths map to the protocol's. The JNI wiring is in `android`; the
//! Linux side of the same features is the server's portal (xdg-desktop-portal and
//! org.freedesktop.Notifications).
//!
//! Kept free of JNI so `cargo test` on the host covers it.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::time::{Duration, SystemTime};

use lindroids_proto::SystemState;

/// Changes to the system state are collected for this long before one message goes out: a
/// configuration change, a battery tick and a network callback often arrive together.
pub const SYSTEM_DEBOUNCE: Duration = Duration::from_millis(300);

/// A memory reading alone is worth a message only when the available memory moved by more than
/// this share of the total since the last message; it changes all the time.
const MEMORY_STEP: f64 = 0.05;

/// Whether `new` is worth sending to a link that last got `last`.
///
/// Everything but memory counts on any change. Memory counts when the low flag flips or the
/// available memory moved by more than [`MEMORY_STEP`] of the total since `last` (not since the
/// last reading, so slow drift is still reported once it adds up).
pub fn worth_sending(last: Option<&SystemState>, new: &SystemState) -> bool {
    let Some(last) = last else { return true };
    let strip = |s: &SystemState| SystemState { memory: Default::default(), ..s.clone() };
    if strip(last) != strip(new) {
        return true;
    }
    let (a, b) = (&last.memory, &new.memory);
    if a.low != b.low || a.total != b.total {
        return true;
    }
    if b.total == 0 {
        return a.available != b.available;
    }
    (a.available.abs_diff(b.available) as f64) > MEMORY_STEP * b.total as f64
}

/// An Android colour int (ARGB) as the protocol's sRGB triple in 0..1.
pub fn accent_from_argb(argb: i32) -> [f32; 3] {
    let c = argb as u32;
    let ch = |shift: u32| ((c >> shift) & 0xff) as f32 / 255.0;
    [ch(16), ch(8), ch(0)]
}

/// The NetworkMonitor portal's connectivity from what Android says about the default network:
/// 4 full (validated internet), 3 captive portal, 2 limited (internet, not validated), 1 local
/// only or no network at all.
pub fn connectivity(available: bool, internet: bool, validated: bool, captive: bool) -> u8 {
    if !available || !internet {
        1
    } else if validated {
        4
    } else if captive {
        3
    } else {
        2
    }
}

/// `ComponentCallbacks2.onTrimMemory` levels as the MemoryMonitor portal's: RUNNING_MODERATE 50,
/// RUNNING_LOW 100, RUNNING_CRITICAL 255; the app's UI hidden or the process in the background
/// (UI_HIDDEN, BACKGROUND, MODERATE, COMPLETE, and anything Android adds above them) 50.
pub fn trim_level(android: i32) -> Option<u8> {
    match android {
        5 => Some(50),
        10 => Some(100),
        15 => Some(255),
        l if l >= 20 => Some(50),
        _ => None,
    }
}

/// Notification channels, created by `App.java` with these ids.
pub const CHANNEL_LINUX: &str = "linux";
pub const CHANNEL_URGENT: &str = "linux_urgent";

/// Urgent notifications (2) get the channel that may pop up over the screen.
pub fn channel_for(urgency: u8) -> &'static str {
    if urgency >= 2 {
        CHANNEL_URGENT
    } else {
        CHANNEL_LINUX
    }
}

/// Android notification ids for the servers' string ids. A (link, id) keeps its int for as long
/// as the notification lives, so showing the same id again replaces it. Ints start high and are
/// posted under their own tag, so they never meet the app's own notifications.
#[derive(Default)]
pub struct NoteIds {
    next: i32,
    by_key: HashMap<(i32, String), i32>,
    by_note: HashMap<i32, (i32, String)>,
}

impl NoteIds {
    const FIRST: i32 = 1000;

    pub fn assign(&mut self, link: i32, id: &str) -> i32 {
        if let Some(&n) = self.by_key.get(&(link, id.to_string())) {
            return n;
        }
        if self.next < Self::FIRST {
            self.next = Self::FIRST;
        }
        let n = self.next;
        self.next = self.next.checked_add(1).unwrap_or(Self::FIRST);
        self.by_key.insert((link, id.to_string()), n);
        self.by_note.insert(n, (link, id.to_string()));
        n
    }

    pub fn get(&self, link: i32, id: &str) -> Option<i32> {
        self.by_key.get(&(link, id.to_string())).copied()
    }

    /// Which link and id an Android notification belongs to.
    pub fn owner(&self, note: i32) -> Option<(i32, String)> {
        self.by_note.get(&note).cloned()
    }

    pub fn remove(&mut self, note: i32) -> Option<(i32, String)> {
        let k = self.by_note.remove(&note)?;
        self.by_key.remove(&k);
        Some(k)
    }

    /// Forget a link's notifications; returns their Android ids, to cancel them.
    pub fn remove_link(&mut self, link: i32) -> Vec<i32> {
        let notes: Vec<i32> = self.by_note.iter().filter(|(_, (l, _))| *l == link).map(|(n, _)| *n).collect();
        for n in &notes {
            self.remove(*n);
        }
        notes
    }
}

/// ExternalStorageProvider: documents in shared storage, ids "primary:<relative path>".
pub const EXTERNAL_STORAGE_AUTHORITY: &str = "com.android.externalstorage.documents";
/// DownloadsProvider: ids are numbers, "msf:<n>", or "raw:<absolute path>".
pub const DOWNLOADS_AUTHORITY: &str = "com.android.providers.downloads.documents";

/// Where a picked document is in primary shared storage as a path, when its provider says.
/// `primary_root` is Environment.getExternalStorageDirectory() ("/storage/emulated/0").
///
/// Other volumes (SD cards, USB) are left out: the Linux side usually cannot reach them, and it
/// can always ask for the content instead (FetchFile).
pub fn document_path(authority: &str, doc_id: &str, primary_root: &str) -> Option<String> {
    let root = primary_root.trim_end_matches('/');
    match authority {
        EXTERNAL_STORAGE_AUTHORITY => {
            let (vol, rel) = doc_id.split_once(':')?;
            if vol != "primary" || rel.split('/').any(|c| c == "..") {
                return None;
            }
            let rel = rel.trim_matches('/');
            Some(if rel.is_empty() { root.to_string() } else { format!("{root}/{rel}") })
        }
        DOWNLOADS_AUTHORITY => {
            let p = doc_id.strip_prefix("raw:")?;
            (p.starts_with("/storage/") && !p.split('/').any(|c| c == "..")).then(|| p.trim_end_matches('/').to_string())
        }
        _ => None,
    }
}

/// A file name from the Linux side, safe to create in the app's cache: no directories, no
/// "." or "..", no NUL, not absurdly long.
pub fn safe_file_name(name: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or("").replace('\0', "");
    let base = base.trim();
    if base.is_empty() || base == "." || base == ".." {
        return "file".into();
    }
    let mut end = base.len().min(200);
    while !base.is_char_boundary(end) {
        end -= 1;
    }
    base[..end].to_string()
}

/// Remove the entries of `dir` not modified within `max_age` (files shared with other apps by
/// OpenFile; by then the app that opened them has long read them).
pub fn clean_old(dir: &Path, max_age: Duration) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let now = SystemTime::now();
    for e in entries.flatten() {
        let old = e.metadata().and_then(|m| m.modified()).map(|t| now.duration_since(t).unwrap_or_default() > max_age).unwrap_or(false);
        if old {
            let p = e.path();
            let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
        }
    }
}

/// Size of the FileChunk payloads a fetched document is sent in.
pub const CHUNK: usize = 256 * 1024;

/// Read `r` to the end in chunks of `chunk` bytes, handing each to `emit` with whether it is the
/// last one (an empty document is one empty last chunk). `emit` returns false to stop early (the
/// link went away).
pub fn stream_chunks(mut r: impl Read, chunk: usize, mut emit: impl FnMut(&[u8], bool) -> bool) -> std::io::Result<()> {
    fn fill(r: &mut impl Read, cap: usize) -> std::io::Result<Vec<u8>> {
        let mut buf = vec![0u8; cap];
        let mut n = 0;
        while n < cap {
            match r.read(&mut buf[n..]) {
                Ok(0) => break,
                Ok(k) => n += k,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        buf.truncate(n);
        Ok(buf)
    }
    let mut cur = fill(&mut r, chunk)?;
    loop {
        // A full chunk may or may not be the end: read ahead to know whether it is the last.
        let next = if cur.len() == chunk { fill(&mut r, chunk)? } else { Vec::new() };
        let last = next.is_empty();
        if !emit(&cur, last) || last {
            return Ok(());
        }
        cur = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lindroids_proto::{BatteryState, MemoryState};

    fn state() -> SystemState {
        SystemState {
            dark: true,
            font_scale: 1.0,
            memory: MemoryState { total: 8_000, available: 4_000, low: false },
            battery: Some(BatteryState { level: 80, charging: false }),
            locale: "en-AU".into(),
            timezone: "Australia/Sydney".into(),
            ..Default::default()
        }
    }

    #[test]
    fn first_state_is_always_sent() {
        assert!(worth_sending(None, &state()));
    }

    #[test]
    fn identical_state_is_not_sent_again() {
        assert!(!worth_sending(Some(&state()), &state()));
    }

    #[test]
    fn any_other_change_is_sent() {
        let mut s = state();
        s.dark = false;
        assert!(worth_sending(Some(&state()), &s));
        let mut s = state();
        s.battery = Some(BatteryState { level: 79, charging: false });
        assert!(worth_sending(Some(&state()), &s));
    }

    #[test]
    fn memory_only_counts_past_five_percent() {
        let mut s = state();
        s.memory.available = 4_000 + 400; // exactly 5 %
        assert!(!worth_sending(Some(&state()), &s));
        s.memory.available = 4_000 - 401;
        assert!(worth_sending(Some(&state()), &s));
        let mut s = state();
        s.memory.low = true;
        assert!(worth_sending(Some(&state()), &s));
    }

    #[test]
    fn small_memory_change_rides_along_with_a_real_one() {
        let mut s = state();
        s.memory.available += 10;
        s.power_saver = true;
        assert!(worth_sending(Some(&state()), &s));
    }

    #[test]
    fn accent_colour_from_argb() {
        assert_eq!(accent_from_argb(0xff_ff_80_00u32 as i32), [1.0, 128.0 / 255.0, 0.0]);
        assert_eq!(accent_from_argb(0x00_00_00_ff), [0.0, 0.0, 1.0]);
    }

    #[test]
    fn connectivity_levels() {
        assert_eq!(connectivity(false, false, false, false), 1);
        assert_eq!(connectivity(true, false, false, false), 1);
        assert_eq!(connectivity(true, true, false, false), 2);
        assert_eq!(connectivity(true, true, false, true), 3);
        assert_eq!(connectivity(true, true, true, false), 4);
    }

    #[test]
    fn trim_levels() {
        assert_eq!(trim_level(5), Some(50));
        assert_eq!(trim_level(10), Some(100));
        assert_eq!(trim_level(15), Some(255));
        for l in [20, 40, 60, 80] {
            assert_eq!(trim_level(l), Some(50));
        }
        assert_eq!(trim_level(0), None);
    }

    #[test]
    fn urgent_notifications_get_their_own_channel() {
        assert_eq!(channel_for(0), CHANNEL_LINUX);
        assert_eq!(channel_for(1), CHANNEL_LINUX);
        assert_eq!(channel_for(2), CHANNEL_URGENT);
    }

    #[test]
    fn notification_ids_are_stable_per_link_and_id() {
        let mut ids = NoteIds::default();
        let a = ids.assign(1, "mail");
        assert_eq!(ids.assign(1, "mail"), a);
        let b = ids.assign(2, "mail");
        let c = ids.assign(1, "chat");
        assert!(a != b && a != c && b != c);
        assert!(a >= 1000);
        assert_eq!(ids.owner(b), Some((2, "mail".into())));
        assert_eq!(ids.remove(a), Some((1, "mail".into())));
        assert_eq!(ids.get(1, "mail"), None);
        assert_eq!(ids.owner(a), None);
        let mut gone = ids.remove_link(1);
        gone.sort();
        assert_eq!(gone, vec![c]);
        assert_eq!(ids.owner(b), Some((2, "mail".into())));
    }

    #[test]
    fn paths_of_shared_storage_documents() {
        let root = "/storage/emulated/0";
        assert_eq!(document_path(EXTERNAL_STORAGE_AUTHORITY, "primary:Download/a.pdf", root).as_deref(), Some("/storage/emulated/0/Download/a.pdf"));
        assert_eq!(document_path(EXTERNAL_STORAGE_AUTHORITY, "primary:", root).as_deref(), Some("/storage/emulated/0"));
        assert_eq!(document_path(EXTERNAL_STORAGE_AUTHORITY, "primary:Music/", "/storage/emulated/10/").as_deref(), Some("/storage/emulated/10/Music"));
        assert_eq!(document_path(EXTERNAL_STORAGE_AUTHORITY, "1234-ABCD:Music/x.ogg", root), None);
        assert_eq!(document_path(EXTERNAL_STORAGE_AUTHORITY, "primary:../../data", root), None);
        assert_eq!(document_path(EXTERNAL_STORAGE_AUTHORITY, "nocolon", root), None);
        assert_eq!(document_path(DOWNLOADS_AUTHORITY, "raw:/storage/emulated/0/Download/b.zip", root).as_deref(), Some("/storage/emulated/0/Download/b.zip"));
        assert_eq!(document_path(DOWNLOADS_AUTHORITY, "msf:42", root), None);
        assert_eq!(document_path(DOWNLOADS_AUTHORITY, "17", root), None);
        assert_eq!(document_path(DOWNLOADS_AUTHORITY, "raw:/data/data/x", root), None);
        assert_eq!(document_path("com.google.android.apps.docs.storage", "primary:x", root), None);
    }

    #[test]
    fn file_names_stay_in_their_directory() {
        assert_eq!(safe_file_name("report.pdf"), "report.pdf");
        assert_eq!(safe_file_name("/home/u/report.pdf"), "report.pdf");
        assert_eq!(safe_file_name("../../x"), "x");
        assert_eq!(safe_file_name(".."), "file");
        assert_eq!(safe_file_name("dir/"), "file");
        assert_eq!(safe_file_name(""), "file");
        assert_eq!(safe_file_name("a\0b"), "ab");
        let long = "é".repeat(150);
        let s = safe_file_name(&long);
        assert!(s.len() <= 200 && s.chars().all(|c| c == 'é'));
    }

    #[test]
    fn old_entries_are_cleaned() {
        let dir = std::env::temp_dir().join(format!("lindroids-open-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("old")).unwrap();
        std::fs::create_dir_all(dir.join("new")).unwrap();
        std::fs::write(dir.join("old/a.txt"), b"x").unwrap();
        let day_ago = SystemTime::now() - Duration::from_secs(2 * 86_400);
        std::fs::File::open(dir.join("old")).unwrap().set_modified(day_ago).unwrap();
        clean_old(&dir, Duration::from_secs(86_400));
        assert!(!dir.join("old").exists());
        assert!(dir.join("new").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn chunks_of(data: &[u8], chunk: usize) -> Vec<(Vec<u8>, bool)> {
        let mut got = Vec::new();
        stream_chunks(data, chunk, |c, last| {
            got.push((c.to_vec(), last));
            true
        })
        .unwrap();
        got
    }

    #[test]
    fn documents_stream_in_chunks_with_the_last_marked() {
        assert_eq!(chunks_of(b"", 4), vec![(vec![], true)]);
        assert_eq!(chunks_of(b"abc", 4), vec![(b"abc".to_vec(), true)]);
        assert_eq!(chunks_of(b"abcd", 4), vec![(b"abcd".to_vec(), true)]);
        assert_eq!(chunks_of(b"abcdefghij", 4), vec![(b"abcd".to_vec(), false), (b"efgh".to_vec(), false), (b"ij".to_vec(), true)]);
    }

    #[test]
    fn streaming_stops_when_told() {
        let mut n = 0;
        stream_chunks(&b"abcdefgh"[..], 2, |_, _| {
            n += 1;
            n < 2
        })
        .unwrap();
        assert_eq!(n, 2);
    }
}
