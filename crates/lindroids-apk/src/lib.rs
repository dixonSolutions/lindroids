//! A program's own Android app, built on the phone in Rust.
//!
//! `lindroids export` gives every exported program an app of its own: its name and icon in the
//! app list, on the home screen and in Recents, and a window that is Android's WebView rather than
//! a browser's tab. The app is the window app's code (`packaging/webview-app`) under a package,
//! name, icon and start page of the program's. The template (`template/window-app.apk`, made by
//! `packaging/build-app-template.sh`) is built once by the project and carried in the Lindroids
//! binary; per program, this crate only:
//!
//! - changes the compiled manifest's strings (package, label, start page, launch URL) and version,
//! - swaps the icon PNGs,
//! - writes the ZIP again (stored entries 4-byte aligned) and signs it with APK Signature
//!   Scheme v2, with a key made once on the phone.
//!
//! No Android SDK, aapt, zipalign, apksigner or Java on the phone, nothing sent anywhere, and a
//! build takes milliseconds.

mod axml;
pub mod sign;
mod zip;

pub use sign::Key;

/// The template, as `packaging/build-app-template.sh` made it.
pub static TEMPLATE: &[u8] = include_bytes!("../template/window-app.apk");

const PH_PACKAGE: &str = "org.lindroids.app.placeholder.package";
const PH_NAME: &str = "@@LINDROIDS_PROGRAM_NAME@@";
const PH_START: &str = "@@LINDROIDS_PROGRAM_START@@";
const PH_LAUNCH: &str = "@@LINDROIDS_PROGRAM_LAUNCH@@";
const PH_TERMUX: &str = "@@LINDROIDS_TERMUX_PACKAGE@@";
const PH_ID: &str = "@@LINDROIDS_PROGRAM_ID@@";

/// What makes one program's app.
pub struct Program<'a> {
    /// The Android package, `org.lindroids.app.<...>` (see [`package_for`]).
    pub package: &'a str,
    /// The name under the icon and in Recents.
    pub name: &'a str,
    /// The page the icon opens: the program's `/app/<id>/` on the Lindroids server.
    pub start: &'a str,
    /// What the app hands to the terminal app when the server is not running.
    pub launch: &'a str,
    /// The terminal app that runs Lindroids (`com.termux`).
    pub termux: &'a str,
    /// The export's id (`com.vscodium.codium`).
    pub id: &'a str,
    /// Must grow with every build of the same package, or Android refuses the update.
    pub version_code: u32,
    pub version_name: &'a str,
    /// The plain icon (PNG; Android 7 and launchers without adaptive icons).
    pub icon: &'a [u8],
    /// The adaptive icon's foreground (PNG, the program's icon inside the middle 66 of 108).
    pub foreground: &'a [u8],
    /// The adaptive icon's background (PNG).
    pub background: &'a [u8],
}

/// The package name of an export's app: `org.lindroids.app.` and the id, made a valid Java
/// package (letters, digits and underscores, no segment starting with a digit or a keyword-like
/// clash). Distinct ids may only collide if they differ in characters outside `[A-Za-z0-9_.]`.
pub fn package_for(id: &str) -> String {
    let mut segs = Vec::new();
    for seg in id.split(|c| c == '.' || c == '-' || c == '/' || c == ' ').filter(|s| !s.is_empty()) {
        let mut s: String = seg.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c.to_ascii_lowercase() } else { '_' }).collect();
        if s.chars().next().map_or(true, |c| c.is_ascii_digit()) {
            s.insert(0, 'p');
        }
        segs.push(s);
    }
    if segs.is_empty() {
        segs.push("program".into());
    }
    format!("org.lindroids.app.{}", segs.join("."))
}

/// Builds and signs the program's app. Returns the APK.
pub fn build(p: &Program, key: &Key) -> Result<Vec<u8>, String> {
    build_from(TEMPLATE, p, key)
}

pub fn build_from(template: &[u8], p: &Program, key: &Key) -> Result<Vec<u8>, String> {
    let mut entries = zip::read(template)?;
    for e in entries.iter_mut() {
        let replace = match e.name.as_str() {
            "AndroidManifest.xml" => Some(manifest(&e.data()?, p)?),
            n if n.starts_with("res/mipmap-") && n.ends_with("/ic_launcher.png") => Some(p.icon.to_vec()),
            n if n.starts_with("res/mipmap-") && n.ends_with("/ic_launcher_foreground.png") => Some(p.foreground.to_vec()),
            n if n.starts_with("res/mipmap-") && n.ends_with("/ic_launcher_background.png") => Some(p.background.to_vec()),
            _ => None,
        };
        if let Some(data) = replace {
            *e = zip::Entry::stored(&e.name, data);
        }
    }
    let unsigned = zip::write(&entries);
    sign::sign_v2(&unsigned, key)
}

fn manifest(bin: &[u8], p: &Program) -> Result<Vec<u8>, String> {
    let mut m = axml::Manifest::parse(bin)?;
    for (from, to) in [(PH_PACKAGE, p.package), (PH_NAME, p.name), (PH_START, p.start), (PH_LAUNCH, p.launch), (PH_TERMUX, p.termux), (PH_ID, p.id)] {
        if m.replace(from, to) == 0 {
            return Err(format!("the app template has no {from}: rebuild it (packaging/build-app-template.sh)"));
        }
    }
    m.set_int("versionCode", p.version_code)?;
    m.set_string("versionName", p.version_name)?;
    Ok(m.write())
}

/// The strings of a compiled manifest, for tests and `lindroids app inspect`.
pub fn manifest_strings(apk: &[u8]) -> Result<Vec<String>, String> {
    let entries = zip::read(apk)?;
    let e = entries.iter().find(|e| e.name == "AndroidManifest.xml").ok_or("no AndroidManifest.xml")?;
    Ok(axml::Manifest::parse(&e.data()?)?.strings().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packages_are_valid_java_names() {
        assert_eq!(package_for("com.vscodium.codium"), "org.lindroids.app.com.vscodium.codium");
        assert_eq!(package_for("debian.xournal-pp"), "org.lindroids.app.debian.xournal.pp");
        assert_eq!(package_for("7zip"), "org.lindroids.app.p7zip");
        assert_eq!(package_for("Foo Bar!"), "org.lindroids.app.foo.bar_");
    }

    #[test]
    fn manifest_round_trips() {
        let entries = zip::read(TEMPLATE).unwrap();
        let bin = entries.iter().find(|e| e.name == "AndroidManifest.xml").unwrap().data().unwrap();
        let m = axml::Manifest::parse(&bin).unwrap();
        assert_eq!(m.write(), bin, "an unchanged manifest is written byte for byte");
    }

    #[test]
    fn builds_a_program() {
        let key = Key::generate("Lindroids test").unwrap();
        let p = Program {
            package: "org.lindroids.app.test.program",
            name: "Test Program",
            start: "http://abc.localhost:47000/app/test.program/",
            launch: "http://127.0.0.1:47000/launch/termux/test.program",
            termux: "com.termux",
            id: "test.program",
            version_code: 42,
            version_name: "0.1.0",
            icon: &[1, 2, 3],
            foreground: &[4],
            background: &[5],
        };
        let apk = build(&p, &key).unwrap();
        let strings = manifest_strings(&apk).unwrap();
        assert!(strings.iter().any(|s| s == "Test Program"));
        assert!(!strings.iter().any(|s| s.contains("@@")));
        let e = zip::read(&apk).unwrap();
        assert!(e.iter().any(|e| e.name.ends_with("/ic_launcher.png") && e.raw == [1, 2, 3]));
    }
}
