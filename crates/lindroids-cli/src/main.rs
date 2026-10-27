//! `lindroids`: run Linux apps on Android, their windows shown in the phone's web browser.

mod bwrap;
mod client;
mod debug;
mod desktop;
mod export;
mod flatpak;
mod run;
mod sandbox;
mod elf;
mod integration;
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
  lindroids list | remove <id> | launch <id>
  lindroids setup                    start the server and open the windows page in the browser
  lindroids setup x11 [--yes]        install what X11 programs need (Xwayland), restart the server
  lindroids setup flatpak            install Flatpak: natively in Termux (no root), in a proot-distro
                                     distro (yours or a new Debian), or the system's package
  lindroids update [--check]         update lindroids to the latest release
  lindroids open <file|URI>          open with Android (what xdg-open does for Linux programs)
  lindroids notify <title> [body]    an Android notification
  lindroids info [--json]            Android's theme, accent, network, memory, battery
  lindroids status | start | stop | pair | doctor | version

RUN OPTIONS
  --distro <name>     run inside a proot-distro distribution (Termux); also --prootdistro=<name>
  --x11               prefer X11 over Wayland for this program (GTK, Qt and SDL)
  --name <name>       window and shortcut name
  --icon <file>       icon (PNG or SVG)
  -e, --env K=V       extra environment (repeatable)
  --touch pointer     treat touch as a mouse for this program's windows

EXPORT OPTIONS (as run, plus)
  --id <id>           shortcut id (default: derived from the program)

A program name that looks like a Flatpak id (org.gnome.Calculator) runs that Flatpak.

Programs also get the browser's side of the desktop through the standard portal on their own
session bus: dark mode, the browser's file chooser and downloads, links opened in new tabs,
notifications in the page, network state.

X11 programs need no option: every program gets DISPLAY as well as WAYLAND_DISPLAY, and each X11
window becomes its own tab in the page (xwayland-satellite, started on the first X11 connection).
`lindroids doctor` shows whether X11 is ready; `lindroids setup x11` installs what is missing.

Windows show in the phone's web browser: `lindroids run` opens http://127.0.0.1:<port>/ there (no
app to install, no permission to grant). A browser opened by hand asks for the pairing code that
`lindroids pair` shows. `lindroids export` puts a program on the home screen (a shortcut that has
Termux start it).
";

fn main() {
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
        Some("pair") => cmd_pair(),
        Some("setup") if args.get(1).map(String::as_str) == Some("x11") => setup_x11::cmd_setup_x11(&args[2..]),
        Some("setup") if args.get(1).map(String::as_str) == Some("flatpak") => cmd_setup_flatpak(),
        Some("setup") => cmd_setup(),
        Some("doctor") => cmd_doctor(),
        // Older releases run `<new binary> update --app-only` after upgrading; there is no app now.
        Some("update") if args.iter().any(|a| a == "--app-only") => update::cmd_update_app_only(),
        Some("update") => update::cmd_update(&args[1..]),
        Some("open") => integration::cmd_open(&args[1..]),
        Some("notify") => integration::cmd_notify(&args[1..]),
        Some("info") => integration::cmd_info(&args[1..]),
        Some("run") => run::cmd_run(&args[1..]),
        Some("export") => export::cmd_export(&args[1..]),
        Some("launch") => export::cmd_launch(&args[1..]),
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
    lindroids_server::run(cfg)
}

/// Start the server in the background unless it is running. Returns its runtime dir.
pub fn ensure_server(sb: &sandbox::Sandbox) -> Result<std::path::PathBuf, String> {
    let rt = sandbox::runtime_dir();
    if client::status(&rt).is_ok() {
        return Ok(rt);
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

/// The page `ensure_display` opens: an exported program's own (`/app/<id>`, maybe installed as an
/// app) for `lindroids launch`, else the plain one.
pub static DISPLAY_PAGE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
/// `lindroids launch` started because a page asked: that page is in front already.
pub static LAUNCHED_BY_PAGE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Make sure a display (the browser page) is linked: open the page if not, and on Android bring it
/// to the front, where the windows are about to appear. Never blocks for long: windows show
/// whenever the page links. Returns whether it is linked.
pub fn ensure_display(sb: &sandbox::Sandbox, rt: &Path, quiet: bool) -> bool {
    let linked = || matches!(client::status(rt), Ok(CliReply::Status { app_connected: true, .. }));
    let page = match client::status(rt) {
        Ok(CliReply::Status { app_connected: true, display_page, .. }) => Some(display_page.unwrap_or_else(|| "/".into())),
        _ => None,
    };
    let port = lindroids_proto::server_tcp_port(sb.uid);
    let wanted = DISPLAY_PAGE.get().cloned();
    if let Some(page) = page {
        // On a desktop a linked page stays where it is; a new tab each time would only pile up.
        if sandbox::on_android() && !LAUNCHED_BY_PAGE.load(std::sync::atomic::Ordering::Relaxed) {
            // Exactly a page's URL: Firefox switches to that tab, Chrome reuses its tab, and an
            // installed app takes the URLs of its own page.
            sandbox::open_browser(&format!("http://127.0.0.1:{port}{}", wanted.unwrap_or(page)));
        }
        return true;
    }
    let secret = lindroids_server::load_or_create_secret();
    let how = match &wanted {
        // `run=0`: the program is being started here; the page must not start it again.
        Some(p) => sandbox::open_browser(&format!("http://127.0.0.1:{port}{p}#k={secret}&run=0")),
        None => sandbox::open_browser(&sandbox::display_url(sb.uid, Some(&secret))),
    };
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
        let url = sandbox::display_url(sb.uid, None);
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

fn cmd_stop() -> i32 {
    let rt = sandbox::runtime_dir();
    match client::request(&rt, &CliRequest::Stop, Duration::from_secs(5)) {
        Ok(_) => {
            println!("server stopped");
            0
        }
        Err(e) => fail(&e),
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
                println!("display     none: open {} in a browser, or `lindroids run <program>`", sandbox::display_url(sandbox::detect().uid, None));
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

fn cmd_pair() -> i32 {
    let sb = sandbox::detect();
    let rt = match ensure_server(&sb) {
        Ok(rt) => rt,
        Err(e) => return fail(&e),
    };
    if ensure_display(&sb, &rt, true) {
        println!("The browser is showing this server's windows ({}).", sandbox::display_url(sb.uid, None));
        return 0;
    }
    match client::status(&rt) {
        Ok(CliReply::Status { pairing_code: Some(c), .. }) => {
            println!("Open {} in a web browser on this device and enter this code:\n\n    {c}\n", sandbox::display_url(sb.uid, None));
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
    if ensure_display(&sb, &rt, false) {
        println!("Ready: windows show in the browser ({}). Try: lindroids run <program>", sandbox::display_url(sb.uid, None));
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
            println!("windows at  {}", sandbox::display_url(sb.uid, None));
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
