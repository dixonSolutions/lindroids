//! Home screen shortcuts with no app of Lindroids' own.
//!
//! A shortcut belongs to the terminal app (Termux): `shortcut.dex` (built from
//! `shortcut/Shortcut.java` by scripts/build-shortcut-dex.sh, embedded here) runs under Android's
//! `app_process` with the terminal app's uid, so Android's shortcut service takes it as that
//! package and the launcher asks the user to add it. Tapping it sends the terminal app a shared
//! URL (`http://127.0.0.1:<port>/launch/termux/<id>`) through its share receiver, which needs no
//! permission or setting: Termux starts if it is closed and runs `~/bin/termux-url-opener <url>`,
//! which this module installs (keeping an opener the user had, for every other URL), and that runs
//! `lindroids launch <id>`.
//!
//! On a desktop, an exported program gets a `.desktop` file instead.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use lindroids_proto::{self as proto, Exported, TERMUX_HOME};

use crate::sandbox::{self, Kind, Sandbox};

const DEX: &[u8] = include_bytes!("../shortcut/shortcut.dex");

/// Termux's share receiver, which runs `~/bin/termux-url-opener` for a shared URL: its name in
/// Termux 0.119 first, then 0.118's. The helper takes the first that exists in the installed
/// Termux (else any activity of it that takes the share), so a rename cannot break the icon.
const RECEIVER: &str = "com.termux/.app.api.file.FileShareReceiverActivity,com.termux/com.termux.filepicker.TermuxFileReceiverActivity";

/// Marks the opener as ours, so it is replaced on updates and never saved as "the user's".
const OPENER_MARK: &str = "# lindroids: termux-url-opener";

/// The terminal app's package (Termux forks name theirs).
fn package() -> String {
    ["TERMUX_APP__PACKAGE_NAME", "TERMUX_APP_PACKAGE_NAME"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .unwrap_or_else(|| "com.termux".into())
}

/// Browsers whose Custom Tabs open as windows of their own, in the order tried after the phone's
/// default browser (when it is one of them, and not Firefox). Chromium's first: they put the
/// program's name and icon on its window in Recents, close the window when the program ends, and
/// let the program write into files and folders picked on Android; Firefox does none of that, so
/// it comes last, even as the default. A browser that opens an ordinary tab instead (Cromite
/// without "Allow custom tab intents", WebView-based browsers) is passed over: the helper sees
/// which activity it opened.
const WINDOW_BROWSERS: &[&str] = &[
    "com.android.chrome",
    "com.microsoft.emmx",
    "com.brave.browser",
    "com.vivaldi.browser",
    "com.sec.android.app.sbrowser",
    "com.chrome.beta",
    "org.chromium.chrome",
    "org.cromite.cromite",
    "org.mozilla.firefox",
    "org.mozilla.firefox_beta",
    "org.mozilla.fenix",
];

/// Shows `url` in a window of its own: a Custom Tab, in its own task, of the first browser that
/// opens one (see `browsers`). Sending the same URL again brings that window back instead of
/// opening another. Returns the browser used, which notifications then open windows in.
pub fn open_window(url: &str) -> Result<String, String> {
    if let Some((id, app)) = APP_WINDOW.get() {
        if let Some(r) = open_in_app(url, id, app) {
            return r;
        }
    }
    let rt = sandbox::runtime_dir();
    let opened = std::fs::read_to_string(rt.join("windows")).unwrap_or_default();
    // (url, browser) lines: whether this page (the URL without its ticket, after #) has had a
    // window, and where. Bringing it back takes that window's URL as it was: Android finds the
    // window by its URL, and a new ticket would open another.
    let page = |u: &str| u.split('#').next().unwrap_or(u).to_string();
    let before = opened
        .lines()
        .rev()
        .find_map(|l| l.split_once(' ').filter(|(u, _)| page(u) == page(url)).map(|(u, b)| (u.to_string(), b.to_string())));
    let url = before.as_ref().map_or(url, |(u, _)| u.as_str());
    let mode = if before.is_some() { "raise" } else { "first" };
    let mut args = vec!["open".to_string(), package(), mode.into(), url.to_string()];
    args.extend(before.as_ref().map(|(_, b)| b.clone()));
    args.extend(browsers());
    let out = helper(&args.iter().map(String::as_str).collect::<Vec<_>>())?;
    let browser = out.lines().find_map(|l| l.strip_prefix("browser ")).unwrap_or("?").to_string();
    if browser != "?" {
        let _ = std::fs::write(rt.join("window-browser"), &browser);
        note_window(&rt, url, &browser);
    }
    Ok(browser)
}

/// Notes in `<runtime>/windows` that the window for `url` went to `browser` (the last 64 kept).
fn note_window(rt: &Path, url: &str, browser: &str) {
    let opened = std::fs::read_to_string(rt.join("windows")).unwrap_or_default();
    let mut lines: Vec<&str> = opened.lines().collect();
    let line = format!("{url} {browser}");
    lines.push(&line);
    let keep = lines.len().saturating_sub(64);
    let _ = std::fs::write(rt.join("windows"), lines[keep..].join("\n") + "\n");
}

/// The run a window's URL is for (`?window&run=<pid>`).
fn run_of_url(url: &str) -> Option<u32> {
    let query = url.split('#').next()?.split_once('?')?.1;
    query.split('&').find_map(|kv| kv.strip_prefix("run=")).and_then(|v| v.parse().ok())
}

/// The window last noted for run `run`: its URL and where it went.
fn noted_window(rt: &Path, run: u32) -> Option<(String, String)> {
    let opened = std::fs::read_to_string(rt.join("windows")).ok()?;
    opened.lines().rev().find_map(|l| l.split_once(' ').filter(|(u, _)| run_of_url(u) == Some(run)).map(|(u, b)| (u.to_string(), b.trim().to_string())))
}

// ------------------------------------------------------------------ installed web apps

/// The exported program `lindroids launch` starts, when the browser installed it as an app of its
/// own (`export --webapk`): its id and the app's package (empty if not known). Its window opens in
/// that app.
static APP_WINDOW: std::sync::OnceLock<(String, String)> = std::sync::OnceLock::new();

pub fn set_app_window(id: &str, app: &str) {
    let _ = APP_WINDOW.set((id.to_string(), app.to_string()));
}

/// `url` (a run's window, `<origin>/?window&run=<pid>...`) as a page of the exported program `id`'s
/// own (`<origin>/app/<id>/?window&run=<pid>...`): in the app's scope.
fn app_page_url(url: &str, id: &str) -> Option<String> {
    let (origin, rest) = url.split_once("/?")?;
    (!origin.contains("://") || !origin.split_once("://")?.1.contains('/')).then(|| format!("{origin}/app/{id}/?{rest}"))
}

/// `url` naming where it was sent (`&via=<package>`, before the fragment), as the helper does
/// for browsers: the server keeps the page in the window it was sent to.
fn with_via(url: &str, via: &str) -> String {
    let (base, fragment) = url.split_once('#').map_or((url, String::new()), |(b, f)| (b, format!("#{f}")));
    format!("{base}&via={via}{fragment}")
}

/// Shows a run's window in the exported program's installed app (`export --webapk`). None when
/// it is not for that app: not a run's window, or the app shows another run of the program that
/// still runs (an app is one window), or the app did not open; the window is then one of the
/// usual kind.
fn open_in_app(url: &str, id: &str, app: &str) -> Option<Result<String, String>> {
    let run = run_of_url(url)?;
    let rt = sandbox::runtime_dir();
    let via = if app.is_empty() { "app" } else { app };
    let notes = std::fs::read_to_string(rt.join("windows")).unwrap_or_default();
    let other = notes.lines().rev().find_map(|l| l.split_once(' ').filter(|(_, b)| b.trim() == via).and_then(|(u, _)| run_of_url(u)));
    if let Some(o) = other.filter(|o| *o != run && Path::new(&format!("/proc/{o}")).exists()) {
        eprintln!("lindroids: {id}'s app shows another of its windows (run {o}): this one opens in the browser");
        return None;
    }
    let page = app_page_url(url, id)?;
    let sent = with_via(&page, via);
    // The app's page asked for this launch, and goes there itself: the terminal app is in the
    // background then, and Android would not let it open the app.
    if std::env::var("LINDROIDS_PAGE_LAUNCH").ok().as_deref() == Some(id) {
        let req = proto::CliRequest::AppWindow { id: id.to_string(), url: sent.clone() };
        if let Ok(proto::CliReply::Ok { .. }) = crate::client::request(&rt, &req, std::time::Duration::from_secs(3)) {
            note_window(&rt, &page, via);
            return Some(Ok(via.to_string()));
        }
    }
    if app.is_empty() {
        return None;
    }
    match helper(&["view", &package(), app, &sent]) {
        Ok(_) => {
            note_window(&rt, &page, via);
            Some(Ok(app.to_string()))
        }
        Err(e) => {
            eprintln!("lindroids: {id}'s app ({app}) did not open ({e}): its window opens in the browser");
            None
        }
    }
}

/// Browser packages and the names users know them by.
pub fn browser_name(pkg: &str) -> &str {
    match pkg {
        "com.android.chrome" => "Chrome",
        "com.chrome.beta" => "Chrome Beta",
        "com.chrome.dev" => "Chrome Dev",
        "com.chrome.canary" => "Chrome Canary",
        "com.sec.android.app.sbrowser" => "Samsung Internet",
        "com.sec.android.app.sbrowser.beta" => "Samsung Internet Beta",
        p => p,
    }
}

/// The installed browsers that can install a program as an app of its own (a WebAPK), in order,
/// and those that cannot, with why.
pub fn app_browsers() -> Result<(Vec<String>, Vec<(String, String)>), String> {
    helper(&["app-browsers"]).map(|out| parse_app_browsers(&out))
}

/// Known WebAPK browsers whose APK is on the device's system image but which are not installed
/// for this user: `(package, apk path)`. Android cannot start them until they are installed.
pub fn system_browsers() -> Vec<(String, String)> {
    helper(&["system-browsers"]).map(|out| parse_system_browsers(&out)).unwrap_or_default()
}

fn parse_system_browsers(out: &str) -> Vec<(String, String)> {
    out.lines()
        .filter_map(|l| {
            let (pkg, path) = l.strip_prefix("system ")?.split_once(' ')?;
            Some((pkg.to_string(), path.to_string()))
        })
        .collect()
}

/// The browser that opens web links, if one is chosen.
pub fn default_browser() -> Option<String> {
    let out = helper(&["default-browser"]).ok()?;
    out.lines().find_map(|l| l.strip_prefix("default ")).map(str::trim).filter(|d| *d != "none").map(str::to_string)
}

/// The helper's `app-browsers` answer: the browsers that can, and those that cannot, with why.
fn parse_app_browsers(out: &str) -> (Vec<String>, Vec<(String, String)>) {
    let (mut can, mut cannot) = (Vec::new(), Vec::new());
    for line in out.lines() {
        if let Some(b) = line.strip_prefix("can ") {
            can.push(b.trim().to_string());
        } else if let Some((b, why)) = line.strip_prefix("cannot ").and_then(|r| r.split_once(' ')) {
            cannot.push((b.to_string(), why.trim().to_string()));
        }
    }
    (can, cannot)
}

/// The app installed for `scope` (an exported program's pages, `<origin>/app/<id>/`), if any.
pub fn app_for(scope: &str) -> Option<String> {
    let out = helper(&["apps-for", scope]).ok()?;
    out.lines().find_map(|l| l.strip_prefix("app ")).map(|p| p.trim().to_string())
}

/// Opens `url` in `target` (an ordinary tab of a browser, or an installed app).
pub fn view(target: &str, url: &str) -> Result<(), String> {
    helper(&["view", &package(), target, url]).map(|_| ())
}

/// Android's settings page of an installed app (where it is uninstalled).
pub fn app_settings(app: &str) -> Result<(), String> {
    helper(&["app-settings", &package(), app]).map(|_| ())
}

/// Keeps an exported program's icons for its app's manifest (`<state>/apps/<id>.*.png`, made by the
/// helper): 512 pixels square (`.app.png`; browsers install no app whose icon is under 144), and
/// as Android's adaptive icons draw it (`.maskable.png`). Without them, the app gets the icon as
/// it is.
pub fn save_app_icons(id: &str, icon_png: &[u8]) -> Result<(), String> {
    let dir = lindroids_server::state_dir().join("apps");
    let icon = dir.join(format!("{id}.png"));
    std::fs::write(&icon, icon_png).map_err(|e| format!("{}: {e}", icon.display()))?;
    let (maskable, plain) = (dir.join(format!("{id}.maskable.png")), dir.join(format!("{id}.app.png")));
    if let Err(e) = helper(&["app-icons", &icon.display().to_string(), &maskable.display().to_string(), &plain.display().to_string()]) {
        eprintln!("lindroids: the app gets the icon as it is ({e}); a small one can stop the browser installing it");
        let _ = std::fs::remove_file(&maskable);
        let _ = std::fs::remove_file(&plain);
    }
    Ok(())
}

/// What Termux needs for an exported program's app to start it with Termux closed: the opener for
/// the launch URL the app's offline page shares with Termux (as a Termux icon does).
pub fn prepare_termux(sb: &Sandbox) -> Result<(), String> {
    install_opener(sb).map(|_| ())
}

/// The browsers to show windows in, in order: `LINDROIDS_BROWSER` (comma-separated packages),
/// the default browser (`default`, which the helper resolves), then the list.
fn browsers() -> Vec<String> {
    let mut v: Vec<String> = std::env::var("LINDROIDS_BROWSER")
        .map(|v| v.split(',').map(|b| b.trim().to_string()).filter(|b| !b.is_empty()).collect())
        .unwrap_or_default();
    v.push("default".into());
    v.extend(WINDOW_BROWSERS.iter().map(|b| b.to_string()));
    v
}

/// For a notification's tap, which cannot see what the browser opens: the browser the last window
/// opened in first (one that opened an ordinary tab was passed over then).
fn notice_browsers() -> Vec<String> {
    let mut v = browsers();
    if let Ok(b) = std::fs::read_to_string(sandbox::runtime_dir().join("window-browser")) {
        v.insert(0, b.trim().to_string());
    }
    v
}

/// The URL a shortcut shares with the terminal app.
pub fn launch_url(uid: u32, id: &str) -> String {
    format!("http://127.0.0.1:{}/launch/termux/{id}", proto::server_tcp_port(uid))
}

/// What `app_process` needs from Android's environment (BOOTCLASSPATH and friends). Termux's
/// shells have it; under proot, or from a program that cleaned its environment, it is taken from
/// the nearest ancestor process that has it.
fn android_env() -> Vec<(String, String)> {
    const KEYS: [&str; 8] = [
        "BOOTCLASSPATH", "DEX2OATBOOTCLASSPATH", "SYSTEMSERVERCLASSPATH", "ANDROID_ROOT", "ANDROID_DATA",
        "ANDROID_ART_ROOT", "ANDROID_I18N_ROOT", "ANDROID_TZDATA_ROOT",
    ];
    if std::env::var_os("BOOTCLASSPATH").is_some() {
        return Vec::new();
    }
    let mut pid = std::process::id();
    for _ in 0..32 {
        let Some(ppid) = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|s| s.rsplit_once(')').and_then(|(_, r)| r.split_whitespace().nth(1)?.parse::<u32>().ok()))
        else {
            break;
        };
        if ppid <= 1 {
            break;
        }
        pid = ppid;
        let Ok(env) = std::fs::read(format!("/proc/{pid}/environ")) else { continue };
        let vars: Vec<(String, String)> = env
            .split(|&b| b == 0)
            .filter_map(|kv| std::str::from_utf8(kv).ok()?.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
            .filter(|(k, _)| KEYS.contains(&k.as_str()))
            .collect();
        if vars.iter().any(|(k, _)| k == "BOOTCLASSPATH") {
            return vars;
        }
    }
    vec![("ANDROID_ROOT".into(), "/system".into()), ("ANDROID_DATA".into(), "/data".into())]
}

/// Runs the helper; Ok with what it printed when it says "ok".
fn helper(args: &[&str]) -> Result<String, String> {
    if sandbox::native_bridge() {
        // app_process loads the ARM translation layer there, and seccomp kills it.
        return Err("this Android has an ARM translation layer (x86 image), which stops app_process: no shortcut".into());
    }
    let dir = lindroids_server::state_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let dex = dir.join("shortcut.dex");
    if std::fs::read(&dex).ok().as_deref() != Some(DEX) {
        let _ = std::fs::set_permissions(&dex, std::fs::Permissions::from_mode(0o644));
        std::fs::write(&dex, DEX).map_err(|e| format!("{}: {e}", dex.display()))?;
    }
    // Android refuses to load code from a file anyone may write to.
    let _ = std::fs::set_permissions(&dex, std::fs::Permissions::from_mode(0o444));
    let out = Command::new("/system/bin/app_process")
        .arg(format!("-Djava.class.path={}", dex.display()))
        .arg("/system/bin")
        .arg("lindroids.Shortcut")
        .args(args)
        .envs(android_env())
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("app_process: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.lines().any(|l| l.trim() == "ok") {
        return Ok(stdout.into_owned());
    }
    let msg = stdout.lines().find_map(|l| l.strip_prefix("error: ")).map(str::to_string).unwrap_or_else(|| {
        let e = String::from_utf8_lossy(&out.stderr);
        e.lines().filter(|l| !l.trim().is_empty()).last().unwrap_or("no answer").to_string()
    });
    Err(msg)
}

/// Puts `text` on Android's clipboard (the helper does it as the terminal app; Android takes at
/// most about 1 MB through one call, so callers keep it shorter).
pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    let file = lindroids_server::state_dir().join("clip.txt");
    std::fs::write(&file, text).map_err(|e| format!("{}: {e}", file.display()))?;
    let r = helper(&["clip", &package(), &file.display().to_string()]).map(|_| ());
    let _ = std::fs::remove_file(&file);
    r
}

/// Termux's home as this process sees it.
fn termux_home(sb: &Sandbox) -> Option<PathBuf> {
    match sb.kind {
        Kind::Termux => std::env::var_os("HOME").map(PathBuf::from).or_else(|| Some(TERMUX_HOME.into())),
        _ => Path::new(TERMUX_HOME).is_dir().then(|| TERMUX_HOME.into()),
    }
}

fn opener_script(lindroids: &str, before: &Path) -> String {
    format!(
        r#"#!/data/data/com.termux/files/usr/bin/sh
{OPENER_MARK} (rewritten by `lindroids export`; do not edit, your own opener is kept below)
# Home screen shortcuts made by `lindroids export` share http://127.0.0.1:<port>/launch/termux/<id>
# with Termux, which runs this; every other URL goes to the opener you had, if any.
case "$1" in
  http://127.0.0.1:*/launch/termux/*)
    id=${{1##*/launch/termux/}}
    exec "{lindroids}" launch "$id"
    ;;
esac
if [ -x "{before}" ]; then exec "{before}" "$@"; fi
echo "No opener for $1 (put yours in {before})"
"#,
        before = before.display()
    )
}

/// Make `~/bin/termux-url-opener` start exported programs, keeping any opener already there.
fn install_opener(sb: &Sandbox) -> Result<bool, String> {
    install_termux_script(sb, "termux-url-opener", OPENER_MARK, opener_script, "still opens every other URL")
}

/// Make `~/bin/termux-file-editor` open the files Termux receives (Files' "Open with", Share) in
/// Linux programs (`lindroids open --linux`), keeping any editor already there for the rest.
/// Whether it was (re)written.
pub fn install_file_editor(sb: &Sandbox) -> Result<bool, String> {
    install_termux_script(sb, "termux-file-editor", crate::open::EDITOR_MARK, crate::open::editor_script, "opens the files no Linux program opens")
}

/// Write `~/bin/<name>` (Termux runs it), marked as ours by `mark`. A script of the user's there
/// is moved to `~/.local/share/lindroids/<name>.before` first, once, and ours hands it what we do
/// not handle. Whether the script was (re)written.
fn install_termux_script(sb: &Sandbox, name: &str, mark: &str, script: fn(&str, &Path) -> String, theirs_does: &str) -> Result<bool, String> {
    let home = termux_home(sb).ok_or("Termux's home is not reachable from here")?;
    let bin = home.join("bin");
    let path = bin.join(name);
    let before = home.join(".local/share/lindroids").join(format!("{name}.before"));
    // Bytes, not text: Termux also takes a symlink to a program here.
    let ours = std::fs::read(&path).map_or(false, |old| old.windows(mark.len()).any(|w| w == mark.as_bytes()));
    if path.symlink_metadata().is_ok() && !ours && before.symlink_metadata().is_err() {
        std::fs::create_dir_all(before.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::rename(&path, &before).map_err(|e| format!("{}: {e}", path.display()))?;
        eprintln!("lindroids: your {name} is kept as {} and {theirs_does}", before.display());
    }
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| format!("{}/bin/lindroids", proto::TERMUX_PREFIX));
    let text = script(&exe, &before);
    if std::fs::read_to_string(&path).ok().as_deref() == Some(text.as_str()) {
        return Ok(false);
    }
    std::fs::create_dir_all(&bin).map_err(|e| format!("{}: {e}", bin.display()))?;
    let tmp = bin.join(format!(".{name}.lindroids"));
    std::fs::File::create(&tmp).and_then(|mut f| f.write_all(text.as_bytes())).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(true)
}

/// Put an exported program on the home screen (Android: the launcher asks the user first).
pub fn create(sb: &Sandbox, rec: &Exported, icon_png: &[u8]) -> Result<String, String> {
    match sb.kind {
        Kind::Desktop => desktop_entry(rec, icon_png),
        Kind::Proot => Err("make the shortcut from Termux: `lindroids export` there runs programs in proot-distro distros too".into()),
        Kind::Termux => {
            install_opener(sb)?;
            if let Err(e) = install_file_editor(sb) {
                eprintln!("lindroids: files shared to Termux will not open in Linux programs: {e}");
            }
            let icon = lindroids_server::state_dir().join("apps").join(format!("{}.png", rec.id));
            std::fs::write(&icon, icon_png).map_err(|e| format!("{}: {e}", icon.display()))?;
            let url = launch_url(sb.uid, &rec.id);
            let icon_s = icon.display().to_string();
            let out = helper(&["pin", &package(), &rec.id, &rec.name, &icon_s, "android.intent.action.SEND", RECEIVER, "text/plain", &url])?;
            if out.lines().any(|l| l == "updated") {
                return Ok("Its home screen icon is updated.".into());
            }
            Ok("Android asks to add it to the home screen: tap \"Add\" (or drag it into place).".into())
        }
    }
}

/// Take an exported program's shortcut away (Android only lets its app disable a pinned one).
pub fn remove(sb: &Sandbox, id: &str) -> Result<(), String> {
    match sb.kind {
        Kind::Desktop => {
            let _ = std::fs::remove_file(desktop_path(id));
            Ok(())
        }
        Kind::Proot => Ok(()),
        Kind::Termux => {
            let _ = std::fs::remove_file(lindroids_server::state_dir().join("apps").join(format!("{id}.png")));
            helper(&["remove", &package(), id]).map(|_| ())
        }
    }
}

fn desktop_path(id: &str) -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
    });
    base.join("applications").join(format!("lindroids-{id}.desktop"))
}

fn desktop_entry(rec: &Exported, icon_png: &[u8]) -> Result<String, String> {
    let icon = lindroids_server::state_dir().join("apps").join(format!("{}.png", rec.id));
    std::fs::write(&icon, icon_png).map_err(|e| format!("{}: {e}", icon.display()))?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let path = desktop_path(&rec.id);
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let text = format!(
        "[Desktop Entry]\nType=Application\nName={}\nIcon={}\nExec=\"{}\" launch {}\nTerminal=false\n",
        rec.name.replace('\n', " "),
        icon.display(),
        exe.display(),
        rec.id
    );
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(format!("It is in the applications menu ({}).", path.display()))
}


/// The helper's `device` answer: `battery <percent> <charging 0|1>`, `powersave <0|1>`.
fn parse_device(out: &str) -> lindroids_server::android::Device {
    let mut d = lindroids_server::android::Device::default();
    for line in out.lines() {
        let w: Vec<&str> = line.split_whitespace().collect();
        match w.as_slice() {
            ["battery", level, charging] => d.battery = level.parse().ok().map(|l| (l, *charging == "1")),
            ["powersave", on] => d.power_saver = Some(*on == "1"),
            _ => {}
        }
    }
    d
}

/// The helper's arguments for a program's notification. Its tap brings back the run's window
/// where it went (an installed app's, for one: `export --webapk`), else the browsers' way.
fn notice_args(what: &str, n: lindroids_server::android::Notice) -> Vec<String> {
    let icon = n.icon.map(|p| p.display().to_string()).unwrap_or_default();
    let small = n.small_icon.map(|p| p.display().to_string()).unwrap_or_default();
    let mut url = n.url.unwrap_or_default();
    let mut browsers = notice_browsers();
    if let Some((noted, to)) = run_of_url(&url).and_then(|r| noted_window(&sandbox::runtime_dir(), r)) {
        // The ticket in the URL the window opened with: the window is found again by it.
        url = noted;
        browsers.insert(0, to);
    }
    let mut args = vec![what.to_string(), package(), n.id.to_string(), n.app, n.title, n.body, icon, small, url];
    args.extend(browsers);
    args
}

/// Programs' notifications in Android's shade, posted by the helper as the terminal app (which
/// may post them: no permission of Lindroids' own). One at a time, in order: each runs
/// app_process, which takes a moment.
pub struct AndroidServices {
    queue: std::sync::Mutex<std::sync::mpsc::Sender<Vec<String>>>,
}

impl AndroidServices {
    /// None where the helper cannot run (no app_process, an ARM translation layer).
    pub fn new() -> Option<AndroidServices> {
        if !sandbox::on_android() || sandbox::native_bridge() || !Path::new("/system/bin/app_process").exists() {
            return None;
        }
        let (tx, rx) = std::sync::mpsc::channel::<Vec<String>>();
        std::thread::spawn(move || {
            for args in rx {
                if let Err(e) = helper(&args.iter().map(String::as_str).collect::<Vec<_>>()) {
                    log::warn!("android notification: {e}");
                }
            }
        });
        Some(AndroidServices { queue: std::sync::Mutex::new(tx) })
    }
}

impl lindroids_server::android::Android for AndroidServices {
    fn notify(&self, n: lindroids_server::android::Notice) {
        let _ = self.queue.lock().unwrap().send(notice_args("notify", n));
    }

    fn running(&self, n: lindroids_server::android::Notice) {
        let _ = self.queue.lock().unwrap().send(notice_args("running", n));
    }

    fn device(&self) -> Option<lindroids_server::android::Device> {
        let out = helper(&["device"]).ok()?;
        Some(parse_device(&out))
    }

    fn cancel(&self, id: i32) {
        let _ = self.queue.lock().unwrap().send(vec!["cancel".into(), package(), id.to_string()]);
    }
}

#[cfg(test)]
mod device_tests {
    #[test]
    fn device_answer() {
        let d = super::parse_device("battery 57 1\npowersave 0\nok\n");
        assert_eq!(d.battery, Some((57, true)));
        assert_eq!(d.power_saver, Some(false));
        assert_eq!(super::parse_device("ok\n"), lindroids_server::android::Device::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opener_dispatches_launch_urls_and_keeps_the_rest() {
        let s = opener_script("/p/bin/lindroids", Path::new("/h/.local/share/lindroids/termux-url-opener.before"));
        assert!(s.starts_with("#!/data/data/com.termux/files/usr/bin/sh\n"));
        assert!(s.contains(OPENER_MARK));
        assert!(s.contains("exec \"/p/bin/lindroids\" launch \"$id\""));
        assert!(s.contains("exec \"/h/.local/share/lindroids/termux-url-opener.before\" \"$@\""));
    }

    #[test]
    fn dex_is_embedded() {
        assert_eq!(&DEX[..4], b"dex\n");
    }

    #[test]
    fn run_windows_go_into_the_programs_app() {
        let url = "http://k7.localhost:47282/?window&run=42#t=abc";
        assert_eq!(app_page_url(url, "org.gnome.Calculator").as_deref(), Some("http://k7.localhost:47282/app/org.gnome.Calculator/?window&run=42#t=abc"));
        assert_eq!(app_page_url("http://k7.localhost:47282/app/x/?window&run=1", "y"), None);
        assert_eq!(with_via("http://k/app/x/?window&run=42#t=abc", "org.chromium.webapk.a1"), "http://k/app/x/?window&run=42&via=org.chromium.webapk.a1#t=abc");
        assert_eq!(run_of_url(url), Some(42));
        assert_eq!(run_of_url("http://k/?window"), None);
    }

    #[test]
    fn app_browsers_answer() {
        let (can, cannot) = parse_app_browsers("cannot com.android.chrome microG instead of Google Play services\ncan com.sec.android.app.sbrowser\nok\n");
        assert_eq!(can, vec!["com.sec.android.app.sbrowser".to_string()]);
        assert_eq!(cannot, vec![("com.android.chrome".to_string(), "microG instead of Google Play services".to_string())]);
    }

    #[test]
    fn system_browsers_answer() {
        assert_eq!(
            parse_system_browsers("system com.android.chrome /product/app/Chrome/Chrome.apk\nok\n"),
            vec![("com.android.chrome".to_string(), "/product/app/Chrome/Chrome.apk".to_string())]
        );
        assert!(parse_system_browsers("ok\n").is_empty());
    }

    #[test]
    fn launch_urls_name_the_id() {
        assert!(launch_url(10282, "debian.xournalpp").ends_with(":47282/launch/termux/debian.xournalpp"));
    }
}
