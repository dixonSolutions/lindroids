//! `lindroids run`: start a Linux program with its windows going to Android.

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
    pub link: bool,
    pub bundle: bool,
    pub id: Option<String>,
    pub command: Vec<String>,
}

pub fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts::default();
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
            "--distro" => o.distro = Some(value()?),
            "--x11" => o.x11 = true,
            "--name" => o.name = Some(value()?),
            "--icon" => o.icon = Some(PathBuf::from(value()?)),
            "--id" => o.id = Some(value()?),
            "--link" => o.link = true,
            "--bundle" => o.bundle = true,
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

pub fn distro_rootfs(distro: &str) -> PathBuf {
    PathBuf::from(TERMUX_PREFIX).join("var/lib/proot-distro/installed-rootfs").join(distro)
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
            let inner: Vec<String> = if o.x11 {
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
                    c.args(["login", d, "--shared-tmp", "--"]);
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
    let o = match parse(args) {
        Ok(o) => o,
        Err(e) => return crate::fail(&e),
    };
    let sb = sandbox::detect();
    let target = match resolve(&o) {
        Ok(t) => t,
        Err(e) => return crate::fail(&e),
    };
    let rt = match crate::ensure_server(&sb) {
        Ok(rt) => rt,
        Err(e) => return crate::fail(&e),
    };
    crate::ensure_app(&sb, &rt, false);
    let (name, icon) = identify(&o, &target);
    let mut cmd = match build_command(&o, &target, &rt) {
        Ok(c) => c,
        Err(e) => return crate::fail(&e),
    };
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return crate::fail(&format!("cannot start {}: {e}", o.command[0])),
    };
    if o.x11 {
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
    let _ = client::request(
        &rt,
        &CliRequest::Launch { pid: child.id(), name, icon_png_b64: Some(base64::engine::general_purpose::STANDARD.encode(icon)) },
        Duration::from_secs(3),
    );
    match child.wait() {
        Ok(st) => st.code().unwrap_or(1),
        Err(e) => crate::fail(&e.to_string()),
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
    fn parses_options_then_command() {
        let o = parse(&s(&["--distro", "debian", "--name", "Ed", "gedit", "--new-window"])).unwrap();
        assert_eq!(o.distro.as_deref(), Some("debian"));
        assert_eq!(o.command, s(&["gedit", "--new-window"]));
    }

    #[test]
    fn shell_lines() {
        assert!(is_shell_line(&s(&["foot -e htop"])));
        assert!(!is_shell_line(&s(&["foot", "-e", "htop"])));
        assert_eq!(program_of(&s(&["LANG=C /usr/bin/foot -e htop"])), "foot");
    }
}
