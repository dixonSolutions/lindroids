//! `lindroids export`: a home screen shortcut for a Linux program, with its own name and icon.
//!
//! The program stays where it is installed (Termux, a proot-distro distro, a Flatpak) and its
//! launch is recorded in `<state>/apps/<id>.json`; the shortcut belongs to Termux (see
//! shortcut.rs) and runs `lindroids launch <id>` through it, with Termux open or not. Nothing is
//! copied and no app of Lindroids' own is involved.
//!
//! `--webapk` (opt-in) has the browser install the program as an app of its own instead (a WebAPK,
//! made by Chrome or Samsung Internet: docs/WEBAPK.md). Where no browser here can, it says so and
//! does nothing.

use base64::Engine;
use lindroids_proto::Exported;

use crate::run::{self, Target};
use crate::{sandbox, shortcut};

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
    let (new_window, rest) = split_new_window(&args);
    if new_window && webapk {
        return crate::fail("--webapk and --new-window do not go together: a program's app is one window");
    }
    if new_window {
        return export_new_window(&rest);
    }
    export(&args, "", webapk)
}

/// `export`, its shortcut named with `suffix` after the program's name; with `webapk`, an app
/// the browser installs instead of a shortcut.
fn export(args: &[String], suffix: &str, webapk: bool) -> i32 {
    // `lindroids export <id>` of something exported before: again, as it was (its icon is updated
    // in place), rather than a new export of a command named like the id.
    if let [one] = args {
        if let Some(rec) = lindroids_server::load_exported(one).filter(|r| r.run.first() != Some(one) || r.run.len() > 1) {
            let mut again = vec!["--id".to_string(), rec.id.clone()];
            again.extend(rec.run);
            return export(&again, suffix, webapk || rec.webapk.is_some());
        }
    }
    // Before anything is written: where no browser can make the app, nothing is exported.
    let app_browser = if webapk {
        if !sandbox::on_android() {
            return crate::fail("--webapk is for Android: there the browser installs the program as an app of its own");
        }
        match shortcut::app_browser() {
            Ok(b) => Some(b),
            Err(e) => return crate::fail(&e),
        }
    } else {
        None
    };
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
    // The program found its distro above; the launcher keeps it, so it does not depend on looking
    // again (or on which distros exist) when the icon is tapped.
    let mut run = run_args(args);
    if let (Some(d), false) = (&o.distro, args.iter().any(|a| a == "--distro")) {
        run.splice(0..0, ["--distro".to_string(), d.clone()]);
    }
    // An app found for it before stays (exported again: the browser updates it from its manifest).
    let installed = lindroids_server::load_exported(&id).and_then(|r| r.webapk).unwrap_or_default();
    let webapk = app_browser.as_ref().map(|_| installed);
    let rec = Exported { id: id.clone(), name: name.clone(), icon_png_b64: base64::engine::general_purpose::STANDARD.encode(&icon), run, webapk };
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Err(e) = save(&rec) {
        return crate::fail(&e);
    }
    if let Some((browser, browser_name)) = app_browser {
        return install_app(&sb, rec, &icon, &browser, &browser_name);
    }
    match shortcut::create(&sb, &rec, &icon) {
        Ok(how) => {
            println!("{name} exported as {id}. {how}");
            println!("Tapping it starts {name} through Termux (opened if it is closed), in a window of its own.");
            0
        }
        Err(e) => {
            println!("{name} exported as {id}, but no shortcut: {e}");
            println!("`lindroids launch {id}` starts it.");
            1
        }
    }
}

fn save(rec: &Exported) -> Result<(), String> {
    let path = lindroids_server::exported_path(&rec.id).ok_or_else(|| format!("{:?} cannot be an id", rec.id))?;
    std::fs::write(&path, serde_json::to_vec_pretty(rec).unwrap()).map_err(|e| format!("{}: {e}", path.display()))
}

/// How long `export --webapk` waits for the browser to install the app (`LINDROIDS_WEBAPK_WAIT`,
/// seconds; 180).
fn install_wait() -> std::time::Duration {
    std::time::Duration::from_secs(std::env::var("LINDROIDS_WEBAPK_WAIT").ok().and_then(|v| v.parse().ok()).unwrap_or(180))
}

/// `export --webapk`: `browser` installs the program as an app of its own, from the program's
/// install page (`/app/<id>/?install`), which it shows in an ordinary tab: the user taps Install
/// there. Waits for the app to be there, and keeps its package.
fn install_app(sb: &sandbox::Sandbox, mut rec: Exported, icon: &[u8], browser: &str, browser_name: &str) -> i32 {
    let name = rec.name.clone();
    if let Err(e) = shortcut::save_app_icons(&rec.id, icon).and_then(|_| shortcut::prepare_termux(sb)) {
        return crate::fail(&e);
    }
    let rt = match crate::ensure_server(sb) {
        Ok(rt) => rt,
        Err(e) => return crate::fail(&e),
    };
    let (ticket, origin) = match crate::client::request(&rt, &lindroids_proto::CliRequest::Ticket { run: None }, std::time::Duration::from_secs(3)) {
        Ok(lindroids_proto::CliReply::Ticket { ticket, origin }) => (ticket, origin),
        Ok(other) => return crate::fail(&format!("the server answered {other:?}")),
        Err(e) => return crate::fail(&e),
    };
    let scope = format!("{origin}/app/{}/", rec.id);
    if let Some(app) = shortcut::app_for(&scope) {
        rec.webapk = Some(app.clone());
        let _ = save(&rec);
        println!("{name} is an app of its own already ({app}); {browser_name} updates its name and icon by itself.");
        return 0;
    }
    // The ticket pairs the browser, if it has not shown a window yet: the app shares its storage.
    if let Err(e) = shortcut::view(browser, &format!("{scope}?install#t={ticket}")) {
        return crate::fail(&format!("{browser_name} did not open: {e}"));
    }
    println!("{browser_name} shows {name}'s install page: tap Install, then Install in {browser_name}'s dialog.");
    let (t0, wait) = (std::time::Instant::now(), install_wait());
    while t0.elapsed() < wait {
        std::thread::sleep(std::time::Duration::from_secs(2));
        if let Some(app) = shortcut::app_for(&scope) {
            rec.webapk = Some(app.clone());
            if let Err(e) = save(&rec) {
                return crate::fail(&e);
            }
            println!("{name} is an app of its own ({app}): its icon is on the home screen and in the app list.");
            println!("Its icon starts {name}, in a window with its name and icon in Recents. With Termux closed, it asks to start Termux first.");
            return 0;
        }
    }
    println!("No app was installed for {name} in {} s.", wait.as_secs());
    println!("If {browser_name} added a home screen shortcut instead, it could not make an app here (it takes Google Play services, or Samsung's Galaxy Store).");
    println!("`lindroids export {}` again finds an app installed later; without --webapk it gets a Termux icon instead.", rec.id);
    1
}

/// `export --new-window <id or command>`: a second home screen icon, "<name> (new window)", that
/// always starts another instance (`run --new`).
fn export_new_window(args: &[String]) -> i32 {
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
    export(&again, " (new window)", false)
}

/// `lindroids launch [--new] <id>`: what an exported program's shortcut runs (through Termux).
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
        println!("{:<40} {:<24} lindroids run {}", r.id, r.name, r.run.join(" "));
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
    fn run_args_drop_the_id_and_keep_the_command_whole() {
        assert_eq!(run_args(&s(&["--id", "x", "--x11", "xclock", "--id", "y"])), s(&["--x11", "xclock", "--id", "y"]));
        assert_eq!(run_args(&s(&["--name", "N", "--id", "x", "xclock"])), s(&["--name", "N", "xclock"]));
        assert_eq!(run_args(&s(&["--name", "N", "--", "a", "--id"])), s(&["--name", "N", "--", "a", "--id"]));
    }
}
