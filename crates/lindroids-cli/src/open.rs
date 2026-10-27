//! `lindroids open --linux <file>`: open a file in a Linux program here, in a window of its own.
//!
//! This is the way in for Android's files. Termux receives a file from the Files app ("Open with")
//! or from Share, asks for a name, saves it in `~/downloads` and, when the user taps "Edit", runs
//! `~/bin/termux-file-editor <path>` in a new session; Lindroids installs that script (see
//! shortcut.rs) to run `lindroids open --linux <path>`.
//!
//! The program is chosen by the file's MIME type among the programs that say they open it in their
//! .desktop file (`MimeType=`): programs exported to the home screen first, then Termux's own,
//! Flatpaks, and programs in proot-distro distros. The user's choice for a type is kept where the
//! desktop keeps it, `~/.config/mimeapps.list`. The file keeps its path: a distro or Flatpak is
//! shown the file's folder at the same path (`lindroids run --bind`).
//!
//! (Plain `lindroids open <file>` is the other way: Android opens it; Linux programs' `xdg-open`.)

use std::io::{BufRead, IsTerminal, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};

use lindroids_proto::{TERMUX_HOME, TERMUX_PREFIX};

use crate::{desktop, export, flatpak, run, sandbox};

/// A program that can open files, from its .desktop file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct App {
    /// Its desktop id (`org.gnome.TextEditor.desktop`), what mimeapps.list names.
    pub desktop_id: String,
    pub name: String,
    /// Where it lives: "Termux", "Flatpak", a distro's name.
    pub place: String,
    pub mime_types: Vec<String>,
    /// The command: `Exec=` split into words, field codes (%f, %U...) still in.
    pub exec: Vec<String>,
    pub icon: Option<String>,
    pub desktop_file: Option<PathBuf>,
    /// `lindroids run` options before the command (`--distro`, an exported program's own).
    pub opts: Vec<String>,
    /// The id it is exported (on the home screen) as.
    pub exported: Option<String>,
}

const USAGE: &str = "lindroids open --linux <file> [--with <program>] [--choose] [--list]";

/// `lindroids open --linux|--with <program>|--choose|--list <file>`.
pub fn cmd_open_linux(args: &[String]) -> i32 {
    let mut with = None;
    let mut choose = false;
    let mut list = false;
    let mut file = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--linux" => {}
            "--with" => match it.next() {
                Some(w) => with = Some(w.clone()),
                None => return crate::fail(&format!("--with needs a program ({USAGE})")),
            },
            s if s.starts_with("--with=") => with = Some(s["--with=".len()..].to_string()),
            "--choose" => choose = true,
            "--list" => list = true,
            "--" => file = it.next().cloned(),
            s if s.starts_with("--") => return crate::fail(&format!("unknown option {s} ({USAGE})")),
            _ if file.is_none() => file = Some(a.clone()),
            _ => return crate::fail(&format!("one file at a time ({USAGE})")),
        }
    }
    let Some(file) = file else { return crate::fail(&format!("open what? ({USAGE})")) };
    let path = absolute(Path::new(file.strip_prefix("file://").unwrap_or(&file)));
    if !path.is_file() {
        return crate::fail(&format!("{}: no such file", path.display()));
    }
    let mime = guess_mime(&path);
    let all = catalogue();

    if let Some(w) = with {
        return open_with(&with_app(&all, &w), &path);
    }
    let (cands, used) = candidates(&all, &mime);
    let config = mimeapps_path();
    let defaults_text = std::fs::read_to_string(&config).unwrap_or_default();
    let mut defaults = default_apps(&defaults_text, &mime);
    if used != mime {
        defaults.extend(default_apps(&defaults_text, &used));
    }
    let interactive = std::io::stdin().is_terminal();
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();

    if list {
        println!("{name}: {mime}{}", if used != mime { format!(" (programs for {used})") } else { String::new() });
        let chosen = pick(&cands, &defaults, false);
        for (i, c) in cands.iter().enumerate() {
            let mark = if chosen == Some(Pick::Use(i)) { "*" } else { " " };
            println!("{mark} {:<36} {}", c.desktop_id, describe(c));
        }
        if cands.is_empty() {
            println!("  no program here says it opens these files");
        }
        return 0;
    }

    let previous = previous_editor();
    let picked = if choose && !cands.is_empty() { Some(Pick::Ask) } else { pick(&cands, &defaults, interactive) };
    match picked {
        Some(Pick::Use(i)) => open_with(&cands[i], &path),
        Some(Pick::Ask) if interactive => {
            let title = format!("Open {name} ({mime}) with:");
            match ask(&title, &cands, previous.as_deref()) {
                Answer::App(i) => {
                    let c = &cands[i];
                    let text = set_default(&defaults_text, &mime, &c.desktop_id);
                    match write_file(&config, &text) {
                        Ok(()) => eprintln!(
                            "lindroids: {} opens {mime} files from now on ({}; `lindroids open --choose <file>` to change it)",
                            c.name,
                            config.display()
                        ),
                        Err(e) => eprintln!("lindroids: cannot remember the choice: {e}"),
                    }
                    open_with(c, &path)
                }
                Answer::Previous => exec_previous(previous.as_deref().unwrap(), &path),
                Answer::Cancel => 1,
            }
        }
        Some(Pick::Ask) => open_with(&cands[0], &path),
        None => {
            if let Some(p) = &previous {
                return exec_previous(p, &path);
            }
            // Nothing says it opens these: an exported program may still (the user's guess).
            let exported: Vec<App> = all.iter().filter(|a| a.exported.is_some()).cloned().collect();
            eprintln!("lindroids: no Linux program here says it opens {mime} files ({name}).");
            if interactive && !exported.is_empty() {
                if let Answer::App(i) = ask("Try one of the programs on the home screen?", &exported, None) {
                    return open_with(&exported[i], &path);
                }
                return 1;
            }
            eprintln!("  Install one, or name it: lindroids open --with <program> \"{}\"", path.display());
            1
        }
    }
}

/// `p` made absolute as given (no symlinks resolved: a sandbox knows the path the user sees).
fn absolute(p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().map(|d| d.join(p)).unwrap_or_else(|_| p.to_path_buf())
    }
}

fn describe(a: &App) -> String {
    let mut s = format!("{} ({})", a.name, a.place);
    if let Some(id) = &a.exported {
        s += &format!(", on the home screen as {id}");
    }
    s
}

// ---------------------------------------------------------------------------------------------
// Choosing

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pick {
    Use(usize),
    Ask,
}

/// Which of `cands` opens the file: the user's default (desktop ids from mimeapps.list), else the
/// only one, else ask, or with no one to ask, the first exported program (else the first).
pub fn pick(cands: &[App], defaults: &[String], interactive: bool) -> Option<Pick> {
    if cands.is_empty() {
        return None;
    }
    for d in defaults {
        if let Some(i) = cands.iter().position(|c| &c.desktop_id == d) {
            return Some(Pick::Use(i));
        }
    }
    if cands.len() == 1 {
        return Some(Pick::Use(0));
    }
    if interactive {
        return Some(Pick::Ask);
    }
    Some(Pick::Use(cands.iter().position(|c| c.exported.is_some()).unwrap_or(0)))
}

enum Answer {
    App(usize),
    Previous,
    Cancel,
}

fn ask(title: &str, cands: &[App], previous: Option<&Path>) -> Answer {
    let mut err = std::io::stderr();
    let _ = writeln!(err, "{title}");
    for (i, c) in cands.iter().enumerate() {
        let _ = writeln!(err, "  {}) {}", i + 1, describe(c));
    }
    let prev_n = cands.len() + 1;
    if let Some(p) = previous {
        let _ = writeln!(err, "  {prev_n}) the termux-file-editor you had ({})", p.display());
    }
    let max = if previous.is_some() { prev_n } else { cands.len() };
    let stdin = std::io::stdin();
    loop {
        let _ = write!(err, "Number (Enter: 1, q: cancel): ");
        let _ = err.flush();
        let mut line = String::new();
        if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
            return Answer::Cancel;
        }
        let line = line.trim();
        if line.is_empty() {
            return Answer::App(0);
        }
        if line.eq_ignore_ascii_case("q") || line == "0" {
            return Answer::Cancel;
        }
        match line.parse::<usize>() {
            Ok(n) if n >= 1 && n <= cands.len() => return Answer::App(n - 1),
            Ok(n) if n == prev_n && n <= max => return Answer::Previous,
            _ => {
                let _ = writeln!(err, "1 to {max}, please");
            }
        }
    }
}

/// The user's own termux-file-editor, kept when Lindroids' took its place (the script says where).
fn previous_editor() -> Option<PathBuf> {
    std::env::var_os("LINDROIDS_PREVIOUS_EDITOR").map(PathBuf::from).filter(|p| p.is_file())
}

fn exec_previous(p: &Path, file: &Path) -> i32 {
    let e = std::process::Command::new(p).arg(file).env_remove("LINDROIDS_PREVIOUS_EDITOR").exec();
    crate::fail(&format!("{}: {e}", p.display()))
}

/// The program `--with` names: an exported program's id, a desktop id (with or without
/// `.desktop`), else a command (a Flatpak id, a program in Termux or in a distro, a command line).
fn with_app(all: &[App], with: &str) -> App {
    if let Some(a) = all.iter().find(|a| a.exported.as_deref() == Some(with)) {
        return a.clone();
    }
    let id = if with.ends_with(".desktop") { with.to_string() } else { format!("{with}.desktop") };
    if let Some(a) = all.iter().find(|a| a.desktop_id == id) {
        return a.clone();
    }
    let exec = if with.contains(char::is_whitespace) {
        vec!["sh".into(), "-c".into(), format!("{with} \"$1\""), "sh".into(), "%f".into()]
    } else {
        vec![with.to_string(), "%f".into()]
    };
    App { name: with.to_string(), place: "command".into(), exec, ..Default::default() }
}

/// Run `app` with the file, in a window of its own (`lindroids run`).
fn open_with(app: &App, file: &Path) -> i32 {
    let args = run_args(app, file);
    eprintln!("lindroids: opening {} with {}", file.display(), app.name);
    run::cmd_run(&args)
}

/// The `lindroids run` arguments that open `file` with `app`.
pub fn run_args(app: &App, file: &Path) -> Vec<String> {
    let mut args = app.opts.clone();
    if let Some(dir) = bind_dir(file) {
        args.push("--bind".into());
        args.push(dir.display().to_string());
    }
    args.push("--".into());
    args.extend(expand_exec(&app.exec, &file.display().to_string(), &app.name, app.icon.as_deref(), app.desktop_file.as_deref()));
    args
}

/// The folder a distro or a Flatpak is shown at the same path so the file is there too; None when
/// it is one they always have (Termux's home, shared storage).
pub fn bind_dir(file: &Path) -> Option<PathBuf> {
    let dir = file.parent()?;
    let always = ["/", TERMUX_HOME, TERMUX_PREFIX, "/storage", "/sdcard", "/storage/emulated/0"];
    (!always.iter().any(|a| dir == Path::new(a))).then(|| dir.to_path_buf())
}

// ---------------------------------------------------------------------------------------------
// .desktop files

/// Exec= split into words (the desktop entry spec's quoting), field codes left in.
pub fn exec_words(exec: &str) -> Vec<String> {
    // The string-value escapes first (\s \n \t \r \\), then the Exec quoting.
    let mut s = String::new();
    let mut ch = exec.chars();
    while let Some(c) = ch.next() {
        if c != '\\' {
            s.push(c);
            continue;
        }
        match ch.next() {
            Some('s') => s.push(' '),
            Some('n') => s.push('\n'),
            Some('t') => s.push('\t'),
            Some('r') => s.push('\r'),
            Some('\\') => s.push('\\'),
            Some(o) => {
                s.push('\\');
                s.push(o);
            }
            None => s.push('\\'),
        }
    }
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut quoted = false;
    let mut ch = s.chars();
    while let Some(c) = ch.next() {
        match c {
            '"' => {
                quoted = !quoted;
                in_word = true;
            }
            '\\' => {
                if let Some(n) = ch.next() {
                    cur.push(n);
                }
                in_word = true;
            }
            c if c.is_whitespace() && !quoted => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            c => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    words
}

/// The command for one file: %f %F %u %U become the file (a path: allowed for URLs too), %i the
/// icon, %c the name, %k the .desktop file, %% a %; deprecated codes go. A command with no file
/// code gets the file last.
pub fn expand_exec(words: &[String], file: &str, name: &str, icon: Option<&str>, desktop_file: Option<&Path>) -> Vec<String> {
    let mut out = Vec::new();
    let mut used = false;
    for w in words {
        match w.as_str() {
            "%f" | "%F" | "%u" | "%U" => {
                if !used {
                    out.push(file.to_string());
                    used = true;
                }
                continue;
            }
            "%i" => {
                if let Some(i) = icon.filter(|i| !i.is_empty()) {
                    out.push("--icon".into());
                    out.push(i.into());
                }
                continue;
            }
            _ => {}
        }
        let mut s = String::new();
        let mut had_code = false;
        let mut ch = w.chars();
        while let Some(c) = ch.next() {
            if c != '%' {
                s.push(c);
                continue;
            }
            had_code = true;
            match ch.next() {
                Some('f' | 'F' | 'u' | 'U') => {
                    s += file;
                    used = true;
                }
                Some('c') => s += name,
                Some('k') => s += &desktop_file.map(|p| p.display().to_string()).unwrap_or_default(),
                Some('%') => s.push('%'),
                _ => {}
            }
        }
        if !(had_code && s.is_empty()) {
            out.push(s);
        }
    }
    if !used {
        out.push(file.to_string());
    }
    out
}

/// What Lindroids needs from a .desktop file.
#[derive(Debug, Default, PartialEq)]
pub struct DesktopFile {
    pub name: String,
    pub exec: Option<String>,
    pub icon: Option<String>,
    pub mime_types: Vec<String>,
    /// Hidden=true, Terminal=true (needs a terminal, not a window), not an application.
    pub skip: bool,
}

pub fn parse_desktop(text: &str) -> DesktopFile {
    let mut d = DesktopFile::default();
    let mut main = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            main = line == "[Desktop Entry]";
            continue;
        }
        if !main {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "Name" => d.name = v.to_string(),
            "Exec" => d.exec = Some(v.to_string()),
            "Icon" => d.icon = Some(v.to_string()),
            "MimeType" => d.mime_types = v.split(';').map(str::trim).filter(|m| !m.is_empty()).map(String::from).collect(),
            "Hidden" | "Terminal" if v == "true" => d.skip = true,
            "Type" if v != "Application" => d.skip = true,
            _ => {}
        }
    }
    if d.exec.is_none() {
        d.skip = true;
    }
    d
}

fn read_app(path: &Path, place: &str, opts: &[String]) -> Option<App> {
    let d = parse_desktop(&std::fs::read_to_string(path).ok()?);
    if d.skip {
        return None;
    }
    let exec = exec_words(d.exec.as_deref()?);
    if exec.is_empty() {
        return None;
    }
    let id = path.file_name()?.to_string_lossy().into_owned();
    Some(App {
        name: if d.name.is_empty() { id.trim_end_matches(".desktop").to_string() } else { d.name },
        desktop_id: id,
        place: place.into(),
        mime_types: d.mime_types,
        exec,
        icon: d.icon,
        desktop_file: Some(path.to_path_buf()),
        opts: opts.to_vec(),
        exported: None,
    })
}

/// .desktop files in `dir`, sorted.
fn desktop_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map_or(false, |x| x == "desktop"))
        .collect();
    v.sort();
    v
}

/// A Flatpak's command from its exported .desktop file: `flatpak run [opts] <id> @@u %U @@`
/// (or the app's own `prog %U`) as `<id> %U`, which `lindroids run` runs as that Flatpak.
pub fn flatpak_exec(words: &[String], id: &str) -> Vec<String> {
    let flatpak_run = words.first().map_or(false, |w| w.ends_with("flatpak"));
    let rest: &[String] = match words.iter().position(|w| w == id) {
        Some(i) if flatpak_run => &words[i + 1..],
        _ => words.get(1..).unwrap_or(&[]),
    };
    std::iter::once(id.to_string()).chain(rest.iter().filter(|w| !w.starts_with("@@")).cloned()).collect()
}

// ---------------------------------------------------------------------------------------------
// The programs here

/// Every program that can be started with a file: exported ones first, then Termux's own (or this
/// system's), Flatpaks, and those in proot-distro distros. The same desktop id in the same place
/// only once (the exported one).
pub fn catalogue() -> Vec<App> {
    let sb = sandbox::detect();
    let mut v = exported_apps();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let native_place = if sb.kind == sandbox::Kind::Termux { "Termux".to_string() } else { sb.name.clone() };
    let mut native: Vec<PathBuf> = home.iter().map(|h| h.join(".local/share/applications")).collect();
    match std::env::var("PREFIX") {
        Ok(p) if sb.kind == sandbox::Kind::Termux && !p.is_empty() => {
            native.push(Path::new(&p).join("local/share/applications"));
            native.push(Path::new(&p).join("share/applications"));
        }
        _ => {
            native.push("/usr/local/share/applications".into());
            native.push("/usr/share/applications".into());
        }
    }
    for dir in native {
        v.extend(desktop_files(&dir).iter().filter_map(|p| read_app(p, &native_place, &[])));
    }
    v.extend(flatpak_apps());
    if sb.kind == sandbox::Kind::Termux {
        for root in run::distro_rootfses() {
            let Some(name) = distro_name(&root) else { continue };
            let opts = ["--distro".to_string(), name.clone()];
            for d in ["usr/local/share/applications", "usr/share/applications"] {
                v.extend(desktop_files(&root.join(d)).iter().filter_map(|p| read_app(p, &name, &opts)));
            }
        }
    }
    // Ours (exported programs' entries on a desktop) and Flatpak's launchers: their programs are
    // here already, run by Lindroids itself.
    v.retain(|a| a.exported.is_some() || (!a.desktop_id.starts_with("lindroids-") && !a.exec[0].ends_with("flatpak")));
    let mut out: Vec<App> = Vec::new();
    for a in v {
        if !out.iter().any(|o| o.desktop_id == a.desktop_id && o.place == a.place) {
            out.push(a);
        }
    }
    out
}

fn distro_name(root: &Path) -> Option<String> {
    let dir = if root.file_name()? == "rootfs" { root.parent()? } else { root };
    Some(dir.file_name()?.to_string_lossy().into_owned())
}

fn flatpak_apps() -> Vec<App> {
    let mut roots = vec![PathBuf::from("/")];
    if Path::new("/run/host/var/lib/flatpak").is_dir() {
        roots.push("/run/host".into());
    }
    roots.extend(run::distro_rootfses());
    let mut v: Vec<App> = Vec::new();
    for root in roots {
        for (id, deploy) in flatpak::installed_apps(&root) {
            if v.iter().any(|a| a.place == "Flatpak" && a.exec[0] == id) {
                continue;
            }
            v.extend(flatpak_entries(&id, &deploy));
        }
    }
    v
}

/// A Flatpak's launchers, from its deploy's `export/` (or its own files).
fn flatpak_entries(id: &str, deploy: &Path) -> Vec<App> {
    let dirs = [deploy.join("export/share/applications"), deploy.join("files/share/applications")];
    let files = dirs.iter().map(|d| desktop_files(d)).find(|f| !f.is_empty()).unwrap_or_default();
    files
        .iter()
        .filter_map(|p| read_app(p, "Flatpak", &[]))
        .map(|mut a| {
            a.exec = flatpak_exec(&a.exec, id);
            a
        })
        .collect()
}

/// The `lindroids run` options of an exported program's arguments, and its command.
fn split_run(run: &[String]) -> (Vec<String>, Vec<String>) {
    let mut i = 0;
    while i < run.len() {
        let a = &run[i];
        if a == "--" {
            return (run[..i].to_vec(), run[i + 1..].to_vec());
        }
        if !a.starts_with('-') {
            break;
        }
        i += if run::takes_value(a) { 2 } else { 1 };
    }
    let i = i.min(run.len());
    (run[..i].to_vec(), run[i..].to_vec())
}

fn exported_apps() -> Vec<App> {
    export::load_all().into_iter().filter_map(|rec| exported_app(&rec)).collect()
}

/// An exported program with what its .desktop file says it opens (nothing when it has none: it
/// is still there for `--with` and as a guess).
fn exported_app(rec: &lindroids_proto::Exported) -> Option<App> {
    let (opts, command) = split_run(&rec.run);
    if command.is_empty() {
        return None;
    }
    let o = run::parse(&rec.run).ok()?;
    let place = o.distro.clone().unwrap_or_else(|| if sandbox::detect().kind == sandbox::Kind::Termux { "Termux".into() } else { "Linux".into() });
    let mut app = match run::resolve(&o).ok()? {
        run::Target::Flatpak(fa, _) => {
            let mut a = flatpak_entries(&fa.id, &fa.app.dir)
                .into_iter()
                .find(|a| a.desktop_id == format!("{}.desktop", fa.id))
                .unwrap_or_else(|| App { desktop_id: format!("{}.desktop", fa.id), exec: vec![fa.id.clone(), "%F".into()], ..Default::default() });
            a.place = "Flatpak".into();
            a
        }
        run::Target::Program(argv) => {
            let root = o.distro.as_deref().map(run::distro_rootfs).unwrap_or_else(|| PathBuf::from("/"));
            let dirs = desktop::data_dirs(&root);
            let found = if run::is_shell_line(&argv) { None } else { desktop::find(&dirs, &run::program_of(&argv)) };
            let file = found.and_then(|e| dirs.iter().map(|d| d.join("applications").join(format!("{}.desktop", e.id))).find(|p| p.is_file()));
            match file.and_then(|f| read_app(&f, &place, &[])).filter(|a| !a.exec[0].ends_with("flatpak")) {
                Some(a) => a,
                None => {
                    let exec = if run::is_shell_line(&argv) {
                        vec!["sh".into(), "-c".into(), format!("{} \"$1\"", argv[0]), "sh".into(), "%f".into()]
                    } else {
                        let mut e = argv.clone();
                        e.push("%f".into());
                        e
                    };
                    App { desktop_id: format!("lindroids-{}.desktop", rec.id), exec, ..Default::default() }
                }
            }
        }
    };
    app.place = if app.place.is_empty() { place } else { app.place };
    app.name = rec.name.clone();
    app.opts = opts;
    app.exported = Some(rec.id.clone());
    Some(app)
}

/// The programs for `mime`, and the type they were found for: the file's own, else the first of
/// its parents that has some (a Python file opens in a text editor).
pub fn candidates(all: &[App], mime: &str) -> (Vec<App>, String) {
    for m in std::iter::once(mime.to_string()).chain(parents(mime)) {
        let v: Vec<App> = all.iter().filter(|a| handles(a, &m)).cloned().collect();
        if !v.is_empty() {
            return (v, m);
        }
    }
    (Vec::new(), mime.to_string())
}

fn handles(a: &App, mime: &str) -> bool {
    let names = aliases(mime);
    a.mime_types.iter().any(|t| {
        names.iter().any(|n| t.eq_ignore_ascii_case(n)) || t.strip_suffix("/*").map_or(false, |major| mime.split('/').next() == Some(major))
    })
}

/// Other names programs use for the same type.
fn aliases(mime: &str) -> Vec<&str> {
    const SAME: &[&[&str]] = &[
        &["audio/x-wav", "audio/wav", "audio/vnd.wave"],
        &["audio/mpeg", "audio/mp3", "audio/x-mp3", "audio/x-mpeg"],
        &["audio/flac", "audio/x-flac"],
        &["text/javascript", "application/javascript", "application/x-javascript"],
        &["application/x-yaml", "application/yaml", "text/yaml", "text/x-yaml"],
        &["text/x-python", "text/x-python3", "application/x-python"],
        &["image/vnd.microsoft.icon", "image/x-icon"],
        &["application/vnd.rar", "application/x-rar", "application/x-rar-compressed"],
        &["application/xml", "text/xml"],
        &["image/heif", "image/heic"],
        &["application/x-shellscript", "text/x-shellscript", "text/x-sh"],
    ];
    SAME.iter().find(|g| g.contains(&mime)).map(|g| g.to_vec()).unwrap_or_else(|| vec![mime])
}

/// The types `mime` is a kind of, nearest first (shared-mime-info's sub-class-of).
fn parents(mime: &str) -> Vec<String> {
    let direct: Option<&str> = match mime {
        "text/plain" => None,
        "image/svg+xml" | "application/xhtml+xml" | "application/rss+xml" | "application/atom+xml" => Some("application/xml"),
        "application/x-compressed-tar" => Some("application/gzip"),
        "application/x-xz-compressed-tar" => Some("application/x-xz"),
        "application/vnd.oasis.opendocument.text"
        | "application/vnd.oasis.opendocument.spreadsheet"
        | "application/vnd.oasis.opendocument.presentation"
        | "application/vnd.oasis.opendocument.graphics"
        | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        | "application/epub+zip"
        | "application/vnd.android.package-archive"
        | "application/x-krita"
        | "image/openraster" => Some("application/zip"),
        "application/json" | "application/xml" | "application/x-shellscript" | "application/x-yaml" | "application/toml" | "application/x-desktop"
        | "application/x-subrip" | "text/javascript" => Some("text/plain"),
        m if m.starts_with("text/") => Some("text/plain"),
        _ => None,
    };
    match direct {
        Some(p) => std::iter::once(p.to_string()).chain(parents(p)).collect(),
        None => Vec::new(),
    }
}

// ---------------------------------------------------------------------------------------------
// MIME types

/// The file's MIME type: by its name, else by its first bytes, else text or binary.
pub fn guess_mime(path: &Path) -> String {
    let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    if let Some(m) = mime_for_name(&name) {
        return m.to_string();
    }
    if let Some(m) = mime_from_globs(&name) {
        return m;
    }
    let mut head = Vec::new();
    if let Ok(f) = std::fs::File::open(path) {
        let _ = f.take(4096).read_to_end(&mut head);
    }
    mime_for_bytes(&head).to_string()
}

/// By name (lower case): the common types, as shared-mime-info names them.
pub fn mime_for_name(name: &str) -> Option<&'static str> {
    for (suffix, m) in [
        (".tar.gz", "application/x-compressed-tar"),
        (".tar.xz", "application/x-xz-compressed-tar"),
        (".tar.bz2", "application/x-bzip-compressed-tar"),
        (".tar.zst", "application/x-zstd-compressed-tar"),
    ] {
        if name.ends_with(suffix) {
            return Some(m);
        }
    }
    let (_, ext) = name.rsplit_once('.')?;
    Some(match ext {
        "txt" | "text" | "log" | "conf" | "cfg" | "ini" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "html" | "htm" => "text/html",
        "xhtml" => "application/xhtml+xml",
        "css" => "text/css",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        "xml" => "application/xml",
        "json" => "application/json",
        "js" | "mjs" => "text/javascript",
        "py" => "text/x-python",
        "sh" | "bash" => "application/x-shellscript",
        "c" => "text/x-csrc",
        "h" => "text/x-chdr",
        "cc" | "cpp" | "cxx" => "text/x-c++src",
        "hh" | "hpp" => "text/x-c++hdr",
        "rs" => "text/rust",
        "java" => "text/x-java",
        "go" => "text/x-go",
        "yaml" | "yml" => "application/x-yaml",
        "toml" => "application/toml",
        "tex" => "text/x-tex",
        "diff" | "patch" => "text/x-patch",
        "srt" => "application/x-subrip",
        "ics" => "text/calendar",
        "vcf" => "text/vcard",
        "desktop" => "application/x-desktop",
        "rtf" => "application/rtf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "svgz" => "image/svg+xml-compressed",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "ico" => "image/vnd.microsoft.icon",
        "heic" | "heif" => "image/heif",
        "avif" => "image/avif",
        "jxl" => "image/jxl",
        "xcf" => "image/x-xcf",
        "psd" => "image/vnd.adobe.photoshop",
        "kra" => "application/x-krita",
        "ora" => "image/openraster",
        "pdf" => "application/pdf",
        "epub" => "application/epub+zip",
        "djvu" => "image/vnd.djvu",
        "ps" => "application/postscript",
        "xopp" => "application/x-xopp",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "odp" => "application/vnd.oasis.opendocument.presentation",
        "odg" => "application/vnd.oasis.opendocument.graphics",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "mp3" => "audio/mpeg",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "flac" => "audio/flac",
        "wav" => "audio/x-wav",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "mid" | "midi" => "audio/midi",
        "mp4" | "m4v" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "mov" => "video/quicktime",
        "3gp" => "video/3gpp",
        "ogv" => "video/ogg",
        "zip" => "application/zip",
        "tar" => "application/x-tar",
        "gz" => "application/gzip",
        "tgz" => "application/x-compressed-tar",
        "xz" => "application/x-xz",
        "bz2" => "application/x-bzip2",
        "zst" => "application/zstd",
        "7z" => "application/x-7z-compressed",
        "rar" => "application/vnd.rar",
        "deb" => "application/vnd.debian.binary-package",
        "apk" => "application/vnd.android.package-archive",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "blend" => "application/x-blender",
        "stl" => "model/stl",
        "sqlite" | "sqlite3" | "db" => "application/vnd.sqlite3",
        "torrent" => "application/x-bittorrent",
        "eml" => "message/rfc822",
        _ => return None,
    })
}

/// By name, from shared-mime-info's database when one is installed (`*.ext` globs).
fn mime_from_globs(name: &str) -> Option<String> {
    let (_, ext) = name.rsplit_once('.')?;
    let glob = format!("*.{ext}");
    let mut files: Vec<PathBuf> = Vec::new();
    if let Some(h) = std::env::var_os("HOME") {
        files.push(PathBuf::from(h).join(".local/share/mime/globs2"));
    }
    if let Ok(p) = std::env::var("PREFIX") {
        files.push(Path::new(&p).join("share/mime/globs2"));
    }
    files.push("/usr/share/mime/globs2".into());
    files.iter().filter_map(|f| std::fs::read_to_string(f).ok()).find_map(|t| glob_lookup(&t, &glob))
}

/// globs2 lines are `weight:type:glob[:flags]`; the heaviest match wins.
fn glob_lookup(text: &str, glob: &str) -> Option<String> {
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let mut p = l.splitn(4, ':');
            let w: u32 = p.next()?.parse().ok()?;
            let m = p.next()?;
            (p.next()?.to_lowercase() == glob).then(|| (w, m.to_string()))
        })
        .max_by_key(|(w, _)| *w)
        .map(|(_, m)| m)
}

/// By the first bytes.
pub fn mime_for_bytes(b: &[u8]) -> &'static str {
    let starts = |p: &[u8]| b.starts_with(p);
    if starts(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if starts(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if starts(b"GIF87a") || starts(b"GIF89a") {
        "image/gif"
    } else if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        "image/webp"
    } else if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WAVE" {
        "audio/x-wav"
    } else if starts(b"%PDF-") {
        "application/pdf"
    } else if starts(b"%!PS") {
        "application/postscript"
    } else if starts(b"PK\x03\x04") {
        "application/zip"
    } else if starts(b"\x1f\x8b") {
        "application/gzip"
    } else if starts(b"OggS") {
        "audio/ogg"
    } else if starts(b"fLaC") {
        "audio/flac"
    } else if starts(b"ID3") {
        "audio/mpeg"
    } else if starts(b"\x1a\x45\xdf\xa3") {
        "video/x-matroska"
    } else if b.len() >= 12 && &b[4..8] == b"ftyp" {
        if matches!(&b[8..12], b"heic" | b"heix" | b"mif1") {
            "image/heif"
        } else if &b[8..12] == b"avif" {
            "image/avif"
        } else {
            "video/mp4"
        }
    } else if starts(b"SQLite format 3\0") {
        "application/vnd.sqlite3"
    } else if starts(b"\x7fELF") {
        "application/x-executable"
    } else if looks_like_text(b) {
        let t = String::from_utf8_lossy(b);
        let t = t.trim_start();
        if t.starts_with("<svg") || (t.starts_with("<?xml") && t.contains("<svg")) {
            "image/svg+xml"
        } else if t.starts_with("#!") && (t.lines().next().map_or(false, |l| l.contains("sh"))) {
            "application/x-shellscript"
        } else if t.starts_with("<?xml") {
            "application/xml"
        } else if t.as_bytes().get(..5).map_or(false, |h| h.eq_ignore_ascii_case(b"<!doc")) || t.starts_with("<html") {
            "text/html"
        } else {
            "text/plain"
        }
    } else {
        "application/octet-stream"
    }
}

/// UTF-8 with no NUL (a character cut at the end of the sample is fine).
fn looks_like_text(b: &[u8]) -> bool {
    !b.contains(&0)
        && match std::str::from_utf8(b) {
            Ok(_) => true,
            Err(e) => e.error_len().is_none(),
        }
}

// ---------------------------------------------------------------------------------------------
// mimeapps.list

pub fn mimeapps_path() -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
    });
    config.join("mimeapps.list")
}

/// The desktop ids `[Default Applications]` gives for `mime`, in order.
pub fn default_apps(text: &str, mime: &str) -> Vec<String> {
    let mut section = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            section = line == "[Default Applications]";
            continue;
        }
        if !section {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            if k.trim() == mime {
                return v.split(';').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect();
            }
        }
    }
    Vec::new()
}

/// `text` with `mime=<id>;` as the default for `mime` (first, the others it had after it); every
/// other line kept.
pub fn set_default(text: &str, mime: &str, id: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let header = lines.iter().position(|l| l.trim() == "[Default Applications]");
    let entry = |old: &[String]| {
        let mut ids = vec![id.to_string()];
        ids.extend(old.iter().filter(|o| *o != id).cloned());
        format!("{mime}={};", ids.join(";"))
    };
    match header {
        Some(h) => {
            let end = lines[h + 1..].iter().position(|l| l.trim_start().starts_with('[')).map_or(lines.len(), |i| h + 1 + i);
            let existing = (h + 1..end).find(|&i| lines[i].split_once('=').map_or(false, |(k, _)| k.trim() == mime));
            match existing {
                Some(i) => {
                    let old: Vec<String> = lines[i].split_once('=').unwrap().1.split(';').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect();
                    lines[i] = entry(&old);
                }
                None => lines.insert(h + 1, entry(&[])),
            }
        }
        None => {
            if lines.last().map_or(false, |l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push("[Default Applications]".into());
            lines.push(entry(&[]));
        }
    }
    lines.join("\n") + "\n"
}

fn write_file(path: &Path, text: &str) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let tmp = path.with_extension("list.lindroids");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

// ---------------------------------------------------------------------------------------------
// ~/bin/termux-file-editor

/// Marks the file editor script as ours (see shortcut.rs, which installs it).
pub const EDITOR_MARK: &str = "# lindroids: termux-file-editor";

/// The `~/bin/termux-file-editor` Termux runs with a received file: `lindroids open --linux`, with
/// the editor the user had (`before`) for files no Linux program opens.
pub fn editor_script(lindroids: &str, before: &Path) -> String {
    format!(
        r#"#!/data/data/com.termux/files/usr/bin/sh
{EDITOR_MARK} (rewritten by `lindroids setup` and `lindroids export`; do not edit, your own editor is kept below)
# Termux runs this with a file opened with it or shared to it ("Edit"; saved in ~/downloads): it opens
# in a Linux program for its type, in a window of its own. A file no Linux program here opens goes to
# the editor you had, if any.
if [ -x "{before}" ]; then export LINDROIDS_PREVIOUS_EDITOR="{before}"; fi
exec "{lindroids}" open --linux "$@"
"#,
        before = before.display()
    )
}

// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn app(id: &str, mimes: &[&str], exported: Option<&str>) -> App {
        App { desktop_id: id.into(), name: id.into(), place: "Termux".into(), mime_types: s(mimes), exec: s(&["x", "%f"]), exported: exported.map(String::from), ..Default::default() }
    }

    #[test]
    fn guesses_mime_types() {
        assert_eq!(mime_for_name("notes.txt"), Some("text/plain"));
        assert_eq!(mime_for_name("photo.jpeg"), Some("image/jpeg"));
        assert_eq!(mime_for_name("backup.tar.gz"), Some("application/x-compressed-tar"));
        assert_eq!(mime_for_name("report.docx"), Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document"));
        assert_eq!(mime_for_name("README"), None);
        assert_eq!(mime_for_bytes(b"\x89PNG\r\n\x1a\n...."), "image/png");
        assert_eq!(mime_for_bytes(b"%PDF-1.7\n"), "application/pdf");
        assert_eq!(mime_for_bytes(b"\0\0\0\x18ftypmp42"), "video/mp4");
        assert_eq!(mime_for_bytes(b"hello\nworld\n"), "text/plain");
        assert_eq!(mime_for_bytes(b"#!/bin/sh\necho hi\n"), "application/x-shellscript");
        assert_eq!(mime_for_bytes(b"<?xml version=\"1.0\"?>\n<svg xmlns=\"...\">"), "image/svg+xml");
        assert_eq!(mime_for_bytes(b"\x00\x01\x02binary"), "application/octet-stream");
        // A multi-byte character cut by the sample is still text.
        assert_eq!(mime_for_bytes("caf\u{e9}".as_bytes().split_last().unwrap().1), "text/plain");
        assert_eq!(glob_lookup("50:text/x-foo:*.foo\n80:text/x-foo2:*.foo:cs\n", "*.foo").as_deref(), Some("text/x-foo2"));
        // A file with no extension goes by its bytes.
        let dir = std::env::temp_dir().join(format!("lindroids-open-mime-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("IMG_0001"), b"\xff\xd8\xff\xe0JFIF").unwrap();
        assert_eq!(guess_mime(&dir.join("IMG_0001")), "image/jpeg");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn splits_exec_lines() {
        assert_eq!(exec_words("gedit %U"), s(&["gedit", "%U"]));
        assert_eq!(exec_words(r#"sh -c "echo \"a b\" \$HOME" x"#), s(&["sh", "-c", "echo \"a b\" $HOME", "x"]));
        // The string escapes come first: \\ in the file is one backslash for the quoting.
        assert_eq!(exec_words(r#""/opt/My\\ App/run" --x"#), s(&["/opt/My App/run", "--x"]));
        assert_eq!(exec_words(r"a\sb c"), s(&["a", "b", "c"]));
    }

    #[test]
    fn expands_field_codes() {
        let f = "/data/data/com.termux/files/home/downloads/a b.txt";
        assert_eq!(expand_exec(&s(&["gedit", "%U"]), f, "Text", None, None), s(&["gedit", f]));
        assert_eq!(expand_exec(&s(&["foo", "--file=%f", "%F"]), f, "Foo", None, None), s(&["foo", &format!("--file={f}")]));
        assert_eq!(
            expand_exec(&s(&["foo", "%i", "--title", "%c", "%k", "%d", "100%%"]), f, "Foo", Some("foo-icon"), Some(Path::new("/a/foo.desktop"))),
            s(&["foo", "--icon", "foo-icon", "--title", "Foo", "/a/foo.desktop", "100%", f])
        );
        // No file code: the file goes last.
        assert_eq!(expand_exec(&s(&["mousepad"]), f, "M", None, None), s(&["mousepad", f]));
        // A Flatpak's exported launcher runs the app with the file.
        let words = exec_words("/usr/bin/flatpak run --branch=stable --arch=aarch64 --command=gnome-text-editor --file-forwarding org.gnome.TextEditor @@u %U @@");
        assert_eq!(flatpak_exec(&words, "org.gnome.TextEditor"), s(&["org.gnome.TextEditor", "%U"]));
        assert_eq!(flatpak_exec(&s(&["gnome-text-editor", "%U"]), "org.gnome.TextEditor"), s(&["org.gnome.TextEditor", "%U"]));
    }

    #[test]
    fn parses_desktop_files() {
        let d = parse_desktop("[Desktop Entry]\nType=Application\nName=Mousepad\nExec=mousepad %U\nMimeType=text/plain;text/x-python;\n\n[Desktop Action new]\nName=New\nExec=mousepad --new\n");
        assert_eq!(d.name, "Mousepad");
        assert_eq!(d.exec.as_deref(), Some("mousepad %U"));
        assert_eq!(d.mime_types, s(&["text/plain", "text/x-python"]));
        assert!(!d.skip);
        assert!(parse_desktop("[Desktop Entry]\nName=Vim\nExec=vim %F\nTerminal=true\n").skip);
        assert!(parse_desktop("[Desktop Entry]\nName=Gone\nExec=x\nHidden=true\n").skip);
    }

    #[test]
    fn finds_candidates_by_type_then_parent() {
        let all = vec![app("ed.desktop", &["text/plain"], None), app("img.desktop", &["image/*"], None), app("py.desktop", &["text/x-python3"], None)];
        let ids = |v: &[App]| v.iter().map(|a| a.desktop_id.clone()).collect::<Vec<_>>();
        let (c, used) = candidates(&all, "text/x-python");
        assert_eq!((ids(&c), used.as_str()), (s(&["py.desktop"]), "text/x-python"));
        let (c, used) = candidates(&all, "text/markdown");
        assert_eq!((ids(&c), used.as_str()), (s(&["ed.desktop"]), "text/plain"));
        assert_eq!(ids(&candidates(&all, "image/png").0), s(&["img.desktop"]));
        assert_eq!(ids(&candidates(&all, "application/json").0), s(&["ed.desktop"]));
        assert!(candidates(&all, "application/pdf").0.is_empty());
    }

    #[test]
    fn picks_the_default_the_only_one_or_asks() {
        let cands = vec![app("a.desktop", &["text/plain"], None), app("b.desktop", &["text/plain"], Some("termux.b"))];
        let list = "[Added Associations]\ntext/plain=a.desktop;\n\n[Default Applications]\nimage/png=x.desktop;\ntext/plain=gone.desktop;b.desktop;\n";
        let defaults = default_apps(list, "text/plain");
        assert_eq!(defaults, s(&["gone.desktop", "b.desktop"]));
        // The first default that is here.
        assert_eq!(pick(&cands, &defaults, true), Some(Pick::Use(1)));
        // No default: ask, or without a terminal the exported one.
        assert_eq!(pick(&cands, &[], true), Some(Pick::Ask));
        assert_eq!(pick(&cands, &[], false), Some(Pick::Use(1)));
        assert_eq!(pick(&cands[..1], &[], true), Some(Pick::Use(0)));
        assert_eq!(pick(&[], &defaults, true), None);
    }

    #[test]
    fn remembers_choices_in_mimeapps_list() {
        let t = set_default("", "text/plain", "a.desktop");
        assert_eq!(t, "[Default Applications]\ntext/plain=a.desktop;\n");
        let t = set_default("[Added Associations]\nimage/png=v.desktop;\n", "text/plain", "a.desktop");
        assert_eq!(t, "[Added Associations]\nimage/png=v.desktop;\n\n[Default Applications]\ntext/plain=a.desktop;\n");
        let t = set_default("[Default Applications]\nimage/png=v.desktop;\ntext/plain=b.desktop;a.desktop;\n[Other]\ntext/plain=z\n", "text/plain", "a.desktop");
        assert_eq!(t, "[Default Applications]\nimage/png=v.desktop;\ntext/plain=a.desktop;b.desktop;\n[Other]\ntext/plain=z\n");
        assert_eq!(default_apps(&t, "text/plain"), s(&["a.desktop", "b.desktop"]));
        let t = set_default("[Default Applications]\nimage/png=v.desktop;\n", "text/plain", "a.desktop");
        assert_eq!(default_apps(&t, "image/png"), s(&["v.desktop"]));
        assert_eq!(default_apps(&t, "text/plain"), s(&["a.desktop"]));
    }

    #[test]
    fn run_args_show_the_folder_to_sandboxes() {
        let mut a = app("org.gnome.TextEditor.desktop", &["text/plain"], None);
        a.exec = s(&["org.gnome.TextEditor", "%U"]);
        a.opts = s(&["--name", "Text"]);
        let f = Path::new("/data/data/com.termux/files/home/downloads/n.txt");
        assert_eq!(
            run_args(&a, f),
            s(&["--name", "Text", "--bind", "/data/data/com.termux/files/home/downloads", "--", "org.gnome.TextEditor", "/data/data/com.termux/files/home/downloads/n.txt"])
        );
        // Termux's home itself is in every distro and Flatpak already.
        assert_eq!(bind_dir(Path::new("/data/data/com.termux/files/home/n.txt")), None);
        assert_eq!(split_run(&s(&["--distro", "debian", "--x11", "mousepad", "--opt"])), (s(&["--distro", "debian", "--x11"]), s(&["mousepad", "--opt"])));
        assert_eq!(split_run(&s(&["--name", "N", "--", "-weird"])), (s(&["--name", "N"]), s(&["-weird"])));
        let w = with_app(&[], "foot -e nano");
        assert_eq!(expand_exec(&w.exec, "/f", "", None, None), s(&["sh", "-c", "foot -e nano \"$1\"", "sh", "/f"]));
    }

    #[test]
    fn editor_script_opens_in_linux_and_keeps_the_users_editor() {
        let t = editor_script("/p/bin/lindroids", Path::new("/h/.local/share/lindroids/termux-file-editor.before"));
        assert!(t.starts_with("#!/data/data/com.termux/files/usr/bin/sh\n"));
        assert!(t.contains(EDITOR_MARK));
        assert!(t.contains("if [ -x \"/h/.local/share/lindroids/termux-file-editor.before\" ]; then export LINDROIDS_PREVIOUS_EDITOR=\"/h/.local/share/lindroids/termux-file-editor.before\"; fi\n"));
        assert!(t.ends_with("exec \"/p/bin/lindroids\" open --linux \"$@\"\n"));
    }
}
