//! `lindroids`: run Linux apps on Android, their windows shown in the phone's web browser.

mod app;
mod bwrap;
mod client;
mod config;
mod cortex;
mod debug;
mod desktop;
mod export;
mod flatpak;
mod instances;
mod run;
mod sandbox;
mod elf;
mod integration;
mod open;
mod setup_x11;
mod shortcut;
mod update;
mod x11wm;

use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use lindroids_proto::{CliReply, CliRequest};

const HELP: &str = "\
lindroids — Linux apps on Android, their windows in the web browser

USAGE
  lindroids run [options] <command> [args...]
  lindroids run [options] \"<command line>\"
  lindroids export [options] <command> [args...]   a home screen shortcut that starts it
  lindroids export [--run] [--browser <value>] <program>   (--run: start it now too)
  lindroids app [status|install [<file.apk>]]   the shared window app (export installs it)
  lindroids export <id|name> convert home-screen --to=webapk|app   (or convert webapk|app --to=home-screen)
  lindroids run --export <command> [args...]       export with the defaults, then run it
  lindroids config [browser [app|webapk|default|<browser package>]]
  lindroids list | remove <id> | launch [--new] <id>
  lindroids instances [<app>]        the programs running, numbered per app (VSCodium, VSCodium 2)
  lindroids show <pid|app>           bring back the window of a running program (`instances`)
  lindroids setup                    start the server and open the windows page in the browser
  lindroids setup x11 [--yes]        install what X11 programs need (Xwayland), restart the server
  lindroids setup flatpak            install Flatpak: natively in Termux (no root), in a proot-distro
                                     distro (yours or a new Debian), or the system's package
  lindroids update [--check]         update lindroids to the latest release
  lindroids open <file|URI>          open with Android (what xdg-open does for Linux programs)
  lindroids open --linux <file>      open in the Linux program for its type, in a window of its own
        [--with <program>] [--choose] [--list]   (asks once when several can; --choose asks again)
  lindroids notify <title> [body]    an Android notification
  lindroids info [--json]            Android's theme, accent, network, memory, battery
  lindroids displays [forget <id>]    the browsers paired with this server
  lindroids status | start | stop | pair | doctor | version
  lindroids log [--all]              what happened to the windows (the server's log; --all: all of it)

RUN OPTIONS
  --distro <name>     run inside a proot-distro distribution (Termux); also --prootdistro=<name>
  --x11               prefer X11 over Wayland for this program (GTK, Qt and SDL)
  --name <name>       window and shortcut name
  --icon <file>       icon (PNG or SVG)
  -e, --env K=V       extra environment (repeatable)
  --touch pointer     treat touch as a mouse for this program's windows
  --bind <folder>     show a folder at the same path inside a distro or Flatpak
  --new               another window of a program that runs already: the program is asked for a
                      new window (its desktop entry's new-window action, or a known option)
  --force             start even when little memory is free or many programs run

EXPORT OPTIONS (as run, plus)
  --id <id>           shortcut id (default: derived from the program)
  --new-window        a second icon, \"<name> (new window)\", that always opens another window

Tapping a program's icon while it runs brings back the window used last; `launch --new`, the
\"(new window)\" icon and the window's own ＋ button open another one. Each window of its own is
numbered (VSCodium, VSCodium 2). Limits: LINDROIDS_MAX_RUNS (8 programs), LINDROIDS_MAX_INSTANCES
(6 windows of one app), LINDROIDS_MAX_APP_RUNS (3 processes of one app), and free memory (warns
below 700 MB, stops below 300 MB); --force passes them.

A program name that looks like a Flatpak id (org.gnome.Calculator) runs that Flatpak.

Programs also get the browser's side of the desktop through the standard portal on their own
session bus: dark mode, the browser's file chooser and downloads, links opened in new tabs,
notifications in the page, network state.

X11 programs need no option: every program gets DISPLAY as well as WAYLAND_DISPLAY
(xwayland-satellite, started on the first X11 connection). `lindroids doctor` shows whether X11 is
ready; `lindroids setup x11` installs what is missing.

On Android every program gets a window of its own: by default in the Lindroids window app (one
small APK, no browser; `lindroids export` installs it), or in a full-screen Custom Tab of a browser
that has them (Firefox, Chrome, Edge, Samsung Internet, Brave…; `config browser default` or a
package). Closing the window ends the program; the program ending leaves its window saying so.
Elsewhere the windows show in a browser page.
The first time, the page pairs with a ticket in its URL; a page opened by hand asks for the code
`lindroids pair` shows. `lindroids export` puts a program on the home screen (a shortcut, with the
program's icon, that has Termux start it, or bring back its window).

Files from Android open in Linux programs: open a file with Termux in the Files app, or share it to
Termux, and tap \"Edit\". Termux saves it in ~/downloads and runs ~/bin/termux-file-editor, which
`lindroids setup` and `lindroids export` install: it runs `lindroids open --linux <file>`. The
choice of program per file type is kept in ~/.config/mimeapps.list.
";

fn main() {
    cortex::hand_over();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        None | Some("-h") | Some("--help") | Some("help") => {
            print!("{HELP}");
            0
        }
        Some("version") | Some("--version") | Some("-V") => {
            println!("lindroids {}", lindroids_server::BUILD);
            0
        }
        Some("server") => cmd_server(&args[1..]),
        Some("start") => match ensure_server(&sandbox::detect()) {
            Ok(_) => {
                println!("server running");
                0
            }
            Err(e) => fail(&e),
        },
        Some("stop") => cmd_stop(),
        Some("status") => cmd_status(),
        Some("log") => cmd_log(&args[1..]),
        Some("pair") => cmd_pair(),
        Some("displays") => cmd_displays(&args[1..]),
        Some("setup") if args.get(1).map(String::as_str) == Some("x11") => setup_x11::cmd_setup_x11(&args[2..]),
        Some("setup") if args.get(1).map(String::as_str) == Some("flatpak") => cmd_setup_flatpak(),
        Some("setup") => cmd_setup(),
        Some("doctor") => cmd_doctor(),
        // Older releases run `<new binary> update --app-only` after upgrading; there is no app now.
        Some("update") if args.iter().any(|a| a == "--app-only") => update::cmd_update_app_only(),
        Some("update") => update::cmd_update(&args[1..]),
        // `open` alone hands a file or link to Android (what Linux programs' xdg-open runs);
        // `--linux`/`--with` open a file in a Linux program here instead.
        Some("open") if args[1..].iter().any(|a| matches!(a.as_str(), "--linux" | "--with" | "--choose" | "--list") || a.starts_with("--with=")) => {
            open::cmd_open_linux(&args[1..])
        }
        Some("open") => integration::cmd_open(&args[1..]),
        Some("notify") => integration::cmd_notify(&args[1..]),
        Some("info") => integration::cmd_info(&args[1..]),
        Some("run") if args[1..].iter().any(|a| a == "--export") => {
            let mut export_args = vec!["--run".to_string()];
            export_args.extend(args[1..].iter().filter(|a| *a != "--export").cloned());
            export::cmd_export(&export_args)
        }
        Some("run") => run::cmd_run(&args[1..]),
        Some("export") => export::cmd_export(&args[1..]),
        Some("config") => config::cmd_config(&args[1..]),
        Some("app") => app::cmd_app(&args[1..]),
        Some("launch") => export::cmd_launch(&args[1..]),
        Some("instances") => cmd_instances(&args[1..]),
        Some("show") => cmd_show(&args[1..]),
        // Flatpak's bubblewrap on rooted Android (see bwrap.rs).
        Some("bwrap") => bwrap::cmd_bwrap(&args[1..]),
        Some("list") => export::cmd_list(),
        Some("remove") => export::cmd_remove(&args[1..]),
        Some("pick-folder") => cmd_pick_folder(),
        Some("debug-app") => debug::cmd_debug_app(&args[1..]),
        Some(other) => {
            eprintln!("lindroids: unknown command {other:?}\n");
            eprint!("{HELP}");
            2
        }
    };
    std::process::exit(code);
}

pub fn fail(msg: &str) -> i32 {
    eprintln!("lindroids: {msg}");
    1
}

struct StderrLog(log::LevelFilter);

impl log::Log for StderrLog {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= self.0
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
            let _ = writeln!(std::io::stderr(), "{t:.3} {} {}: {}", r.level(), r.target(), r.args());
        }
    }
    fn flush(&self) {}
}

fn cmd_server(args: &[String]) -> i32 {
    let level = match std::env::var("LINDROIDS_LOG").as_deref() {
        Ok("debug") => log::LevelFilter::Debug,
        Ok("warn") => log::LevelFilter::Warn,
        Ok("trace") => log::LevelFilter::Trace,
        _ => log::LevelFilter::Info,
    };
    let _ = log::set_boxed_logger(Box::new(StderrLog(level)));
    log::set_max_level(level);
    let mut cfg = lindroids_server::Config::from_args(args);
    if !args.iter().any(|a| a == "--sandbox") {
        cfg.sandbox = sandbox::detect().name;
    }
    lindroids_server::stop_on_signals();
    if let Some(a) = shortcut::AndroidServices::new() {
        lindroids_server::android::set(Box::new(a));
    }
    lindroids_server::run(cfg)
}

/// Start the server in the background unless it is running. Returns its runtime dir. A server
/// of another protocol (from before an update) is restarted when it shows nothing; otherwise it
/// keeps running, and what needs the new protocol says so.
pub fn ensure_server(sb: &sandbox::Sandbox) -> Result<std::path::PathBuf, String> {
    let rt = sandbox::runtime_dir();
    match client::status(&rt) {
        Ok(CliReply::Status { version, windows, shown_runs, .. }) if version != lindroids_proto::PROTOCOL_VERSION => {
            if windows.is_empty() && shown_runs.is_empty() {
                eprintln!("lindroids: the server running is from another version (protocol {version}): restarting it");
                let _ = cmd_stop_quiet(&rt);
            } else {
                static TOLD: std::sync::Once = std::sync::Once::new();
                TOLD.call_once(|| {
                    eprintln!("lindroids: the server running is from another version (protocol {version}, this is {}); `lindroids stop` once its windows are closed", lindroids_proto::PROTOCOL_VERSION)
                });
                return Ok(rt);
            }
        }
        Ok(_) => return Ok(rt),
        Err(_) => {}
    }
    std::fs::create_dir_all(&rt).map_err(|e| format!("{}: {e}", rt.display()))?;
    let log = std::fs::File::create(rt.join("server.log")).map_err(|e| e.to_string())?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    cmd.arg("server").arg("--sandbox").arg(&sb.name).arg("--runtime-dir").arg(&rt);
    cmd.stdin(Stdio::null()).stdout(log.try_clone().map_err(|e| e.to_string())?).stderr(log);
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    cmd.spawn().map_err(|e| format!("cannot start the server: {e}"))?;
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(10) {
        if client::status(&rt).is_ok() {
            return Ok(rt);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let tail = std::fs::read_to_string(rt.join("server.log")).unwrap_or_default();
    Err(format!("the server did not start:\n{tail}"))
}

/// The URL of a page of the running server (`run`: the page for that run; else the shared one),
/// with a ticket to pair with.
pub fn page_url(rt: &Path, run: Option<u32>, window: bool) -> Option<String> {
    match client::request(rt, &CliRequest::Ticket { run }, Duration::from_secs(3)) {
        Ok(CliReply::Ticket { ticket, origin }) => Some(sandbox::page_url(&origin, run, window, Some(&ticket))),
        _ => None,
    }
}

/// The running server's pages, to tell the user (no ticket: they pair by code).
fn shown_origin(rt: &Path) -> String {
    match client::status(rt) {
        Ok(CliReply::Status { origin, .. }) if !origin.is_empty() => format!("{origin}/"),
        _ => "the windows page (start a program first: `lindroids run <program>`)".into(),
    }
}

/// Opens the window of its own for the run `pid` (Android): a Custom Tab showing only that run.
/// False when no browser here has Custom Tabs; the caller falls back to the shared page.
pub fn open_run_window(_sb: &sandbox::Sandbox, rt: &Path, pid: u32) -> bool {
    let Some(url) = page_url(rt, Some(pid), true) else { return false };
    if let Err(e) = shortcut::open_window(&url) {
        eprintln!("lindroids: no window of its own ({e}); showing it in the browser");
        return false;
    }
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(10) {
        if matches!(client::status(rt), Ok(CliReply::Status { ref shown_runs, .. }) if shown_runs.contains(&pid)) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    eprintln!("lindroids: the window has not linked yet; it shows the program as soon as it does");
    true
}

/// Make sure a display (the browser page) is linked: open the page if not, and on Android bring it
/// to the front, where the windows are about to appear. Never blocks for long: windows show
/// whenever the page links. Returns whether it is linked.
pub fn ensure_display(sb: &sandbox::Sandbox, rt: &Path, quiet: bool) -> bool {
    let linked = || matches!(client::status(rt), Ok(CliReply::Status { app_connected: true, .. }));
    // A linked shared page, to bring back; another program's window of its own is not one (and
    // its URL carries no ticket for a browser that has not paired).
    let page = match client::status(rt) {
        Ok(CliReply::Status { app_connected: true, display_page, .. }) => Some(display_page.unwrap_or_else(|| "/".into())).filter(|p| !p.contains("run=")),
        _ => None,
    };
    let _ = sb;
    // On Android the windows are a Custom Tab of their own (no tabs, no address bar), raised again
    // by the same URL; a browser tab when no browser here has Custom Tabs.
    let window = || sandbox::on_android() && page_url(rt, None, true).map_or(false, |u| shortcut::open_window(&u).is_ok());
    if let Some(page) = page {
        // On a desktop a linked page stays where it is; a new tab each time would only pile up.
        if sandbox::on_android() && !window() {
            // Exactly the page's URL: Firefox switches to that tab, Chrome reuses its tab.
            if let Ok(CliReply::Status { origin, .. }) = client::status(rt) {
                sandbox::open_browser(&format!("{origin}{page}"));
            }
        }
        return true;
    }
    let how = if window() { Some("custom tab") } else { page_url(rt, None, false).and_then(|u| sandbox::open_browser(&u)) };
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(if how.is_some() { 10 } else { 1 }) {
        if linked() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if !quiet {
        let code = match client::status(rt) {
            Ok(CliReply::Status { pairing_code: Some(c), .. }) => c,
            _ => "(see `lindroids pair`)".into(),
        };
        let url = shown_origin(rt);
        if how.is_some() {
            eprintln!("lindroids: the browser has not opened the windows page yet. It shows the windows as soon as");
            eprintln!("  it does. If it asks for a pairing code: {code}");
        } else {
            eprintln!("lindroids: open {url} in a web browser on this device to see the windows");
            eprintln!("  (pairing code {code}).");
        }
    }
    false
}

/// Stop the server and wait until it is gone.
fn cmd_stop_quiet(rt: &Path) -> Result<(), String> {
    let pid = std::fs::read_to_string(rt.join("server.pid")).ok().and_then(|p| p.trim().parse::<u32>().ok());
    client::request(rt, &CliRequest::Stop, Duration::from_secs(5))?;
    let t0 = Instant::now();
    while pid.is_some_and(|p| Path::new(&format!("/proc/{p}")).exists()) && t0.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

/// `lindroids instances [<app>]`: what runs, numbered per app.
fn cmd_instances(args: &[String]) -> i32 {
    let rt = sandbox::runtime_dir();
    let key = args.first().cloned().unwrap_or_default();
    match client::request(&rt, &CliRequest::Instances { app_key: key }, Duration::from_secs(3)) {
        Ok(CliReply::Instances { instances, runs }) => {
            for i in &instances {
                println!("{:<8} {:<3} {:<32} {}{}", i.pid, i.ordinal, i.name, if i.shown { "shown" } else { "not shown" }, if i.claimed { ", a window of another instance's process" } else { "" });
            }
            if instances.is_empty() {
                println!("Nothing runs.");
            }
            println!("{runs} program(s) running");
            0
        }
        Ok(CliReply::Error { message }) => fail(&format!("the server does not know instances ({message}): `lindroids stop`, then start again")),
        Ok(other) => fail(&format!("unexpected reply {other:?}")),
        Err(e) => fail(&e),
    }
}

/// `lindroids show <pid|app>`: bring back a running instance's window (an app: the one used last).
fn cmd_show(args: &[String]) -> i32 {
    let Some(what) = args.first() else { return fail("which one? (lindroids instances)") };
    let sb = sandbox::detect();
    let rt = sandbox::runtime_dir();
    let (all, _) = instances::query(&rt, "");
    let found = match what.parse::<u32>() {
        Ok(pid) => all.iter().find(|i| i.pid == pid),
        Err(_) => instances::query(&rt, what).0.first().and_then(|f| all.iter().find(|i| i.pid == f.pid)),
    };
    let Some(i) = found else { return fail(&format!("{what} is not running (lindroids instances)")) };
    if open_run_window(&sb, &rt, i.pid) {
        println!("{}: its window is back", i.name);
        0
    } else {
        fail("its window could not be opened")
    }
}

fn cmd_stop() -> i32 {
    let rt = sandbox::runtime_dir();
    let pid = std::fs::read_to_string(rt.join("server.pid")).ok().and_then(|p| p.trim().parse::<u32>().ok());
    match client::request(&rt, &CliRequest::Stop, Duration::from_secs(5)) {
        Ok(_) => {
            // Gone, not only told to go: until it has, it holds the port, and a server started
            // right after (`lindroids stop; lindroids run ...`) could not take it.
            let t0 = Instant::now();
            while pid.is_some_and(|p| Path::new(&format!("/proc/{p}")).exists()) && t0.elapsed() < Duration::from_secs(10) {
                std::thread::sleep(Duration::from_millis(50));
            }
            println!("server stopped");
            0
        }
        Err(e) => fail(&e),
    }
}

/// The server's log, for a report: by default what happened to windows and runs (pages linking,
/// leaving and how, runs ending, the pages' own trace), with the time of day. `--copy` puts it on
/// Android's clipboard (its last 200 KB) instead of printing it, to paste into a report.
fn cmd_log(args: &[String]) -> i32 {
    let path = sandbox::runtime_dir().join("server.log");
    let Ok(bytes) = std::fs::read(&path) else { return fail(&format!("no log yet ({})", path.display())) };
    let all = args.iter().any(|a| a == "--all");
    let copy = args.iter().any(|a| a == "--copy");
    let mut out = String::new();
    const ABOUT: [&str; 14] = [
        "page for ", "linked", "lost in the background", "ended", "mapped", "unmapped", "moves from", "page ", "the window is gone",
        "failed to authenticate", "stands down", "already has its window", "lindroids-server ready", "closed",
    ];
    for line in String::from_utf8_lossy(&bytes).lines() {
        if !all && !ABOUT.iter().any(|a| line.contains(a)) {
            continue;
        }
        // "<unix seconds>.<ms> LEVEL target: message" → "HH:MM:SS.mmm message" (local time).
        let shown = line.split_once(' ').and_then(|(t, rest)| {
            let secs: f64 = t.parse().ok()?;
            let msg = rest.splitn(3, ' ').nth(2).unwrap_or(rest);
            let mut tm: libc::tm = unsafe { std::mem::zeroed() };
            let tt = secs as libc::time_t;
            unsafe { libc::localtime_r(&tt, &mut tm) };
            Some(format!("{:02}:{:02}:{:02}.{:03} {msg}", tm.tm_hour, tm.tm_min, tm.tm_sec, ((secs.fract()) * 1000.0) as u32))
        });
        out.push_str(&shown.unwrap_or_else(|| line.to_string()));
        out.push('\n');
    }
    if !copy {
        print!("{out}");
        return 0;
    }
    const MAX: usize = 200 * 1024;
    if out.len() > MAX {
        let mut from = out.len() - MAX;
        while !out.is_char_boundary(from) {
            from += 1;
        }
        // Whole lines: from the next line start.
        from = out[from..].find('\n').map_or(from, |n| from + n + 1);
        out.drain(..from);
    }
    match shortcut::copy_to_clipboard(&out) {
        Ok(()) => {
            println!("{} lines copied to the clipboard: paste them where you report the problem.", out.lines().count());
            0
        }
        Err(e) => fail(&format!("could not copy to the clipboard ({e}); `lindroids log` prints it instead")),
    }
}

fn cmd_status() -> i32 {
    let rt = sandbox::runtime_dir();
    match client::status(&rt) {
        Ok(CliReply::Status { build, app_connected, app_build, pairing_code, windows, clients, .. }) => {
            println!("server      {build} ({})", rt.display());
            let _ = app_build;
            if app_connected {
                println!("display     linked (browser)");
            } else {
                println!("display     none: open {} in a browser, or `lindroids run <program>`", shown_origin(&rt));
            }
            if !app_connected {
                if let Some(c) = pairing_code {
                    println!("pairing     {c}");
                }
            }
            println!("clients     {clients}");
            for w in windows {
                println!("window {:<4} {} ({}) {}x{}", w.win, w.title, w.app_id, w.width, w.height);
            }
            0
        }
        Ok(other) => fail(&format!("unexpected reply {other:?}")),
        Err(e) => fail(&e),
    }
}

/// `lindroids displays [forget <id>]`: the browsers (and debug apps) paired with this server.
fn cmd_displays(args: &[String]) -> i32 {
    let rt = sandbox::runtime_dir();
    match args.first().map(String::as_str) {
        Some("forget") => {
            let Some(id) = args.get(1) else { return fail("which one? (lindroids displays)") };
            match client::request(&rt, &CliRequest::ForgetDisplay { id: id.clone() }, Duration::from_secs(3)) {
                Ok(CliReply::Ok { message }) => {
                    println!("{message}");
                    0
                }
                Ok(CliReply::Error { message }) => fail(&message),
                Ok(other) => fail(&format!("unexpected reply {other:?}")),
                Err(e) => fail(&e),
            }
        }
        None => match client::request(&rt, &CliRequest::Displays, Duration::from_secs(3)) {
            Ok(CliReply::Displays { displays }) => {
                if displays.is_empty() {
                    println!("No display has paired yet.");
                }
                for d in displays {
                    println!("{:<14} {}", d.id, if d.name.is_empty() { "(unnamed)" } else { &d.name });
                }
                0
            }
            Ok(other) => fail(&format!("unexpected reply {other:?}")),
            Err(e) => fail(&e),
        },
        Some(other) => fail(&format!("unknown: displays {other}")),
    }
}

fn cmd_pair() -> i32 {
    let sb = sandbox::detect();
    let rt = match ensure_server(&sb) {
        Ok(rt) => rt,
        Err(e) => return fail(&e),
    };
    if ensure_display(&sb, &rt, true) {
        println!("The browser is showing this server's windows ({}).", shown_origin(&rt));
        return 0;
    }
    match client::status(&rt) {
        Ok(CliReply::Status { pairing_code: Some(c), .. }) => {
            println!("Open {} in a web browser on this device and enter this code:\n\n    {c}\n", shown_origin(&rt));
            let t0 = Instant::now();
            while t0.elapsed() < Duration::from_secs(120) {
                if let Ok(CliReply::Status { app_connected: true, .. }) = client::status(&rt) {
                    println!("Paired.");
                    return 0;
                }
                std::thread::sleep(Duration::from_millis(300));
            }
            fail("not paired yet; run `lindroids pair` again when ready")
        }
        Ok(_) => fail("pairing is closed after too many wrong codes; run `lindroids stop` and try again"),
        Err(e) => fail(&e),
    }
}

/// `lindroids setup`: start the server and open the windows page, so the first program has
/// somewhere to show. Nothing to install on the Android side: the browser is the display.
pub fn cmd_setup() -> i32 {
    let sb = sandbox::detect();
    let rt = match ensure_server(&sb) {
        Ok(rt) => rt,
        Err(e) => return fail(&e),
    };
    // Files opened with or shared to Termux ("Edit") open in Linux programs.
    if sb.kind == sandbox::Kind::Termux {
        match shortcut::install_file_editor(&sb) {
            Ok(true) => println!("Files opened with Termux or shared to it (\"Edit\") now open in Linux programs (~/bin/termux-file-editor)."),
            Ok(false) => {}
            Err(e) => eprintln!("lindroids: files shared to Termux will not open in Linux programs: {e}"),
        }
    }
    if ensure_display(&sb, &rt, false) {
        println!("Ready: windows show in the browser ({}). Try: lindroids run <program>", shown_origin(&rt));
        0
    } else {
        1
    }
}

/// `lindroids setup flatpak`: the installer's Flatpak step (its menu in Termux), from the site.
fn cmd_setup_flatpak() -> i32 {
    let url = format!("{}/install.sh", option_env!("LINDROIDS_SITE").unwrap_or("https://dixonsolutions.github.io/lindroids"));
    let tmp = std::env::temp_dir().join(format!("lindroids-install-{}.sh", std::process::id()));
    let ok = Command::new("curl").args(["-fsSL", "-o"]).arg(&tmp).arg(&url).status().map_or(false, |s| s.success());
    if !ok {
        return fail(&format!("cannot download {url}"));
    }
    let code = Command::new("sh").arg(&tmp).arg("--flatpak-only").status().map_or(1, |s| s.code().unwrap_or(1));
    let _ = std::fs::remove_file(&tmp);
    code
}

fn cmd_pick_folder() -> i32 {
    let sb = sandbox::detect();
    let rt = match ensure_server(&sb) {
        Ok(rt) => rt,
        Err(e) => return fail(&e),
    };
    ensure_display(&sb, &rt, false);
    match client::request(&rt, &CliRequest::PickFolder, Duration::from_secs(300)) {
        Ok(CliReply::Folder { path: Some(p) }) => {
            println!("{p}");
            0
        }
        Ok(CliReply::Folder { path: None }) => fail("no folder chosen"),
        Ok(CliReply::Error { message }) => fail(&message),
        Ok(other) => fail(&format!("unexpected reply {other:?}")),
        Err(e) => fail(&e),
    }
}

fn cmd_doctor() -> i32 {
    let sb = sandbox::detect();
    println!("lindroids   {}", lindroids_server::BUILD);
    println!("sandbox     {} ({:?}, uid {})", sb.name, sb.kind, sb.uid);
    println!("host        {}", sb.host.describe());
    println!("runtime     {}", sandbox::runtime_dir().display());
    for (what, ok) in [
        ("proot", sandbox::have("proot")),
        ("proot-distro", sandbox::have("proot-distro")),
        ("Xwayland", sandbox::have("Xwayland")),
        ("satellite", lindroids_server::x11::find_satellite().is_some()),
        ("dbus-daemon", sandbox::have("dbus-daemon")),
        ("flatpak", sandbox::have("flatpak")),
        ("termux-am", sandbox::have("termux-am") || Path::new(&format!("{}/bin/termux-am", lindroids_proto::TERMUX_PREFIX)).exists()),
    ] {
        println!("{:<11} {}", what, if ok { "found" } else { "not found" });
    }
    if let Some(p) = lindroids_server::x11::find_satellite() {
        println!("            {}", p.display());
    }
    let rt = sandbox::runtime_dir();
    let running = match client::status(&rt) {
        Ok(CliReply::Status { app_connected, .. }) => {
            println!("server      running, display {}", if app_connected { "linked (browser)" } else { "not linked" });
            println!("windows at  {}", shown_origin(&rt));
            true
        }
        _ => {
            println!("server      not running (`lindroids start`)");
            false
        }
    };
    match (rt.join("bus").exists(), running) {
        (true, true) => println!("desktop     portal on (theme, file picker, open, notifications)"),
        (false, true) => println!("desktop     portal off in the running server (needs dbus-daemon: `lindroids setup x11`)"),
        (_, false) if sandbox::have("dbus-daemon") => println!("desktop     ready; starts with the server"),
        (_, false) => println!("desktop     off: no dbus-daemon (`lindroids setup x11`)"),
    }
    let missing = setup_x11::missing();
    match (sandbox::x11_display(&rt), running) {
        (Some(d), _) => println!("X11         display {d} (xwayland-satellite starts on the first X11 connection)"),
        (None, _) if !missing.is_empty() => println!("X11         off: missing {} (`lindroids setup x11`)", missing.join(", ")),
        (None, true) => println!("X11         off in the running server (see {}; `lindroids stop` and start again)", rt.join("server.log").display()),
        (None, false) => println!("X11         ready; starts with the server"),
    }
    0
}
