//! `lindroids export`: a launcher for a Linux program, with its own name and icon.
//!
//! The server serves `/app/<id>`: the windows page, named and iconed after the program, which
//! asks the server to start it once it is linked. The browser's "Add to Home screen" turns it
//! into a home screen icon. Nothing is copied and no app is involved: tapping the icon opens the
//! page, the page starts the program through the server, and its windows show there.

use base64::Engine;
use lindroids_proto::{self as proto, Exported};

use crate::run::{self, Target};
use crate::sandbox;

fn default_id(o: &run::Opts, target: &Target, sb: &sandbox::Sandbox) -> String {
    match target {
        Target::Flatpak(app, _) => app.id.clone(),
        Target::Program(_) => {
            let place = o.distro.clone().unwrap_or_else(|| match sb.kind {
                sandbox::Kind::Termux => "termux".into(),
                _ => "linux".into(),
            });
            let prog = run::program_of(&o.command);
            let safe: String = prog.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
            format!("{place}.{safe}")
        }
    }
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

fn load_all() -> Vec<Exported> {
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
    if let Err(e) = crate::ensure_server(&sb) {
        return crate::fail(&e);
    }
    let url = format!("http://127.0.0.1:{}/app/{id}", proto::server_tcp_port(sb.uid));
    println!("{name} exported as {id}.");
    // The launcher page starts the program, so this also shows it works.
    let secret = lindroids_server::load_or_create_secret();
    if sandbox::open_browser(&format!("{url}#k={secret}")).is_some() {
        println!("Its page is open in the browser: choose \"Add to Home screen\" (or \"Install app\") in the");
        println!("browser's menu for an icon that starts it.");
    } else {
        println!("Open {url} in the browser and choose \"Add to Home screen\" in its menu.");
    }
    println!("The icon works while the Lindroids server runs (`lindroids start` in Termux).");
    0
}

/// `lindroids launch <id>`: what a launcher page starts (through the server).
pub fn cmd_launch(args: &[String]) -> i32 {
    let Some(id) = args.first() else { return crate::fail("which one? (lindroids list)") };
    // The server says so when a page asked; the program itself should not inherit that.
    if std::env::var_os("LINDROIDS_LAUNCHED").is_some() {
        crate::LAUNCHED_BY_PAGE.store(true, std::sync::atomic::Ordering::Relaxed);
        std::env::remove_var("LINDROIDS_LAUNCHED");
    }
    match lindroids_server::load_exported(id) {
        Some(rec) => run::cmd_run(&rec.run),
        None => crate::fail(&format!("nothing exported as {id:?} (lindroids list)")),
    }
}

pub fn cmd_list() -> i32 {
    let recs = load_all();
    if recs.is_empty() {
        println!("Nothing exported from here yet.");
    }
    let port = proto::server_tcp_port(sandbox::detect().uid);
    for r in recs {
        println!("{:<40} {:<24} http://127.0.0.1:{port}/app/{}", r.id, r.name, r.id);
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
    println!("Removed {id}. A home screen icon for it now only shows that it is gone; remove it there.");
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn run_args_drop_the_id_and_keep_the_command_whole() {
        assert_eq!(run_args(&s(&["--id", "x", "--x11", "xclock", "--id", "y"])), s(&["--x11", "xclock", "--id", "y"]));
        assert_eq!(run_args(&s(&["--name", "N", "--id", "x", "xclock"])), s(&["--name", "N", "xclock"]));
        assert_eq!(run_args(&s(&["--name", "N", "--", "a", "--id"])), s(&["--name", "N", "--", "a", "--id"]));
    }
}
