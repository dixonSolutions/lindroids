//! `lindroids config`: the settings, kept in `<state>/config` as `key=value` lines.
//!
//! `browser` says how an exported program is shown: `webapk` (the default: an app of its own
//! wherever a browser can make one), `default` (the default browser's window, a home screen
//! shortcut), or a browser's package (that browser, if it can make the app). An export's own tag
//! (`export --browser <value>`) overrides it; see export.rs.

use std::collections::BTreeMap;
use std::path::PathBuf;

pub const BROWSER_DEFAULT: &str = "webapk";

fn path() -> PathBuf {
    lindroids_server::state_dir().join("config")
}

fn read() -> BTreeMap<String, String> {
    std::fs::read_to_string(path())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

pub fn browser_setting() -> String {
    read().remove("browser").filter(|v| valid_browser(v)).unwrap_or_else(|| BROWSER_DEFAULT.to_string())
}

/// Whether `v` is a browser setting or an export tag: `default`, `webapk`, `home-screen`, or a
/// package name (letters, digits, `.`, `_`, `-`, with a dot).
pub fn valid_browser(v: &str) -> bool {
    matches!(v, "default" | "webapk" | "home-screen")
        || (v.contains('.') && v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
}

fn set(key: &str, value: &str) -> Result<(), String> {
    let mut all = read();
    all.insert(key.to_string(), value.to_string());
    let text: String = all.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    let p = path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))
}

pub fn cmd_config(args: &[String]) -> i32 {
    match args {
        [] => {
            println!("browser  {}", browser_setting());
            println!("  default: webapk (an app of its own wherever a browser can make one; the default browser first)");
            println!("  webapk | default | <browser package>; an export's own: lindroids export --browser <value> <program>");
            0
        }
        [key] if key == "browser" => {
            println!("{}", browser_setting());
            0
        }
        [key, value] if key == "browser" => {
            if !valid_browser(value) {
                return crate::fail(&format!("{value:?}: use webapk, default, or a browser's package (e.g. com.android.chrome)"));
            }
            match set("browser", value) {
                Ok(()) => {
                    println!("browser is {value} for exported programs");
                    0
                }
                Err(e) => crate::fail(&e),
            }
        }
        _ => crate::fail("usage: lindroids config [browser [webapk|default|<browser package>]]"),
    }
}

#[cfg(test)]
mod tests {
    use super::valid_browser;

    #[test]
    fn browser_values() {
        assert!(valid_browser("webapk") && valid_browser("default") && valid_browser("home-screen"));
        assert!(valid_browser("com.android.chrome") && valid_browser("com.sec.android.app.sbrowser"));
        assert!(!valid_browser("chrome") && !valid_browser("") && !valid_browser("com.x/y"));
    }
}
