//! Running Flatpak apps without bubblewrap: proot assembles the same filesystem view flatpak would
//! (runtime at /usr, app at /app).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::run::Opts;

#[derive(Debug, Clone)]
pub struct App {
    pub id: String,
    pub root: PathBuf,
}

/// org.gnome.Calculator, io.github.foo.Bar: at least two dots, no slashes.
pub fn looks_like_id(s: &str) -> bool {
    s.matches('.').count() >= 2 && !s.contains('/') && s.split('.').all(|p| !p.is_empty())
}

pub fn find(_root: &Path, _id: &str) -> Option<App> {
    None
}

pub fn command(app: &App, _args: &[String], _o: &Opts, _rt: &Path) -> Result<Command, String> {
    Err(format!("running Flatpak {} is not supported yet", app.id))
}

pub fn identify(app: &App, _o: &Opts) -> (String, Vec<u8>) {
    (app.id.clone(), crate::desktop::fallback_icon(&app.id, 192))
}
