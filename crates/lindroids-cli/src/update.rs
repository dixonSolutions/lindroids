//! `lindroids update`: bring the CLI, its xwayland-satellite and the Android app to the newest
//! release, the way the CLI was installed.
//!
//! - From the APT repository (Termux, Debian/Ubuntu): `apt-get update` and an upgrade of the
//!   `lindroids` package, which carries the matching APK.
//! - A static install: the published install script, which checks the release's SHA256SUMS, into
//!   the same directory.
//!
//! Then an idle server is restarted on the new binary, and `lindroids setup` brings the app to the
//! same build (or says it already is).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use lindroids_proto::{CliReply, CliRequest};

use crate::{client, fail, sandbox};

const DEFAULT_REPO_URL: &str = "https://dixonsolutions.github.io/lindroids";

fn repo_url() -> String {
    std::env::var("LINDROIDS_REPO_URL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| DEFAULT_REPO_URL.into()).trim_end_matches('/').to_string()
}

/// "0.1.12" from "0.1.12+abc1234"; dev builds have no comparable version.
fn version_of(build: &str) -> &str {
    build.split('+').next().unwrap_or(build)
}

fn parse(v: &str) -> Option<Vec<u64>> {
    v.trim().split('.').map(|p| p.parse().ok()).collect()
}

/// Whether `latest` is newer than `current` (dev builds are always older).
pub fn newer(latest: &str, current_build: &str) -> bool {
    if current_build.ends_with("+dev") {
        return true;
    }
    match (parse(latest), parse(version_of(current_build))) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

fn fetch(url: &str) -> Result<String, String> {
    let try_cmd = |cmd: &str, args: &[&str]| -> Option<String> {
        let out = Command::new(cmd).args(args).arg(url).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    try_cmd("curl", &["-fsSL", "--max-time", "30"])
        .or_else(|| try_cmd("wget", &["-q", "-T", "30", "-O", "-"]))
        .ok_or_else(|| format!("cannot fetch {url} (needs curl or wget, and the network)"))
}

pub fn latest_version() -> Result<String, String> {
    let v = fetch(&format!("{}/VERSION", repo_url()))?;
    let v = v.trim().to_string();
    parse(&v).map(|_| v.clone()).ok_or_else(|| format!("unexpected VERSION {v:?}"))
}

#[derive(Debug, PartialEq)]
enum Method {
    /// dpkg owns the binary: upgrade the package (Termux or Debian/Ubuntu).
    Apt { termux: bool },
    /// A static install in this directory.
    Static(PathBuf),
}

fn method() -> Result<Method, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let sb = sandbox::detect();
    if sandbox::have("dpkg-query") {
        let owned = Command::new("dpkg-query")
            .args(["-S", &exe.display().to_string()])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .map_or(false, |o| o.status.success() && String::from_utf8_lossy(&o.stdout).starts_with("lindroids:"));
        if owned {
            return Ok(Method::Apt { termux: sb.kind == sandbox::Kind::Termux });
        }
    }
    let dir = exe.parent().ok_or("where is this binary?")?.to_path_buf();
    Ok(Method::Static(dir))
}

fn run(cmd: &[String]) -> Result<(), String> {
    println!("+ {}", cmd.join(" "));
    let ok = Command::new(&cmd[0]).args(&cmd[1..]).status().map_or(false, |s| s.success());
    ok.then_some(()).ok_or_else(|| format!("`{}` failed", cmd.join(" ")))
}

fn root(cmd: &str) -> Vec<String> {
    let mut v: Vec<String> = cmd.split_whitespace().map(String::from).collect();
    if unsafe { libc::geteuid() } != 0 && sandbox::have("sudo") {
        v.insert(0, "sudo".into());
    }
    v
}

fn update_cli(m: &Method) -> Result<(), String> {
    match m {
        Method::Apt { termux: true } => {
            run(&["apt-get".into(), "update".into()])?;
            run(&["apt-get".into(), "install".into(), "--only-upgrade".into(), "-y".into(), "lindroids".into()])
        }
        Method::Apt { termux: false } => {
            run(&root("apt-get update"))?;
            run(&root("apt-get install --only-upgrade -y lindroids"))
        }
        Method::Static(dir) => {
            let script = fetch(&format!("{}/install.sh", repo_url()))?;
            println!("+ install.sh (static) into {}", dir.display());
            let mut child = Command::new("sh")
                .arg("-s")
                .env("LINDROIDS_METHOD", "static")
                .env("LINDROIDS_INSTALL_DIR", dir)
                .stdin(Stdio::piped())
                .spawn()
                .map_err(|e| format!("sh: {e}"))?;
            {
                use std::io::Write;
                let mut stdin = child.stdin.take().ok_or("sh: no stdin")?;
                stdin.write_all(script.as_bytes()).map_err(|e| e.to_string())?;
            }
            let ok = child.wait().map_or(false, |s| s.success());
            ok.then_some(()).ok_or_else(|| "the install script failed".to_string())
        }
    }
}

/// Where this binary is installed. After the update replaced it, the kernel names the running
/// (old) file `<path> (deleted)`; the new one is at `<path>`.
fn installed_exe() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("lindroids"));
    match exe.to_str().and_then(|s| s.strip_suffix(" (deleted)")) {
        Some(p) => PathBuf::from(p),
        None => exe,
    }
}

/// The installed binary's build, asked of the binary itself (this process is the old one).
fn installed_build(exe: &Path) -> Option<String> {
    let out = Command::new(exe).arg("version").output().ok()?;
    String::from_utf8_lossy(&out.stdout).split_whitespace().nth(1).map(String::from)
}

pub fn cmd_update(args: &[String]) -> i32 {
    let check = args.iter().any(|a| a == "--check");
    let force = args.iter().any(|a| a == "--force");
    let current = lindroids_server::BUILD;
    let latest = match latest_version() {
        Ok(v) => v,
        Err(e) => return fail(&e),
    };
    let m = match method() {
        Ok(m) => m,
        Err(e) => return fail(&e),
    };
    let how = match &m {
        Method::Apt { termux: true } => "Termux package".to_string(),
        Method::Apt { termux: false } => "APT package".to_string(),
        Method::Static(d) => format!("static install in {}", d.display()),
    };
    println!("lindroids {current} ({how}); latest release {latest}");
    if !newer(&latest, current) && !force {
        println!("Up to date.");
        if check {
            return 0;
        }
        return app_step();
    }
    if check {
        println!("An update is available: `lindroids update`.");
        return 0;
    }
    if let Err(e) = update_cli(&m) {
        return fail(&e);
    }
    let exe = installed_exe();
    match installed_build(&exe) {
        Some(b) => println!("Installed lindroids {b}."),
        None => return fail("the updated lindroids does not run"),
    }
    restart_idle_server();
    // The new binary finishes the job: it knows its own build and carries the matching APK.
    match Command::new(&exe).arg("update").arg("--app-only").status() {
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => fail(&e.to_string()),
    }
}

/// `update --app-only`, run by the new binary: the app to this build.
pub fn cmd_update_app_only() -> i32 {
    app_step()
}

fn app_step() -> i32 {
    let rt = sandbox::runtime_dir();
    if let Ok(CliReply::Status { app_connected: true, app_build: Some(b), .. }) = client::status(&rt) {
        if b == lindroids_server::BUILD {
            println!("The Lindroids app is at the same build ({b}).");
            return 0;
        }
    }
    println!("Bringing the Lindroids app to {}:", lindroids_server::BUILD);
    crate::cmd_setup()
}

/// A server with no windows is restarted so it runs the new binary; with windows, it is left be.
fn restart_idle_server() {
    let rt = sandbox::runtime_dir();
    match client::status(&rt) {
        Ok(CliReply::Status { windows, .. }) if windows.is_empty() => {
            let _ = client::request(&rt, &CliRequest::Stop, Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(300));
            println!("Restarted the idle server on the new version.");
        }
        Ok(_) => println!("The server keeps running with windows open; it uses the new version after `lindroids stop`."),
        Err(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(newer("0.1.12", "0.1.9+abc"));
        assert!(!newer("0.1.9", "0.1.9+abc"));
        assert!(!newer("0.1.8", "0.1.9+abc"));
        assert!(newer("0.2.0", "0.1.99+abc"));
        assert!(newer("0.1.0", "0.1.0+dev"));
        assert!(!newer("garbage", "0.1.0+abc"));
    }
}
