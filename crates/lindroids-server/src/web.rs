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

use base64::Engine;
use sha1::{Digest, Sha1};

const INDEX_HTML: &str = include_str!("../data/web/index.html");
const CLIENT_JS: &str = include_str!("../data/web/lindroids.js");
const ICON_SVG: &str = include_str!("../data/web/icon.svg");
/// The microphone's AudioWorklet module: a file of its own, as the page's CSP allows no blob: one.
const MIC_WORKLET_JS: &str = include_str!("../data/web/mic-worklet.js");

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
        "/lindroids.js" => ("text/javascript; charset=utf-8", CLIENT_JS.replace("@BUILD@", page_build()).into_bytes().into()),
        "/icon.svg" => ("image/svg+xml", ICON_SVG.as_bytes().into()),
        "/mic-worklet.js" => ("text/javascript; charset=utf-8", MIC_WORKLET_JS.as_bytes().into()),
        p if p.starts_with("/run-icon/") => match run_icon(p) {
            Some(png) => ("image/png", png.into()),
            None => return Answer::Close(response(404, "text/plain", b"not found\n")),
        },
        _ => return Answer::Close(response(404, "text/plain", b"not found\n")),
    };
    Answer::Close(response(200, ctype, &body))
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
