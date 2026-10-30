//! The browser as the display: the server's loopback port also speaks HTTP.
//!
//! `GET /` serves a small web client (embedded in the binary, so it always matches the server),
//! and `GET /link` upgrades to a WebSocket that carries the ordinary link protocol: every
//! server→browser WebSocket message is exactly one protocol message (its JSON line and payload);
//! browser→server messages are a plain byte stream of the same lines. The browser authenticates
//! like the app did (HMAC over nonces, WebCrypto), with the secret `lindroids run` put in the
//! URL's fragment, which never reaches any server.
//!
//! Nothing here needs an Android app or permission: any browser on the phone can open
//! `http://127.0.0.1:<port>/`, and Termux opens it with `termux-open-url`.

use std::path::{Path, PathBuf};

use base64::Engine;
use sha1::{Digest, Sha1};

const INDEX_HTML: &str = include_str!("../data/web/index.html");
const CLIENT_JS: &str = include_str!("../data/web/lindroids.js");
const ICON_SVG: &str = include_str!("../data/web/icon.svg");
/// The microphone's AudioWorklet module: a file of its own, as the page's CSP allows no blob: one.
const MIC_WORKLET_JS: &str = include_str!("../data/web/mic-worklet.js");
/// An exported program's own pages, when the server cannot be reached (`export --webapk`).
const APP_SW_JS: &str = include_str!("../data/web/app-sw.js");
const APP_OFFLINE_HTML: &str = include_str!("../data/web/app-offline.html");
const APP_OFFLINE_JS: &str = include_str!("../data/web/app-offline.js");

/// The build of the page as served: the program's build and a hash of the page itself, so that
/// it changes with every change to the page, dev builds included. The server says it in its
/// Hello; a page left open across an update (a window raised rather than reloaded) reloads.
pub fn page_build() -> &'static str {
    static B: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    B.get_or_init(|| {
        let mut h = Sha1::new();
        h.update(INDEX_HTML.as_bytes());
        h.update(CLIENT_JS.as_bytes());
        h.update(MIC_WORKLET_JS.as_bytes());
        let d = h.finalize();
        format!("{}+{:02x}{:02x}{:02x}{:02x}", crate::BUILD, d[0], d[1], d[2], d[3])
    })
}

/// Path of the WebSocket.
pub const LINK_PATH: &str = "/link";

/// Most a request head may take.
const MAX_HEAD: usize = 16 << 10;
/// Largest WebSocket message accepted from a browser.
const MAX_MESSAGE: u64 = 64 << 20;

/// What the first bytes on a loopback connection are.
pub enum Sniff {
    /// Not enough bytes to tell.
    More,
    /// The link protocol (a JSON line).
    Raw,
    /// An HTTP request head, complete; the bytes after it.
    Http(Request, Vec<u8>),
    /// HTTP, but unusable.
    Bad,
}

#[derive(Debug, Default)]
pub struct Request {
    pub method: String,
    pub path: String,
    /// What follows `?`, without the fragment.
    pub query: String,
    pub headers: Vec<(String, String)>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    fn has_token(&self, name: &str, token: &str) -> bool {
        self.header(name).map_or(false, |v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case(token)))
    }
}

pub fn sniff(buf: &[u8]) -> Sniff {
    const GET: &[u8] = b"GET ";
    let n = buf.len().min(GET.len());
    if buf[..n] != GET[..n] {
        return Sniff::Raw;
    }
    let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
        return if buf.len() > MAX_HEAD { Sniff::Bad } else { Sniff::More };
    };
    let Ok(head) = std::str::from_utf8(&buf[..end]) else { return Sniff::Bad };
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split(' ');
    let method = first.next().unwrap_or("").to_string();
    let target = first.next().unwrap_or("");
    let target = target.split('#').next().unwrap_or("");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let (path, query) = (path.to_string(), query.to_string());
    let headers = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
    Sniff::Http(Request { method, path, query, headers }, buf[end + 4..].to_vec())
}

static LABEL: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// The label of the one origin pages are served in (`crate::origin_label`).
pub fn set_origin_label(label: String) {
    let _ = LABEL.set(label);
}

pub fn origin_label() -> &'static str {
    LABEL.get().map_or("", String::as_str)
}

/// The origin this server answers to, `<label>.localhost:<port>`. Anything else (a web site, a
/// site that rebinds its DNS name to 127.0.0.1, an app guessing at 127.0.0.1:<port>) gets
/// nothing, not even the label.
fn our_host(host: &str, port: u16) -> bool {
    LABEL.get().map_or(false, |l| auth_same(host, &format!("{l}.localhost:{port}")))
}

fn auth_same(a: &str, b: &str) -> bool {
    lindroids_proto::auth::same(&a.to_ascii_lowercase(), b)
}

fn our_origin(origin: &str, port: u16) -> bool {
    origin.strip_prefix("http://").map_or(false, |h| our_host(h, port))
}

pub enum Answer {
    /// Write this and close.
    Close(Vec<u8>),
    /// Write this; the connection is a WebSocket from now on.
    Upgrade(Vec<u8>),
}

pub fn answer(req: &Request, port: u16) -> Answer {
    if req.method != "GET" {
        return Answer::Close(response(405, "text/plain", b"GET only\n"));
    }
    if !req.header("Host").map_or(false, |h| our_host(h, port)) {
        return Answer::Close(response(421, "text/plain", b"wrong host\n"));
    }
    if req.path == "/ls" {
        let token = picker_token();
        if query_value(&req.query, "t").as_deref() != Some(token.as_str()) {
            return Answer::Close(response(403, "text/plain", b"forbidden\n"));
        }
        let (status, body) = match list_dir(&req.query) {
            Ok(json) => (200, json),
            Err(e) => (200, serde_json::json!({ "error": e }).to_string()),
        };
        return Answer::Close(response(status, "application/json", body.as_bytes()));
    }
    if req.path == LINK_PATH {
        if !req.has_token("Upgrade", "websocket") || !req.has_token("Connection", "upgrade") {
            return Answer::Close(response(426, "text/plain", b"WebSocket only\n"));
        }
        if !req.header("Origin").map_or(false, |o| our_origin(o, port)) {
            return Answer::Close(response(403, "text/plain", b"wrong origin\n"));
        }
        let Some(key) = req.header("Sec-WebSocket-Key") else {
            return Answer::Close(response(400, "text/plain", b"no key\n"));
        };
        let head = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
            accept_key(key)
        );
        return Answer::Upgrade(head.into_bytes());
    }
    let (ctype, body): (&str, std::borrow::Cow<[u8]>) = match req.path.as_str() {
        "/" | "/index.html" => ("text/html; charset=utf-8", index_html(&req.query).into_bytes().into()),
        "/lindroids.js" => (
            "text/javascript; charset=utf-8",
            CLIENT_JS.replace("@BUILD@", page_build()).replace("@PICKER_TOKEN@", &picker_token()).into_bytes().into(),
        ),
        "/icon.svg" => ("image/svg+xml", ICON_SVG.as_bytes().into()),
        "/mic-worklet.js" => ("text/javascript; charset=utf-8", MIC_WORKLET_JS.as_bytes().into()),
        p if p.starts_with("/app/") => match app_file(p, &req.query, port) {
            Some(file) => file,
            None => return Answer::Close(response(404, "text/plain", b"not found\n")),
        },
        p if p.starts_with("/run-icon/") => match run_icon(p) {
            Some(png) => ("image/png", png.into()),
            None => return Answer::Close(response(404, "text/plain", b"not found\n")),
        },
        _ => return Answer::Close(response(404, "text/plain", b"not found\n")),
    };
    Answer::Close(response(200, ctype, &body))
}

/// A secret only the page this server served knows: `/ls` lists directories for its picker. It
/// comes from the origin label (kept across restarts), so a page left open over a server restart
/// can still browse.
fn picker_token() -> String {
    format!("{}-picker", crate::origin_label())
}

/// One `key=value` of a query, percent-decoded (no `+`: a path may have one).
fn query_value(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| percent_decode(v))
    })
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What the picker means by "Linux environment": Termux's prefix, else the container's root.
fn env_root() -> PathBuf {
    let host = crate::host::detect();
    match host.layer {
        crate::host::Layer::Termux => PathBuf::from(std::env::var("PREFIX").unwrap_or_else(|_| lindroids_proto::TERMUX_PREFIX.into())),
        _ => PathBuf::from("/"),
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).filter(|p| p.is_dir()).unwrap_or_else(env_root)
}

/// This process may write here (the picker's Save belongs in such a directory).
fn writable(p: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(p.as_os_str().as_bytes()).is_ok_and(|c| unsafe { libc::access(c.as_ptr(), libc::W_OK) } == 0)
}

/// `GET /ls?t=<token>&path=<path>`: a directory for the page's picker. `path` is an absolute
/// path, `~` (the home), `~/...`, or `root` (the Linux environment: Termux's prefix or the
/// container's `/`). Only names, sizes and kinds: the page picks paths, and the server hands them
/// to the program as they are.
fn list_dir(query: &str) -> Result<String, String> {
    let root = env_root();
    let home = home_dir();
    let asked = query_value(query, "path").unwrap_or_default();
    let p = match asked.as_str() {
        "" | "~" => home.clone(),
        "root" => root.clone(),
        _ if asked.starts_with("~/") => home.join(&asked[2..]),
        _ => PathBuf::from(&asked),
    };
    if !p.is_absolute() {
        return Err(format!("{}: not an absolute path", p.display()));
    }
    let path = std::fs::canonicalize(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    if !path.is_dir() {
        return Err(format!("{}: not a directory", path.display()));
    }
    let mut entries = Vec::new();
    let dir = std::fs::read_dir(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let meta = std::fs::metadata(e.path()).or_else(|_| e.metadata()).ok();
        let is_dir = meta.as_ref().map_or(false, |m| m.is_dir());
        let size = meta.as_ref().map_or(0, |m| m.len());
        let entry_path = e.path();
        entries.push(serde_json::json!({
            "name": name,
            "dir": is_dir,
            "hidden": name.starts_with('.'),
            "size": size,
            "writable": writable(&entry_path),
        }));
        if entries.len() >= 5000 {
            break;
        }
    }
    entries.sort_by(|a, b| {
        let (ad, an) = (a["dir"].as_bool().unwrap_or(false), a["name"].as_str().unwrap_or(""));
        let (bd, bn) = (b["dir"].as_bool().unwrap_or(false), b["name"].as_str().unwrap_or(""));
        bd.cmp(&ad).then_with(|| an.to_lowercase().cmp(&bn.to_lowercase())).then_with(|| an.cmp(bn))
    });
    let parent = path.parent().filter(|p| *p != path.as_path()).map(|p| p.to_string_lossy().into_owned());
    Ok(serde_json::json!({
        "path": path.to_string_lossy(),
        "parent": parent,
        "home": home.to_string_lossy(),
        "root": root.to_string_lossy(),
        "writable": writable(&path),
        "entries": entries,
    })
    .to_string())
}

/// The run a window's URL is for (`?window&run=<pid>`).
fn run_of_query(query: &str) -> Option<u32> {
    query.split('&').find_map(|kv| kv.strip_prefix("run=")).and_then(|v| v.parse().ok())
}

/// The page, as a window of a run's own: the program's name and icon from the start, so the
/// browser shows them outside the page too (Chrome's Custom Tabs put the page's title and favicon
/// on the window's task in Recents, and take the favicon from a URL, not from a `data:` one).
fn index_html(query: &str) -> String {
    let Some((run, (name, Some(_)))) = run_of_query(query).and_then(|r| Some((r, crate::icons::launch_info(r)?))) else {
        return INDEX_HTML.to_string();
    };
    let name = name.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    INDEX_HTML
        .replacen("<title>Lindroids</title>", &format!("<title>{name}</title>"), 1)
        .replacen(r#"<link rel="icon" href="/icon.svg">"#, &format!(r#"<link rel="icon" type="image/png" sizes="192x192" href="/run-icon/{run}.png">"#), 1)
}

/// `/app/<id>/...`: an exported program's own pages (`lindroids export --webapk`), which the
/// browser installs as an app of its own (a WebAPK: Chrome, Samsung Internet), named and iconed
/// after the program, its scope `/app/<id>/`:
/// - `/app/<id>/` the page: `?install` installs it; with no run, it is what the app opens with
///   (it asks for the program and goes to its window); `?window&run=<pid>` is a run's window;
/// - `manifest.webmanifest`, `icon.png` (512 pixels square), `maskable.png` (as Android's adaptive
///   icons draw it), made by `export`;
/// - `sw.js`, `offline`, `offline.js`: the page shown when the server cannot be reached (Termux
///   closed), which starts the program through Termux;
/// - `<name>.apk`: the program's own app, when `export` built one (`<state>/apk/<id>.apk`).
fn app_file(path: &str, query: &str, port: u16) -> Option<(&'static str, std::borrow::Cow<'static, [u8]>)> {
    let (id, file) = path.strip_prefix("/app/")?.split_once('/')?;
    let app = crate::load_exported(id)?;
    app.webapk.as_ref()?;
    let base = format!("/app/{id}/");
    let apps = crate::state_dir().join("apps");
    let icon = || std::fs::read(apps.join(format!("{id}.app.png"))).ok().or_else(|| base64::engine::general_purpose::STANDARD.decode(&app.icon_png_b64).ok());
    let maskable = || std::fs::read(apps.join(format!("{id}.maskable.png"))).ok();
    let html = |s: String| -> Option<(&'static str, std::borrow::Cow<'static, [u8]>)> { Some(("text/html; charset=utf-8", s.into_bytes().into())) };
    match file {
        "" => {
            let page = if run_of_query(query).is_some() { index_html(query) } else { INDEX_HTML.to_string() };
            let mut page = page.replacen(r#"<link rel="icon" href="/icon.svg">"#, &format!(r#"<link rel="icon" type="image/png" href="{base}icon.png">"#), 1);
            if run_of_query(query).is_none() {
                page = page.replacen("<title>Lindroids</title>", &format!("<title>{}</title>", html_escape(&app.name)), 1);
            }
            html(page.replacen("</title>", &format!("</title>\n<link rel=\"manifest\" href=\"{base}manifest.webmanifest\">"), 1))
        }
        "manifest.webmanifest" => Some(("application/manifest+json", app_manifest(&app, &icon()?, maskable().as_deref()).into_bytes().into())),
        "icon.png" => Some(("image/png", icon()?.into())),
        "maskable.png" => Some(("image/png", maskable()?.into())),
        "sw.js" => Some(("text/javascript; charset=utf-8", APP_SW_JS.replace("@BUILD@", page_build()).into_bytes().into())),
        "offline" => html(
            APP_OFFLINE_HTML
                .replace("@NAME@", &html_escape(&app.name))
                .replace("@LAUNCH@", &html_escape(&format!("http://127.0.0.1:{port}/launch/termux/{id}"))),
        ),
        "offline.js" => Some(("text/javascript; charset=utf-8", APP_OFFLINE_JS.as_bytes().into())),
        // The program's own app (`lindroids export`, built on the phone), for a browser to
        // download when the terminal app may not open Android's installer.
        // Any `<name>.apk`, so the browser saves it under the program's name.
        f if f.ends_with(".apk") && !f.contains('/') => Some(("application/vnd.android.package-archive", std::fs::read(crate::state_dir().join("apk").join(format!("{id}.apk"))).ok()?.into())),
        _ => None,
    }
}

/// An exported program's web app manifest: what the browser installs it as. Full screen from the
/// start; the icons' URLs change with them (Chrome takes an icon's URL as never changing).
fn app_manifest(app: &lindroids_proto::Exported, icon: &[u8], maskable: Option<&[u8]>) -> String {
    let base = format!("/app/{}/", app.id);
    let tag = |png: &[u8]| {
        let d = Sha1::digest(png);
        format!("{:02x}{:02x}{:02x}{:02x}", d[0], d[1], d[2], d[3])
    };
    let size = |png: &[u8]| png_size(png).map_or_else(|| "192x192".to_string(), |(w, h)| format!("{w}x{h}"));
    let mut icons = vec![serde_json::json!({ "src": format!("{base}icon.png?v={}", tag(icon)), "sizes": size(icon), "type": "image/png", "purpose": "any" })];
    if let Some(m) = maskable {
        icons.push(serde_json::json!({ "src": format!("{base}maskable.png?v={}", tag(m)), "sizes": size(m), "type": "image/png", "purpose": "maskable" }));
    }
    serde_json::json!({
        "name": app.name,
        "short_name": app.name,
        "description": format!("{} from Linux, through Lindroids", app.name),
        "id": base,
        "start_url": base,
        "scope": base,
        "display": "fullscreen",
        "background_color": "#000000",
        "theme_color": "#0F1720",
        "icons": icons,
        // The app says it is this page's app: `getInstalledRelatedApps()` on the install page
        // tells it when Android hides the installed package from lookups.
        "related_applications": [{
            "platform": "webapp",
            "id": base,
            "url": format!("{base}manifest.webmanifest"),
        }],
    })
    .to_string()
}

/// Width and height from a PNG's header.
fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    if png.len() < 24 || &png[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    Some((u32::from_be_bytes(png[16..20].try_into().ok()?), u32::from_be_bytes(png[20..24].try_into().ok()?)))
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// `/run-icon/<pid>.png`: a run's icon.
fn run_icon(path: &str) -> Option<Vec<u8>> {
    let pid = path.strip_prefix("/run-icon/")?.strip_suffix(".png")?.parse().ok()?;
    let b64 = crate::icons::launch_info(pid)?.1?;
    base64::engine::general_purpose::STANDARD.decode(b64).ok()
}

fn response(status: u16, ctype: &str, body: &[u8]) -> Vec<u8> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        421 => "Misdirected Request",
        _ => "Upgrade Required",
    };
    let mut v = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\
         X-Frame-Options: DENY\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\n\
         Content-Security-Policy: default-src 'self'; connect-src 'self'; img-src 'self' data: blob:; style-src 'self' 'unsafe-inline'; frame-ancestors 'none'\r\n\
         Connection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    v.extend_from_slice(body);
    v
}

fn accept_key(key: &str) -> String {
    let mut h = Sha1::new();
    h.update(key.trim().as_bytes());
    h.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    base64::engine::general_purpose::STANDARD.encode(h.finalize())
}

/// One binary WebSocket message from the server (unmasked).
pub fn frame(opcode: u8, data: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(data.len() + 10);
    v.push(0x80 | opcode);
    match data.len() {
        n if n < 126 => v.push(n as u8),
        n if n <= 0xffff => {
            v.push(126);
            v.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            v.push(127);
            v.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    v.extend_from_slice(data);
    v
}

pub const OP_BINARY: u8 = 2;
const OP_CLOSE: u8 = 8;
const OP_PING: u8 = 9;

/// What came out of the browser's frames.
#[derive(Default)]
pub struct Unframed {
    /// Message bytes (the link's stream).
    pub data: Vec<u8>,
    /// Frames to send back (pongs, the close answer).
    pub reply: Vec<u8>,
    pub closed: bool,
    /// The close frame's status code, if it had one (1001: the page is going away).
    pub close_code: Option<u16>,
}

/// Take every complete frame off the front of `buf`.
pub fn unframe(buf: &mut Vec<u8>) -> Unframed {
    let mut out = Unframed::default();
    let mut pos = 0;
    loop {
        let b = &buf[pos..];
        if b.len() < 2 {
            break;
        }
        let opcode = b[0] & 0x0f;
        let masked = b[1] & 0x80 != 0;
        let (len, mut at) = match b[1] & 0x7f {
            126 if b.len() >= 4 => (u16::from_be_bytes([b[2], b[3]]) as u64, 4),
            127 if b.len() >= 10 => (u64::from_be_bytes(b[2..10].try_into().unwrap()), 10),
            126 | 127 => break,
            n => (n as u64, 2),
        };
        // Browsers always mask; RFC 6455 says to fail a connection that does not.
        if !masked || len > MAX_MESSAGE {
            out.closed = true;
            break;
        }
        if (b.len() as u64) < at as u64 + 4 + len {
            break;
        }
        let mask = [b[at], b[at + 1], b[at + 2], b[at + 3]];
        at += 4;
        let payload: Vec<u8> = b[at..at + len as usize].iter().enumerate().map(|(i, x)| x ^ mask[i % 4]).collect();
        pos += at + len as usize;
        match opcode {
            0 | 1 | 2 => out.data.extend_from_slice(&payload),
            OP_CLOSE => {
                out.reply.extend_from_slice(&frame(OP_CLOSE, &payload[..payload.len().min(2)]));
                out.close_code = (payload.len() >= 2).then(|| u16::from_be_bytes([payload[0], payload[1]]));
                out.closed = true;
                break;
            }
            OP_PING => out.reply.extend_from_slice(&frame(0x0a, &payload)),
            _ => {}
        }
    }
    buf.drain(..pos);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_query_and_lists_a_directory() {
        assert_eq!(percent_decode("a%20b%2Fc"), "a b/c");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(query_value("t=x&path=%2Ftmp", "path").as_deref(), Some("/tmp"));
        assert_eq!(query_value("path=a+b", "path").as_deref(), Some("a+b"));
        let dir = std::env::temp_dir().join(format!("lindroids-ls-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), b"hi").unwrap();
        let enc: String = dir
            .to_string_lossy()
            .bytes()
            .map(|b| if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
            .collect();
        let j: serde_json::Value = serde_json::from_str(&list_dir(&format!("path={enc}")).unwrap()).unwrap();
        assert_eq!(j["path"], dir.to_string_lossy().as_ref());
        assert_eq!(j["entries"][0]["name"], "sub");
        assert_eq!(j["entries"][0]["dir"], true);
        assert_eq!(j["entries"][1]["name"], "a.txt");
        assert_eq!(j["entries"][1]["size"], 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn close_codes() {
        let mut buf = masked(8, &1001u16.to_be_bytes());
        let u = unframe(&mut buf);
        assert!(u.closed);
        assert_eq!(u.close_code, Some(1001));
        let mut buf = masked(8, &[]);
        let u = unframe(&mut buf);
        assert!(u.closed);
        assert_eq!(u.close_code, None);
    }

    fn masked(opcode: u8, data: &[u8]) -> Vec<u8> {
        let mask = [1u8, 2, 3, 4];
        let mut v = vec![0x80 | opcode];
        if data.len() < 126 {
            v.push(0x80 | data.len() as u8);
        } else {
            v.push(0x80 | 126);
            v.extend_from_slice(&(data.len() as u16).to_be_bytes());
        }
        v.extend_from_slice(&mask);
        v.extend(data.iter().enumerate().map(|(i, x)| x ^ mask[i % 4]));
        v
    }

    #[test]
    fn accept_key_matches_rfc6455() {
        assert_eq!(accept_key("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn a_runs_window_has_its_name_and_icon() {
        let _s = crate::icons::tests::serial();
        // A live pid: launches whose process is gone are forgotten.
        let pid = std::process::id();
        crate::icons::register_launch(pid, "A <b>".into(), Some(base64::engine::general_purpose::STANDARD.encode(b"png")), true);
        let html = index_html(&format!("window&run={pid}"));
        assert!(html.contains("<title>A &lt;b&gt;</title>"), "{html}");
        assert!(html.contains(&format!(r#"href="/run-icon/{pid}.png""#)));
        assert_eq!(run_icon(&format!("/run-icon/{pid}.png")), Some(b"png".to_vec()));
        assert_eq!(index_html(""), INDEX_HTML);
        assert_eq!(run_icon("/run-icon/x.png"), None);
    }

    #[test]
    fn sniffs_http_and_raw() {
        assert!(matches!(sniff(b"{\"t\":\"hello\"}"), Sniff::Raw));
        assert!(matches!(sniff(b"GE"), Sniff::More));
        assert!(matches!(sniff(b"GET / HTTP/1.1\r\nHost: x"), Sniff::More));
        match sniff(b"GET /link?x=1 HTTP/1.1\r\nHost: 127.0.0.1:47282\r\nUpgrade: websocket\r\n\r\nrest") {
            Sniff::Http(r, rest) => {
                assert_eq!(r.path, "/link");
                assert_eq!(r.query, "x=1");
                assert_eq!(r.header("host"), Some("127.0.0.1:47282"));
                assert_eq!(rest, b"rest");
            }
            _ => panic!("not http"),
        }
    }

    #[test]
    fn refuses_other_hosts_and_origins() {
        let req = |host: &str, origin: &str| Request {
            method: "GET".into(),
            path: LINK_PATH.into(),
            query: String::new(),
            headers: vec![
                ("Host".into(), host.into()),
                ("Origin".into(), origin.into()),
                ("Upgrade".into(), "websocket".into()),
                ("Connection".into(), "keep-alive, Upgrade".into()),
                ("Sec-WebSocket-Key".into(), "dGhlIHNhbXBsZSBub25jZQ==".into()),
            ],
        };
        set_origin_label("k7f3q9abcdefghijklmnopqrst".into());
        let ours = "k7f3q9abcdefghijklmnopqrst.localhost:47000";
        assert!(matches!(answer(&req(ours, &format!("http://{ours}")), 47000), Answer::Upgrade(_)));
        // Names DNS is the same as ours, case and all.
        assert!(matches!(answer(&req(&ours.to_uppercase(), &format!("http://{ours}")), 47000), Answer::Upgrade(_)));
        // Not the label: an app guessing at the port, or a web site.
        assert!(matches!(answer(&req("127.0.0.1:47000", "http://127.0.0.1:47000"), 47000), Answer::Close(_)));
        assert!(matches!(answer(&req("localhost:47000", "http://localhost:47000"), 47000), Answer::Close(_)));
        assert!(matches!(answer(&req("other.localhost:47000", "http://other.localhost:47000"), 47000), Answer::Close(_)));
        assert!(matches!(answer(&req("evil.example:47000", "http://evil.example:47000"), 47000), Answer::Close(_)));
        assert!(matches!(answer(&req(ours, "https://evil.example"), 47000), Answer::Close(_)));
        assert!(matches!(answer(&req(ours, "http://127.0.0.1:47000"), 47000), Answer::Close(_)));
    }

    #[test]
    fn unframes_split_messages_and_pings() {
        let mut all = masked(2, b"hello\n");
        all.extend(masked(9, b"p"));
        all.extend(masked(2, &[7u8; 300]));
        let mut buf = all[..5].to_vec();
        let u = unframe(&mut buf);
        assert!(u.data.is_empty() && !u.closed);
        buf.extend_from_slice(&all[5..]);
        let u = unframe(&mut buf);
        assert_eq!(&u.data[..6], b"hello\n");
        assert_eq!(u.data.len(), 306);
        assert_eq!(u.reply, frame(0x0a, b"p"));
        assert!(buf.is_empty());
    }

    #[test]
    fn own_windows_ask_for_full_screen_on_the_inputs_browsers_allow() {
        // Measured in Custom Tabs (docs/DISPLAY.md, "Full screen"): a touch's start is refused,
        // its end granted; a mouse press and a key press count from the press. Not asking on a
        // touch's end would leave the bar until a second tap.
        let line = CLIENT_JS
            .lines()
            .find(|l| l.contains("addEventListener(ev, full,"))
            .expect("own windows ask for full screen");
        for ev in ["'pointerdown'", "'pointerup'", "'touchend'", "'click'", "'keydown'"] {
            assert!(line.contains(ev), "{ev} missing: {line}");
        }
        assert!(!line.contains("touchstart"), "{line}");
        assert!(CLIENT_JS.contains("e.type === 'pointerdown' && e.pointerType !== 'mouse'"));
        assert!(CLIENT_JS.contains("trace(`opened ${document.fullscreenElement ? 'in' : 'out of'} full screen"));
    }

    #[test]
    fn frames_use_the_right_length_form() {
        assert_eq!(frame(2, &[0; 5])[1], 5);
        assert_eq!(frame(2, &[0; 200])[1], 126);
        assert_eq!(frame(2, &[0; 70000])[1], 127);
    }
}
