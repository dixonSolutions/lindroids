//! The shared Lindroids window app: one small APK that shows every program's page in Android's
//! own WebView, with no browser and no permission but INTERNET (BROWSER_AGNOSTIC_APK.md).
//!
//! The app's code is in `packaging/webview-app`; this module installs it and asks it to open a
//! run's window. It is chosen with `lindroids config browser app` (or `export --browser app`),
//! and preferred whenever it is installed and the setting asks for it. Nothing about the server,
//! the pages or the runs changes: the app is one more place a page is shown.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::{config, sandbox, shortcut};

/// The app's package name: one app for every program (A in BROWSER_AGNOSTIC_APK.md).
pub const PACKAGE: &str = "org.lindroids";

/// The exported program whose icon launched this run, and the launch URL its app's offline page
/// hands to Termux when Termux is closed.
static LAUNCH: OnceLock<(String, String)> = OnceLock::new();

/// `lindroids launch <id>` says which export asked, so a window opened by the app can start it
/// again from its offline page.
pub fn set_launch(id: &str, url: &str) {
    let _ = LAUNCH.set((id.to_string(), url.to_string()));
}

/// Whether windows should go to the shared app: the browser setting (or `LINDROIDS_BROWSER`),
/// or an export whose icon says the app.
pub fn enabled() -> bool {
    if config::browser_setting() == "app" {
        return true;
    }
    if std::env::var("LINDROIDS_BROWSER")
        .map(|v| v.split(',').any(|b| b.trim() == PACKAGE))
        .unwrap_or(false)
    {
        return true;
    }
    LAUNCH.get().is_some()
}

pub fn installed() -> bool {
    shortcut::app_installed()
}

/// Shows `url` in the shared app, when it is chosen and installed. None: not our business (the
/// browser path goes on). Err: it was chosen but cannot open (the caller falls back and says so).
pub fn maybe_open(url: &str) -> Option<Result<String, String>> {
    if !enabled() {
        return None;
    }
    if !installed() {
        return Some(Err(format!(
            "the Lindroids window app is not installed (lindroids app install){}",
            if sandbox::on_android() { "" } else { ": it is for Android" }
        )));
    }
    Some(open(url))
}

fn open(url: &str) -> Result<String, String> {
    let run = shortcut::run_of_url(url);
    let (name, icon) = run
        .and_then(lindroids_server::icons::launch_info)
        .unwrap_or_else(|| ("Lindroids".to_string(), None));
    let launch = LAUNCH.get().map(|(_, u)| u.clone()).unwrap_or_default();
    shortcut::open_app_window(url, &name, icon.as_deref(), &launch, false)?;
    shortcut::note_window(&sandbox::runtime_dir(), url, PACKAGE);
    Ok(PACKAGE.to_string())
}

fn repo() -> String {
    std::env::var("LINDROIDS_GITHUB_REPO")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "dixonSolutions/lindroids".into())
}

fn version() -> String {
    lindroids_server::BUILD.split('+').next().unwrap_or(lindroids_server::BUILD).to_string()
}

fn release_apk_url() -> Option<String> {
    let v = version();
    if v.ends_with("dev") {
        return None;
    }
    let base = std::env::var("LINDROIDS_RELEASE_URL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("https://github.com/{}/releases/download", repo()));
    Some(format!("{}/v{v}/lindroids-{v}.apk", base.trim_end_matches('/')))
}

fn fetch(url: &str, path: &Path) -> Result<(), String> {
    let curl = Command::new("curl")
        .args(["-fsSL", "--retry", "3", "-o"])
        .arg(path)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let wget = || {
        Command::new("wget")
            .args(["-q", "-O"])
            .arg(path)
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    if curl || wget() {
        Ok(())
    } else {
        Err(format!("cannot download {url} (needs curl or wget, and the network)"))
    }
}

/// Downloads (or takes) the app's APK and opens Android's installer. `lindroids` cannot install it
/// itself: tapping Install is the user's.
fn install(arg: Option<&str>) -> Result<PathBuf, String> {
    let path = match arg.map(str::to_string).or_else(|| std::env::var("LINDROIDS_APP_APK").ok()).filter(|p| !p.is_empty()) {
        Some(p) => PathBuf::from(p),
        None => {
            let url = release_apk_url().ok_or_else(|| {
                "this is a development build: give the APK's path (`lindroids app install <file.apk>`) or set LINDROIDS_APP_APK".to_string()
            })?;
            let dir = std::env::temp_dir();
            let file = dir.join(format!("lindroids-{}.apk", version()));
            println!("Downloading the Lindroids window app from {url}");
            fetch(&url, &file)?;
            file
        }
    };
    if !path.is_file() {
        return Err(format!("{}: no such APK", path.display()));
    }
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    let opener = if Path::new("termux-open").exists() || std::env::var_os("PATH").map_or(true, |p| std::env::split_paths(&p).any(|d| d.join("termux-open").exists())) {
        "termux-open".to_string()
    } else {
        format!("{}/bin/termux-open", lindroids_proto::TERMUX_PREFIX)
    };
    let ok = Command::new(opener)
        .arg("--content-type")
        .arg("application/vnd.android.package-archive")
        .arg(&path)
        .stdin(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        return Err(format!(
            "cannot open Android's installer for {} (termux-tools' termux-open is needed)",
            path.display()
        ));
    }
    println!("Android shows the installer: tap Install.");
    Ok(path)
}

/// Makes sure the app is installed, installing it if needed and waiting for the user's tap.
pub fn ensure() -> Result<(), String> {
    if installed() {
        return Ok(());
    }
    install(None)?;
    let wait = std::env::var("LINDROIDS_APP_WAIT").ok().and_then(|v| v.parse().ok()).unwrap_or(180);
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(wait) {
        std::thread::sleep(Duration::from_secs(1));
        if installed() {
            println!("The Lindroids window app is installed.");
            return Ok(());
        }
    }
    Err("the Lindroids window app was not installed; tap Install in Android's dialog, then run this again".into())
}

pub fn cmd_app(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        None | Some("status") => {
            println!("app      {PACKAGE}");
            println!("installed {}", if installed() { "yes" } else { "no" });
            println!("windows  {}", if enabled() { "the shared app" } else { "the browser (config browser app for the app)" });
            0
        }
        Some("install") if !sandbox::on_android() => crate::fail("the Lindroids window app is for Android"),
        Some("install") => match install(args.get(1).map(String::as_str)) {
            Ok(p) => {
                println!("{}", p.display());
                0
            }
            Err(e) => crate::fail(&e),
        },
        _ => crate::fail("usage: lindroids app [status|install [<file.apk>]]"),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn release_names_the_apk() {
        assert!(super::PACKAGE.contains('.'));
    }
}
