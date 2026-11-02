//! Installing the apps Lindroids builds with no taps: Termux's own `adb`, paired once with this
//! phone's Wireless debugging (`lindroids adb pair`).
//!
//! Android's installer always wants the user's tap, and Termux from Google Play may not even open
//! it (no REQUEST_INSTALL_PACKAGES), so an export ends in Download and the file manager. ADB is
//! the other way in: the shell user installs and opens apps by itself. Wireless debugging
//! (Android 11+) runs adbd on a random port of every address, 127.0.0.1 included, and pairs with a
//! six-digit code; Termux's `adb` (package android-tools) pairs with it like a computer would,
//! once. After that, while Wireless debugging is on, `export` installs the program's app in a
//! second and opens it. When it is off (it goes off with Wi-Fi, and after a restart), Lindroids
//! falls back to the installer as before and says how to turn it on.
//!
//! The ports are found on this phone only: mDNS (`_adb-tls-pairing._tcp`, `_adb-tls-connect._tcp`)
//! when Android answers it, else every port of 127.0.0.1, asked what it is: adbd answers an ADB
//! CNXN with STLS (Wireless debugging), AUTH (`adb tcpip`, a key to allow) or CNXN (no keys); the
//! pairing server is TLS, which answers it with an alert.

use std::io::{BufRead, IsTerminal, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::sandbox;

/// What answers on a port of this phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// adbd of Wireless debugging (TLS, paired keys).
    Tls,
    /// adbd over plain TCP (`adb tcpip 5555`): the phone asks to allow this key.
    Auth,
    /// adbd that lets anyone in (emulators, some test builds).
    Open,
    /// A TLS server that is not adbd: on Android, while its dialog is open, the pairing server.
    TlsOther,
}

fn adb_bin() -> PathBuf {
    PathBuf::from(format!("{}/bin/adb", lindroids_proto::TERMUX_PREFIX))
}

/// Termux's `adb` (android-tools), when it is installed.
pub fn have_adb() -> bool {
    adb_bin().is_file()
}

/// Lindroids' note that Termux was paired: `export` then tries adb first.
fn state() -> PathBuf {
    lindroids_server::state_dir().join("adb")
}

/// Whether `lindroids adb pair` was done here (and not `adb forget`).
pub fn paired() -> bool {
    state().is_file()
}

fn last_port() -> Option<u16> {
    std::fs::read_to_string(state())
        .ok()?
        .lines()
        .find_map(|l| l.strip_prefix("port ")?.trim().parse().ok())
}

fn remember(port: u16) {
    let _ = std::fs::write(state(), format!("paired\nport {port}\n"));
}

fn adb(args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut child = Command::new(adb_bin())
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("adb: {e}"))?;
    let t0 = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                let mut out = String::new();
                if let Some(mut o) = child.stdout.take() {
                    let _ = o.read_to_string(&mut out);
                }
                if let Some(mut e) = child.stderr.take() {
                    let _ = e.read_to_string(&mut out);
                }
                let out = out.trim().to_string();
                return if st.success() {
                    Ok(out)
                } else {
                    Err(if out.is_empty() {
                        format!("adb {} failed", args[0])
                    } else {
                        out
                    })
                };
            }
            Ok(None) if t0.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "adb {} did not answer in {} s",
                    args[0],
                    timeout.as_secs()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(30)),
            Err(e) => return Err(format!("adb: {e}")),
        }
    }
}

// ------------------------------------------------------------------ what answers on a port

const A_CNXN: u32 = 0x4e58_4e43;
const A_AUTH: u32 = 0x4854_5541;
const A_STLS: u32 = 0x534c_5453;

fn cnxn() -> Vec<u8> {
    let payload = b"host::lindroids\0";
    let mut p = Vec::with_capacity(24 + payload.len());
    for w in [
        A_CNXN,
        0x0100_0001,
        256 * 1024,
        payload.len() as u32,
        payload.iter().map(|&b| b as u32).sum(),
        A_CNXN ^ 0xffff_ffff,
    ] {
        p.extend_from_slice(&w.to_le_bytes());
    }
    p.extend_from_slice(payload);
    p
}

/// Asks 127.0.0.1:`port` what it is.
pub fn probe(port: u16) -> Option<Kind> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(300)).ok()?;
    s.set_read_timeout(Some(Duration::from_millis(700))).ok()?;
    s.write_all(&cnxn()).ok()?;
    let mut head = [0u8; 24];
    let mut got = 0;
    while got < head.len() {
        match s.read(&mut head[got..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => got += n,
        }
    }
    if got >= 2 && head[0] == 0x15 && head[1] == 0x03 {
        return Some(Kind::TlsOther);
    }
    if got < 24 {
        return None;
    }
    let cmd = u32::from_le_bytes(head[0..4].try_into().unwrap());
    let magic = u32::from_le_bytes(head[20..24].try_into().unwrap());
    if magic != cmd ^ 0xffff_ffff {
        return None;
    }
    match cmd {
        A_STLS => Some(Kind::Tls),
        A_AUTH => Some(Kind::Auth),
        A_CNXN => Some(Kind::Open),
        _ => None,
    }
}

/// The ports adbd may be on: `adb tcpip`'s 5555, and the kernel's range for port 0, where Wireless
/// debugging's servers are (32768–60999 unless the phone says otherwise).
fn candidate_ports() -> Vec<u16> {
    let range = std::fs::read_to_string("/proc/sys/net/ipv4/ip_local_port_range")
        .ok()
        .and_then(|s| {
            let mut it = s.split_whitespace().filter_map(|w| w.parse::<u16>().ok());
            Some((it.next()?, it.next()?))
        })
        .filter(|(a, b)| a < b)
        .unwrap_or((32768, 60999));
    let mut v = vec![5555];
    v.extend((range.0..=range.1).filter(|&p| p != 5555));
    v
}

/// Which of `ports` are open on 127.0.0.1 (in parallel: a closed one refuses at once).
fn open_ports(ports: Vec<u16>) -> Vec<u16> {
    let threads = 64;
    let chunks: Vec<Vec<u16>> = ports
        .chunks(ports.len().div_ceil(threads).max(1))
        .map(<[u16]>::to_vec)
        .collect();
    let handles: Vec<_> = chunks
        .into_iter()
        .map(|chunk| {
            std::thread::spawn(move || {
                chunk
                    .into_iter()
                    .filter(|&p| {
                        TcpStream::connect_timeout(
                            &SocketAddr::from((Ipv4Addr::LOCALHOST, p)),
                            Duration::from_millis(100),
                        )
                        .is_ok()
                    })
                    .collect::<Vec<u16>>()
            })
        })
        .collect();
    let mut v: Vec<u16> = handles
        .into_iter()
        .flat_map(|h| h.join().unwrap_or_default())
        .collect();
    v.sort_unstable();
    v
}

/// Every port of this phone that answers as one of `kinds`, asked in parallel.
fn scan(kinds: &[Kind]) -> Vec<(u16, Kind)> {
    let open: Vec<u16> = open_ports(candidate_ports())
        .into_iter()
        .filter(|&p| p != 5037)
        .collect();
    let handles: Vec<_> = open
        .into_iter()
        .map(|p| std::thread::spawn(move || probe(p).map(|k| (p, k))))
        .collect();
    handles
        .into_iter()
        .filter_map(|h| h.join().ok().flatten())
        .filter(|(_, k)| kinds.contains(k))
        .collect()
}

// ------------------------------------------------------------------ mDNS

/// Ports Android advertises for `service` (`_adb-tls-pairing._tcp.local`), asked over mDNS with a
/// unicast answer. Every phone on the Wi-Fi may answer: only ports that answer on 127.0.0.1 are
/// this phone's, and the callers check that.
fn mdns_ports(service: &str, wait: Duration) -> Vec<u16> {
    let Ok(sock) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) else {
        return Vec::new();
    };
    let _ = sock.set_multicast_loop_v4(true);
    let mut q = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    for label in service.trim_end_matches('.').split('.') {
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    // PTR, class IN with the unicast-response bit.
    q.extend_from_slice(&[0, 0, 0x00, 0x0c, 0x80, 0x01]);
    if sock
        .send_to(&q, (Ipv4Addr::new(224, 0, 0, 251), 5353))
        .is_err()
    {
        return Vec::new();
    }
    let _ = sock.set_read_timeout(Some(Duration::from_millis(200)));
    let t0 = Instant::now();
    let mut ports = Vec::new();
    let mut buf = [0u8; 4096];
    while t0.elapsed() < wait {
        if let Ok((n, _)) = sock.recv_from(&mut buf) {
            for p in srv_ports(&buf[..n], service) {
                if !ports.contains(&p) {
                    ports.push(p);
                }
            }
            if !ports.is_empty() {
                break;
            }
        }
    }
    ports
}

/// Reads a DNS name at `at`; returns it and where the record goes on.
fn dns_name(m: &[u8], mut at: usize) -> Option<(String, usize)> {
    let mut name = String::new();
    let mut end = None;
    for _ in 0..64 {
        let len = *m.get(at)? as usize;
        if len == 0 {
            return Some((name, end.unwrap_or(at + 1)));
        }
        if len & 0xc0 == 0xc0 {
            let ptr = ((len & 0x3f) << 8) | *m.get(at + 1)? as usize;
            end.get_or_insert(at + 2);
            at = ptr;
            continue;
        }
        let label = m.get(at + 1..at + 1 + len)?;
        if !name.is_empty() {
            name.push('.');
        }
        name.push_str(&String::from_utf8_lossy(label));
        at += 1 + len;
    }
    None
}

/// The SRV ports, in a DNS message, of instances of `service`.
fn srv_ports(m: &[u8], service: &str) -> Vec<u16> {
    let mut ports = Vec::new();
    if m.len() < 12 {
        return ports;
    }
    let count = |i: usize| u16::from_be_bytes([m[i], m[i + 1]]) as usize;
    let (qd, records) = (count(4), count(6) + count(8) + count(10));
    let mut at = 12;
    for _ in 0..qd {
        let Some((_, next)) = dns_name(m, at) else {
            return ports;
        };
        at = next + 4;
    }
    let want = service.trim_end_matches('.').to_ascii_lowercase();
    for _ in 0..records {
        let Some((name, next)) = dns_name(m, at) else {
            break;
        };
        let Some(h) = m.get(next..next + 10) else {
            break;
        };
        let (typ, len) = (
            u16::from_be_bytes([h[0], h[1]]),
            u16::from_be_bytes([h[8], h[9]]) as usize,
        );
        let data = next + 10;
        if typ == 33 && name.to_ascii_lowercase().ends_with(&want) {
            if let Some(p) = m.get(data + 4..data + 6) {
                ports.push(u16::from_be_bytes([p[0], p[1]]));
            }
        }
        at = data + len;
    }
    ports
}

// ------------------------------------------------------------------ connecting and installing

/// The adbd port to use now: the one used last, mDNS's, or a scan. None when Wireless debugging
/// (or `adb tcpip`) is off.
fn find_adbd() -> Option<(u16, Kind)> {
    let ok = |k: Kind| matches!(k, Kind::Tls | Kind::Auth | Kind::Open);
    if let Some(p) = last_port() {
        if let Some(k) = probe(p).filter(|&k| ok(k)) {
            return Some((p, k));
        }
    }
    for p in mdns_ports("_adb-tls-connect._tcp.local", Duration::from_millis(800)) {
        if let Some(k) = probe(p).filter(|&k| ok(k)) {
            return Some((p, k));
        }
    }
    let mut found = scan(&[Kind::Tls, Kind::Auth, Kind::Open]);
    // Wireless debugging before `adb tcpip`.
    found.sort_by_key(|&(p, k)| (k != Kind::Tls, p));
    found.into_iter().next()
}

/// Connects Termux's adb to this phone's adbd; the serial to use, checked to be the shell user.
fn connect() -> Result<String, String> {
    let Some((port, kind)) = find_adbd() else {
        return Err("Wireless debugging is off".into());
    };
    let serial = format!("127.0.0.1:{port}");
    let out = adb(&["connect", &serial], Duration::from_secs(15))?;
    if out.contains("failed") || out.contains("unable") || out.contains("cannot") {
        return Err(out);
    }
    let t0 = Instant::now();
    loop {
        let state = adb(&["-s", &serial, "get-state"], Duration::from_secs(5)).unwrap_or_default();
        if state == "device" {
            break;
        }
        if t0.elapsed() > Duration::from_secs(if kind == Kind::Auth { 60 } else { 8 }) {
            let _ = adb(&["disconnect", &serial], Duration::from_secs(5));
            return Err(match kind {
                Kind::Auth => {
                    "the phone did not allow Termux's key (\"Allow USB debugging?\")".into()
                }
                _ => format!(
                    "adbd on port {port} refused Termux: pair again with `lindroids adb pair`"
                ),
            });
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    let uid = adb(
        &["-s", &serial, "shell", "id", "-u"],
        Duration::from_secs(10),
    )?;
    if uid.trim() != "2000" && uid.trim() != "0" {
        return Err(format!(
            "adb on port {port} is not this phone's shell (uid {uid})"
        ));
    }
    remember(port);
    Ok(serial)
}

fn disconnect(serial: &str) {
    let _ = adb(&["disconnect", serial], Duration::from_secs(5));
}

/// Installs (or updates) `apk` through adb, with no taps. Err says why not, for the caller to fall
/// back to the installer.
pub fn install(apk: &Path) -> Result<(), String> {
    if !have_adb() {
        return Err("Termux's adb is not installed (pkg install android-tools)".into());
    }
    let serial = connect()?;
    let r = adb(
        &["-s", &serial, "install", "-r", &apk.display().to_string()],
        Duration::from_secs(120),
    );
    disconnect(&serial);
    match r {
        Ok(out) if out.lines().any(|l| l.trim() == "Success") => Ok(()),
        Ok(out) | Err(out) => Err(install_error(&out)),
    }
}

fn install_error(out: &str) -> String {
    let last = out
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or(out)
        .trim();
    if last.contains("INSTALL_FAILED_UPDATE_INCOMPATIBLE") {
        return "Android has this app signed by another key (made before Termux's data was cleared, or on another phone): uninstall it, then export again".into();
    }
    if last.contains("INSTALL_FAILED_VERIFICATION_FAILURE")
        || last.contains("INSTALL_FAILED_USER_RESTRICTED")
    {
        return format!("Android refused the install over adb ({last}): Developer options → \"Install via USB\" / \"Verify apps over USB\" may stop it");
    }
    last.to_string()
}

// ------------------------------------------------------------------ lindroids adb

fn confirm(question: &str) -> bool {
    if !std::io::stdin().is_terminal() {
        return false;
    }
    print!("{question} [Y/n] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line).is_err() {
        return false;
    }
    let a = line.trim().to_lowercase();
    a.is_empty() || a == "y" || a == "yes"
}

fn read_line(prompt: &str) -> Option<String> {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .ok()
        .filter(|&n| n > 0)?;
    Some(line.trim().to_string())
}

fn ensure_adb() -> Result<(), String> {
    if have_adb() {
        return Ok(());
    }
    println!("Termux's adb is not installed: it comes in the package android-tools (about 7 MB).");
    if !confirm("Install it now (pkg install android-tools)?") {
        return Err("android-tools is needed: pkg install android-tools".into());
    }
    let st = Command::new("pkg")
        .args(["install", "-y", "android-tools"])
        .status()
        .map_err(|e| format!("pkg: {e}"))?;
    if !st.success() || !have_adb() {
        return Err("pkg install android-tools failed".into());
    }
    Ok(())
}

/// The pairing server's port: mDNS's, else a TLS port of 127.0.0.1 that is not adbd, else asked.
fn pairing_port() -> Option<u16> {
    for p in mdns_ports("_adb-tls-pairing._tcp.local", Duration::from_millis(1500)) {
        if probe(p) == Some(Kind::TlsOther) {
            return Some(p);
        }
    }
    let found = scan(&[Kind::TlsOther]);
    if found.len() == 1 {
        return Some(found[0].0);
    }
    let shown = read_line(
        "The port of \"IP address & Port\" under the code (the number after the colon): ",
    )?;
    shown.rsplit(':').next()?.trim().parse().ok()
}

fn pair(args: &[String]) -> Result<(), String> {
    if !sandbox::on_android() {
        return Err("this is for Android: on a computer, use the SDK's adb".into());
    }
    ensure_adb()?;
    if let Ok(serial) = connect() {
        disconnect(&serial);
        println!(
            "Termux's adb reaches this phone already ({serial}): exports now install with no taps."
        );
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(state());
        return Ok(());
    }
    let mut code = args.first().cloned();
    let mut port = args.get(1).and_then(|p| p.rsplit(':').next()?.parse().ok());
    if code.is_none() {
        println!("Lindroids installs the apps it builds through Android's Wireless debugging, once Termux is paired");
        println!("with it, here and now (Android 11 or later, on Wi-Fi):");
        println!("  1. Developer options: Settings → About → Software information → tap Build number 7 times,");
        println!("     if they are not on yet.");
        println!(
            "  2. Developer options → Wireless debugging → On → Pair device with pairing code."
        );
        println!("  3. Keep that code on screen (Termux beside Settings, in split screen or a pop-up window) and");
        println!("     type it here. Leaving the code's dialog cancels it.");
        let _ = Command::new("am")
            .args([
                "start",
                "-a",
                "android.settings.APPLICATION_DEVELOPMENT_SETTINGS",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        code = read_line("Pairing code: ");
    }
    let code: String = code
        .ok_or("no pairing code")?
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect();
    if code.len() != 6 {
        return Err("the pairing code has 6 digits".into());
    }
    if port.is_none() {
        port = pairing_port();
    }
    let port = port.ok_or("no pairing port: Wireless debugging's pairing dialog is not open")?;
    let out = adb(
        &["pair", &format!("127.0.0.1:{port}"), &code],
        Duration::from_secs(30),
    )?;
    if !out.contains("Successfully paired") {
        return Err(format!("pairing failed: {out}"));
    }
    println!("Paired. Looking for Wireless debugging's own port…");
    let t0 = Instant::now();
    let serial = loop {
        match connect() {
            Ok(s) => break s,
            Err(e) if t0.elapsed() > Duration::from_secs(20) => return Err(e),
            Err(_) => std::thread::sleep(Duration::from_secs(1)),
        }
    };
    disconnect(&serial);
    println!("Termux's adb reaches this phone ({serial}). From now on `lindroids export` installs a program's");
    println!("app with no taps while Wireless debugging is on (it goes off with Wi-Fi and after a restart: its");
    println!("Quick Settings tile turns it on again). `lindroids adb forget` goes back to Android's installer.");
    Ok(())
}

fn status() {
    println!(
        "adb          {}",
        if have_adb() {
            "Termux's (android-tools)"
        } else {
            "not installed (lindroids adb pair offers it)"
        }
    );
    println!(
        "paired       {}",
        if paired() {
            "yes: exports install through adb first"
        } else {
            "no (lindroids adb pair)"
        }
    );
    if have_adb() {
        match find_adbd() {
            Some((p, Kind::Tls)) => println!("now          Wireless debugging on, port {p}"),
            Some((p, Kind::Auth)) => println!(
                "now          adb over TCP on port {p} (the phone asks to allow Termux's key once)"
            ),
            Some((p, _)) => println!("now          adbd on port {p}, open to all"),
            None => {
                println!("now          Wireless debugging is off: exports use Android's installer")
            }
        }
    }
}

pub fn cmd_adb(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        None | Some("status") => {
            status();
            0
        }
        Some("pair") => match pair(&args[1..]) {
            Ok(()) => {
                let _ = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(state());
                0
            }
            Err(e) => crate::fail(&e),
        },
        Some("forget") => {
            let _ = std::fs::remove_file(state());
            println!("Exports use Android's installer again. (The pairing stays in Developer options → Wireless debugging → Paired devices: forget it there.)");
            0
        }
        _ => crate::fail("usage: lindroids adb [status|pair [<code> [<port>]]|forget]"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cnxn_is_a_valid_header() {
        let p = cnxn();
        let w = |i: usize| u32::from_le_bytes(p[i..i + 4].try_into().unwrap());
        assert_eq!(w(0), A_CNXN);
        assert_eq!(w(20), A_CNXN ^ 0xffff_ffff);
        assert_eq!(w(12) as usize, p.len() - 24);
    }

    #[test]
    fn srv_port_from_an_mdns_answer() {
        // An answer as Android sends it: PTR to the instance, SRV with its port (compressed names).
        let mut m = vec![0, 0, 0x84, 0, 0, 0, 0, 2, 0, 0, 0, 0];
        let svc = 12;
        for l in ["_adb-tls-pairing", "_tcp", "local"] {
            m.push(l.len() as u8);
            m.extend_from_slice(l.as_bytes());
        }
        m.push(0);
        m.extend_from_slice(&[0, 12, 0, 1, 0, 0, 0, 120]);
        let inst = m.len() + 2;
        let rdata = [&[11u8][..], b"adb-X-abcde", &[0xc0, svc as u8]].concat();
        m.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
        m.extend_from_slice(&rdata);
        m.extend_from_slice(&[
            0xc0 | (inst >> 8) as u8,
            inst as u8,
            0,
            33,
            0x80,
            1,
            0,
            0,
            0,
            120,
            0,
            8,
        ]);
        m.extend_from_slice(&[0, 0, 0, 0, 0x91, 0x19, 0xc0, svc as u8]);
        assert_eq!(srv_ports(&m, "_adb-tls-pairing._tcp.local"), vec![37145]);
        assert!(srv_ports(&m, "_adb-tls-connect._tcp.local").is_empty());
    }

    #[test]
    fn nothing_listens_is_none() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let p = l.local_addr().unwrap().port();
        drop(l);
        assert_eq!(probe(p), None);
    }
}
