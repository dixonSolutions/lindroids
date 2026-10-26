//! `lindroids setup x11`: install what the integrated X display and the desktop integration need
//! (Xwayland; dbus-daemon for the portal), with the package manager of the environment Lindroids
//! runs in. Xwayland comes from the distribution everywhere. Lindroids
//! ships xwayland-satellite itself where the distribution has none (Termux, Debian, Ubuntu), and
//! prefers the distribution's own package where there is one (Fedora, Arch, Alpine, openSUSE).

use std::io::{BufRead, IsTerminal, Write};
use std::process::Command;
use std::time::Duration;

use lindroids_proto::{CliReply, CliRequest};

use crate::{client, fail, sandbox};

struct Manager {
    name: &'static str,
    /// Commands to run, in order.
    steps: Vec<Vec<String>>,
}

fn words(s: &str) -> Vec<String> {
    s.split_whitespace().map(String::from).collect()
}

/// Root, or sudo when there is one and we are not root.
fn as_root(mut cmd: Vec<String>) -> Vec<String> {
    if unsafe { libc::geteuid() } != 0 && sandbox::have("sudo") {
        cmd.insert(0, "sudo".into());
    }
    cmd
}

/// Whether the distribution's repositories have `pkg` (only asked where it varies by release).
fn apt_has(pkg: &str) -> bool {
    Command::new("apt-cache")
        .args(["policy", pkg])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().any(|l| l.trim().starts_with("Candidate:") && !l.contains("(none)")))
        .unwrap_or(false)
}

fn manager(sb: &sandbox::Sandbox) -> Option<Manager> {
    let satellite_shipped = lindroids_server::x11::find_satellite().is_some();
    if sb.kind == sandbox::Kind::Termux {
        // Xwayland is in Termux's x11 repository; satellite comes with Lindroids.
        return Some(Manager {
            name: "pkg",
            steps: vec![words("pkg install -y x11-repo"), words("pkg install -y xwayland libxcb xcb-util-cursor dbus")],
        });
    }
    if sandbox::have("apt-get") {
        let mut pkgs = words("xwayland libxcb1 libxcb-cursor0 dbus");
        if !satellite_shipped && apt_has("xwayland-satellite") {
            pkgs.push("xwayland-satellite".into());
        }
        let mut install = words("apt-get install -y");
        install.extend(pkgs);
        return Some(Manager { name: "apt", steps: vec![as_root(words("apt-get update")), as_root(install)] });
    }
    // (manager, install command without weak/recommended extras, Xwayland, satellite)
    let table: [(&str, &str, &str, &str, &str); 4] = [
        ("dnf", "dnf install -y --setopt=install_weak_deps=False", "xorg-x11-server-Xwayland", "dbus-daemon", "xwayland-satellite"),
        ("pacman", "pacman -S --needed --noconfirm", "xorg-xwayland", "dbus", "xwayland-satellite"),
        ("apk", "apk add", "xwayland", "dbus", "xwayland-satellite"),
        ("zypper", "zypper --non-interactive install --no-recommends", "xwayland", "dbus-1", "xwayland-satellite"),
    ];
    for (bin, cmd, xwayland, dbus, satellite) in table {
        if sandbox::have(bin) {
            let mut c = words(cmd);
            c.push(xwayland.into());
            c.push(dbus.into());
            // The distro's satellite only where Lindroids' own cannot run (a musl distro).
            if !satellite_shipped {
                c.push(satellite.into());
            }
            return Some(Manager { name: bin, steps: vec![as_root(c)] });
        }
    }
    None
}

fn confirm(question: &str) -> bool {
    if !std::io::stdin().is_terminal() {
        return false;
    }
    print!("{question} [Y/n] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line).is_err() {
        return false;
    }
    let a = line.trim().to_lowercase();
    a.is_empty() || a == "y" || a == "yes"
}

/// What the integrated X display needs, and what is missing.
pub fn missing() -> Vec<&'static str> {
    let mut v = Vec::new();
    if !sandbox::have("Xwayland") {
        v.push("Xwayland");
    }
    if lindroids_server::x11::find_satellite().is_none() {
        v.push("xwayland-satellite");
    }
    if !sandbox::have("dbus-daemon") {
        v.push("dbus-daemon");
    }
    v
}

pub fn cmd_setup_x11(args: &[String]) -> i32 {
    let yes = args.iter().any(|a| a == "-y" || a == "--yes");
    let sb = sandbox::detect();
    let need = missing();
    if need.is_empty() {
        println!("X11 is ready: Xwayland and xwayland-satellite are installed.");
    } else {
        let Some(m) = manager(&sb) else {
            return fail(&format!("missing {}; install them with this system's package manager", need.join(" and ")));
        };
        println!("Missing for X11 programs: {}. With {}:", need.join(", "), m.name);
        for s in &m.steps {
            println!("  {}", s.join(" "));
        }
        if !yes && !confirm("Run these now?") {
            println!("Nothing installed. (`lindroids setup x11 --yes` installs without asking.)");
            return 1;
        }
        for s in &m.steps {
            let ok = Command::new(&s[0]).args(&s[1..]).status().map_or(false, |st| st.success());
            if !ok {
                return fail(&format!("`{}` failed", s.join(" ")));
            }
        }
        let still = missing();
        if !still.is_empty() {
            return fail(&format!("still missing: {}", still.join(", ")));
        }
        println!("Installed.");
    }
    restart_for_x11()
}

/// The server picks up X11 when it starts: restart it if it runs without, unless windows are open.
fn restart_for_x11() -> i32 {
    let rt = sandbox::runtime_dir();
    if sandbox::x11_display(&rt).is_some() {
        println!("X11 programs run on display {}.", sandbox::x11_display(&rt).unwrap_or_default());
        return 0;
    }
    match client::status(&rt) {
        Ok(CliReply::Status { windows, .. }) if !windows.is_empty() => {
            println!("The server is running with windows open; X11 starts with it next time (`lindroids stop`, then run again).");
            0
        }
        Ok(_) => {
            let _ = client::request(&rt, &CliRequest::Stop, Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(300));
            match crate::ensure_server(&sandbox::detect()) {
                Ok(rt) => match sandbox::x11_display(&rt) {
                    Some(d) => {
                        println!("X11 programs run on display {d}.");
                        0
                    }
                    None => fail(&format!("the server started without X11; see {}", rt.join("server.log").display())),
                },
                Err(e) => fail(&e),
            }
        }
        Err(_) => {
            println!("X11 starts with the server (`lindroids start`).");
            0
        }
    }
}
