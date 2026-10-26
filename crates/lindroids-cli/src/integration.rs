//! Terminal commands that reach Android through the server: `lindroids open` (what `xdg-open`
//! runs for Linux programs), `lindroids notify` and `lindroids info`.

use std::os::fd::AsRawFd;
use std::path::Path;
use std::time::Duration;

use lindroids_proto::{CliReply, CliRequest, SystemState};

use crate::{client, fail, sandbox};

/// `lindroids open <file|URI>`: an existing file is handed over as a descriptor (the server may
/// see another filesystem than this command does, as from inside a proot distro); anything else
/// is a URI (https:, mailto:, tel:, geo:, file:...).
pub fn cmd_open(args: &[String]) -> i32 {
    let Some(target) = args.first() else { return fail("open what? (lindroids open <file|URI>)") };
    let rt = sandbox::runtime_dir();
    let local = target.strip_prefix("file://").map(percent_decode).unwrap_or_else(|| target.clone());
    let path = Path::new(&local);
    let reply = if !target.contains("://") || target.starts_with("file://") {
        if !path.exists() && !target.starts_with("file://") && target.contains(':') {
            // mailto:, tel:, geo: have no "//".
            client::request(&rt, &CliRequest::Open { target: target.clone() }, Duration::from_secs(150))
        } else if path.is_dir() {
            return fail(&format!("{local} is a folder; Android can only open files from here"));
        } else {
            match std::fs::File::open(path) {
                Ok(f) => {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
                    client::request_with_fds(&rt, &CliRequest::Open { target: name }, &[f.as_raw_fd()], Duration::from_secs(150))
                }
                Err(e) => return fail(&format!("{local}: {e}")),
            }
        }
    } else {
        client::request(&rt, &CliRequest::Open { target: target.clone() }, Duration::from_secs(150))
    };
    match reply {
        Ok(CliReply::Ok { .. }) => 0,
        Ok(CliReply::Error { message }) => fail(&message),
        Ok(other) => fail(&format!("unexpected reply {other:?}")),
        Err(e) => fail(&e),
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `lindroids notify <title> [body...]`.
pub fn cmd_notify(args: &[String]) -> i32 {
    let Some(title) = args.first() else { return fail("lindroids notify <title> [body]") };
    let body = args[1..].join(" ");
    match client::request(&sandbox::runtime_dir(), &CliRequest::Notify { title: title.clone(), body }, Duration::from_secs(10)) {
        Ok(CliReply::Ok { .. }) => 0,
        Ok(CliReply::Error { message }) => fail(&message),
        Ok(other) => fail(&format!("unexpected reply {other:?}")),
        Err(e) => fail(&e),
    }
}

/// `lindroids info [--json]`: what Android reports (theme, network, memory, battery...).
pub fn cmd_info(args: &[String]) -> i32 {
    let json = args.iter().any(|a| a == "--json");
    let s = match client::request(&sandbox::runtime_dir(), &CliRequest::Info, Duration::from_secs(10)) {
        Ok(CliReply::Info { system: Some(s) }) => s,
        Ok(CliReply::Info { system: None }) => return fail("the Lindroids app has not reported yet (is it connected, and as new as this CLI?)"),
        Ok(CliReply::Error { message }) => return fail(&message),
        Ok(other) => return fail(&format!("unexpected reply {other:?}")),
        Err(e) => return fail(&e),
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&s).unwrap_or_default());
    } else {
        print!("{}", describe(&s));
    }
    0
}

fn describe(s: &SystemState) -> String {
    let gib = |b: u64| b as f64 / (1u64 << 30) as f64;
    let mut out = String::new();
    out += &format!("theme       {}{}\n", if s.dark { "dark" } else { "light" }, if s.high_contrast { ", high contrast" } else { "" });
    if let Some([r, g, b]) = s.accent {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        out += &format!("accent      #{:02x}{:02x}{:02x}\n", c(r), c(g), c(b));
    }
    out += &format!("text scale  {:.2}{}\n", s.font_scale, if s.reduced_motion { ", animations off" } else { "" });
    let conn = match s.network.connectivity {
        4 => "internet",
        3 => "captive portal",
        2 => "limited",
        _ => "local only",
    };
    out += &format!(
        "network     {}\n",
        if s.network.available { format!("{conn}{}", if s.network.metered { ", metered" } else { "" }) } else { "none".into() }
    );
    out += &format!("memory      {:.1} of {:.1} GiB free{}\n", gib(s.memory.available), gib(s.memory.total), if s.memory.low { " (low)" } else { "" });
    if let Some(b) = &s.battery {
        out += &format!("battery     {}%{}{}\n", b.level, if b.charging { ", charging" } else { "" }, if s.power_saver { ", battery saver" } else { "" });
    }
    if !s.locale.is_empty() {
        out += &format!("locale      {}\n", s.locale);
    }
    if !s.timezone.is_empty() {
        out += &format!("time zone   {}\n", s.timezone);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_file_uris() {
        assert_eq!(percent_decode("/sdcard/My%20Files/a%231.txt"), "/sdcard/My Files/a#1.txt");
        assert_eq!(percent_decode("/a%2"), "/a%2");
    }

    #[test]
    fn describes_android() {
        let s = SystemState { dark: true, accent: Some([1.0, 0.5, 0.0]), font_scale: 1.0, ..Default::default() };
        let d = describe(&s);
        assert!(d.contains("theme       dark"));
        assert!(d.contains("#ff8000"));
    }
}
