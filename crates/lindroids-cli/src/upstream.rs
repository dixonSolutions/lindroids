//! Development builds, and the way back from one to an official ("upstream") release.
//!
//! A build from a checkout is a development build: scripts/build.sh marks it `+<commit>-dev` (or
//! `-dev-dirty`), a plain `cargo build` `+dev`; releases carry the commit alone. Every command says
//! so on stderr when it is a terminal, and `lindroids dev-build` tells how to go back.
//!
//! `lindroids upstream switch [latest|<version>]` asks for the release (Enter: the latest), asks
//! to confirm, and runs scripts/use-release.sh, built into this binary: it installs the release
//! through whatever installed lindroids (a package manager, or a static install).

use std::io::{BufRead, IsTerminal, Write};
use std::process::Command;

use crate::{fail, update};

/// scripts/use-release.sh, the one place that knows how to install a given release.
const USE_RELEASE: &str = include_str!("../../../scripts/use-release.sh");

/// The build id after the version: the commit for a release, `<commit>-dev[-dirty]` or `dev` for
/// a development build.
fn build_id(build: &str) -> &str {
    build.split_once('+').map_or("", |(_, id)| id)
}

pub fn is_dev(build: &str) -> bool {
    let id = build_id(build);
    id == "dev" || id.ends_with("-dev") || id.ends_with("-dev-dirty")
}

/// Whether this binary is a development build.
pub fn dev() -> bool {
    is_dev(lindroids_server::BUILD)
}

/// One line on stderr before a command of a development build, when someone is there to read it
/// (not in programs' logs). LINDROIDS_DEV_NOTICE=0 leaves it out.
pub fn notice(command: Option<&str>) {
    if !dev() || std::env::var("LINDROIDS_DEV_NOTICE").as_deref() == Ok("0") || !std::io::stderr().is_terminal() {
        return;
    }
    // Commands that say it themselves, or that only the server and scripts run.
    if matches!(command, Some("version" | "--version" | "-V" | "dev-build" | "upstream" | "server" | "bwrap" | "debug-app")) {
        return;
    }
    eprintln!(
        "lindroids {}: development build, not a release (`lindroids dev-build` tells how to go back)",
        lindroids_server::BUILD
    );
}

const DEV_HELP: &str = "\
To go back to an official release:

  lindroids upstream switch              asks which release (Enter: the latest), then to confirm
  lindroids upstream switch latest       the latest release
  lindroids upstream switch <version>    a given release, e.g. 0.1.130
  lindroids upstream list                the releases, newest first
  (--yes: no questions)

The release is installed over this build by whatever installed it (pkg/apt, pacman, dnf, apk, or
the static binary), and the usual upgrades take it on from there. An idle server is stopped, so
the next program starts the release.
";

/// `lindroids dev-build [help]`.
pub fn cmd_dev_build(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        None | Some("help" | "-h" | "--help") => {}
        Some(other) => return fail(&format!("dev-build {other}: unknown (`lindroids dev-build help`)")),
    }
    let build = lindroids_server::BUILD;
    if dev() {
        println!("This lindroids ({build}) is a development build: made from a checkout (scripts/build.sh");
        println!("or cargo), not an official release.");
        println!();
        print!("{DEV_HELP}");
    } else {
        println!("This lindroids ({build}) is an official release, not a development build.");
        println!("`lindroids update` brings the latest; `lindroids upstream switch <version>` another one.");
    }
    0
}

/// `lindroids upstream switch|list`.
pub fn cmd_upstream(args: &[String]) -> i32 {
    let yes = args.iter().any(|a| a == "--yes" || a == "-y");
    let rest: Vec<&str> = args.iter().map(String::as_str).filter(|a| !matches!(*a, "--yes" | "-y")).collect();
    match rest.as_slice() {
        ["switch"] => switch(None, yes),
        ["switch", v] => switch(Some(v), yes),
        ["list"] => use_release(&["--list"]),
        [] | ["help" | "-h" | "--help"] => {
            print!("{DEV_HELP}");
            0
        }
        _ => fail("usage: lindroids upstream switch [latest|<version>] [--yes] | list"),
    }
}

/// "0.1.130" from "0.1.130" or "v0.1.130"; None for anything else.
fn version_arg(v: &str) -> Option<String> {
    let v = v.trim().trim_start_matches('v');
    let parts: Vec<&str> = v.split('.').collect();
    (parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))).then(|| v.to_string())
}

/// A line typed at the terminal, trimmed (None at the end of input).
fn ask(question: &str) -> Option<String> {
    print!("{question}");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    match std::io::stdin().lock().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line.trim().to_string()),
    }
}

fn switch(want: Option<&str>, yes: bool) -> i32 {
    let build = lindroids_server::BUILD;
    let interactive = std::io::stdin().is_terminal();
    let latest = update::latest_version();
    let kind = if dev() { "development build" } else { "release" };
    println!("Installed:      lindroids {build} ({kind})");
    match &latest {
        Ok(v) => println!("Latest release: {v}"),
        Err(e) => eprintln!("lindroids: the latest release is not known ({e})"),
    }

    // Which release: as given, else asked (Enter: the latest), else the latest.
    let mut want = want.map(str::to_string);
    if want.is_none() && interactive && !yes {
        match ask("Release to switch to (a version like 0.1.130; Enter: the latest): ") {
            Some(a) => want = Some(a),
            None => return fail("nothing chosen"),
        }
    }
    let want = want.filter(|w| !w.is_empty() && w != "latest");
    let version = match want {
        Some(w) => match version_arg(&w) {
            Some(v) => v,
            None => return fail(&format!("{w:?} is not a version like 0.1.130 (`lindroids upstream list`)")),
        },
        None => match latest {
            Ok(v) => v,
            Err(e) => return fail(&e),
        },
    };
    if !dev() && build.split('+').next() == Some(version.as_str()) {
        println!("Release {version} is installed already.");
        return 0;
    }

    if !yes {
        if !interactive {
            return fail(&format!("not asked at a terminal: `lindroids upstream switch {version} --yes` switches without asking"));
        }
        let a = ask(&format!("Replace lindroids {build} with release {version}? [y/N] ")).unwrap_or_default().to_lowercase();
        if a != "y" && a != "yes" {
            println!("Left as it is.");
            return 0;
        }
    }
    use_release(&[&version])
}

/// Runs the built-in scripts/use-release.sh with `args`, here and now.
fn use_release(args: &[&str]) -> i32 {
    // bash -c keeps the terminal as its stdin (apt and sudo may ask); $0 names it in its messages.
    match Command::new("bash").arg("-c").arg(USE_RELEASE).arg("use-release.sh").args(args).status() {
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => fail(&format!("bash: {e} (switching to a release runs a bash script)")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_builds() {
        assert!(is_dev("0.1.0+dev"));
        assert!(is_dev("0.1.140+abc1234-dev"));
        assert!(is_dev("0.1.140+abc1234-dev-dirty"));
        assert!(!is_dev("0.1.139+abc1234"));
        assert!(!is_dev("0.1.139"));
    }

    #[test]
    fn versions() {
        assert_eq!(version_arg("0.1.130").as_deref(), Some("0.1.130"));
        assert_eq!(version_arg(" v0.1.130 ").as_deref(), Some("0.1.130"));
        assert_eq!(version_arg("0.1"), None);
        assert_eq!(version_arg("0.1.x"), None);
        assert_eq!(version_arg("latest"), None);
    }

    #[test]
    fn script_is_built_in() {
        assert!(USE_RELEASE.starts_with("#!/usr/bin/env bash"));
        assert!(USE_RELEASE.contains("--list"));
    }
}
