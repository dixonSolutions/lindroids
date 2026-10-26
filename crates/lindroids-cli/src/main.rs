//! `lindroids`: run Linux apps in Android windows, and export them to the home screen.

mod client;
mod debug;
mod desktop;
mod export;
mod flatpak;
mod run;
mod sandbox;

use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use lindroids_proto::{CliReply, CliRequest};

const HELP: &str = "\
lindroids — Linux apps in Android windows

USAGE
  lindroids run [options] <command> [args...]
  lindroids run [options] \"<command line>\"
  lindroids export [options] <command> [args...]
  lindroids list | remove <id>
  lindroids status | start | stop | pair | doctor | version

RUN OPTIONS
  --distro <name>     run inside a proot-distro distribution (Termux)
  --x11               run an X11-only program under a rootful Xwayland
  --name <name>       window and shortcut name
  --icon <file>       icon (PNG or SVG)
  -e, --env K=V       extra environment (repeatable)
  --touch pointer     treat touch as a mouse for this program's windows

EXPORT OPTIONS (as run, plus)
  --link              the shortcut asks Termux to run the program (no copy; Termux must be installed)
  --bundle            copy the program into the Lindroids app (default): works with Termux closed
  --id <id>           shortcut id (default: derived from the program)

A program name that looks like a Flatpak id (org.gnome.Calculator) runs that Flatpak.

The first time, the Lindroids app asks for the pairing code that `lindroids pair` shows.
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
        Some("doctor") => cmd_doctor(),
        Some("run") => run::cmd_run(&args[1..]),
        Some("export") => export::cmd_export(&args[1..]),
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

/// Make sure the app is linked; wake it if not. Never blocks for long: windows open whenever the
/// app connects. Returns whether it is connected.
pub fn ensure_app(sb: &sandbox::Sandbox, rt: &Path, quiet: bool) -> bool {
    if matches!(client::status(rt), Ok(CliReply::Status { app_connected: true, .. })) {
        return true;
    }
    let secret = lindroids_server::load_or_create_secret();
    let how = sandbox::wake_app(sb, &secret);
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(if how.is_some() { 6 } else { 2 }) {
        if let Ok(CliReply::Status { app_connected: true, .. }) = client::status(rt) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if !quiet {
        let code = match client::status(rt) {
            Ok(CliReply::Status { pairing_code: Some(c), .. }) => c,
            _ => "(see `lindroids pair`)".into(),
        };
        eprintln!("lindroids: the Lindroids app is not connected yet.");
        if how.is_none() {
            eprintln!("  This terminal cannot send intents, so open the Lindroids app yourself.");
        } else {
            eprintln!("  It was woken up; if it asks, approve it.");
        }
        eprintln!("  Pairing code: {code}");
        eprintln!("  Windows appear as soon as the app is connected.");
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
        Ok(CliReply::Status { build, app_connected, pairing_code, windows, clients, .. }) => {
            println!("server      {build} ({})", rt.display());
            println!("app         {}", if app_connected { "connected" } else { "not connected" });
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
    if ensure_app(&sb, &rt, true) {
        println!("The Lindroids app is connected.");
        return 0;
    }
    match client::status(&rt) {
        Ok(CliReply::Status { pairing_code: Some(c), .. }) => {
            println!("Open the Lindroids app and enter this code when it asks:\n\n    {c}\n");
            println!("(The app finds this server on its own while it is running.)");
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

fn cmd_pick_folder() -> i32 {
    let sb = sandbox::detect();
    let rt = match ensure_server(&sb) {
        Ok(rt) => rt,
        Err(e) => return fail(&e),
    };
    ensure_app(&sb, &rt, false);
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
        ("flatpak", sandbox::have("flatpak")),
        ("termux-am", sandbox::have("termux-am")),
    ] {
        println!("{:<11} {}", what, if ok { "found" } else { "not found" });
    }
    let rt = sandbox::runtime_dir();
    match client::status(&rt) {
        Ok(CliReply::Status { app_connected, .. }) => {
            println!("server      running, app {}", if app_connected { "connected" } else { "not connected" })
        }
        _ => println!("server      not running (`lindroids start`)"),
    }
    0
}
