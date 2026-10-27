//! `lindroids run`: start a Linux program with its windows going to the browser.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use base64::Engine;
use lindroids_proto::{CliRequest, TERMUX_PREFIX};

use crate::{client, desktop, flatpak, sandbox};

#[derive(Debug, Default, Clone)]
pub struct Opts {
    pub distro: Option<String>,
    pub x11: bool,
    pub name: Option<String>,
    pub icon: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub id: Option<String>,
    pub command: Vec<String>,
}

pub fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts::default();
    // `--opt=value` is `--opt value`; `--prootdistro` is `--distro`.
    let mut split = Vec::with_capacity(args.len());
    for (k, a) in args.iter().enumerate() {
        match a.split_once('=') {
            Some((opt, v)) if opt.starts_with("--") && takes_value(opt) || opt == "--prootdistro" => {
                split.push(opt.to_string());
                split.push(v.to_string());
            }
            // Everything from the command on is the command's.
            _ if !a.starts_with('-') => {
                split.extend(args[k..].iter().cloned());
                break;
            }
            _ => split.push(a.clone()),
        }
    }
    let args = &split[..];
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let mut value = || -> Result<String, String> {
            i += 1;
            args.get(i).cloned().ok_or_else(|| format!("{a} needs a value"))
        };
        match a {
            "--" => {
                o.command = args[i + 1..].to_vec();
                break;
            }
            "--distro" | "--prootdistro" => o.distro = Some(value()?),
            "--x11" => o.x11 = true,
            "--name" => o.name = Some(value()?),
            "--icon" => o.icon = Some(PathBuf::from(value()?)),
            "--id" => o.id = Some(value()?),
            "-e" | "--env" => {
                let kv = value()?;
                let (k, v) = kv.split_once('=').ok_or_else(|| format!("--env wants KEY=VALUE, not {kv:?}"))?;
                o.env.push((k.into(), v.into()));
            }
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ => {
                o.command = args[i..].to_vec();
                break;
            }
        }
        i += 1;
    }
    if o.command.is_empty() {
        return Err("what should I run? (lindroids run <command>)".into());
    }
    Ok(o)
}

/// Options of `run` (and `export`) followed by a value.
pub fn takes_value(opt: &str) -> bool {
    matches!(opt, "--distro" | "--prootdistro" | "--name" | "--icon" | "--id" | "-e" | "--env")
}

/// `lindroids run "foo --bar"` (one argument with spaces) is a shell command line.
pub fn is_shell_line(cmd: &[String]) -> bool {
    cmd.len() == 1 && cmd[0].chars().any(|c| c.is_whitespace() || "|&;<>()$`\"'*?".contains(c))
}

/// The program's name as a word: "foo" for "/usr/bin/foo --x" or "foo -x".
pub fn program_of(cmd: &[String]) -> String {
    let first = if is_shell_line(cmd) {
        cmd[0].split_whitespace().find(|w| !w.contains('=')).unwrap_or("").to_string()
    } else {
        cmd[0].clone()
    };
    Path::new(&first).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or(first)
}

/// A proot-distro distro's root filesystem: `containers/<name>/rootfs` since proot-distro 5,
/// `installed-rootfs/<name>` before.
pub fn distro_rootfs(distro: &str) -> PathBuf {
    let base = PathBuf::from(TERMUX_PREFIX).join("var/lib/proot-distro");
    let new = base.join("containers").join(distro).join("rootfs");
    if new.is_dir() {
        new
    } else {
        base.join("installed-rootfs").join(distro)
    }
}

/// Every installed proot-distro distro's root filesystem, sorted by name.
pub fn distro_rootfses() -> Vec<PathBuf> {
    let base = PathBuf::from(TERMUX_PREFIX).join("var/lib/proot-distro");
    let mut names: Vec<String> = ["containers", "installed-rootfs"]
        .iter()
        .filter_map(|d| std::fs::read_dir(base.join(d)).ok())
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names.dedup();
    names.iter().map(|n| distro_rootfs(n)).filter(|p| p.is_dir()).collect()
}

/// Where a distro keeps programs, relative to its root.
const DISTRO_BIN_DIRS: &[&str] = &["usr/local/bin", "usr/bin", "bin", "usr/local/sbin", "usr/sbin", "sbin", "usr/games"];

/// The distro whose root filesystem is `root` (`.../containers/<name>/rootfs` or
/// `.../installed-rootfs/<name>`).
fn distro_name(root: &Path) -> Option<String> {
    let dir = if root.file_name()? == "rootfs" { root.parent()? } else { root };
    Some(dir.file_name()?.to_string_lossy().into_owned())
}

/// The first of `roots` that has the program `prog` (a name, not a path).
fn distro_with(roots: &[PathBuf], prog: &str) -> Option<String> {
    roots
        .iter()
        .find(|root| DISTRO_BIN_DIRS.iter().any(|d| root.join(d).join(prog).symlink_metadata().is_ok()))
        .and_then(|root| distro_name(root))
}

/// In Termux, a program that is not installed in Termux but in a proot-distro distro (installed
/// with `apt` after `proot-distro login`) runs in that distro, as with `--distro <name>`.
pub fn pick_distro(o: &mut Opts) {
    if o.distro.is_some() || is_shell_line(&o.command) || sandbox::detect().kind != sandbox::Kind::Termux {
        return;
    }
    let first = &o.command[0];
    if first.contains('/') || flatpak::looks_like_id(first) || sandbox::have(first) {
        return;
    }
    if let Some(name) = distro_with(&distro_rootfses(), first) {
        eprintln!("lindroids: {first} is not in Termux but in the proot-distro distro {name}: running it there (--distro {name})");
        o.distro = Some(name);
    }
}

/// Whether the program (looked up on PATH, or in the distro's bin directories) only speaks X11.
fn x11_only_program(o: &Opts, argv: &[String]) -> bool {
    if is_shell_line(argv) {
        return false;
    }
    let prog = &argv[0];
    let path = match &o.distro {
        Some(d) if !prog.contains('/') => {
            let root = distro_rootfs(d);
            DISTRO_BIN_DIRS.iter().map(|b| root.join(b).join(prog)).find(|p| p.is_file())
        }
        Some(d) => Some(distro_rootfs(d).join(prog.trim_start_matches('/'))),
        None => sandbox::which(prog),
    };
    path.and_then(|p| crate::elf::needed(&std::fs::canonicalize(&p).unwrap_or(p))).map_or(false, |libs| crate::elf::x11_only(&libs))
}

/// What the user wants to run, resolved.
pub enum Target {
    /// argv in the current sandbox (or a distro).
    Program(Vec<String>),
    Flatpak(flatpak::App, Vec<String>),
}

pub fn resolve(o: &Opts) -> Result<Target, String> {
    let first = &o.command[0];
    if !is_shell_line(&o.command) && flatpak::looks_like_id(first) {
        let root = o.distro.as_deref().map(distro_rootfs).unwrap_or_else(|| PathBuf::from("/"));
        match flatpak::find(&root, first) {
            Some(app) => return Ok(Target::Flatpak(app, o.command[1..].to_vec())),
            None if !sandbox::have(first) => {
                return Err(format!(
                    "{first} looks like a Flatpak, but it is not installed{}",
                    o.distro.as_ref().map(|d| format!(" in {d}")).unwrap_or_default()
                ))
            }
            None => {}
        }
    }
    Ok(Target::Program(o.command.clone()))
}

/// Build the command that runs the program with its windows going to `rt` (the runtime dir as
/// this process sees it).
pub fn build_command(o: &Opts, target: &Target, rt: &Path) -> Result<Command, String> {
    match target {
        Target::Flatpak(app, args) => flatpak::command(app, args, o, rt),
        Target::Program(argv) => {
            // Inside a proot-distro login, Termux's $PREFIX/tmp is the distro's /tmp.
            let rt_inside = match &o.distro {
                Some(_) => PathBuf::from("/tmp").join(rt.file_name().unwrap_or_default()),
                None => rt.to_path_buf(),
            };
            let mut env = sandbox::client_env(&rt_inside);
            env.extend(sandbox::integration_env(rt, &rt_inside));
            let base = if o.distro.is_some() { "/usr/local/share:/usr/share".to_string() } else { sandbox::default_data_dirs() };
            if let Some(d) = sandbox::data_dirs_with_runtime(rt, &rt_inside, &base) {
                env.push(("XDG_DATA_DIRS".into(), d));
            }
            // First on PATH: xdg-open that hands links and files to Android, and under proot (a
            // distro from Termux, or the CLI itself inside one) a bwrap that admits it cannot
            // work (lindroids_proto::BWRAP_UNDER_PROOT).
            let under_proot = o.distro.is_some() || sandbox::detect().kind == sandbox::Kind::Proot;
            let mut first: Vec<String> = sandbox::write_open_helpers(rt, &rt_inside).map(|p| p.display().to_string()).into_iter().collect();
            if under_proot && lindroids_proto::write_proot_bin(&rt.join("proot-bin")).is_ok() {
                first.push(rt_inside.join("proot-bin").display().to_string());
            }
            if !first.is_empty() {
                let path = match &o.distro {
                    Some(_) => lindroids_proto::proot_distro_path(&first.join(":")),
                    None => format!("{}:{}", first.join(":"), std::env::var("PATH").unwrap_or_default()),
                };
                env.push(("PATH".into(), path));
            }
            if under_proot {
                if let Some(name) = sandbox::write_compat_preload(&rt.join("proot-bin")) {
                    let lib = rt_inside.join("proot-bin").join(name).display().to_string();
                    let own = if o.distro.is_some() { None } else { std::env::var("LD_PRELOAD").ok() };
                    env.push(("LD_PRELOAD".into(), sandbox::preload_with(&lib, own.as_deref())));
                }
            }
            let (x11_env, integrated_x11) = sandbox::x11_env(rt);
            env.extend(x11_env);
            if o.x11 {
                env.retain(|(k, _)| !matches!(k.as_str(), "GDK_BACKEND" | "QT_QPA_PLATFORM" | "SDL_VIDEODRIVER" | "WAYLAND_DISPLAY"));
                env.push(("GDK_BACKEND".into(), "x11".into()));
                env.push(("QT_QPA_PLATFORM".into(), "xcb".into()));
                env.push(("SDL_VIDEODRIVER".into(), "x11".into()));
            }
            env.extend(o.env.iter().cloned());
            let shell_argv: Vec<String> = if is_shell_line(argv) {
                vec!["sh".into(), "-c".into(), argv[0].clone()]
            } else {
                argv.clone()
            };
            // --x11 without the integrated display: a rootful Xwayland of its own, with our
            // window manager (one Android window for the whole X screen).
            let inner: Vec<String> = if o.x11 && !integrated_x11 {
                let tmp = if o.distro.is_some() || !Path::new(TERMUX_PREFIX).exists() { "/tmp".to_string() } else { format!("{TERMUX_PREFIX}/tmp") };
                let n = free_display(&x11_dir());
                let mut v = vec!["sh".into(), "-c".into(), X11_WRAPPER.replace("@TMP@", &tmp).replace("@RT@", &rt_inside.display().to_string()), "lindroids-x11".into(), n.to_string()];
                v.extend(shell_argv);
                v
            } else {
                shell_argv
            };
            match &o.distro {
                Some(d) => {
                    let mut c = Command::new("proot-distro");
                    // `env` below sets what the program needs; DISPLAY must not leak in from here.
                    c.env_remove("DISPLAY");
                    c.args(["login", d, "--shared-tmp"]);
                    // The runtime dir also at the path the server knows it by, so files the portal
                    // hands out (picked documents) are where it says.
                    c.arg("--bind").arg(format!("{}:{}", rt.display(), rt.display()));
                    c.arg("--");
                    c.arg("env");
                    for (k, v) in &env {
                        c.arg(format!("{k}={v}"));
                    }
                    c.args(&inner);
                    Ok(c)
                }
                None => {
                    let mut c = Command::new(&inner[0]);
                    c.args(&inner[1..]);
                    if !env.iter().any(|(k, _)| k == "DISPLAY") {
                        c.env_remove("DISPLAY");
                    }
                    c.envs(env);
                    Ok(c)
                }
            }
        }
    }
}

/// Where X sockets are, as this process sees them (a proot-distro's /tmp is Termux's tmp).
pub fn x11_dir() -> PathBuf {
    if Path::new(TERMUX_PREFIX).exists() {
        PathBuf::from(TERMUX_PREFIX).join("tmp/.X11-unix")
    } else {
        PathBuf::from("/tmp/.X11-unix")
    }
}

fn free_display(dir: &Path) -> u32 {
    (1..64).find(|n| !dir.join(format!("X{n}")).exists() && !dir.parent().unwrap_or(dir).join(format!(".X{n}-lock")).exists()).unwrap_or(1)
}

/// Starts a rootful Xwayland on display $1, then the program on it.
const X11_WRAPPER: &str = r#"
n=$1
shift
Xwayland ":$n" -noreset -nolisten tcp >"@RT@/xwayland-$n.log" 2>&1 &
x=$!
i=0
while [ ! -e "@TMP@/.X11-unix/X$n" ] && [ $i -lt 100 ]; do sleep 0.1; i=$((i+1)); done
DISPLAY=":$n" "$@"
rc=$?
kill $x 2>/dev/null
exit $rc
"#;

pub fn cmd_run(args: &[String]) -> i32 {
    cmd_run_launched(args, None)
}

/// Where `launch` notes the pid of an exported program it started, so that tapping its icon again
/// brings back its window instead of starting another.
pub fn launched_marker(id: &str) -> PathBuf {
    sandbox::runtime_dir().join("launched").join(format!("{id}.pid"))
}

/// `run`, for the exported program `launched` (`lindroids launch <id>`) if given.
pub fn cmd_run_launched(args: &[String], launched: Option<&str>) -> i32 {
    let mut o = match parse(args) {
        Ok(o) => o,
        Err(e) => return crate::fail(&e),
    };
    pick_distro(&mut o);
    let sb = sandbox::detect();
    let target = match resolve(&o) {
        Ok(t) => t,
        Err(e) => return crate::fail(&e),
    };
    let rt = match crate::ensure_server(&sb) {
        Ok(rt) => rt,
        Err(e) => return crate::fail(&e),
    };
    // On Android every run gets a window of its own (a Custom Tab for `?run=<pid>`, opened once
    // the program has a pid); elsewhere, and without a browser that has Custom Tabs, the shared page.
    let own_window = sandbox::on_android();
    if !own_window {
        crate::ensure_display(&sb, &rt, false);
    }
    if let Target::Program(argv) = &target {
        if !o.x11 && sandbox::x11_display(&rt).is_none() && x11_only_program(&o, argv) {
            eprintln!(
                "lindroids: {} is an X11 program, and X11 is not set up here yet: run `lindroids setup x11`",
                program_of(argv)
            );
            eprintln!("  (it installs Xwayland), or try `lindroids run --x11 {}`.", argv.join(" "));
        }
    }
    let (name, icon) = identify(&o, &target);
    let mut cmd = match build_command(&o, &target, &rt) {
        Ok(c) => c,
        Err(e) => return crate::fail(&e),
    };
    // Under proot (a distro, a Flatpak) a program killed by a signal shows only in proot's own
    // report on stderr, and some proot builds exit 0 for SIGTRAP: watch for it.
    let under_proot = o.distro.is_some() || matches!(target, Target::Flatpak(..));
    if under_proot {
        cmd.stderr(std::process::Stdio::piped());
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return crate::fail(&format!("cannot start {}: {e}", o.command[0])),
    };
    let signal_watch = child.stderr.take().map(|e| std::thread::spawn(move || forward_and_find_signal(e)));
    if o.x11 && sandbox::x11_display(&rt).is_none() {
        // The display the wrapper was given is the last free one we picked: find its socket.
        let dir = x11_dir();
        let display = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).skip_while(|a| a != "lindroids-x11").nth(1);
        if let Some(n) = display {
            std::thread::spawn(move || {
                let sock = dir.join(format!("X{n}"));
                for _ in 0..200 {
                    if sock.exists() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                if let Err(e) = crate::x11wm::run(&sock) {
                    eprintln!("lindroids: X11 window manager: {e}");
                }
            });
        }
    }
    let marker = launched.map(launched_marker);
    if let Some(m) = &marker {
        let _ = std::fs::create_dir_all(m.parent().unwrap());
        let _ = std::fs::write(m, child.id().to_string());
    }
    let icon_b64 = Some(base64::engine::general_purpose::STANDARD.encode(icon));
    let register = |window: bool| {
        let req = CliRequest::Launch { pid: child.id(), name: name.clone(), icon_png_b64: icon_b64.clone(), window };
        let _ = client::request(&rt, &req, Duration::from_secs(3));
    };
    register(own_window);
    if own_window && !crate::open_run_window(&sb, &rt, child.id()) {
        register(false);
        crate::ensure_display(&sb, &rt, false);
    }
    let waited = child.wait();
    if let Some(m) = &marker {
        let _ = std::fs::remove_file(m);
    }
    let code = match waited {
        // Killed (its window closed, Ctrl+C): 128 + the signal, as a shell reports it.
        Ok(st) => st.code().unwrap_or_else(|| 128 + std::os::unix::process::ExitStatusExt::signal(&st).unwrap_or(0)),
        Err(e) => return crate::fail(&e.to_string()),
    };
    match signal_watch.and_then(|t| t.join().ok()).flatten() {
        Some(sig) => {
            eprintln!("lindroids: {} crashed (signal {sig}, {})", program_of(&o.command), signal_name(sig));
            128 + sig
        }
        None => code,
    }
}

/// Copy a child's stderr to ours as it comes, and return the signal proot reports the first
/// program died of ("proot info: vpid 1: terminated with signal 5"), if it does.
fn forward_and_find_signal(mut from: impl std::io::Read) -> Option<i32> {
    use std::io::Write;
    let mut found = None;
    let mut line = Vec::new();
    let mut buf = [0u8; 8192];
    let mut err = std::io::stderr();
    loop {
        let n = match from.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let _ = err.write_all(&buf[..n]);
        for &b in &buf[..n] {
            if b == b'\n' {
                found = found.or_else(|| proot_signal(&String::from_utf8_lossy(&line)));
                line.clear();
            } else if line.len() < 512 {
                line.push(b);
            }
        }
    }
    found.or_else(|| proot_signal(&String::from_utf8_lossy(&line)))
}

fn proot_signal(line: &str) -> Option<i32> {
    let rest = line.trim().strip_prefix("proot info: vpid 1: terminated with signal ")?;
    rest.split_whitespace().next()?.parse().ok().filter(|s| (1..64).contains(s))
}

fn signal_name(sig: i32) -> &'static str {
    match sig {
        4 => "illegal instruction",
        5 => "trace trap: a failed check or assertion",
        6 => "aborted",
        7 => "bus error",
        8 => "arithmetic error",
        9 => "killed",
        11 => "segmentation fault",
        15 => "terminated",
        _ => "signal",
    }
}

/// Name and PNG icon for what is being run.
pub fn identify(o: &Opts, target: &Target) -> (String, Vec<u8>) {
    match target {
        Target::Flatpak(app, _) => flatpak::identify(app, o),
        Target::Program(argv) => {
            let root = o.distro.as_deref().map(distro_rootfs).unwrap_or_else(|| PathBuf::from("/"));
            let (name, icon, _) = desktop::identify(&root, &program_of(argv), o.name.as_deref(), o.icon.as_deref());
            (name, icon)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn finds_the_distro_that_has_a_program() {
        let base = std::env::temp_dir().join(format!("lindroids-distro-with-{}", std::process::id()));
        let new = base.join("containers/debian/rootfs");
        let old = base.join("installed-rootfs/alpine");
        std::fs::create_dir_all(new.join("usr/bin")).unwrap();
        std::fs::create_dir_all(old.join("usr/bin")).unwrap();
        std::fs::write(new.join("usr/bin/xournalpp"), "").unwrap();
        // A dangling absolute symlink (it points into the distro) still counts.
        std::os::unix::fs::symlink("/usr/lib/foo/foo", old.join("usr/bin/foo")).unwrap();
        let roots = vec![old.clone(), new.clone()];
        assert_eq!(distro_with(&roots, "xournalpp").as_deref(), Some("debian"));
        assert_eq!(distro_with(&roots, "foo").as_deref(), Some("alpine"));
        assert_eq!(distro_with(&roots, "gimp"), None);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn parses_options_then_command() {
        let o = parse(&s(&["--distro", "debian", "--name", "Ed", "gedit", "--new-window"])).unwrap();
        assert_eq!(o.distro.as_deref(), Some("debian"));
        assert_eq!(o.command, s(&["gedit", "--new-window"]));
    }

    #[test]
    fn proot_signal_reports() {
        assert_eq!(proot_signal("proot info: vpid 1: terminated with signal 5"), Some(5));
        assert_eq!(proot_signal("proot info: vpid 2: terminated with signal 11"), None);
        assert_eq!(proot_signal("some program output"), None);
        let found = forward_and_find_signal(&b"x\nproot info: vpid 1: terminated with signal 11\n"[..]);
        assert_eq!(found, Some(11));
    }

    #[test]
    fn prootdistro_and_equals_forms() {
        for a in [s(&["--prootdistro=debian", "gedit", "--x=1"]), s(&["--prootdistro", "debian", "gedit", "--x=1"]), s(&["--distro=debian", "gedit", "--x=1"])] {
            let o = parse(&a).unwrap();
            assert_eq!(o.distro.as_deref(), Some("debian"));
            assert_eq!(o.command, s(&["gedit", "--x=1"]));
        }
    }

    #[test]
    fn shell_lines() {
        assert!(is_shell_line(&s(&["foot -e htop"])));
        assert!(!is_shell_line(&s(&["foot", "-e", "htop"])));
        assert_eq!(program_of(&s(&["LANG=C /usr/bin/foot -e htop"])), "foot");
    }
}
