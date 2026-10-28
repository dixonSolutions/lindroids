//! Several instances of one app: what they are called, how another one opens a new window rather
//! than handing its work to the first (and doing nothing visible), and the limits on how many run.
//!
//! Instances of one app share everything by default (its /tmp, runtime dir, session bus, data), so
//! the app's own single-instance mechanism works: a second VSCodium or Mousepad asks the first for a
//! new window and exits, and the server gives that window to the second run (`CliRequest::Claim`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use lindroids_proto::{CliReply, CliRequest, Instance};

use crate::run::{self, Opts, Target};
use crate::{client, sandbox};

/// What a run is an instance of: the Flatpak's id, or `<place>.<program>` ("termux.foot",
/// "debian.xournalpp"), as `lindroids export` names it by default.
pub fn app_key(o: &Opts, target: &Target, sb: &sandbox::Sandbox) -> String {
    match target {
        Target::Flatpak(app, _) => app.id.clone(),
        Target::Program(_) => {
            let place = o.distro.clone().unwrap_or_else(|| match sb.kind {
                sandbox::Kind::Termux => "termux".into(),
                _ => "linux".into(),
            });
            let prog = run::program_of(&o.command);
            let safe: String = prog.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
            format!("{place}.{safe}")
        }
    }
}

/// The running instances of `key`, most recently used first, and how many runs there are in all.
/// Nothing from a server that does not know instances (an older one).
pub fn query(rt: &Path, key: &str) -> (Vec<Instance>, u32) {
    match client::request(rt, &CliRequest::Instances { app_key: key.into() }, Duration::from_secs(3)) {
        Ok(CliReply::Instances { instances, runs }) => (instances, runs),
        _ => (Vec::new(), 0),
    }
}

/// `[Desktop Action <name>]` sections of a .desktop file: (name, Exec).
pub fn desktop_actions(text: &str) -> Vec<(String, String)> {
    let mut v = Vec::new();
    let mut cur: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(sec) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            cur = sec.strip_prefix("Desktop Action ").map(|n| n.trim().to_string());
            continue;
        }
        if let (Some(name), Some(exec)) = (&cur, line.strip_prefix("Exec=")) {
            v.push((name.clone(), exec.trim().to_string()));
        }
    }
    v
}

/// The actions that open a new window, best first.
const NEW_WINDOW_ACTIONS: &[&str] = &["new-window", "new-empty-window", "new-instance", "new", "NewWindow", "new_window"];

/// Programs whose new-window option no desktop action names.
const NEW_WINDOW_TABLE: &[(&str, &[&str])] = &[
    ("com.vscodium.codium", &["--new-window"]),
    ("codium", &["--new-window"]),
    ("code", &["--new-window"]),
    ("org.gimp.GIMP", &["--new-instance"]),
    ("gimp", &["--new-instance"]),
    // Mousepad (0.6) has no new-window action, and a second plain `mousepad` only brings the
    // first one's window forward.
    ("mousepad", &["--opening-mode=window"]),
    ("org.xfce.mousepad", &["--opening-mode=window"]),
    ("org.mozilla.firefox", &["--new-window"]),
    ("firefox", &["--new-window"]),
    ("chromium", &["--new-window"]),
];

/// The arguments a desktop action's Exec adds to the program: what comes after the program (or,
/// for `flatpak run ... <id>`, after the id), without field codes and Flatpak's `@@` markers.
pub fn action_args(exec: &str, flatpak_id: Option<&str>) -> Vec<String> {
    let words = crate::open::exec_words(exec);
    let rest: Vec<String> = match flatpak_id.and_then(|id| words.iter().position(|w| w == id)) {
        Some(i) if words.first().map_or(false, |w| w.ends_with("flatpak")) => words[i + 1..].to_vec(),
        _ => words.get(1..).unwrap_or(&[]).to_vec(),
    };
    rest.into_iter().filter(|w| !w.starts_with("@@") && !(w.starts_with('%') && w.len() == 2)).collect()
}

/// The new-window arguments from the actions of a .desktop file's text.
pub fn new_window_from_desktop(text: &str, flatpak_id: Option<&str>) -> Option<Vec<String>> {
    let actions = desktop_actions(text);
    NEW_WINDOW_ACTIONS.iter().find_map(|want| actions.iter().find(|(n, _)| n == want)).map(|(_, exec)| action_args(exec, flatpak_id)).filter(|a| !a.is_empty())
}

/// The table's arguments for a program or Flatpak id.
pub fn new_window_from_table(name: &str) -> Option<Vec<String>> {
    NEW_WINDOW_TABLE.iter().find(|(n, _)| *n == name).map(|(_, a)| a.iter().map(|s| s.to_string()).collect())
}

/// The arguments that make what `target` runs open a new window when an instance runs already:
/// its desktop entry's new-window action, else a known option, else none (a plain second start).
pub fn new_window_args(o: &Opts, target: &Target) -> Vec<String> {
    match target {
        Target::Flatpak(app, _) => {
            let file = app.app.dir.join("export/share/applications").join(format!("{}.desktop", app.id));
            std::fs::read_to_string(&file)
                .ok()
                .and_then(|t| new_window_from_desktop(&t, Some(&app.id)))
                .or_else(|| new_window_from_table(&app.id))
                .unwrap_or_default()
        }
        Target::Program(argv) => {
            if run::is_shell_line(argv) {
                return Vec::new();
            }
            let prog = run::program_of(argv);
            let root = o.distro.as_deref().map(run::distro_rootfs).unwrap_or_else(|| PathBuf::from("/"));
            let dirs = crate::desktop::data_dirs(&root);
            crate::desktop::find(&dirs, &prog)
                .and_then(|e| dirs.iter().map(|d| d.join("applications").join(format!("{}.desktop", e.id))).find_map(|p| std::fs::read_to_string(p).ok()))
                .and_then(|t| new_window_from_desktop(&t, None))
                .or_else(|| new_window_from_table(&prog))
                .unwrap_or_default()
        }
    }
}

/// Add `extra` to what `target` runs (after the program, before its own arguments).
pub fn with_args(target: Target, extra: &[String]) -> Target {
    if extra.is_empty() {
        return target;
    }
    match target {
        Target::Flatpak(app, args) => Target::Flatpak(app, extra.iter().cloned().chain(args).collect()),
        Target::Program(argv) if !run::is_shell_line(&argv) => {
            let mut v = vec![argv[0].clone()];
            v.extend(extra.iter().cloned());
            v.extend(argv[1..].iter().cloned());
            Target::Program(v)
        }
        t => t,
    }
}

/// Android's free memory as Linux sees it (MemAvailable), in bytes.
pub fn mem_available() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    parse_mem_available(&text)
}

fn parse_mem_available(meminfo: &str) -> Option<u64> {
    let kb: u64 = meminfo.lines().find_map(|l| l.strip_prefix("MemAvailable:"))?.split_whitespace().next()?.parse().ok()?;
    Some(kb * 1024)
}

/// Below this, a new program is started with a warning.
pub const WARN_MEMORY: u64 = 700 << 20;
/// Below this, no new program starts without --force: Android would kill something for it,
/// likely the browser showing the windows, or Termux.
pub const REFUSE_MEMORY: u64 = 300 << 20;

/// The caps: programs running in all (`LINDROIDS_MAX_RUNS`, 8), instances of one app
/// (`LINDROIDS_MAX_INSTANCES`, 6), processes of one app (`LINDROIDS_MAX_APP_RUNS`, 3). Android
/// kills apps' child processes beyond 32 (its phantom process limit), and each program here is
/// several (proot, the program, its helpers).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Caps {
    pub runs: u32,
    pub instances: u32,
    pub app_runs: u32,
}

impl Caps {
    pub fn from_env() -> Caps {
        let get = |k: &str, d: u32| std::env::var(k).ok().and_then(|v| v.parse().ok()).filter(|n| *n > 0).unwrap_or(d);
        Caps { runs: get("LINDROIDS_MAX_RUNS", 8), instances: get("LINDROIDS_MAX_INSTANCES", 6), app_runs: get("LINDROIDS_MAX_APP_RUNS", 3) }
    }
}

#[derive(Debug, PartialEq)]
pub enum Verdict {
    Go,
    Warn(String),
    Refuse(String),
}

/// Whether a new program may start: `mem` free, `runs` running in all, `instances` of its app
/// (`app_runs` of them processes of their own).
pub fn check_limits(name: &str, mem: Option<u64>, runs: u32, instances: u32, app_runs: u32, caps: Caps) -> Verdict {
    let mb = |b: u64| b >> 20;
    if instances >= caps.instances {
        return Verdict::Refuse(format!("{name} has {instances} windows open already (LINDROIDS_MAX_INSTANCES={}): close one, or add --force", caps.instances));
    }
    if app_runs >= caps.app_runs {
        return Verdict::Refuse(format!("{app_runs} copies of {name} are running already (LINDROIDS_MAX_APP_RUNS={}): close one, or add --force", caps.app_runs));
    }
    if runs >= caps.runs {
        return Verdict::Refuse(format!("{runs} programs are running already (LINDROIDS_MAX_RUNS={}): close one, or add --force", caps.runs));
    }
    match mem {
        Some(m) if m < REFUSE_MEMORY => Verdict::Refuse(format!("only {} MB of memory is free: Android would close something (likely the browser) to start {name}. Close a program, or add --force", mb(m))),
        Some(m) if m < WARN_MEMORY => Verdict::Warn(format!("only {} MB of memory is free: {name} may be slow, and Android may close the browser in the background", mb(m))),
        _ => Verdict::Go,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_window_actions_from_desktop_files() {
        let codium = "[Desktop Entry]\nName=VSCodium\nExec=/usr/bin/flatpak run --branch=stable --command=com.vscodium.codium --file-forwarding com.vscodium.codium @@ %F @@\n\n\
                      [Desktop Action new-empty-window]\nName=New Empty Window\nExec=/usr/bin/flatpak run --branch=stable --command=com.vscodium.codium --file-forwarding com.vscodium.codium --new-window @@ %F @@\n";
        assert_eq!(new_window_from_desktop(codium, Some("com.vscodium.codium")), Some(vec!["--new-window".to_string()]));
        let gimp = "[Desktop Entry]\nExec=gimp %U\n[Desktop Action new]\nExec=gimp --new-instance\n[Desktop Action new-window]\nExec=gimp --new-window %U\n";
        // new-window is preferred over new.
        assert_eq!(new_window_from_desktop(gimp, None), Some(vec!["--new-window".to_string()]));
        // Mousepad 0.6: only "preferences".
        let mousepad = "[Desktop Entry]\nExec=mousepad %U\n[Desktop Action preferences]\nExec=mousepad --preferences\n";
        assert_eq!(new_window_from_desktop(mousepad, None), None);
        // An action that adds nothing is no new-window action.
        assert_eq!(new_window_from_desktop("[Desktop Action new-window]\nExec=foo %U\n", None), None);
        assert_eq!(new_window_from_table("org.gimp.GIMP"), Some(vec!["--new-instance".to_string()]));
        assert_eq!(new_window_from_table("mousepad"), Some(vec!["--opening-mode=window".to_string()]));
    }

    #[test]
    fn extra_arguments_go_after_the_program() {
        let t = with_args(Target::Program(vec!["mousepad".into(), "a.txt".into()]), &["--x".into()]);
        assert!(matches!(t, Target::Program(v) if v == ["mousepad", "--x", "a.txt"]));
        // A shell line stays as it is.
        let t = with_args(Target::Program(vec!["foot -e htop".into()]), &["--x".into()]);
        assert!(matches!(t, Target::Program(v) if v == ["foot -e htop"]));
    }

    #[test]
    fn limits() {
        let caps = Caps { runs: 8, instances: 6, app_runs: 3 };
        let gb = Some(2 << 30);
        assert_eq!(check_limits("X", gb, 0, 0, 0, caps), Verdict::Go);
        assert!(matches!(check_limits("X", Some(500 << 20), 0, 0, 0, caps), Verdict::Warn(m) if m.contains("500 MB")));
        assert!(matches!(check_limits("X", Some(200 << 20), 0, 0, 0, caps), Verdict::Refuse(m) if m.contains("--force")));
        assert!(matches!(check_limits("X", gb, 8, 0, 0, caps), Verdict::Refuse(m) if m.contains("LINDROIDS_MAX_RUNS=8")));
        assert!(matches!(check_limits("X", gb, 3, 6, 1, caps), Verdict::Refuse(m) if m.contains("LINDROIDS_MAX_INSTANCES=6")));
        assert!(matches!(check_limits("X", gb, 3, 3, 3, caps), Verdict::Refuse(m) if m.contains("LINDROIDS_MAX_APP_RUNS=3")));
        // Unknown memory: no memory check.
        assert_eq!(check_limits("X", None, 0, 0, 0, caps), Verdict::Go);
        assert_eq!(parse_mem_available("MemTotal: 4000 kB\nMemAvailable:    1943176 kB\n"), Some(1943176 * 1024));
    }
}
