//! `lindroids export`: put a Linux program on the Android home screen, with its own name and icon.
//!
//! By default the program is copied into the Lindroids app as a bundle — the program and every
//! package it depends on, nothing else — so its shortcut works with the terminal app closed: the
//! app runs it under its own proot, against a server running inside the app. `--link` instead
//! makes a shortcut that asks Termux to run `lindroids run ...` (nothing copied; Termux needed).

use std::collections::BTreeSet;
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use lindroids_proto::{BundleKind, BundleManifest, CliReply, CliRequest, ExportRequest, LaunchSpec, TERMUX_PREFIX};

use crate::pkgdb::{self, Db};
use crate::run::{self, Opts, Target};
use crate::{client, flatpak, sandbox};

/// What goes into a bundle.
pub struct Plan {
    pub manifest: BundleManifest,
    /// (file on this system, path inside the bundle)
    pub files: Vec<(PathBuf, String)>,
    /// Empty directories to create inside the bundle.
    pub dirs: Vec<String>,
    pub argv: Vec<String>,
}

fn which_in(dirs: &[PathBuf], prog: &str) -> Option<PathBuf> {
    if prog.starts_with('/') {
        return Some(PathBuf::from(prog));
    }
    dirs.iter().map(|d| d.join(prog)).find(|p| p.exists())
}

/// Where an X11 bundle from a distro keeps satellite, in its own view.
const SATELLITE_IN_ROOTFS: &str = "/usr/libexec/lindroids/xwayland-satellite";

/// Whether a bundle needs an X display: asked for (`--x11`), or a program that only speaks X11.
fn needs_x11(o: &Opts, program: &Path) -> bool {
    if o.x11 {
        return true;
    }
    let libs = crate::elf::needed(program).unwrap_or_default();
    let x11 = crate::elf::x11_only(&libs);
    if x11 {
        eprintln!("lindroids: {} uses X11; the bundle gets Xwayland and xwayland-satellite", program.display());
    }
    x11
}

/// A glibc build of satellite for a Debian-family distro: the distro's own (Lindroids' Debian
/// package, or a distro package), else the one the Termux package carries for this CPU.
fn glibc_satellite(rootfs: &Path) -> Option<PathBuf> {
    let candidates = [rootfs.join("usr/libexec/lindroids/xwayland-satellite"), rootfs.join("usr/bin/xwayland-satellite")];
    if let Some(p) = candidates.into_iter().find(|p| p.is_file()) {
        return Some(p);
    }
    let bin_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let p = bin_dir.join("../libexec/lindroids/glibc/xwayland-satellite");
    p.is_file().then_some(p)
}

/// Turn a command into argv for inside the bundle: shell lines go through sh.
fn bundle_argv(o: &Opts, sh: &str, resolved: &Path) -> Vec<String> {
    if run::is_shell_line(&o.command) {
        vec![sh.into(), "-c".into(), o.command[0].clone()]
    } else {
        let mut v = vec![resolved.display().to_string()];
        v.extend(o.command[1..].iter().cloned());
        v
    }
}

/// The files of the packages a program needs, from a dpkg database whose recorded paths are
/// under `root` on this system; `strip` is removed from recorded paths before `prefix_in_bundle`
/// is put in front.
fn closure_files(db: &Db, roots: Vec<String>, essential: bool, root: &Path, prefix_in_bundle: &str, strip: &str) -> Vec<(PathBuf, String)> {
    let all_files = db.all_files();
    let pkgs = db.with_plugins(db.closure(&roots, essential), &all_files);
    let owned = db.files(&pkgs);
    let dirs: BTreeSet<String> = owned.iter().filter(|p| root.join(p.trim_start_matches('/')).is_dir()).cloned().collect();
    let all = db.all_owned();
    let generated = pkgdb::generated_files(&dirs, &all, |p| root.join(p.trim_start_matches('/')));
    let mut files = Vec::new();
    for p in owned.iter().chain(generated.iter()) {
        let host = root.join(p.trim_start_matches('/'));
        let rel = p.strip_prefix(strip).unwrap_or(p).trim_start_matches('/');
        if rel.is_empty() || std::fs::symlink_metadata(&host).is_err() {
            continue;
        }
        files.push((host, format!("{prefix_in_bundle}/{rel}")));
    }
    files
}

fn plan(o: &Opts, target: &Target, sb: &sandbox::Sandbox, id: &str, name: &str) -> Result<Plan, String> {
    let mut manifest = BundleManifest { id: id.into(), name: name.into(), kind: BundleKind::Termux, env: Vec::new(), workdir: None, x11_satellite: None };
    match target {
        Target::Flatpak(app, args) => {
            let p = flatpak::bundle_plan(app, args, o)?;
            manifest.kind = p.kind;
            manifest.env = p.env;
            manifest.workdir = p.workdir;
            Ok(Plan { manifest, files: p.files, dirs: p.dirs, argv: p.argv })
        }
        Target::Program(_) if o.distro.is_some() => {
            let distro = o.distro.as_deref().unwrap();
            let rootfs = run::distro_rootfs(distro);
            let db = Db::open(&rootfs.join("var/lib/dpkg")).map_err(|e| format!("only Debian-family distros can be exported yet ({e})"))?;
            let prog = run::program_of(&o.command);
            let bin_dirs: Vec<PathBuf> = ["usr/local/bin", "usr/bin", "bin", "usr/games", "usr/sbin", "sbin"].iter().map(|d| rootfs.join(d)).collect();
            let host = which_in(&bin_dirs, &prog).ok_or_else(|| format!("{prog} is not installed in {distro}"))?;
            let inside = format!("/{}", host.strip_prefix(&rootfs).unwrap_or(&host).display());
            // dpkg records /usr/bin/foo; merged-usr systems may have it as /bin/foo too.
            let owner = db.owner(&inside).or_else(|| db.owner(&inside.replacen("/usr", "", 1))).or_else(|| db.owner(&format!("/usr{inside}")));
            let mut roots: Vec<String> = owner.into_iter().collect();
            let x11 = needs_x11(o, &host);
            if x11 {
                let missing: Vec<&str> = ["xwayland", "libxcb1", "libxcb-cursor0"].into_iter().filter(|p| !db.installed(p)).collect();
                if !missing.is_empty() {
                    return Err(format!(
                        "{prog} is an X11 program; its bundle needs {} in {distro} (proot-distro login {distro} -- apt install {})",
                        missing.join(", "),
                        missing.join(" ")
                    ));
                }
                roots.extend(["xwayland".to_string(), "libxcb1".into(), "libxcb-cursor0".into()]);
            }
            let mut files = closure_files(&db, roots, true, &rootfs, "root", "");
            if x11 {
                let sat = glibc_satellite(&rootfs).ok_or_else(|| {
                    format!("{prog} is an X11 program, and no xwayland-satellite built for {distro} was found (Lindroids ships one in libexec/lindroids/glibc/)")
                })?;
                files.push((sat, format!("root{SATELLITE_IN_ROOTFS}")));
                manifest.x11_satellite = Some(SATELLITE_IN_ROOTFS.into());
            }
            // /etc whole (small; holds generated files like ld.so.cache and passwd).
            for e in walk(&rootfs.join("etc")) {
                let rel = e.strip_prefix(&rootfs).unwrap().display().to_string();
                files.push((e.clone(), format!("root/{rel}")));
            }
            manifest.kind = BundleKind::Rootfs { binds: Vec::new() };
            manifest.workdir = Some("/root".into());
            let dirs = ["root/tmp", "root/root", "root/root/.config", "root/root/.local/share", "root/root/.cache", "root/run", "root/proc", "root/dev", "root/sys", "root/sdcard", "root/storage"]
                .iter()
                .map(|s| s.to_string())
                .collect();
            Ok(Plan { manifest, files, dirs, argv: bundle_argv(o, "/bin/sh", Path::new(&inside)) })
        }
        Target::Program(_) => {
            if sb.kind != sandbox::Kind::Termux {
                return Err("bundles can be made from Termux, a proot-distro distro (--distro) or a Flatpak; from here, use --link".into());
            }
            let db = Db::open(&Path::new(TERMUX_PREFIX).join("var/lib/dpkg"))?;
            let prog = run::program_of(&o.command);
            let path_dirs: Vec<PathBuf> = std::env::var("PATH").unwrap_or_default().split(':').map(PathBuf::from).collect();
            let host = which_in(&path_dirs, &prog).ok_or_else(|| format!("{prog}: command not found"))?;
            let real = std::fs::canonicalize(&host).unwrap_or(host.clone());
            let owner = db.owner(&host.display().to_string()).or_else(|| db.owner(&real.display().to_string()));
            let mut roots: Vec<String> = owner.into_iter().collect();
            if roots.is_empty() {
                eprintln!("lindroids: {} is not from a package; copying only it", host.display());
            }
            // A shell and the basic tools: terminals start $SHELL, shell lines need sh.
            roots.extend(["bash".to_string(), "dash".into(), "coreutils".into()]);
            let x11 = needs_x11(o, &real);
            if x11 {
                if !db.installed("xwayland") {
                    return Err(format!("{prog} is an X11 program; its bundle needs Xwayland (`lindroids setup x11`)"));
                }
                roots.extend(["xwayland".to_string(), "libxcb".into(), "xcb-util-cursor".into()]);
            }
            let mut files = closure_files(&db, roots, false, Path::new("/"), "usr", TERMUX_PREFIX);
            if x11 {
                let sat = lindroids_server::x11::find_satellite().ok_or("X11 bundles need xwayland-satellite (`lindroids doctor`)")?;
                let inside = format!("{TERMUX_PREFIX}/libexec/lindroids/xwayland-satellite");
                files.push((sat, format!("usr{}", inside.strip_prefix(TERMUX_PREFIX).unwrap())));
                manifest.x11_satellite = Some(inside);
            }
            if host.starts_with(TERMUX_PREFIX) && !files.iter().any(|(h, _)| *h == host) {
                let rel = host.strip_prefix(TERMUX_PREFIX).unwrap().display().to_string();
                files.push((host.clone(), format!("usr{rel}")));
            }
            // A fresh home with the usual XDG directories: some programs create only their own
            // subdirectory and expect ~/.config to be there.
            let dirs = ["home", "home/.config", "home/.local/share", "home/.cache", "usr/tmp"].iter().map(|s| s.to_string()).collect();
            Ok(Plan { manifest, files, dirs, argv: bundle_argv(o, &format!("{TERMUX_PREFIX}/bin/sh"), &host) })
        }
    }
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        out.push(p.clone());
        if e.file_type().map_or(false, |t| t.is_dir()) {
            out.extend(walk(&p));
        }
    }
    out
}

/// Write the bundle as a tar stream to `out`.
fn write_tar(plan: &Plan, out: std::fs::File) -> Result<(), String> {
    let mut b = tar::Builder::new(std::io::BufWriter::with_capacity(1 << 20, out));
    b.follow_symlinks(false);
    let manifest = serde_json::to_vec_pretty(&plan.manifest).unwrap();
    let mut h = tar::Header::new_gnu();
    h.set_size(manifest.len() as u64);
    h.set_mode(0o644);
    h.set_cksum();
    b.append_data(&mut h, "manifest.json", &manifest[..]).map_err(|e| e.to_string())?;
    for d in &plan.dirs {
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Directory);
        h.set_mode(0o755);
        h.set_size(0);
        h.set_cksum();
        b.append_data(&mut h, format!("{d}/"), std::io::empty()).map_err(|e| e.to_string())?;
    }
    let mut seen = BTreeSet::new();
    for (host, name) in &plan.files {
        if !seen.insert(name.clone()) {
            continue;
        }
        // proot's emulated hard links (symlinks into a distro's .l2s directory, by this side's
        // path) would dangle inside the bundle: copy what they point to.
        let target = std::fs::read_link(host).ok().filter(|t| t.to_string_lossy().contains("/.l2s/.l2s."));
        let res = match target {
            Some(t) => match std::fs::File::open(&t) {
                Ok(mut f) => {
                    let meta = f.metadata().map_err(|e| e.to_string())?;
                    let mut h = tar::Header::new_gnu();
                    h.set_metadata(&meta);
                    h.set_cksum();
                    b.append_data(&mut h, name, &mut f)
                }
                Err(e) => Err(e),
            },
            None => b.append_path_with_name(host, name),
        };
        if let Err(e) = res {
            // Unreadable files (sockets, permissions) are skipped, not fatal.
            if e.kind() == std::io::ErrorKind::BrokenPipe {
                return Err("the app stopped reading the bundle".into());
            }
        }
    }
    b.into_inner().map_err(|e| e.to_string())?.flush().map_err(|e| e.to_string())
}

fn default_id(o: &Opts, target: &Target, sb: &sandbox::Sandbox) -> String {
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

fn records_path() -> PathBuf {
    lindroids_server::secret_path().with_file_name("exports.json")
}

fn load_records() -> Vec<(String, String)> {
    std::fs::read(records_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_records(r: &[(String, String)]) {
    let p = records_path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, serde_json::to_vec_pretty(r).unwrap());
}

/// Termux only runs commands for other apps when allowed in ~/.termux/termux.properties.
fn allow_termux_external_apps() {
    let home = std::env::var("HOME").unwrap_or_default();
    let dir = Path::new(&home).join(".termux");
    let props = dir.join("termux.properties");
    let text = std::fs::read_to_string(&props).unwrap_or_default();
    if text.lines().any(|l| l.trim().replace(' ', "") == "allow-external-apps=true") {
        return;
    }
    let _ = std::fs::create_dir_all(&dir);
    let new = format!("{}{}allow-external-apps = true\n", text, if text.is_empty() || text.ends_with('\n') { "" } else { "\n" });
    if std::fs::write(&props, new).is_ok() {
        eprintln!("lindroids: allowed the Lindroids app to start commands in Termux (~/.termux/termux.properties)");
        let _ = std::process::Command::new("termux-reload-settings").status();
    }
}

fn human(n: u64) -> String {
    match n {
        n if n >= 1 << 30 => format!("{:.1} GB", n as f64 / (1u64 << 30) as f64),
        n if n >= 1 << 20 => format!("{:.0} MB", n as f64 / (1u64 << 20) as f64),
        n => format!("{} KB", n / 1024),
    }
}

pub fn cmd_export(args: &[String]) -> i32 {
    let o = match run::parse(args) {
        Ok(o) => o,
        Err(e) => return crate::fail(&e),
    };
    let sb = sandbox::detect();
    let target = match run::resolve(&o) {
        Ok(t) => t,
        Err(e) => return crate::fail(&e),
    };
    let (name, icon) = run::identify(&o, &target);
    let id = o.id.clone().unwrap_or_else(|| default_id(&o, &target, &sb));
    let rt = match crate::ensure_server(&sb) {
        Ok(rt) => rt,
        Err(e) => return crate::fail(&e),
    };
    if !crate::ensure_app(&sb, &rt, false) {
        return crate::fail("exporting needs the Lindroids app connected (see `lindroids pair`)");
    }

    let launch = if o.link {
        if sb.kind != sandbox::Kind::Termux {
            return crate::fail("--link shortcuts start Termux; from here, export a bundle instead");
        }
        allow_termux_external_apps();
        let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| format!("{TERMUX_PREFIX}/bin/lindroids"));
        // Same options as given, minus the export-only ones.
        let mut argv = vec![exe, "run".to_string()];
        let mut skip = false;
        for a in args {
            if skip {
                skip = false;
                continue;
            }
            match a.as_str() {
                "--link" | "--bundle" => {}
                "--id" => skip = true,
                _ => argv.push(a.clone()),
            }
        }
        LaunchSpec::Termux { argv }
    } else {
        let plan = match plan(&o, &target, &sb, &id, &name) {
            Ok(p) => p,
            Err(e) => return crate::fail(&e),
        };
        let bytes: u64 = plan.files.iter().filter_map(|(h, _)| std::fs::symlink_metadata(h).ok()).filter(|m| m.is_file()).map(|m| m.len()).sum();
        println!("Copying {name} into Lindroids: {} files, {}", plan.files.len(), human(bytes));
        let mut fds = [0; 2];
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
            return crate::fail("no pipe");
        }
        let (r, w) = unsafe { (OwnedFd::from_raw_fd(fds[0]), std::fs::File::from_raw_fd(fds[1])) };
        let argv = plan.argv.clone();
        let writer = std::thread::spawn(move || write_tar(&plan, w));
        let reply = client::request_with_fds(&rt, &CliRequest::ImportBundle { id: id.clone() }, &[r.as_raw_fd()], Duration::from_secs(3600));
        drop(r);
        if let Ok(Err(e)) = writer.join() {
            return crate::fail(&e);
        }
        match reply {
            Ok(CliReply::Ok { message }) => println!("{message}"),
            Ok(CliReply::Error { message }) => return crate::fail(&format!("the app could not take the bundle: {message}")),
            Ok(other) => return crate::fail(&format!("unexpected reply {other:?}")),
            Err(e) => return crate::fail(&e),
        }
        LaunchSpec::Bundle { bundle: id.clone(), argv, env: o.env.clone() }
    };

    let req = ExportRequest { id: id.clone(), name: name.clone(), icon_png_b64: base64::engine::general_purpose::STANDARD.encode(&icon), launch };
    match client::request(&rt, &CliRequest::Export(req), Duration::from_secs(120)) {
        Ok(CliReply::Ok { message }) => {
            let mut recs = load_records();
            recs.retain(|(i, _)| *i != id);
            recs.push((id.clone(), name.clone()));
            save_records(&recs);
            println!("{name} exported as {id}: {message}");
            0
        }
        Ok(CliReply::Error { message }) => crate::fail(&message),
        Ok(other) => crate::fail(&format!("unexpected reply {other:?}")),
        Err(e) => crate::fail(&e),
    }
}

pub fn cmd_list() -> i32 {
    let recs = load_records();
    if recs.is_empty() {
        println!("Nothing exported from here yet.");
    }
    for (id, name) in recs {
        println!("{id:<40} {name}");
    }
    0
}

pub fn cmd_remove(args: &[String]) -> i32 {
    let Some(id) = args.first() else { return crate::fail("which one? (lindroids list)") };
    let sb = sandbox::detect();
    let rt = match crate::ensure_server(&sb) {
        Ok(rt) => rt,
        Err(e) => return crate::fail(&e),
    };
    if !crate::ensure_app(&sb, &rt, false) {
        return crate::fail("the Lindroids app is not connected");
    }
    match client::request(&rt, &CliRequest::Unexport { id: id.clone() }, Duration::from_secs(60)) {
        Ok(CliReply::Ok { message }) => {
            let mut recs = load_records();
            recs.retain(|(i, _)| i != id);
            save_records(&recs);
            println!("{message}");
            0
        }
        Ok(CliReply::Error { message }) => crate::fail(&message),
        Ok(other) => crate::fail(&format!("unexpected reply {other:?}")),
        Err(e) => crate::fail(&e),
    }
}
