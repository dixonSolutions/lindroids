//! Names and icons for windows.
//!
//! The CLI resolves the name and icon of what it launches (it can read .desktop files inside
//! distros and flatpaks, and render SVG) and registers them with the launched pid. A window then
//! takes the name and icon of the nearest registered ancestor of its client process. When nothing
//! was registered, the app id is looked up in the usual places on this side.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine;
use wayland_server::backend::ClientId;
use wayland_server::DisplayHandle;

/// A program `lindroids run` (or `launch`) started: an instance of an app.
struct Launch {
    name: String,
    icon: Option<String>,
    /// It has a window of its own (a page for this run), not a place in the shared one.
    window: bool,
    /// The app it is an instance of; empty: not known (an older CLI).
    app_key: String,
    /// Its number among the app's instances (1, 2, ...): the lowest free one when it started.
    ordinal: u32,
    /// `lindroids` arguments that start one more instance.
    again: Vec<String>,
    /// When its process started (field 22 of /proc/<pid>/stat): a pid used again by another
    /// process is not this launch.
    start_time: Option<u64>,
    /// When it was last used (ms since the server started; 0: never).
    last_focus: u64,
    claim: Claim,
    /// When `lindroids run` said it ended (RunEnded), or the server saw its process gone.
    ended: Option<Instant>,
    gone: Option<Instant>,
    registered: Instant,
}

/// A run whose process handed its work to another instance and ended (see `CliRequest::Claim`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Claim {
    /// Not a claiming run: it lives as long as its process.
    None,
    /// Waiting (until then) for a new window of its app.
    Pending(Instant),
    /// It shows windows of another instance's process: it lives as long as they do.
    Holding,
}

static LAUNCHES: Mutex<Option<HashMap<u32, Launch>>> = Mutex::new(None);
static EPOCH: OnceLock<Instant> = OnceLock::new();

/// Milliseconds since the server started (for `last_focus`), at least 1.
pub fn now_ms() -> u64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

/// When process `pid` started, in clock ticks since boot (field 22 of /proc/<pid>/stat).
pub fn start_time(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // pid (comm) state ppid ...: field 3 is the first after the comm.
    stat[stat.rfind(')')? + 2..].split_whitespace().nth(19)?.parse().ok()
}

/// Whether process `pid` is the one that started at `start` (None: any process with that pid).
fn process_is(pid: u32, start: Option<u64>) -> bool {
    match (start_time(pid), start) {
        (Some(now), Some(then)) => now == then,
        (Some(_), None) => true,
        (None, _) => false,
    }
}

/// The lowest number from 1 not in `used`.
pub fn lowest_free(used: &[u32]) -> u32 {
    (1..).find(|n| !used.contains(n)).unwrap_or(1)
}

/// The nearest of `pid` and its ancestors that `is_launch`, following `parent`.
pub fn nearest_launch(pid: u32, is_launch: impl Fn(u32) -> bool, parent: impl Fn(u32) -> Option<u32>) -> Option<u32> {
    let mut pid = pid;
    for _ in 0..64 {
        if is_launch(pid) {
            return Some(pid);
        }
        pid = parent(pid)?;
        if pid <= 1 {
            break;
        }
    }
    None
}

/// What `lindroids run` registers about a launch.
#[derive(Debug, Clone, Default)]
pub struct NewLaunch {
    pub name: String,
    pub icon: Option<String>,
    pub window: bool,
    pub app_key: String,
    pub again: Vec<String>,
}

/// Register a launch of no app in particular (an older CLI's).
pub fn register_launch(pid: u32, name: String, icon: Option<String>, window: bool) {
    register(pid, NewLaunch { name, icon, window, ..Default::default() });
}

/// Register a launch; returns its number among its app's instances.
pub fn register(pid: u32, l: NewLaunch) -> u32 {
    let mut g = LAUNCHES.lock().unwrap();
    let map = g.get_or_insert_with(HashMap::new);
    // A pid seen before is a new process now.
    map.remove(&pid);
    let used: Vec<u32> = if l.app_key.is_empty() { Vec::new() } else { map.values().filter(|o| o.app_key == l.app_key).map(|o| o.ordinal).collect() };
    let ordinal = lowest_free(&used);
    map.insert(
        pid,
        Launch {
            name: l.name,
            icon: l.icon,
            window: l.window,
            app_key: l.app_key,
            ordinal,
            again: l.again,
            start_time: start_time(pid),
            last_focus: now_ms(),
            claim: Claim::None,
            ended: None,
            gone: None,
            registered: Instant::now(),
        },
    );
    ordinal
}

/// Whether `p` names a launch: a registered pid whose process is that launch's, or gone (a run
/// that ended is still named by its pid until it is forgotten).
fn is_launch(map: &HashMap<u32, Launch>, p: u32) -> bool {
    map.get(&p).map_or(false, |l| match start_time(p) {
        None => true,
        Some(now) => l.start_time.map_or(true, |s| s == now),
    })
}

fn named(l: &Launch) -> String {
    if l.ordinal >= 2 {
        format!("{} {}", l.name, l.ordinal)
    } else {
        l.name.clone()
    }
}

/// The name and icon `lindroids run` gave a launch (with its number from the second instance on:
/// "VSCodium 2").
pub fn launch_info(pid: u32) -> Option<(String, Option<String>)> {
    let g = LAUNCHES.lock().unwrap();
    g.as_ref()?.get(&pid).map(|l| (named(l), l.icon.clone()))
}

/// A launch's number among its app's instances.
pub fn ordinal(pid: u32) -> u32 {
    let g = LAUNCHES.lock().unwrap();
    g.as_ref().and_then(|m| m.get(&pid)).map_or(0, |l| l.ordinal)
}

/// Whether this launch shows its windows in a window of its own.
pub fn wants_own_window(pid: u32) -> bool {
    let g = LAUNCHES.lock().unwrap();
    g.as_ref().and_then(|m| m.get(&pid)).map_or(false, |l| l.window)
}

pub fn app_key(pid: u32) -> String {
    let g = LAUNCHES.lock().unwrap();
    g.as_ref().and_then(|m| m.get(&pid)).map(|l| l.app_key.clone()).unwrap_or_default()
}

pub fn again(pid: u32) -> Vec<String> {
    let g = LAUNCHES.lock().unwrap();
    g.as_ref().and_then(|m| m.get(&pid)).map(|l| l.again.clone()).unwrap_or_default()
}

/// When the launch was registered.
pub fn registered(pid: u32) -> Option<Instant> {
    let g = LAUNCHES.lock().unwrap();
    g.as_ref().and_then(|m| m.get(&pid)).map(|l| l.registered)
}

pub fn claim(pid: u32) -> Claim {
    let g = LAUNCHES.lock().unwrap();
    g.as_ref().and_then(|m| m.get(&pid)).map_or(Claim::None, |l| l.claim)
}

pub fn set_claim(pid: u32, c: Claim) {
    let mut g = LAUNCHES.lock().unwrap();
    if let Some(l) = g.as_mut().and_then(|m| m.get_mut(&pid)) {
        l.claim = c;
        if c == Claim::None && l.ended.is_none() && !process_is(pid, l.start_time) {
            l.ended = Some(Instant::now());
        }
    }
}

/// The launch was used (a window focused, its page came on screen).
pub fn touch(pid: u32) {
    let mut g = LAUNCHES.lock().unwrap();
    if let Some(l) = g.as_mut().and_then(|m| m.get_mut(&pid)) {
        l.last_focus = now_ms();
    }
}

/// `lindroids run` said the launch ended.
pub fn ended(pid: u32) {
    let mut g = LAUNCHES.lock().unwrap();
    if let Some(l) = g.as_mut().and_then(|m| m.get_mut(&pid)) {
        l.ended.get_or_insert_with(Instant::now);
    }
}

/// Whether the run `pid` is still on: its process runs, or (a claiming run) it waits for or
/// shows a handed-over window.
pub fn run_alive(pid: u32) -> bool {
    let g = LAUNCHES.lock().unwrap();
    let Some(l) = g.as_ref().and_then(|m| m.get(&pid)) else { return false };
    match l.claim {
        Claim::Holding => true,
        Claim::Pending(until) => until > Instant::now() || process_is(pid, l.start_time),
        Claim::None => process_is(pid, l.start_time),
    }
}

/// The instances of `app_key` (every launch for ""), most recently used first; and how many
/// launches have a process of their own running.
pub fn instances(app_key: &str) -> (Vec<lindroids_proto::Instance>, u32) {
    let g = LAUNCHES.lock().unwrap();
    let Some(map) = g.as_ref() else { return (Vec::new(), 0) };
    let live = |p: u32, l: &Launch| match l.claim {
        Claim::None => l.ended.is_none() && process_is(p, l.start_time),
        Claim::Pending(_) => false,
        Claim::Holding => true,
    };
    let runs = map.iter().filter(|(p, l)| l.claim == Claim::None && live(**p, l)).count() as u32;
    let mut v: Vec<lindroids_proto::Instance> = map
        .iter()
        .filter(|(p, l)| (app_key.is_empty() || l.app_key == app_key) && live(**p, l))
        .map(|(p, l)| lindroids_proto::Instance { pid: *p, ordinal: l.ordinal, name: named(l), shown: false, last_focus: l.last_focus, claimed: l.claim == Claim::Holding })
        .collect();
    v.sort_by(|a, b| b.last_focus.cmp(&a.last_focus).then(b.ordinal.cmp(&a.ordinal)));
    (v, runs)
}

/// What the reaper found: launches whose process went without `lindroids run` saying so (its
/// watcher was killed), and launches to forget.
#[derive(Debug, Default, PartialEq)]
pub struct Reaped {
    pub lost: Vec<u32>,
    pub forgotten: Vec<u32>,
}

/// How long a launch is kept after it ended, for what still names it (its RunEnded, its page).
const KEEP_ENDED: Duration = Duration::from_secs(3);

/// Look for launches whose process is gone (or is another process now): those that ended (and
/// were told so) are forgotten after a few seconds, freeing their number; those that went
/// without a word are reported lost, then forgotten.
pub fn reap() -> Reaped {
    let mut g = LAUNCHES.lock().unwrap();
    let Some(map) = g.as_mut() else { return Reaped::default() };
    let mut r = Reaped::default();
    let now = Instant::now();
    for (p, l) in map.iter_mut() {
        let waiting = match l.claim {
            Claim::Holding => true,
            Claim::Pending(until) => until > now,
            Claim::None => false,
        };
        if waiting || process_is(*p, l.start_time) {
            continue;
        }
        let gone = *l.gone.get_or_insert(now);
        match l.ended {
            Some(t) if now.duration_since(t) >= KEEP_ENDED => r.forgotten.push(*p),
            Some(_) => {}
            // Kept a while longer, so that what is sent about its end reaches its page.
            None if now.duration_since(gone) >= KEEP_ENDED => {
                r.lost.push(*p);
                l.ended = Some(now);
            }
            None => {}
        }
    }
    for p in &r.forgotten {
        map.remove(p);
    }
    r
}

/// The launch (its pid) a process descends from, if any.
pub fn launch_of_pid(pid: u32) -> Option<u32> {
    let g = LAUNCHES.lock().unwrap();
    let map = g.as_ref()?;
    nearest_launch(pid, |p| is_launch(map, p), parent_of)
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
    g.as_ref().map(|m| m.iter().filter(|(p, l)| process_is(**p, l.start_time)).map(|(p, _)| *p).collect()).unwrap_or_default()
}

/// The process `pid` belongs under: its parent, or, for an orphan (adopted by init), the process
/// tracing it. An app under proot that detaches a process (VSCodium: VS Code's CLI starts the
/// editor and exits) leaves it with init as its parent, but proot, the run's, still traces it:
/// its windows are the run's, and it ends with the run.
fn parent_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // pid (comm) state ppid ...  — comm may contain spaces and parentheses.
    let after = &stat[stat.rfind(')')? + 2..];
    let ppid: u32 = after.split_whitespace().nth(1)?.parse().ok()?;
    if ppid > 1 {
        return Some(ppid);
    }
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let tracer = status.lines().find_map(|l| l.strip_prefix("TracerPid:"))?.trim().parse().ok();
    tracer.filter(|t| *t > 1).or(Some(ppid))
}

pub fn client_pid(dh: &DisplayHandle, client: &ClientId) -> Option<u32> {
    let c = dh.backend_handle().get_client_credentials(client.clone()).ok()?;
    Some(c.pid as u32)
}

fn with_launch<T>(dh: &DisplayHandle, client: &ClientId, f: impl Fn(&Launch) -> Option<T>) -> Option<T> {
    let pid = client_pid(dh, client)?;
    let g = LAUNCHES.lock().unwrap();
    let map = g.as_ref()?;
    let run = nearest_launch(pid, |p| is_launch(map, p), parent_of)?;
    f(map.get(&run)?)
}

pub fn name_for_window(dh: &DisplayHandle, client: &ClientId) -> Option<String> {
    with_launch(dh, client, |l| Some(named(l)))
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
    use super::*;

    /// Tests that use the launch registry (one process-wide map) one at a time.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn launch(key: &str) -> NewLaunch {
        NewLaunch { name: "VSCodium".into(), app_key: key.into(), window: true, ..Default::default() }
    }

    #[test]
    fn numbers_are_the_lowest_free() {
        assert_eq!(lowest_free(&[]), 1);
        assert_eq!(lowest_free(&[1, 2]), 3);
        assert_eq!(lowest_free(&[2, 3]), 1);
        assert_eq!(lowest_free(&[1, 3]), 2);
    }

    #[test]
    fn instances_are_numbered_per_app_and_numbers_come_back() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let mut kids: Vec<std::process::Child> = (0..4).map(|_| std::process::Command::new("sleep").arg("30").spawn().unwrap()).collect();
        let pids: Vec<u32> = kids.iter().map(|c| c.id()).collect();
        assert_eq!(register(pids[0], launch("t.a")), 1);
        assert_eq!(register(pids[1], launch("t.a")), 2);
        assert_eq!(register(pids[2], launch("t.b")), 1);
        assert_eq!(launch_info(pids[1]).unwrap().0, "VSCodium 2");
        assert_eq!(launch_info(pids[0]).unwrap().0, "VSCodium");
        // Most recently used first.
        std::thread::sleep(Duration::from_millis(5));
        touch(pids[0]);
        let (v, runs) = instances("t.a");
        assert_eq!(v.iter().map(|i| i.pid).collect::<Vec<_>>(), [pids[0], pids[1]]);
        assert!(runs >= 3);
        // The first ends: once forgotten, its number is free again.
        kids[0].kill().unwrap();
        kids[0].wait().unwrap();
        ended(pids[0]);
        assert!(!run_alive(pids[0]));
        assert_eq!(instances("t.a").0.len(), 1);
        std::thread::sleep(KEEP_ENDED + Duration::from_millis(100));
        let r = reap();
        assert!(r.forgotten.contains(&pids[0]) && r.lost.is_empty(), "{r:?}");
        assert_eq!(register(pids[3], launch("t.a")), 1);
        for k in &mut kids {
            let _ = k.kill();
            let _ = k.wait();
        }
    }

    #[test]
    fn a_launch_gone_without_a_word_is_lost() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let mut k = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let pid = k.id();
        register(pid, launch("t.lost"));
        k.kill().unwrap();
        k.wait().unwrap();
        assert!(!reap().lost.contains(&pid));
        std::thread::sleep(KEEP_ENDED + Duration::from_millis(100));
        assert!(reap().lost.contains(&pid));
        // Still known while its end is reported, then forgotten.
        assert!(launch_info(pid).is_some());
        std::thread::sleep(KEEP_ENDED + Duration::from_millis(100));
        assert!(reap().forgotten.contains(&pid));
        assert!(launch_info(pid).is_none());
    }

    #[test]
    fn a_pid_used_again_is_not_the_launch() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let k = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let pid = k.id();
        let mut k = k;
        register(pid, launch("t.reuse"));
        assert!(run_alive(pid));
        assert_eq!(launch_of_pid(pid), Some(pid));
        // As if the process had started at another time: another process with the same pid.
        {
            let mut g = LAUNCHES.lock().unwrap();
            g.as_mut().unwrap().get_mut(&pid).unwrap().start_time = Some(1);
        }
        assert!(!run_alive(pid));
        assert_ne!(launch_of_pid(pid), Some(pid));
        assert!(start_time(pid).is_some_and(|t| t > 1));
        let _ = k.kill();
        let _ = k.wait();
    }

    #[test]
    fn a_claiming_run_lives_while_it_holds_windows() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let mut k = std::process::Command::new("true").spawn().unwrap();
        let pid = k.id();
        register(pid, launch("t.claim"));
        k.wait().unwrap();
        set_claim(pid, Claim::Pending(Instant::now() + Duration::from_secs(5)));
        assert!(run_alive(pid));
        set_claim(pid, Claim::Holding);
        assert!(run_alive(pid));
        let (v, _) = instances("t.claim");
        assert!(v.len() == 1 && v[0].claimed);
        std::thread::sleep(KEEP_ENDED + Duration::from_millis(100));
        assert!(!reap().lost.contains(&pid), "held runs are not reaped");
        set_claim(pid, Claim::None);
        assert!(!run_alive(pid));
    }

    #[test]
    fn windows_belong_to_their_nearest_launch() {
        // Two instances of one app, sharing everything but their process trees: 10 -> 11 -> 12
        // (instance A, a detached editor process 12) and 20 -> 21 (instance B).
        let parents: HashMap<u32, u32> = [(11, 10), (12, 11), (21, 20), (10, 2), (20, 2)].into_iter().collect();
        let launches = [10, 20];
        let find = |p| nearest_launch(p, |q| launches.contains(&q), |q| parents.get(&q).copied());
        assert_eq!(find(12), Some(10));
        assert_eq!(find(21), Some(20));
        assert_eq!(find(20), Some(20));
        assert_eq!(find(99), None);
    }
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
