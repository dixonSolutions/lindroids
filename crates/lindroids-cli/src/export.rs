//! `lindroids export`: a home screen icon for a Linux program, with its own name and icon.
//!
//! The program stays where it is installed (Termux, a proot-distro distro, a Flatpak) and its
//! launch is recorded in `<state>/apps/<id>.json`; the icon belongs to Termux (see shortcut.rs)
//! and runs `lindroids launch <id>` through it, with Termux open or not. Nothing is copied and no
//! app of Lindroids' own is involved.
//!
//! Where a browser can make it an app of its own (a WebAPK, made by Chrome or Samsung Internet:
//! docs/WEBAPK.md), the icon is that app. Which browser, and whether at all, is the export's
//! `--browser` tag, else the setting (`lindroids config browser`), with the default browser
//! first whenever it can. `--run` launches the export at once.

use base64::Engine;
use lindroids_proto::Exported;

use crate::run::{self, Target};
use crate::{config, sandbox, shortcut};

fn default_id(o: &run::Opts, target: &Target, sb: &sandbox::Sandbox) -> String {
    crate::instances::app_key(o, target, sb)
}

/// `export --new-window ...`: the export of a second icon that always opens a new window
/// (`lindroids run --new`), next to the program's own icon; its id gets `.new-window`.
fn split_new_window(args: &[String]) -> (bool, Vec<String>) {
    split_flag(args, "--new-window")
}

/// Takes export's own `flag` out of its arguments (before the command: the command's own options
/// stay), and says whether it was there.
fn split_flag(args: &[String], flag: &str) -> (bool, Vec<String>) {
    let mut out = Vec::new();
    let mut found = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == flag && !found {
            found = true;
            continue;
        }
        let command = !a.starts_with('-') || a == "--";
        out.push(a.clone());
        if run::takes_value(a) {
            out.extend(it.next().cloned());
        } else if command {
            out.extend(it.cloned());
            break;
        }
    }
    (found, out)
}

/// Like `split_flag` for `flag <value>` or `flag=<value>`: the value, and the rest.
fn split_valued(args: &[String], flag: &str) -> (Option<String>, Vec<String>) {
    let mut out = Vec::new();
    let mut value = None;
    let mut it = args.iter();
    let prefix = format!("{flag}=");
    while let Some(a) = it.next() {
        if value.is_none() {
            if a == flag {
                value = Some(it.next().cloned().unwrap_or_default());
                continue;
            }
            if let Some(v) = a.strip_prefix(&prefix) {
                value = Some(v.to_string());
                continue;
            }
        }
        let command = !a.starts_with('-') || a == "--";
        out.push(a.clone());
        if run::takes_value(a) {
            out.extend(it.next().cloned());
        } else if command {
            out.extend(it.cloned());
            break;
        }
    }
    (value, out)
}

/// The `lindroids run` arguments that start the same thing: the export's own, minus `--id`.
fn run_args(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--" {
            out.push(a.clone());
            out.extend(it.cloned());
            break;
        }
        if a == "--id" {
            it.next();
            continue;
        }
        out.push(a.clone());
        if run::takes_value(a) {
            out.extend(it.next().cloned());
        } else if !a.starts_with('-') {
            // The command: everything after it is its own.
            out.extend(it.cloned());
            break;
        }
    }
    out
}

fn apps_dir() -> std::path::PathBuf {
    lindroids_server::state_dir().join("apps")
}

pub(crate) fn load_all() -> Vec<Exported> {
    let mut v: Vec<Exported> = std::fs::read_dir(apps_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().map_or(false, |x| x == "json"))
        .filter_map(|e| std::fs::read(e.path()).ok())
        .filter_map(|b| serde_json::from_slice(&b).ok())
        .collect();
    v.sort_by(|a, b| a.id.cmp(&b.id));
    v
}

pub fn cmd_export(args: &[String]) -> i32 {
    let (webapk, args) = split_flag(args, "--webapk");
    let (run_now, args) = split_flag(&args, "--run");
    let (browser, args) = split_valued(&args, "--browser");
    if webapk && browser.is_some() {
        return crate::fail("--webapk and --browser both choose the browser: give one");
    }
    let tag = browser.or_else(|| webapk.then(|| "webapk".to_string()));
    if let Some(t) = &tag {
        if !config::valid_browser(t) {
            return crate::fail(&format!("--browser {t:?}: use webapk, default, home-screen, or a browser's package (e.g. com.android.chrome)"));
        }
    }
    if let Some(pos) = args.iter().position(|a| a == "convert") {
        return convert(&args[..pos], &args[pos + 1..]);
    }
    let (new_window, rest) = split_new_window(&args);
    if new_window && tag.is_some() {
        return crate::fail("--webapk and --browser do not go with --new-window: a second icon takes the browser setting");
    }
    if new_window {
        return export_new_window(&rest, run_now);
    }
    export(&args, "", tag, run_now)
}

/// What an export becomes: an app made by that browser, or a home screen icon (the flag: an app
/// was wanted but no browser can make one).
enum Plan {
    App(String),
    Shortcut(bool),
}

/// The browser that makes the app for `choice` (`--browser` or the setting), if any: the default
/// browser when it can, whatever the choice; else the choice's. `explicit` says the choice was
/// given for this export, where a missing browser is an error rather than a home screen icon.
fn plan(choice: &str, explicit: bool) -> Result<Plan, String> {
    if choice == "home-screen" {
        return Ok(Plan::Shortcut(false));
    }
    if !sandbox::on_android() {
        return if explicit && choice != "default" {
            Err("a WebAPK is for Android: there the browser installs the program as an app of its own".into())
        } else {
            Ok(Plan::Shortcut(false))
        };
    }
    let (can, cannot) = match shortcut::app_browsers() {
        Ok(x) => x,
        Err(e) if explicit => return Err(e),
        Err(_) => return Ok(Plan::Shortcut(true)),
    };
    let default = shortcut::default_browser();
    if let Some(b) = choose(choice, default.as_deref(), &can) {
        return Ok(Plan::App(b));
    }
    if !explicit || choice == "default" {
        return Ok(Plan::Shortcut(true));
    }
    if let Some((b, why)) = cannot.iter().find(|(b, _)| b == choice) {
        return Err(format!("{} cannot make an app of its own: {why}", shortcut::browser_name(b)));
    }
    let system = shortcut::system_browsers();
    let found = if matches!(choice, "webapk" | "default") {
        system.first()
    } else {
        system.iter().find(|(p, _)| p == choice)
    };
    if let Some((p, _)) = found {
        if let Some((_, why)) = cannot.iter().find(|(b, _)| b == p) {
            return Err(format!("{} cannot make an app of its own: {why}", shortcut::browser_name(p)));
        }
        // On the system image and hidden from lookups (or installed for the user and invisible):
        // the install page will tell whether it opens. Android may have hidden the browser from
        // queries but still runs it by package (the tablet's Chrome).
        return Ok(Plan::App(p.clone()));
    }
    // Explicitly asked for a WebAPK, and Play services/Play Store leave Chrome possible but
    // Android hides packages from lookups: try the install page. `view` says if Chrome is absent.
    if explicit && matches!(choice, "webapk" | "com.android.chrome" | "com.chrome.beta" | "com.chrome.dev" | "com.chrome.canary") {
        let pkg = if choice == "webapk" { "com.android.chrome".to_string() } else { choice.to_string() };
        if let Some((_, why)) = cannot.iter().find(|(b, _)| b == &pkg) {
            return Err(format!("{} cannot make an app of its own: {why}", shortcut::browser_name(&pkg)));
        }
        return Ok(Plan::App(pkg));
    }
    if config::valid_browser(choice) && choice.contains('.') {
        return Err(format!("{choice} is not installed"));
    }
    Err(no_browser(&cannot))
}

/// The browser that makes the app, for `choice` (`webapk`, `default`, or a package): the default
/// browser if it can, whatever the choice (`home-screen` never: it is an explicit icon); else the
/// choice's own, if it can.
pub(crate) fn choose(choice: &str, default: Option<&str>, can: &[String]) -> Option<String> {
    if choice != "home-screen" {
        if let Some(d) = default.filter(|d| can.iter().any(|c| c == d)) {
            return Some(d.to_string());
        }
    }
    match choice {
        "webapk" => can.first().cloned(),
        "default" | "home-screen" => None,
        pkg => can.iter().find(|c| c.as_str() == pkg).cloned(),
    }
}

fn no_browser(cannot: &[(String, String)]) -> String {
    let mut msg = String::from(
        "no browser here can install a program as an app of its own. That takes Chrome with Google Play \
         services, or Samsung Internet on a Samsung phone with the Galaxy Store (other browsers make only \
         shortcuts)",
    );
    if cannot.is_empty() {
        msg.push_str(": neither is installed");
    }
    for (b, why) in cannot {
        msg.push_str(&format!("; {}: {why}", shortcut::browser_name(b)));
    }
    msg.push_str(". Without --browser webapk, it gets a home screen icon instead.");
    msg
}

/// `export`, its icon named with `suffix` after the program's name; `tag` is its browser (for
/// this export), `launch` starts it when exported.
fn export(args: &[String], suffix: &str, tag: Option<String>, launch: bool) -> i32 {
    // `lindroids export <id>` of something exported before: again, as it was (its icon is updated
    // in place), rather than a new export of a command named like the id.
    if let [one] = args {
        if let Some(rec) = lindroids_server::load_exported(one).filter(|r| r.run.first() != Some(one) || r.run.len() > 1) {
            let mut again = vec!["--id".to_string(), rec.id.clone()];
            let tag = tag.or(rec.browser.clone());
            again.extend(rec.run);
            return export(&again, suffix, tag, launch);
        }
    }
    let mut o = match run::parse(args) {
        Ok(o) => o,
        Err(e) => return crate::fail(&e),
    };
    run::pick_distro(&mut o);
    let sb = sandbox::detect();
    let target = match run::resolve(&o) {
        Ok(t) => t,
        Err(e) => return crate::fail(&e),
    };
    let (name, icon) = run::identify(&o, &target);
    let name = format!("{name}{suffix}");
    let id = o.id.clone().unwrap_or_else(|| default_id(&o, &target, &sb));
    let Some(path) = lindroids_server::exported_path(&id) else {
        return crate::fail(&format!("{id:?} cannot be an id: use letters, digits, '.', '-' and '_'"));
    };
    let choice = tag.clone().unwrap_or_else(config::browser_setting);
    let plan = match plan(&choice, tag.is_some()) {
        Ok(p) => p,
        Err(e) => return crate::fail(&e),
    };
    // The program found its distro above; the launcher keeps it, so it does not depend on looking
    // again (or on which distros exist) when the icon is tapped.
    let mut run = run_args(args);
    if let (Some(d), false) = (&o.distro, args.iter().any(|a| a == "--distro")) {
        run.splice(0..0, ["--distro".to_string(), d.clone()]);
    }
    let previous = lindroids_server::load_exported(&id);
    let previous_app = previous.as_ref().and_then(|r| r.webapk.clone());
    let kept_tag = tag.or_else(|| previous.and_then(|r| r.browser));
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let mut rec = Exported {
        id: id.clone(),
        name: name.clone(),
        icon_png_b64: base64::engine::general_purpose::STANDARD.encode(&icon),
        run,
        webapk: None,
        browser: kept_tag,
    };
    if let Plan::App(browser) = &plan {
        // An app found for it before stays (exported again: the browser updates it from its manifest).
        rec.webapk = Some(previous_app.clone().unwrap_or_default());
        if let Err(e) = save(&rec) {
            return crate::fail(&e);
        }
        let browser_name = shortcut::browser_name(browser).to_string();
        match install_app(&sb, &id, &name, &icon, browser, &browser_name) {
            Err(e) => {
                rec.webapk = None;
                let _ = save(&rec);
                return crate::fail(&e);
            }
            Ok(Some(app)) => {
                rec.webapk = Some(app);
                if let Err(e) = save(&rec) {
                    return crate::fail(&e);
                }
                // The home screen shortcut is not the app: Android lets only its owner disable it,
                // and only the launcher unpin it (the icon stays, greyed, until dragged off).
                if shortcut::remove(&sb, &id).is_ok() {
                    println!("Its old home screen shortcut is greyed out: drag it off (only the launcher can remove a pinned shortcut).");
                }
                return if launch { export_launch(&id) } else { 0 };
            }
            Ok(None) => {
                println!("No app was installed for {name} in {} s: a home screen icon instead.", install_wait().as_secs());
                println!("`lindroids export --browser webapk {id}` again finds an app installed later.");
            }
        }
    } else if let Plan::Shortcut(true) = plan {
        println!("No browser here makes an app of its own for {name}: a home screen icon.");
    }
    if let Some(app) = previous_app.as_deref().filter(|a| !a.is_empty()) {
        println!("{name} is still an app ({app}) of its own: tap Uninstall in Android's page for it to take it away.");
        let _ = shortcut::app_settings(app);
    }
    rec.webapk = None;
    if let Err(e) = save(&rec) {
        return crate::fail(&e);
    }
    match shortcut::create(&sb, &rec, &icon) {
        Ok(how) => {
            println!("{name} exported as {id}. {how}");
            println!("Tapping it starts {name} through Termux (opened if it is closed), in a window of its own.");
            if launch { export_launch(&id) } else { 0 }
        }
        Err(e) => {
            println!("{name} exported as {id}, but no shortcut: {e}");
            println!("`lindroids launch {id}` starts it.");
            1
        }
    }
}

fn export_launch(id: &str) -> i32 {
    cmd_launch(&[id.to_string()])
}

fn save(rec: &Exported) -> Result<(), String> {
    let path = lindroids_server::exported_path(&rec.id).ok_or_else(|| format!("{:?} cannot be an id", rec.id))?;
    std::fs::write(&path, serde_json::to_vec_pretty(rec).unwrap()).map_err(|e| format!("{}: {e}", path.display()))
}

/// How long `export` waits for the browser to install the app (`LINDROIDS_WEBAPK_WAIT`, seconds;
/// 180).
fn install_wait() -> std::time::Duration {
    std::time::Duration::from_secs(std::env::var("LINDROIDS_WEBAPK_WAIT").ok().and_then(|v| v.parse().ok()).unwrap_or(180))
}

/// `browser` installs the program as an app of its own, from the program's install page
/// (`/app/<id>/?install`), which it shows in an ordinary tab: the user taps Install there. Returns
/// the app's package once it is there; None if it is not in time.
fn install_app(sb: &sandbox::Sandbox, id: &str, name: &str, icon: &[u8], browser: &str, browser_name: &str) -> Result<Option<String>, String> {
    shortcut::save_app_icons(id, icon).and_then(|_| shortcut::prepare_termux(sb))?;
    let rt = crate::ensure_server(sb)?;
    let (ticket, origin) = match crate::client::request(&rt, &lindroids_proto::CliRequest::Ticket { run: None }, std::time::Duration::from_secs(3))
        .map_err(|e| e.to_string())?
    {
        lindroids_proto::CliReply::Ticket { ticket, origin } => (ticket, origin),
        other => return Err(format!("the server answered {other:?}")),
    };
    let scope = format!("{origin}/app/{id}/");
    let flag = sandbox::runtime_dir().join(format!("app-installed-{id}"));
    let _ = std::fs::remove_file(&flag);
    // `LINDROIDS_WEBAPK_FRESH=1`: install again, ignoring an app found by lookup or already
    // recorded (a test: the install page's own `appinstalled` is the only yes).
    let fresh = std::env::var_os("LINDROIDS_WEBAPK_FRESH").is_some();
    if !fresh {
        if let Some(app) = shortcut::app_for(&scope) {
            println!("{name} is an app of its own already ({app}); {browser_name} updates its name and icon by itself.");
            return Ok(Some(app));
        }
    }
    // The ticket pairs the browser, if it has not shown a window yet: the app shares its storage.
    let fresh_query = if fresh { "&fresh" } else { "" };
    shortcut::view(browser, &format!("{scope}?install{fresh_query}#t={ticket}")).map_err(|e| format!("{browser_name} did not open: {e}"))?;
    println!("{browser_name} shows {name}'s install page: tap Install, then Install in {browser_name}'s dialog.");
    let (t0, wait) = (std::time::Instant::now(), install_wait());
    while t0.elapsed() < wait {
        std::thread::sleep(std::time::Duration::from_secs(2));
        if !fresh {
            if let Some(app) = shortcut::app_for(&scope) {
                println!("{name} is an app of its own ({app}): its icon is on the home screen and in the app list.");
                println!("Its icon starts {name}, in a window with its name and icon in Recents. With Termux closed, it asks to start Termux first.");
                return Ok(Some(app));
            }
        }
        if flag.exists() {
            let _ = std::fs::remove_file(&flag);
            println!("{name} is an app of its own: its icon is on the home screen and in the app list.");
            println!("Android hides its package from lookups here, so its page opens the program in the app.");
            return Ok(Some(String::new()));
        }
    }
    if flag.exists() {
        let _ = std::fs::remove_file(&flag);
        return Ok(Some(String::new()));
    }
    Ok(None)
}

/// `export --new-window <id or command>`: a second home screen icon, "<name> (new window)", that
/// always starts another instance (`run --new`).
fn export_new_window(args: &[String], launch: bool) -> i32 {
    // Of something exported already (its id), or of a command as `export` takes it.
    let (base_id, mut run) = match args {
        [one] if lindroids_server::load_exported(one).is_some() => {
            let rec = lindroids_server::load_exported(one).unwrap();
            (rec.id, rec.run)
        }
        _ => {
            let o = match run::parse(args) {
                Ok(o) => o,
                Err(e) => return crate::fail(&e),
            };
            let mut o = o;
            run::pick_distro(&mut o);
            let target = match run::resolve(&o) {
                Ok(t) => t,
                Err(e) => return crate::fail(&e),
            };
            let id = o.id.clone().unwrap_or_else(|| default_id(&o, &target, &sandbox::detect()));
            (id, run_args(args))
        }
    };
    if !run.iter().any(|a| a == "--new") {
        run.insert(0, "--new".into());
    }
    let mut again = vec!["--id".to_string(), format!("{base_id}.new-window")];
    again.extend(run);
    export(&again, " (new window)", None, launch)
}

/// `export <program> convert home-screen --to=webapk`, and `convert webapk --to=home-screen`: the
/// program's icon becomes the other kind (an explicit choice: the browser setting does not
/// override it).
fn convert(target: &[String], rest: &[String]) -> i32 {
    let target = target.join(" ");
    let from = rest.first().map(String::as_str).unwrap_or("");
    let mut to = None;
    let mut it = rest.iter().skip(1);
    while let Some(a) = it.next() {
        if let Some(v) = a.strip_prefix("--to=") {
            to = Some(v.to_string());
        } else if a == "--to" {
            to = it.next().cloned();
        }
    }
    let to = to.unwrap_or_default();
    if !matches!((from, to.as_str()), ("home-screen", "webapk") | ("webapk", "home-screen")) {
        return crate::fail("convert home-screen --to=webapk, or convert webapk --to=home-screen");
    }
    let Some(rec) = load_all().into_iter().find(|r| !target.is_empty() && (r.id == target || r.name == target)) else {
        return crate::fail(&format!("nothing exported as {target:?} (lindroids list)"));
    };
    let is_app = rec.webapk.is_some();
    if (from == "webapk") != is_app {
        let now = if is_app { "an app of its own" } else { "a home screen icon" };
        return crate::fail(&format!("{} is {now} already", rec.name));
    }
    let mut again = vec!["--id".to_string(), rec.id.clone()];
    again.extend(rec.run.clone());
    export(&again, "", Some(to), false)
}

/// `lindroids launch [--new] <id>`: what an exported program's icon runs (through Termux).
/// Tapping the icon of a program that runs brings back its most recently used window; only
/// `--new` (or an icon exported with `--new-window`) starts another.
pub fn cmd_launch(args: &[String]) -> i32 {
    let new = args.iter().any(|a| a == "--new");
    let force = args.iter().any(|a| a == "--force");
    let Some(id) = args.iter().find(|a| !a.starts_with("--")) else { return crate::fail("which one? (lindroids list)") };
    let Some(rec) = lindroids_server::load_exported(id) else {
        return crate::fail(&format!("nothing exported as {id:?} (lindroids list)"));
    };
    // Installed as an app of its own (`export --webapk`): its window opens in that app.
    if let Some(app) = &rec.webapk {
        shortcut::set_app_window(id, app);
    }
    let mut run = rec.run.clone();
    for (flag, on) in [("--new", new), ("--force", force)] {
        if on && !run.iter().any(|a| a == flag) {
            run.insert(0, flag.into());
        }
    }
    let sb = sandbox::detect();
    if !run.iter().any(|a| a == "--new") && sandbox::on_android() {
        if let Some(shown) = running_instance(&sb, &run) {
            return if shown { 0 } else { crate::fail("its window could not be opened") };
        }
    }
    run::cmd_run_launched(&run, Some(id))
}

/// The most recently used instance of what `run` starts, brought forward (its window opened
/// again), if one runs: true if so.
fn running_instance(sb: &sandbox::Sandbox, run: &[String]) -> Option<bool> {
    let mut o = run::parse(run).ok()?;
    run::pick_distro(&mut o);
    let target = run::resolve(&o).ok()?;
    let key = crate::instances::app_key(&o, &target, sb);
    let rt = crate::ensure_server(sb).ok()?;
    let (instances, _) = crate::instances::query(&rt, &key);
    let last = instances.first()?;
    eprintln!("lindroids: {} is running: bringing back its window", last.name);
    Some(crate::open_run_window(sb, &rt, last.pid))
}

pub fn cmd_list() -> i32 {
    let recs = load_all();
    if recs.is_empty() {
        println!("Nothing exported from here yet.");
    }
    for r in recs {
        let kind = if r.webapk.is_some() { "app" } else { "icon" };
        println!("{:<40} {:<24} {:<5} lindroids run {}", r.id, r.name, kind, r.run.join(" "));
    }
    0
}

pub fn cmd_remove(args: &[String]) -> i32 {
    let Some(id) = args.first() else { return crate::fail("which one? (lindroids list)") };
    let Some(path) = lindroids_server::exported_path(id).filter(|p| p.exists()) else {
        return crate::fail(&format!("nothing exported as {id:?} (lindroids list)"));
    };
    let app = lindroids_server::load_exported(id).and_then(|r| r.webapk);
    if let Err(e) = std::fs::remove_file(&path) {
        return crate::fail(&format!("{}: {e}", path.display()));
    }
    if let Some(app) = app {
        let _ = std::fs::remove_file(lindroids_server::state_dir().join("apps").join(format!("{id}.png")));
        let _ = std::fs::remove_file(lindroids_server::state_dir().join("apps").join(format!("{id}.maskable.png")));
        let _ = std::fs::remove_file(lindroids_server::state_dir().join("apps").join(format!("{id}.app.png")));
        // An app cannot uninstall another without asking: Android's page for it has Uninstall.
        if app.is_empty() {
            println!("Removed {id}.");
        } else if shortcut::app_settings(&app).is_ok() {
            println!("Removed {id}. Android shows its app ({app}): tap Uninstall to take it away.");
        } else {
            println!("Removed {id}. Its app ({app}) stays until you uninstall it (long-press its icon).");
        }
        return 0;
    }
    match shortcut::remove(&sandbox::detect(), id) {
        Ok(()) => println!("Removed {id}. Its home screen icon is greyed out now; drag it off to remove it."),
        Err(e) => println!("Removed {id} (its shortcut stays: {e})."),
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn new_window_exports_are_told_apart() {
        assert_eq!(split_new_window(&s(&["--new-window", "com.vscodium.codium"])), (true, s(&["com.vscodium.codium"])));
        assert_eq!(split_new_window(&s(&["--name", "N", "--new-window", "foot"])), (true, s(&["--name", "N", "foot"])));
        // The command's own --new-window is the command's.
        assert_eq!(split_new_window(&s(&["codium", "--new-window"])), (false, s(&["codium", "--new-window"])));
    }

    #[test]
    fn webapk_is_exports_own_flag() {
        assert_eq!(split_flag(&s(&["--webapk", "mousepad"]), "--webapk"), (true, s(&["mousepad"])));
        assert_eq!(split_flag(&s(&["--name", "N", "--webapk", "--distro", "debian", "gimp"]), "--webapk"), (true, s(&["--name", "N", "--distro", "debian", "gimp"])));
        assert_eq!(split_flag(&s(&["foot", "--webapk"]), "--webapk"), (false, s(&["foot", "--webapk"])));
    }

    #[test]
    fn browser_tag_is_taken_before_the_command() {
        assert_eq!(split_valued(&s(&["--browser", "default", "foot"]), "--browser"), (Some("default".into()), s(&["foot"])));
        assert_eq!(split_valued(&s(&["--browser=com.android.chrome", "foot", "--browser", "x"]), "--browser"), (Some("com.android.chrome".into()), s(&["foot", "--browser", "x"])));
        assert_eq!(split_valued(&s(&["foot", "--browser", "x"]), "--browser"), (None, s(&["foot", "--browser", "x"])));
    }

    #[test]
    fn default_browser_wins_over_the_tag_and_the_setting() {
        let chrome = "com.android.chrome";
        let samsung = "com.sec.android.app.sbrowser";
        let can = s(&[chrome, samsung]);
        assert_eq!(choose("default", Some(samsung), &can), Some(samsung.to_string()));
        assert_eq!(choose("webapk", Some(samsung), &can), Some(samsung.to_string()));
        assert_eq!(choose(chrome, Some(samsung), &can), Some(samsung.to_string()));
        assert_eq!(choose("webapk", Some("org.mozilla.firefox"), &can), Some(chrome.to_string()));
        assert_eq!(choose("default", Some("org.mozilla.firefox"), &can), None);
        assert_eq!(choose(samsung, None, &can[..1]), None);
        assert_eq!(choose(chrome, None, &can), Some(chrome.to_string()));
    }

    #[test]
    fn home_screen_is_an_explicit_icon() {
        let chrome = "com.android.chrome";
        assert_eq!(choose("home-screen", Some(chrome), &s(&[chrome])), None);
    }

    #[test]
    fn run_args_drop_the_id_and_keep_the_command_whole() {
        assert_eq!(run_args(&s(&["--id", "x", "--x11", "xclock", "--id", "y"])), s(&["--x11", "xclock", "--id", "y"]));
        assert_eq!(run_args(&s(&["--name", "N", "--id", "x", "xclock"])), s(&["--name", "N", "xclock"]));
        assert_eq!(run_args(&s(&["--name", "N", "--", "a", "--id"])), s(&["--name", "N", "--", "a", "--id"]));
    }
}
