//! `lindroids export`: a home screen shortcut for a Linux program, with its own name and icon.
//!
//! The program stays where it is installed (Termux, a proot-distro distro, a Flatpak) and its
//! launch is recorded in `<state>/apps/<id>.json`; the shortcut belongs to Termux (see
//! shortcut.rs) and runs `lindroids launch <id>` through it, with Termux open or not. Nothing is
//! copied and no app of Lindroids' own is involved.

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
    let mut out = Vec::new();
    let mut found = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--new-window" && !found {
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
    let (new_window, rest) = split_new_window(args);
    if new_window {
        return export_new_window(&rest);
    }
    // `lindroids export <id>` of something exported before: again, as it was (its icon is updated
    // in place), rather than a new export of a command named like the id.
    if let [one] = args {
        if let Some(rec) = lindroids_server::load_exported(one).filter(|r| r.run.first() != Some(one) || r.run.len() > 1) {
            let mut again = vec!["--id".to_string(), rec.id.clone()];
            again.extend(rec.run);
            return cmd_export(&again);
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
    let rec = Exported { id: id.clone(), name: name.clone(), icon_png_b64: base64::engine::general_purpose::STANDARD.encode(&icon), run };
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Err(e) = std::fs::write(&path, serde_json::to_vec_pretty(&rec).unwrap()) {
        return crate::fail(&format!("{}: {e}", path.display()));
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
    let id = format!("{base_id}.new-window");
    let o = run::parse(&run).unwrap_or_default();
    let name = match run::resolve(&o) {
        Ok(t) => run::identify(&o, &t).0,
        Err(e) => return crate::fail(&e),
    };
    let mut again = vec!["--id".to_string(), id, "--name".into(), format!("{name} (new window)")];
    again.extend(run);
    cmd_export(&again)
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
    if let Err(e) = std::fs::remove_file(&path) {
        return crate::fail(&format!("{}: {e}", path.display()));
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
    fn run_args_drop_the_id_and_keep_the_command_whole() {
        assert_eq!(run_args(&s(&["--id", "x", "--x11", "xclock", "--id", "y"])), s(&["--x11", "xclock", "--id", "y"]));
        assert_eq!(run_args(&s(&["--name", "N", "--id", "x", "xclock"])), s(&["--name", "N", "xclock"]));
        assert_eq!(run_args(&s(&["--name", "N", "--", "a", "--id"])), s(&["--name", "N", "--", "a", "--id"]));
    }
}
