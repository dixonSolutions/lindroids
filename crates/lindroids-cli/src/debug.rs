//! `lindroids debug-app`: stand in for the Android app, for tests on any Linux machine.
//!
//! Pairs with the local server, gives every window a size, writes the newest frame of each window
//! to `<dir>/window-<n>.png`, and reads input commands from stdin:
//!
//!   tap <win> <x> <y>        touch down + up
//!   drag <win> <x0> <y0> <x1> <y1>
//!   click <win> <x> <y>      mouse button 1
//!   type <win> <text...>     IME text
//!   key <win> <androidKeyCode>
//!   resize <win> <w> <h>
//!   close <win>
//!   clip <text...>           set the "Android" clipboard
//!
//! Sound streams are acknowledged as they arrive; each one's running byte count is kept in
//! `<dir>/audio-<n>.bytes`. `--socket-name <name>` links to a server started with the same flag.

use std::collections::HashMap;
use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lindroids_proto::{self as proto, PointerAction, ToApp, ToServer, TouchAction};
use lindroids_server::applink::{self, Conn, Outcome};

struct Win {
    width: u32,
    height: u32,
    frame: Vec<u32>,
    frames: u64,
}

fn send(s: &Mutex<Conn>, m: &ToServer) {
    let _ = s.lock().unwrap().write_all(&proto::encode(m));
}

pub fn cmd_debug_app(args: &[String]) -> i32 {
    let mut dir = PathBuf::from("lindroids-frames");
    let mut size = (800u32, 600u32);
    let mut force_tcp = false;
    let mut socket_name: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dir" => {
                i += 1;
                dir = PathBuf::from(&args[i]);
            }
            "--size" => {
                i += 1;
                let (w, h) = args[i].split_once('x').unwrap_or(("800", "600"));
                size = (w.parse().unwrap_or(800), h.parse().unwrap_or(600));
            }
            "--tcp" => force_tcp = true,
            "--socket-name" => {
                i += 1;
                socket_name = args.get(i).cloned();
            }
            _ => {}
        }
        i += 1;
    }
    let _ = std::fs::create_dir_all(&dir);
    let rt = crate::sandbox::runtime_dir();
    let uid = unsafe { libc::getuid() };
    // Pair once by code like the app does, then keep the secret.
    let secret_file = dir.join(".secret");
    let secret = std::fs::read_to_string(&secret_file).ok();
    let code = if secret.is_none() { std::fs::read_to_string(rt.join("pairing-code")).ok() } else { None };
    let socket = if force_tcp { Some("lindroids.nothing-listens-here") } else { socket_name.as_deref() };
    let (conn, mut framer, backlog) = match applink::connect(uid, socket, secret.as_deref(), code.as_deref().map(str::trim), applink::Display { density: 1.0, screen: Some(size) }, "debug-app") {
        Outcome::Linked { conn, framer, paired, sandbox, backlog } => {
            if let Some(s) = paired {
                let _ = std::fs::write(&secret_file, s);
            }
            println!("linked over {} to {sandbox}", conn.kind());
            (conn, framer, backlog)
        }
        Outcome::Denied { reason, .. } => return crate::fail(&format!("denied: {reason}")),
        Outcome::WrongServer => return crate::fail("the server did not prove it knows our secret"),
        Outcome::Absent(e) => return crate::fail(&format!("no server: {e}")),
    };
    let mut reader = conn.try_clone().unwrap();
    let writer = Arc::new(Mutex::new(conn));

    // stdin commands
    {
        let w = writer.clone();
        std::thread::spawn(move || {
            let t0 = Instant::now();
            let t = || (t0.elapsed().as_millis() as u32).wrapping_add(1);
            for line in std::io::stdin().lock().lines().map_while(Result::ok) {
                let p: Vec<&str> = line.split_whitespace().collect();
                let n = |i: usize| p.get(i).and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
                let win = n(1) as u32;
                match p.first().copied() {
                    Some("tap") => {
                        send(&w, &ToServer::Touch { win, action: TouchAction::Down, id: 0, x: n(2), y: n(3), time_ms: t() });
                        std::thread::sleep(Duration::from_millis(60));
                        send(&w, &ToServer::Touch { win, action: TouchAction::Up, id: 0, x: n(2), y: n(3), time_ms: t() });
                    }
                    Some("drag") => {
                        send(&w, &ToServer::Touch { win, action: TouchAction::Down, id: 0, x: n(2), y: n(3), time_ms: t() });
                        for k in 1..=10 {
                            let f = k as f32 / 10.0;
                            std::thread::sleep(Duration::from_millis(16));
                            send(&w, &ToServer::Touch { win, action: TouchAction::Move, id: 0, x: n(2) + (n(4) - n(2)) * f, y: n(3) + (n(5) - n(3)) * f, time_ms: t() });
                        }
                        send(&w, &ToServer::Touch { win, action: TouchAction::Up, id: 0, x: n(4), y: n(5), time_ms: t() });
                    }
                    Some("click") => {
                        send(&w, &ToServer::Pointer { win, action: PointerAction::Motion, x: n(2), y: n(3), button: 0, time_ms: t() });
                        send(&w, &ToServer::Pointer { win, action: PointerAction::Press, x: n(2), y: n(3), button: 1, time_ms: t() });
                        send(&w, &ToServer::Pointer { win, action: PointerAction::Release, x: n(2), y: n(3), button: 1, time_ms: t() });
                    }
                    Some("type") => send(&w, &ToServer::Text { win, text: p[2..].join(" ") }),
                    Some("key") => {
                        send(&w, &ToServer::Key { win, code: n(2) as u32, down: true, time_ms: t() });
                        send(&w, &ToServer::Key { win, code: n(2) as u32, down: false, time_ms: t() });
                    }
                    Some("resize") => send(&w, &ToServer::Resize { win, width: n(2) as u32, height: n(3) as u32, density: 1.0 }),
                    Some("close") => send(&w, &ToServer::Close { win }),
                    Some("clip") => send(&w, &ToServer::Clipboard { text: p[1..].join(" ") }),
                    _ => eprintln!("debug-app: unknown command {line:?}"),
                }
            }
        });
    }

    let mut wins: HashMap<u32, Win> = HashMap::new();
    let mut queue: Vec<(ToApp, Vec<u8>)> = backlog;
    loop {
        for (m, payload) in queue.drain(..) {
            handle(m, payload, &mut wins, &writer, &dir, size);
        }
        let mut buf = vec![0u8; 1 << 16];
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => {
                println!("server closed the link");
                return 0;
            }
            Ok(n) => framer.push(&buf[..n]),
        }
        while let Some(m) = framer.next::<ToApp>() {
            match m {
                Ok(x) => queue.push(x),
                Err(e) => eprintln!("debug-app: {e}"),
            }
        }
    }
}

fn handle(m: ToApp, payload: Vec<u8>, wins: &mut HashMap<u32, Win>, writer: &Mutex<Conn>, dir: &std::path::Path, size: (u32, u32)) {
    match m {
        ToApp::Tiles { win, width, height, rects, .. } => {
            let w = wins.entry(win).or_insert(Win { width, height, frame: vec![], frames: 0 });
            if (w.width, w.height) != (width, height) || w.frame.len() != (width * height) as usize {
                w.width = width;
                w.height = height;
                w.frame = vec![0; (width * height) as usize];
            }
            if let Err(e) = lindroids_server::frames::unpack(&mut w.frame, width, &rects, &payload) {
                eprintln!("debug-app: bad tiles: {e}");
            }
            w.frames += 1;
            write_png(&dir.join(format!("window-{win}.png")), &w.frame, width, height);
            send(writer, &ToServer::FrameDone { win });
        }
        ToApp::WindowNew { win, title, app_id, icon_png_b64 } => {
            println!("window {win} {title:?} app_id={app_id:?} icon={}", icon_png_b64.is_some());
            let _ = std::io::stdout().flush();
            send(writer, &ToServer::Resize { win, width: size.0, height: size.1, density: 1.0 });
            send(writer, &ToServer::Focus { win, focused: true });
        }
        ToApp::AudioOpen { stream, rate, channels, format } => println!("audio {stream} open {rate}Hz {channels}ch {format:?}"),
        ToApp::AudioData { stream, .. } => {
            // Keep a running count per stream in a file, for tests to read.
            let p = dir.join(format!("audio-{stream}.bytes"));
            let n: u64 = std::fs::read_to_string(&p).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
            let _ = std::fs::write(&p, (n + payload.len() as u64).to_string());
            send(writer, &ToServer::AudioCredit { stream, bytes: payload.len() as u32 });
        }
        ToApp::AudioCork { stream, corked } => println!("audio {stream} {}", if corked { "corked" } else { "uncorked" }),
        ToApp::AudioClose { stream } => println!("audio {stream} closed"),
        other => {
            println!("{}", String::from_utf8_lossy(&proto::encode(&other)).trim_end());
            let _ = std::io::stdout().flush();
        }
    }
}

fn write_png(path: &std::path::Path, frame: &[u32], w: u32, h: u32) {
    let tmp = path.with_extension("png.tmp");
    if let Ok(f) = std::fs::File::create(&tmp) {
        let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        if let Ok(mut wr) = enc.write_header() {
            let bytes: Vec<u8> = frame.iter().flat_map(|p| p.to_le_bytes()).collect();
            let _ = wr.write_image_data(&bytes);
        }
    }
    let _ = std::fs::rename(&tmp, path);
}
