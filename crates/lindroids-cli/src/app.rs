//! Lindroids' Android apps: every exported program's own app, and the shared window app.
//!
//! **A program's own app** (the default export): `lindroids export` builds an APK for the
//! program here, on the phone, in Rust (`crates/lindroids-apk`): the window app's code under the
//! program's package (`org.lindroids.app.<id>`), name and icon, signed with a key made once on this
//! phone. Android then lists the program as an app: its icon opens its window, in Android's own
//! WebView with no browser, and Recents shows its name and icon. Nothing is downloaded and nothing
//! leaves the phone. The install itself is the user's tap in Android's installer, reached the way
//! the terminal app allows (see [`offer`]).
//!
//! **The shared window app** (`org.lindroids`, BROWSER_AGNOSTIC_APK.md): one APK for every program,
//! from the release; optional now (`lindroids app install`), for windows of programs that are not
//! exported when `config browser app` asks for it.
//!
//! The app's code is in `packaging/webview-app`. Nothing about the server, the pages or the runs
//! changes: an app is one more place a page is shown.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

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

/// Shows `url` in the shared app, when it is chosen and installed. None: not our business (the
/// browser path goes on). Err: it was chosen but cannot open (the caller falls back and says so).
pub fn maybe_open(url: &str) -> Option<Result<String, String>> {
    if !enabled() || !sandbox::on_android() {
        return None;
    }
    // Termux from Google Play cannot see other apps (no QUERY_ALL_PACKAGES), so the windows app
    // is not looked up: opening it is the test, and the browser takes the window when it fails.
    match open(url) {
        Ok(p) => Some(Ok(p)),
        Err(e) if config::browser_configured() => Some(Err(format!("{e} (lindroids app install builds the windows app)"))),
        Err(_) => None,
    }
}

fn open(url: &str) -> Result<String, String> {
    let run = shortcut::run_of_url(url);
    let (name, icon) = run
        .and_then(lindroids_server::icons::launch_info)
        .unwrap_or_else(|| ("Lindroids".to_string(), None));
    let launch = LAUNCH.get().map(|(_, u)| u.clone()).unwrap_or_default();
    // The windows app built here first; the shared app of older releases (org.lindroids) if
    // that is what is installed.
    let mut last = String::new();
    for app in [WINDOWS_PACKAGE, PACKAGE] {
        match shortcut::open_app_window(app, url, &name, icon.as_deref(), &launch, false) {
            Ok(()) => {
                shortcut::note_window(&sandbox::runtime_dir(), url, app);
                return Ok(app.to_string());
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

fn version() -> String {
    lindroids_server::BUILD.split('+').next().unwrap_or(lindroids_server::BUILD).to_string()
}

/// Termux's file provider refuses other apps unless `allow-external-apps=true`; the installer is
/// another app. Enable it only while the installer copies our APK, then put the user's file back
/// exactly as it was (the window paths themselves never need it).
struct ExternalApps {
    path: PathBuf,
    original: Option<Vec<u8>>,
    changed: bool,
}

impl ExternalApps {
    fn enable() -> Option<ExternalApps> {
        if !sandbox::on_android() {
            return None;
        }
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|p| p.to_string_lossy().contains("com.termux/files/home"))
            .unwrap_or_else(|| PathBuf::from(lindroids_proto::TERMUX_HOME));
        let path = home.join(".termux/termux.properties");
        let original = std::fs::read(&path).ok();
        let text = original.as_deref().map(String::from_utf8_lossy).unwrap_or_default();
        let active = text.lines().any(|l| {
            let l = l.trim();
            !l.starts_with('#') && l.split_once('=').map(|(k, v)| k.trim() == "allow-external-apps" && v.trim() == "true").unwrap_or(false)
        });
        if active {
            return Some(ExternalApps { path, original, changed: false });
        }
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let mut new = text.into_owned();
        if !new.ends_with('\n') && !new.is_empty() {
            new.push('\n');
        }
        new.push_str("allow-external-apps=true\n");
        if std::fs::write(&path, new).is_err() {
            return None;
        }
        reload_settings();
        std::thread::sleep(Duration::from_millis(700));
        Some(ExternalApps { path, original, changed: true })
    }
}

impl Drop for ExternalApps {
    fn drop(&mut self) {
        if !self.changed {
            return;
        }
        match &self.original {
            Some(bytes) => {
                let _ = std::fs::write(&self.path, bytes);
            }
            None => {
                let _ = std::fs::remove_file(&self.path);
            }
        }
        reload_settings();
    }
}

fn reload_settings() {
    let path = format!("{}/bin/termux-reload-settings", lindroids_proto::TERMUX_PREFIX);
    let cmd = if Path::new(&path).exists() { path } else { "termux-reload-settings".to_string() };
    let _ = Command::new(cmd).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
}

/// Builds the windows app here (or takes an APK given) and offers it to Android's installer.
fn install(arg: Option<&str>) -> Result<(), String> {
    let path = match arg.map(str::to_string).or_else(|| std::env::var("LINDROIDS_APP_APK").ok()).filter(|p| !p.is_empty()) {
        Some(p) => PathBuf::from(p),
        None => {
            let (apk, _) = build_app(WINDOWS_PACKAGE, "windows", "Lindroids", LINDROIDS_ICON, "", "")?;
            println!("Built the Lindroids windows app ({WINDOWS_PACKAGE}).");
            apk
        }
    };
    if !path.is_file() {
        return Err(format!("{}: no such APK", path.display()));
    }
    match offer(&path, "Lindroids", "")? {
        Offered::Installer => println!("Android's installer is open: tap Install."),
        Offered::Downloads(file) => println!("It is in Download as {file}, and the file manager is open there: tap it, then Install."),
        Offered::Browser(_) => println!("The browser downloads it: open it, then Install."),
    }
    println!("Then `lindroids run <program>` opens each program in a window of its own, with its name and icon.");
    Ok(())
}

// ------------------------------------------------------------------ a program's own app

/// The windows app built on this phone (`lindroids app install`): windows of programs that are not
/// exported, one task per run, each with the program's name and icon on its task. It closes with
/// the program, and with Termux (the run is gone with the server).
pub const WINDOWS_PACKAGE: &str = "org.lindroids.windows";

static LINDROIDS_ICON: &[u8] = include_bytes!("../../../media/icon-512.png");

/// The prefix of every program's own app (`lindroids_apk::package_for`).
pub const PROGRAM_PREFIX: &str = "org.lindroids.app.";

/// Whether `package` is a program's own app built here (not a browser's WebAPK).
pub fn is_program_app(package: &str) -> bool {
    package.starts_with(PROGRAM_PREFIX)
}

/// This phone's signing key for the programs' apps, made on first use and kept in Lindroids'
/// state (`apk/signing.key`, private). Every app is updated only with it: an app installed with
/// another key (another phone's, a cleared Termux) must be uninstalled first.
fn signing_key() -> Result<lindroids_apk::Key, String> {
    use std::os::unix::fs::OpenOptionsExt;
    let dir = lindroids_server::state_dir().join("apk");
    let path = dir.join("signing.key");
    if let Ok(b) = std::fs::read(&path) {
        return lindroids_apk::Key::from_bytes(&b).map_err(|e| format!("{}: {e}", path.display()));
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let key = lindroids_apk::Key::generate("Lindroids on this phone")?;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    std::io::Write::write_all(&mut f, &key.to_bytes()).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(key)
}

/// The icon layers of a program's app, from its 512-pixel icon: the plain icon, the adaptive
/// icon's foreground (the icon inside the middle 66 of 108, where every launcher's mask leaves it
/// whole) and its background (a pale tint of the icon's own colour).
fn icon_layers(icon: &[u8]) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>), String> {
    use resvg::tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};
    let src = Pixmap::decode_png(icon).map_err(|e| format!("the icon: {e}"))?;
    let place = |size: u32, inner: f32| -> Result<Pixmap, String> {
        let mut out = Pixmap::new(size, size).ok_or("icon size")?;
        let k = inner / src.width().max(src.height()) as f32;
        let (tx, ty) = ((size as f32 - src.width() as f32 * k) / 2.0, (size as f32 - src.height() as f32 * k) / 2.0);
        let paint = PixmapPaint { quality: FilterQuality::Bicubic, ..PixmapPaint::default() };
        out.draw_pixmap(0, 0, src.as_ref(), &paint, Transform::from_row(k, 0.0, 0.0, k, tx, ty), None);
        Ok(out)
    };
    let plain = place(512, 512.0)?;
    let fg = place(432, 288.0)?;
    let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
    for p in src.pixels() {
        if p.alpha() >= 200 {
            let c = p.demultiply();
            r += c.red() as u64;
            g += c.green() as u64;
            b += c.blue() as u64;
            n += 1;
        }
    }
    let tint = |sum: u64| if n == 0 { 255 } else { (255.0 - (255.0 - sum as f32 / n as f32) * 0.3).round() as u8 };
    let mut bg = Pixmap::new(432, 432).ok_or("icon size")?;
    bg.fill(resvg::tiny_skia::Color::from_rgba8(tint(r), tint(g), tint(b), 255));
    let enc = |p: &Pixmap| p.encode_png().map_err(|e| format!("the icon: {e}"));
    Ok((enc(&plain)?, enc(&fg)?, enc(&bg)?))
}

/// Builds the program's own app: `<state>/apk/<id>.apk`. `start` is its page on this phone's
/// server (`http://<label>.localhost:<port>/app/<id>/`), `launch` what starts it through the
/// terminal app when the server is not running. Returns the APK's path and its package.
pub fn build_program_app(id: &str, name: &str, icon: &[u8], start: &str, launch: &str) -> Result<(PathBuf, String), String> {
    build_app(&lindroids_apk::package_for(id), id, name, icon, start, launch)
}

/// Builds an app from the template: `<state>/apk/<file>.apk`. An empty `start` makes the windows
/// app (no page of its own: only the windows Lindroids opens in it).
fn build_app(package: &str, file: &str, name: &str, icon: &[u8], start: &str, launch: &str) -> Result<(PathBuf, String), String> {
    let id = file;
    let key = signing_key()?;
    let package = package.to_string();
    let (plain, fg, bg) = icon_layers(icon)?;
    // Minutes since 1970: a new export of the same program is always a newer version.
    let version_code = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) / 60) as u32;
    let termux = shortcut::package();
    let p = lindroids_apk::Program {
        package: &package,
        name,
        start,
        launch,
        termux: &termux,
        id,
        version_code,
        version_name: &version(),
        icon: &plain,
        foreground: &fg,
        background: &bg,
    };
    let apk = lindroids_apk::build(&p, &key)?;
    let dir = lindroids_server::state_dir().join("apk");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(format!("{file}.apk"));
    std::fs::write(&path, &apk).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((path, package))
}

/// How a program's app reached Android's installer.
pub enum Offered {
    /// The installer is open: the user taps Install.
    Installer,
    /// The APK is in Download and the file manager is open there: the user taps it, then Install.
    Downloads(String),
    /// The browser downloads it from the program's page: the user opens it, then Install.
    Browser(String),
}

/// Whether the terminal app may open Android's installer: it declares
/// REQUEST_INSTALL_PACKAGES (Termux from F-Droid or GitHub; not Termux from Google Play).
pub fn terminal_can_install() -> bool {
    shortcut::helper(&["can-install", &shortcut::package()]).map(|o| o.contains("can-install yes")).unwrap_or(false)
}

/// The shared Download folder, when the terminal app may write it (Termux with storage access:
/// `termux-setup-storage`).
fn writable_downloads() -> Option<PathBuf> {
    let d = PathBuf::from("/storage/emulated/0/Download");
    let probe = d.join(".lindroids-write-test");
    std::fs::write(&probe, b"").ok()?;
    let _ = std::fs::remove_file(&probe);
    Some(d)
}

/// Puts a built app in front of Android's installer, the best way this terminal app allows:
/// 1. the installer itself, when the terminal app may ask for installs;
/// 2. else the Download folder in the file manager, when the terminal app can write there;
/// 3. else the program's page serving the APK (`<page>/<name>.apk`) in the browser, which
///    downloads it.
/// Android's installer only acts for an app that declares REQUEST_INSTALL_PACKAGES: Termux from
/// Google Play does not, and the installer then closes at once.
pub fn offer(apk: &Path, name: &str, page: &str) -> Result<Offered, String> {
    if terminal_can_install() {
        let external = ExternalApps::enable();
        let started = shortcut::install_apk(apk);
        if started {
            std::thread::sleep(Duration::from_secs(8));
            drop(external);
            return Ok(Offered::Installer);
        }
        drop(external);
    }
    let file = format!("{}.apk", name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' { c } else { '_' }).collect::<String>().trim());
    if let Some(d) = writable_downloads() {
        let to = d.join(&file);
        if std::fs::copy(apk, &to).is_ok() {
            let _ = shortcut::helper(&["show-downloads", &shortcut::package()]);
            return Ok(Offered::Downloads(file));
        }
    }
    let browser_url = format!("{page}{}", file.replace(' ', "%20"));
    shortcut::view("default", &browser_url).map_err(|e| format!("cannot open the browser for the app's download: {e}"))?;
    Ok(Offered::Browser(file))
}

pub fn cmd_app(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        None | Some("status") => {
            println!("windows app  {WINDOWS_PACKAGE} (lindroids app install builds it here)");
            println!("windows      {}", if enabled() { "the windows app when it is installed, else the browser" } else { "the browser (config browser app for the windows app)" });
            println!("exports      each program's own app, org.lindroids.app.<id> (lindroids export)");
            0
        }
        Some("install") if !sandbox::on_android() => crate::fail("the Lindroids windows app is for Android"),
        Some("install") => match install(args.get(1).map(String::as_str)) {
            Ok(()) => 0,
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
