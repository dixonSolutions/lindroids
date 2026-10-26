//! Sound: a PulseAudio-protocol server for Linux programs, played through Android (AAudio, or
//! AudioTrack on Android 7).
//!
//! Programs find it at `<runtime dir>/pulse/native` (`PULSE_SERVER` points there) and talk the
//! native protocol to it through their own libpulse, which is what nearly every Linux program can
//! play through: directly, via GStreamer, SDL, or ALSA's pulse plugin. There is one sink, the
//! Android speaker. Each playback stream becomes one stream on the link and plays as its own
//! Android stream, so Android does the mixing and nothing is mixed here.
//!
//! Everything runs on one thread of its own with its own poll loop, so a busy compositor never
//! makes sound skip and sound never stalls the compositor. Data goes to the app through the
//! compositor's link (`Inbox::WithPayload`); the app's credits come back through [`credit`].
//!
//! Stream data travels in protocol packets on the socket. Shared memory and memfd transport are
//! refused at the handshake: libpulse then sends everything on the socket by itself, and nothing
//! here has to map a client's memory.

mod convert;
mod pace;

use std::collections::{BTreeMap, VecDeque};
use std::ffi::{CStr, CString};
use std::io::{Cursor, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use lindroids_proto::ToApp;
use log::{debug, info, warn};
use pulseaudio::protocol::{self as pa, props::PropsUpdateMode, Command, PulseError};

use crate::{shared, Inbox};
use convert::Converter;
use pace::Pacer;

/// How far ahead of real time the app is kept: enough to ride out a late wakeup on either side.
const LEAD_MS: u32 = 100;
/// Most a stream may have on its way to the app or queued there, by the app's credits.
const WINDOW_MS: u32 = 250;
/// Size of each AudioData message.
const CHUNK_MS: u32 = 20;
const TICK: Duration = Duration::from_millis(10);
/// A stream whose data the app has not acknowledged for this long is played into the void at
/// real time until credits come back or a new app links: clients must never stall forever.
const DEAF_AFTER: Duration = Duration::from_secs(1);
/// Defaults for buffer attributes the client leaves to the server. PulseAudio's own default of
/// two seconds is meant for a sound card's deep buffer; programs that care set their own.
const DEFAULT_TLENGTH_MS: u32 = 250;
const DEFAULT_MINREQ_MS: u32 = 20;
/// Largest packet accepted from a client, and most unsent output tolerated before giving up on
/// it (a client that never reads).
const MAX_PACKET: usize = 16 << 20;
const MAX_BACKLOG: usize = 8 << 20;

const SINK_INDEX: u32 = 0;
const SINK_NAME: &CStr = c"lindroids";
const SINK_DESCRIPTION: &CStr = c"Android speaker";
const MONITOR_INDEX: u32 = 0;
const MONITOR_NAME: &CStr = c"lindroids.monitor";
const MONITOR_DESCRIPTION: &CStr = c"Monitor of Android speaker";

// ------------------------------------------------------------------ cross-thread handoff

enum Event {
    Credit(u32, u32),
    LinkUp,
}

struct Wake {
    events: Mutex<Vec<Event>>,
    r: OwnedFd,
    w: OwnedFd,
}

static WAKE: OnceLock<Wake> = OnceLock::new();

fn post_event(e: Event) {
    // Nothing listens when sound is off; then there is nothing to queue either.
    let Some(w) = WAKE.get() else { return };
    w.events.lock().unwrap().push(e);
    unsafe { libc::write(w.w.as_raw_fd(), b"x".as_ptr() as *const _, 1) };
}

/// The app consumed `bytes` of a stream (called from the compositor thread).
pub fn credit(stream: u32, bytes: u32) {
    post_event(Event::Credit(stream, bytes));
}

/// A new app link is up: open streams are announced to it again.
pub fn link_up() {
    post_event(Event::LinkUp);
}

/// Listen on `<runtime_dir>/pulse/native` and run the sound thread.
pub fn start(runtime_dir: &Path) -> Result<(), String> {
    let dir = runtime_dir.join("pulse");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let _ = std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o700));
    let path = dir.join("native");
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let mut fds = [0; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
        return Err(format!("pipe: {}", std::io::Error::last_os_error()));
    }
    let wake = Wake {
        events: Mutex::new(Vec::new()),
        r: unsafe { OwnedFd::from_raw_fd(fds[0]) },
        w: unsafe { OwnedFd::from_raw_fd(fds[1]) },
    };
    if WAKE.set(wake).is_err() {
        return Err("sound is already running".into());
    }
    let mut server = Server::new(listener);
    std::thread::Builder::new().name("sound".into()).spawn(move || server.run()).map_err(|e| e.to_string())?;
    info!("sound on {}", path.display());
    Ok(())
}

// ------------------------------------------------------------------ streams

/// A playback stream: the client's buffer, and how far it has gone to the app.
struct Stream {
    id: u32,
    spec: pa::SampleSpec,
    map: pa::ChannelMap,
    attr: pa::stream::BufferAttr,
    props: pa::Props,
    conv: Converter,
    volume: pa::ChannelVolume,
    muted: bool,
    /// Client bytes not taken yet.
    queue: VecDeque<u8>,
    /// Bytes asked of the client that have not arrived.
    requested: u64,
    /// The client's write index, in bytes.
    written: u64,
    pacer: Pacer,
    corked: bool,
    /// Waiting for `prebuf` bytes before (re)starting.
    prebuffering: bool,
    /// Tell the client STARTED when the clock next starts.
    announce_start: bool,
    /// Underflow already reported for the current starvation.
    underflowed: bool,
    /// Seq of a drain request to answer when everything has played.
    drain: Option<u32>,
    /// Output bytes on their way to the app or queued there (sent minus credited).
    in_app: u64,
    /// When the app last showed it is consuming: a credit, or the first send into an empty
    /// window.
    progress: Instant,
    /// The app is not consuming: data is dropped at real time.
    deaf: bool,
}

impl Stream {
    fn frame(&self) -> usize {
        self.conv.in_frame()
    }

    fn open_msg(&self) -> ToApp {
        ToApp::AudioOpen {
            stream: self.id,
            rate: self.spec.sample_rate,
            channels: self.conv.out_channels() as u32,
            format: self.conv.out_format(),
        }
    }

    fn name(&self) -> CString {
        prop_string(&self.props, pa::Prop::MediaName).unwrap_or_else(|| c"Playback".to_owned())
    }

    fn gains(&self, sink: f32) -> Vec<f32> {
        let n = self.spec.channels as usize;
        if self.muted || sink == 0.0 {
            return vec![0.0; n];
        }
        let v = self.volume.channels();
        (0..n).map(|c| v.get(c).map_or(1.0, |v| v.to_linear()) * sink).collect()
    }

    /// Move data to the app as real time allows, report starvation and drains, and ask the
    /// client for more.
    fn pump(&mut self, now: Instant, sink_gain: f32, out: &mut Vec<u8>, version: u16) {
        let frame = self.frame();
        if !self.corked {
            if !self.pacer.running() {
                let have = self.queue.len();
                if self.prebuffering && (have as u64 >= self.attr.pre_buffering as u64 || (self.drain.is_some() && have >= frame)) {
                    self.prebuffering = false;
                }
                if !self.prebuffering && (have >= frame || self.pacer.buffered(now) > 0) {
                    self.pacer.start(now);
                    self.underflowed = false;
                    if self.announce_start {
                        self.announce_start = false;
                        send_command(out, version, &Command::Started(self.id));
                    }
                }
            }
            if self.pacer.running() {
                self.send_due(now, sink_gain);
                self.check_starved(now, out, version);
            }
            if self.deaf_now(now) {
                info!("sound stream {}: the app is not taking data, dropping it at real time", self.id);
                self.deaf = true;
                self.in_app = 0;
            }
        }
        if let Some(seq) = self.drain {
            if self.queue.len() < frame && self.pacer.buffered(now) == 0 && !self.corked {
                self.drain = None;
                let _ = pa::write_ack_message(out, seq);
            }
        }
        let have = self.queue.len() as u64 + self.requested;
        let missing = (self.attr.target_length as u64).saturating_sub(have);
        if missing > 0 && missing >= self.attr.minimum_request_length as u64 {
            self.requested += missing;
            send_command(out, version, &Command::Request(pa::Request { channel: self.id, length: missing as u32 }));
        }
    }

    fn send_due(&mut self, now: Instant, sink_gain: f32) {
        let frame = self.frame();
        let out_frame = self.conv.out_frame() as u64;
        let rate = self.spec.sample_rate as u64;
        let chunk = (rate * CHUNK_MS as u64 / 1000).max(1);
        let window = rate * WINDOW_MS as u64 / 1000 * out_frame;
        let lead = rate * LEAD_MS as u64 / 1000;
        let mut gains: Option<Vec<f32>> = None;
        loop {
            let avail = (self.queue.len() / frame) as u64;
            if avail == 0 {
                return;
            }
            let n = avail.min(chunk);
            // A short chunk only goes when it is all there is and the app is running low.
            if n < chunk && self.drain.is_none() && self.pacer.buffered(now) > lead / 2 {
                return;
            }
            if n > self.pacer.allowance(now) {
                return;
            }
            if !self.deaf && self.in_app + n * out_frame > window {
                // The app is behind real time: wait for it rather than burst later.
                self.pacer.resync(now);
                return;
            }
            let bytes: Vec<u8> = self.queue.drain(..n as usize * frame).collect();
            self.pacer.take(n);
            if self.deaf {
                continue;
            }
            let gains = gains.get_or_insert_with(|| self.gains(sink_gain));
            let mut pcm = Vec::with_capacity(n as usize * out_frame as usize);
            self.conv.convert(&bytes, gains, &mut pcm);
            if self.in_app == 0 {
                self.progress = now;
            }
            self.in_app += pcm.len() as u64;
            shared().post(Inbox::WithPayload(ToApp::AudioData { stream: self.id, len: pcm.len() as u32 }, pcm));
        }
    }

    fn check_starved(&mut self, now: Instant, out: &mut Vec<u8>, version: u16) {
        if self.queue.len() >= self.frame() || !self.pacer.starved(now) {
            return;
        }
        self.pacer.stop(now);
        if self.drain.is_some() || self.underflowed {
            return;
        }
        self.underflowed = true;
        debug!("sound stream {}: underflow", self.id);
        send_command(out, version, &Command::Underflow(pa::Underflow { channel: self.id, offset: self.written as i64 }));
        if self.attr.pre_buffering > 0 {
            self.prebuffering = true;
            self.announce_start = true;
        }
    }

    fn deaf_now(&self, now: Instant) -> bool {
        !self.deaf && self.in_app > 0 && now.saturating_duration_since(self.progress) > DEAF_AFTER
    }

    fn credit(&mut self, bytes: u32) {
        self.in_app = self.in_app.saturating_sub(bytes as u64);
        self.progress = Instant::now();
        if self.deaf {
            info!("sound stream {}: the app is taking data again", self.id);
            self.deaf = false;
        }
    }

    /// A new link: the app there has nothing of this stream yet.
    fn reannounce(&mut self) {
        self.in_app = 0;
        self.deaf = false;
        self.progress = Instant::now();
        shared().post(Inbox::ToApp(self.open_msg()));
        if self.corked {
            shared().post(Inbox::ToApp(ToApp::AudioCork { stream: self.id, corked: true }));
        }
    }

    fn write(&mut self, data: &[u8], out: &mut Vec<u8>, version: u16) {
        let room = (self.attr.max_length as usize).saturating_sub(self.queue.len());
        let take = data.len().min(room);
        if take < data.len() {
            send_command(out, version, &Command::Overflow((data.len() - take) as u32));
        }
        self.queue.extend(&data[..take]);
        self.written += data.len() as u64;
        self.requested = self.requested.saturating_sub(data.len() as u64);
    }

    fn flush(&mut self, now: Instant) {
        self.queue.clear();
        self.written = self.read_index();
        self.requested = 0;
        if self.attr.pre_buffering > 0 {
            self.pacer.stop(now);
            self.prebuffering = true;
            self.announce_start = true;
        }
    }

    fn read_index(&self) -> u64 {
        self.pacer.sent() * self.frame() as u64
    }

    fn latency(&self, now: Instant, client_time: SystemTime) -> pa::PlaybackLatency {
        pa::PlaybackLatency {
            sink_usec: self.pacer.frames_to_usec(self.pacer.buffered(now)),
            source_usec: 0,
            playing: self.pacer.running() && !self.corked,
            local_time: client_time,
            remote_time: SystemTime::now(),
            write_offset: self.written as i64,
            read_offset: self.read_index() as i64,
            underrun_for: 0,
            playing_for: 0,
        }
    }

    fn info(&self, client: u32, now: Instant) -> pa::SinkInputInfo {
        pa::SinkInputInfo {
            index: self.id,
            name: self.name(),
            owner_module_index: None,
            client_index: Some(client),
            sink_index: SINK_INDEX,
            sample_spec: self.spec,
            channel_map: self.map,
            cvolume: self.volume,
            buffer_latency: self.pacer.frames_to_usec((self.queue.len() / self.frame()) as u64),
            sink_latency: self.pacer.frames_to_usec(self.pacer.buffered(now)),
            resample_method: None,
            driver: Some(c"lindroids".to_owned()),
            props: self.props.clone(),
            muted: self.muted,
            corked: self.corked,
            has_volume: true,
            volume_writable: true,
            format: pa::FormatInfo::new(pa::FormatEncoding::Pcm),
        }
    }
}

/// Fill in buffer attributes the client left to the server, the way PulseAudio does.
fn fix_buffer_attr(a: &mut pa::stream::BufferAttr, spec: &pa::SampleSpec) {
    let frame = (spec.format.bytes_per_sample() * spec.channels as usize).max(1) as u32;
    let ms = |n: u32| (spec.sample_rate as u64 * n as u64 / 1000) as u32 * frame;
    let align = |v: u32| (v / frame).max(1) * frame;
    let max = pa::MAX_MEMBLOCKQ_LENGTH as u32;
    if a.max_length == u32::MAX || a.max_length > max {
        a.max_length = max;
    }
    a.max_length = align(a.max_length);
    if a.target_length == u32::MAX || a.target_length == 0 {
        a.target_length = ms(DEFAULT_TLENGTH_MS);
    }
    a.target_length = align(a.target_length.min(a.max_length));
    if a.minimum_request_length == u32::MAX || a.minimum_request_length == 0 {
        a.minimum_request_length = ms(DEFAULT_MINREQ_MS);
    }
    a.minimum_request_length = align(a.minimum_request_length.min(a.target_length / 2));
    let max_prebuf = a.target_length + frame - a.minimum_request_length;
    if a.pre_buffering == u32::MAX || a.pre_buffering > max_prebuf {
        a.pre_buffering = max_prebuf;
    }
    a.pre_buffering = a.pre_buffering / frame * frame;
    a.fragment_size = 0;
}

// ------------------------------------------------------------------ clients

struct Client {
    index: u32,
    sock: UnixStream,
    rbuf: Vec<u8>,
    wbuf: Vec<u8>,
    version: u16,
    authed: bool,
    props: pa::Props,
    mask: pa::SubscriptionMask,
    streams: BTreeMap<u32, Stream>,
    dead: bool,
}

impl Client {
    fn reply<R: pa::CommandReply>(&mut self, seq: u32, r: &R) {
        if let Err(e) = pa::write_reply_message(&mut self.wbuf, seq, r, self.version) {
            warn!("sound: cannot encode a reply: {e}");
            let _ = pa::write_error(&mut self.wbuf, seq, &PulseError::Internal);
        }
    }

    fn ack(&mut self, seq: u32) {
        let _ = pa::write_ack_message(&mut self.wbuf, seq);
    }

    fn error(&mut self, seq: u32, e: PulseError) {
        let _ = pa::write_error(&mut self.wbuf, seq, &e);
    }

    fn name(&self) -> String {
        prop_string(&self.props, pa::Prop::ApplicationName)
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("client {}", self.index))
    }

    /// Read what the socket has. False once the client has gone.
    fn fill(&mut self) -> bool {
        let mut buf = [0u8; 64 * 1024];
        for _ in 0..16 {
            match self.sock.read(&mut buf) {
                Ok(0) => return false,
                Ok(n) => self.rbuf.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return false,
            }
        }
        true
    }

    fn flush(&mut self) {
        while !self.wbuf.is_empty() {
            match self.sock.write(&self.wbuf) {
                Ok(0) => {
                    self.dead = true;
                    return;
                }
                Ok(n) => {
                    self.wbuf.drain(..n);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => {
                    self.dead = true;
                    return;
                }
            }
        }
        if self.wbuf.len() > MAX_BACKLOG {
            warn!("sound: {} does not read its socket, dropping it", self.name());
            self.dead = true;
        }
    }
}

fn send_command(out: &mut Vec<u8>, version: u16, cmd: &Command) {
    if let Err(e) = pa::write_command_message(out, u32::MAX, cmd, version) {
        warn!("sound: cannot encode {:?}: {e}", cmd.tag());
    }
}

fn prop_string(props: &pa::Props, p: pa::Prop) -> Option<CString> {
    let b = props.get(p)?;
    let s = CStr::from_bytes_until_nul(b).map(CStr::to_owned).ok().or_else(|| CString::new(b.to_vec()).ok())?;
    (!s.is_empty()).then_some(s)
}

fn update_props(dst: &mut pa::Props, mode: PropsUpdateMode, src: pa::Props) {
    match mode {
        PropsUpdateMode::Set => *dst = src,
        PropsUpdateMode::Merge => {
            for (k, v) in src.iter() {
                if dst.get_bytes(k).is_none() {
                    dst.set_bytes(k, v);
                }
            }
        }
        PropsUpdateMode::Replace => {
            for (k, v) in src.iter() {
                dst.set_bytes(k, v);
            }
        }
    }
}

/// The seq of a command packet, read straight from its first two tagstruct fields, to answer a
/// command that could not be parsed.
fn raw_seq(payload: &[u8]) -> u32 {
    if payload.len() >= 10 && payload[0] == b'L' && payload[5] == b'L' {
        u32::from_be_bytes([payload[6], payload[7], payload[8], payload[9]])
    } else {
        u32::MAX
    }
}

// ------------------------------------------------------------------ the server

struct Server {
    listener: UnixListener,
    clients: Vec<Client>,
    next_client: u32,
    next_stream: u32,
    sink_volume: pa::ChannelVolume,
    sink_muted: bool,
    cookie: u32,
    /// Subscription events to fan out once the current work is done.
    events: Vec<(pa::SubscriptionEventFacility, pa::SubscriptionEventType, u32)>,
}

impl Server {
    fn new(listener: UnixListener) -> Server {
        let mut cookie = [0u8; 4];
        let _ = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut cookie));
        Server {
            listener,
            clients: Vec::new(),
            next_client: 0,
            next_stream: 0,
            sink_volume: pa::ChannelVolume::norm(2),
            sink_muted: false,
            cookie: u32::from_ne_bytes(cookie),
            events: Vec::new(),
        }
    }

    fn run(&mut self) {
        let wake_r = WAKE.get().expect("start sets the wake pipe").r.as_raw_fd();
        let mut next_tick = Instant::now();
        loop {
            let mut fds: Vec<libc::pollfd> = Vec::with_capacity(2 + self.clients.len());
            fds.push(libc::pollfd { fd: wake_r, events: libc::POLLIN, revents: 0 });
            fds.push(libc::pollfd { fd: self.listener.as_raw_fd(), events: libc::POLLIN, revents: 0 });
            for c in &self.clients {
                let events = if c.wbuf.is_empty() { libc::POLLIN } else { libc::POLLIN | libc::POLLOUT };
                fds.push(libc::pollfd { fd: c.sock.as_raw_fd(), events, revents: 0 });
            }
            let busy = self.clients.iter().any(|c| c.streams.values().any(|s| !s.corked || s.drain.is_some()));
            let timeout = if busy { next_tick.saturating_duration_since(Instant::now()).as_millis() as i32 } else { 1000 };
            unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, timeout) };

            if fds[0].revents != 0 {
                let mut sink = [0u8; 64];
                while unsafe { libc::read(wake_r, sink.as_mut_ptr() as *mut _, 64) } > 0 {}
                let events = std::mem::take(&mut *WAKE.get().unwrap().events.lock().unwrap());
                for e in events {
                    self.event(e);
                }
            }
            if fds[1].revents != 0 {
                self.accept();
            }
            for i in 0..self.clients.len() {
                let Some(p) = fds.get(2 + i) else { break };
                if p.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
                    if !self.clients[i].fill() {
                        self.clients[i].dead = true;
                    }
                    self.packets(i);
                }
            }
            let now = Instant::now();
            if now >= next_tick || !busy {
                next_tick = now + TICK;
            }
            let sink_gain = self.sink_gain();
            for c in &mut self.clients {
                let version = c.version;
                for s in c.streams.values_mut() {
                    s.pump(now, sink_gain, &mut c.wbuf, version);
                }
            }
            self.fan_out();
            for c in &mut self.clients {
                c.flush();
            }
            let mut i = 0;
            while i < self.clients.len() {
                if self.clients[i].dead {
                    let c = self.clients.remove(i);
                    self.client_gone(c);
                } else {
                    i += 1;
                }
            }
            self.fan_out();
        }
    }

    fn sink_gain(&self) -> f32 {
        if self.sink_muted {
            return 0.0;
        }
        let v = self.sink_volume.channels();
        if v.is_empty() {
            return 1.0;
        }
        v.iter().map(|v| v.to_linear()).sum::<f32>() / v.len() as f32
    }

    fn event(&mut self, e: Event) {
        match e {
            Event::Credit(id, bytes) => {
                if let Some(s) = self.find_stream(id) {
                    s.credit(bytes);
                }
            }
            Event::LinkUp => {
                for s in self.clients.iter_mut().flat_map(|c| c.streams.values_mut()) {
                    s.reannounce();
                }
            }
        }
    }

    fn accept(&mut self) {
        while let Ok((sock, _)) = self.listener.accept() {
            if sock.set_nonblocking(true).is_err() {
                continue;
            }
            let index = self.next_client;
            self.next_client += 1;
            debug!("sound: client {index} connected");
            self.clients.push(Client {
                index,
                sock,
                rbuf: Vec::new(),
                wbuf: Vec::new(),
                version: pa::MAX_VERSION,
                authed: false,
                props: pa::Props::new(),
                mask: pa::SubscriptionMask::empty(),
                streams: BTreeMap::new(),
                dead: false,
            });
        }
    }

    fn client_gone(&mut self, c: Client) {
        if c.authed {
            info!("sound: {} left", c.name());
            self.events.push((pa::SubscriptionEventFacility::Client, pa::SubscriptionEventType::Removed, c.index));
        }
        for s in c.streams.values() {
            shared().post(Inbox::ToApp(ToApp::AudioClose { stream: s.id }));
            self.events.push((pa::SubscriptionEventFacility::SinkInput, pa::SubscriptionEventType::Removed, s.id));
        }
    }

    fn fan_out(&mut self) {
        use pa::SubscriptionEventFacility as F;
        for (facility, kind, index) in self.events.drain(..) {
            let bit = match facility {
                F::Sink => pa::SubscriptionMask::SINK,
                F::Source => pa::SubscriptionMask::SOURCE,
                F::SinkInput => pa::SubscriptionMask::SINK_INPUT,
                F::Client => pa::SubscriptionMask::CLIENT,
                F::Server => pa::SubscriptionMask::SERVER,
                _ => continue,
            };
            let ev = Command::SubscribeEvent(pa::SubscriptionEvent { event_facility: facility, event_type: kind, index: Some(index) });
            for c in self.clients.iter_mut().filter(|c| c.mask.contains(bit)) {
                send_command(&mut c.wbuf, c.version, &ev);
            }
        }
    }

    /// Handle every complete packet a client has sent.
    fn packets(&mut self, ci: usize) {
        let mut pos = 0;
        loop {
            let c = &mut self.clients[ci];
            let rest = &c.rbuf[pos..];
            if rest.len() < pa::DESCRIPTOR_SIZE {
                break;
            }
            let Ok(desc) = pa::read_descriptor(&mut Cursor::new(&rest[..pa::DESCRIPTOR_SIZE])) else {
                c.dead = true;
                break;
            };
            let len = desc.length as usize;
            if len > MAX_PACKET {
                warn!("sound: {} sent a {len}-byte packet, dropping it", c.name());
                c.dead = true;
                break;
            }
            if rest.len() < pa::DESCRIPTOR_SIZE + len {
                break;
            }
            let payload = rest[pa::DESCRIPTOR_SIZE..pa::DESCRIPTOR_SIZE + len].to_vec();
            pos += pa::DESCRIPTOR_SIZE + len;
            if desc.channel == u32::MAX {
                let version = c.version;
                match Command::read_tag_prefixed(&mut Cursor::new(&payload[..]), version) {
                    Ok((seq, cmd)) => self.command(ci, seq, cmd),
                    Err(pa::ProtocolError::Unimplemented(seq, tag)) => {
                        debug!("sound: unsupported command {tag:?}");
                        self.clients[ci].error(seq, PulseError::NotImplemented);
                    }
                    Err(e) => {
                        warn!("sound: bad command from {}: {e}", self.clients[ci].name());
                        self.clients[ci].error(raw_seq(&payload), PulseError::Protocol);
                    }
                }
            } else if desc.flags.bits() & 0xC000_0000 != 0 {
                // Shared-memory release/revoke: never negotiated, nothing to do.
            } else if let Some(s) = c.streams.get_mut(&desc.channel) {
                let version = c.version;
                s.write(&payload, &mut c.wbuf, version);
            } else {
                debug!("sound: data for unknown stream {}", desc.channel);
            }
            if self.clients[ci].dead {
                break;
            }
        }
        self.clients[ci].rbuf.drain(..pos);
    }

    fn find_stream(&mut self, id: u32) -> Option<&mut Stream> {
        self.clients.iter_mut().find_map(|c| c.streams.get_mut(&id))
    }

    fn command(&mut self, ci: usize, seq: u32, cmd: Command) {
        use pa::SubscriptionEventFacility as F;
        use pa::SubscriptionEventType as T;
        debug!("sound: client {} {:?}", self.clients[ci].index, cmd.tag());
        let now = Instant::now();
        let c = &mut self.clients[ci];
        if !c.authed {
            match cmd {
                Command::Auth(p) => {
                    if p.version < pa::MIN_VERSION {
                        c.error(seq, PulseError::Version);
                        c.flush();
                        c.dead = true;
                        return;
                    }
                    c.version = p.version.min(pa::MAX_VERSION);
                    c.authed = true;
                    // No shm, no memfd: data comes on the socket.
                    let reply = pa::AuthReply { version: c.version, ..Default::default() };
                    c.reply(seq, &reply);
                }
                _ => c.error(seq, PulseError::AccessDenied),
            }
            return;
        }
        match cmd {
            Command::Auth(_) => c.error(seq, PulseError::BadState),
            Command::SetClientName(props) => {
                let first = c.props.iter().next().is_none();
                c.props = props;
                let index = c.index;
                info!("sound: {} connected", c.name());
                c.reply(seq, &pa::SetClientNameReply { client_id: index });
                self.events.push((F::Client, if first { T::New } else { T::Changed }, index));
            }
            Command::UpdateClientProplist(p) => {
                update_props(&mut c.props, p.mode, p.props);
                c.ack(seq);
                self.events.push((F::Client, T::Changed, c.index));
            }
            Command::RemoveClientProplist => c.ack(seq),
            Command::GetServerInfo => {
                let info = self.server_info();
                self.clients[ci].reply(seq, &info);
            }
            Command::Stat => c.reply(seq, &pa::StatInfo::default()),
            Command::GetSinkInfo(q) => {
                if is_sink(q.index, q.name.as_deref()) {
                    let info = self.sink_info();
                    self.clients[ci].reply(seq, &info);
                } else {
                    c.error(seq, PulseError::NoEntity);
                }
            }
            Command::GetSinkInfoList => {
                let info = vec![self.sink_info()];
                self.clients[ci].reply(seq, &info);
            }
            Command::GetSourceInfo(q) => {
                if is_source(q.index, q.name.as_deref()) {
                    let info = monitor_info();
                    c.reply(seq, &info);
                } else {
                    c.error(seq, PulseError::NoEntity);
                }
            }
            Command::GetSourceInfoList => c.reply(seq, &vec![monitor_info()]),
            Command::LookupSink(name) => {
                if is_sink(None, Some(&name)) {
                    c.reply(seq, &pa::LookupReply(SINK_INDEX));
                } else {
                    c.error(seq, PulseError::NoEntity);
                }
            }
            Command::LookupSource(name) => {
                if is_source(None, Some(&name)) {
                    c.reply(seq, &pa::LookupReply(MONITOR_INDEX));
                } else {
                    c.error(seq, PulseError::NoEntity);
                }
            }
            Command::GetClientInfo(index) => match self.clients.iter().find(|c| c.authed && c.index == index).map(client_info) {
                Some(info) => self.clients[ci].reply(seq, &info),
                None => self.clients[ci].error(seq, PulseError::NoEntity),
            },
            Command::GetClientInfoList => {
                let list: Vec<pa::ClientInfo> = self.clients.iter().filter(|c| c.authed).map(client_info).collect();
                self.clients[ci].reply(seq, &list);
            }
            Command::GetSinkInputInfo(index) => {
                let info = self.clients.iter().find_map(|c| c.streams.get(&index).map(|s| s.info(c.index, now)));
                match info {
                    Some(info) => self.clients[ci].reply(seq, &info),
                    None => self.clients[ci].error(seq, PulseError::NoEntity),
                }
            }
            Command::GetSinkInputInfoList => {
                let list: Vec<pa::SinkInputInfo> =
                    self.clients.iter().flat_map(|c| c.streams.values().map(|s| s.info(c.index, now))).collect();
                self.clients[ci].reply(seq, &list);
            }
            Command::GetModuleInfoList => c.reply(seq, &Vec::<pa::ModuleInfo>::new()),
            Command::GetCardInfoList => c.reply(seq, &Vec::<pa::CardInfo>::new()),
            Command::GetSampleInfoList => c.reply(seq, &Vec::<pa::SampleInfo>::new()),
            Command::GetSourceOutputInfoList => c.reply(seq, &Vec::<pa::SourceOutputInfo>::new()),
            Command::GetModuleInfo(_) | Command::GetCardInfo(_) | Command::GetSampleInfo(_) | Command::GetSourceOutputInfo(_) => {
                c.error(seq, PulseError::NoEntity)
            }
            Command::Subscribe(mask) => {
                c.mask = mask;
                c.ack(seq);
            }
            Command::SetSinkVolume(p) => {
                if is_sink(p.device_index, p.device_name.as_deref()) {
                    self.sink_volume = p.volume;
                    c.ack(seq);
                    self.events.push((F::Sink, T::Changed, SINK_INDEX));
                } else {
                    c.error(seq, PulseError::NoEntity);
                }
            }
            Command::SetSinkMute(p) => {
                if is_sink(p.device_index, p.device_name.as_deref()) {
                    self.sink_muted = p.mute;
                    c.ack(seq);
                    self.events.push((F::Sink, T::Changed, SINK_INDEX));
                } else {
                    c.error(seq, PulseError::NoEntity);
                }
            }
            Command::SetSinkInputVolume(p) => match self.find_stream(p.index) {
                Some(s) => {
                    s.volume = p.volume;
                    self.clients[ci].ack(seq);
                    self.events.push((F::SinkInput, T::Changed, p.index));
                }
                None => self.clients[ci].error(seq, PulseError::NoEntity),
            },
            Command::SetSinkInputMute(p) => match self.find_stream(p.index) {
                Some(s) => {
                    s.muted = p.mute;
                    self.clients[ci].ack(seq);
                    self.events.push((F::SinkInput, T::Changed, p.index));
                }
                None => self.clients[ci].error(seq, PulseError::NoEntity),
            },
            Command::CreatePlaybackStream(p) => self.create_stream(ci, seq, p),
            Command::DeletePlaybackStream(id) => match c.streams.remove(&id) {
                Some(s) => {
                    debug!("sound stream {}: closed", s.id);
                    shared().post(Inbox::ToApp(ToApp::AudioClose { stream: s.id }));
                    c.ack(seq);
                    self.events.push((F::SinkInput, T::Removed, id));
                }
                None => c.error(seq, PulseError::NoEntity),
            },
            Command::CreateRecordStream(_) => {
                info!("sound: {} wants to record; there is no microphone", c.name());
                c.error(seq, PulseError::NotSupported);
            }
            Command::CreateUploadStream(_) => c.error(seq, PulseError::NotSupported),
            Command::PlaySample(_) | Command::RemoveSample(_) => c.error(seq, PulseError::NoEntity),
            Command::DrainPlaybackStream(id) => match c.streams.get_mut(&id) {
                Some(s) => {
                    // A second drain waits for the same data as the first: both finish together.
                    if let Some(old) = s.drain.replace(seq) {
                        let _ = pa::write_ack_message(&mut c.wbuf, old);
                    }
                }
                None => c.error(seq, PulseError::NoEntity),
            },
            Command::CorkPlaybackStream(p) => match c.streams.get_mut(&p.channel) {
                Some(s) => {
                    if s.corked != p.cork {
                        s.corked = p.cork;
                        if p.cork {
                            s.pacer.stop(now);
                        } else {
                            s.progress = now;
                        }
                        shared().post(Inbox::ToApp(ToApp::AudioCork { stream: s.id, corked: p.cork }));
                        self.events.push((F::SinkInput, T::Changed, p.channel));
                    }
                    c.ack(seq);
                }
                None => c.error(seq, PulseError::NoEntity),
            },
            Command::FlushPlaybackStream(id) => match c.streams.get_mut(&id) {
                Some(s) => {
                    s.flush(now);
                    c.ack(seq);
                }
                None => c.error(seq, PulseError::NoEntity),
            },
            Command::PrebufPlaybackStream(id) => match c.streams.get_mut(&id) {
                Some(s) => {
                    if s.attr.pre_buffering > 0 {
                        s.pacer.stop(now);
                        s.prebuffering = true;
                        s.announce_start = true;
                    }
                    c.ack(seq);
                }
                None => c.error(seq, PulseError::NoEntity),
            },
            Command::TriggerPlaybackStream(id) => match c.streams.get_mut(&id) {
                Some(s) => {
                    s.prebuffering = false;
                    c.ack(seq);
                }
                None => c.error(seq, PulseError::NoEntity),
            },
            Command::GetPlaybackLatency(p) => match c.streams.get(&p.channel) {
                Some(s) => {
                    let l = s.latency(now, p.now);
                    c.reply(seq, &l);
                }
                None => c.error(seq, PulseError::NoEntity),
            },
            Command::SetPlaybackStreamBufferAttr(p) => match c.streams.get_mut(&p.index) {
                Some(s) => {
                    let mut attr = p.buffer_attr;
                    fix_buffer_attr(&mut attr, &s.spec);
                    s.attr = attr;
                    let reply = pa::SetPlaybackStreamBufferAttrReply { buffer_attr: attr, configured_sink_latency: LEAD_MS as u64 * 1000 };
                    c.reply(seq, &reply);
                }
                None => c.error(seq, PulseError::NoEntity),
            },
            Command::UpdatePlaybackStreamSampleRate(p) => match c.streams.get_mut(&p.index) {
                Some(s) if p.sample_rate > 0 && p.sample_rate <= pa::sample_spec::MAX_RATE => {
                    // The app plays a stream at one rate: reopen it at the new one.
                    s.spec.sample_rate = p.sample_rate;
                    s.pacer.set_rate(now, p.sample_rate);
                    s.in_app = 0;
                    shared().post(Inbox::ToApp(ToApp::AudioClose { stream: s.id }));
                    shared().post(Inbox::ToApp(s.open_msg()));
                    if s.corked {
                        shared().post(Inbox::ToApp(ToApp::AudioCork { stream: s.id, corked: true }));
                    }
                    c.ack(seq);
                }
                Some(_) => c.error(seq, PulseError::Invalid),
                None => c.error(seq, PulseError::NoEntity),
            },
            Command::UpdatePlaybackStreamProplist(p) => match c.streams.get_mut(&p.index) {
                Some(s) => {
                    update_props(&mut s.props, p.mode, p.props);
                    c.ack(seq);
                    self.events.push((F::SinkInput, T::Changed, p.index));
                }
                None => c.error(seq, PulseError::NoEntity),
            },
            Command::SetPlaybackStreamName(p) => match c.streams.get_mut(&p.index) {
                Some(s) => {
                    s.props.set(pa::Prop::MediaName, p.name);
                    c.ack(seq);
                    self.events.push((F::SinkInput, T::Changed, p.index));
                }
                None => c.error(seq, PulseError::NoEntity),
            },
            Command::RemovePlaybackStreamProplist(_) => c.ack(seq),
            Command::KillSinkInput(id) => match self.clients.iter().position(|c| c.streams.contains_key(&id)) {
                Some(o) => {
                    let owner = &mut self.clients[o];
                    owner.streams.remove(&id);
                    let v = owner.version;
                    send_command(&mut owner.wbuf, v, &Command::PlaybackStreamKilled(id));
                    shared().post(Inbox::ToApp(ToApp::AudioClose { stream: id }));
                    self.clients[ci].ack(seq);
                    self.events.push((F::SinkInput, T::Removed, id));
                }
                None => self.clients[ci].error(seq, PulseError::NoEntity),
            },
            Command::KillClient(index) => match self.clients.iter_mut().find(|c| c.index == index) {
                Some(k) => {
                    k.dead = true;
                    self.clients[ci].ack(seq);
                }
                None => self.clients[ci].error(seq, PulseError::NoEntity),
            },
            Command::MoveSinkInput(p) => {
                if is_sink(p.device_index, p.device_name.as_deref()) {
                    c.ack(seq);
                } else {
                    c.error(seq, PulseError::NoEntity);
                }
            }
            // One sink, always on: choosing or suspending it changes nothing.
            Command::SetDefaultSink(_) | Command::SetDefaultSource(_) | Command::SuspendSink(_) | Command::SuspendSource(_) => {
                c.ack(seq)
            }
            Command::Extension(_) => c.error(seq, PulseError::NoExtension),
            Command::LoadModule(_) | Command::UnloadModule(_) => c.error(seq, PulseError::NotSupported),
            Command::Exit => c.error(seq, PulseError::AccessDenied),
            other => {
                debug!("sound: {:?} is not supported", other.tag());
                c.error(seq, PulseError::NotImplemented);
            }
        }
    }

    fn create_stream(&mut self, ci: usize, seq: u32, p: pa::PlaybackStreamParams) {
        use pa::SubscriptionEventFacility as F;
        use pa::SubscriptionEventType as T;
        let spec = p.sample_spec;
        let c = &mut self.clients[ci];
        if spec.sample_rate == 0 || spec.sample_rate > pa::sample_spec::MAX_RATE || spec.channels == 0 || spec.channels > pa::sample_spec::MAX_CHANNELS {
            c.error(seq, PulseError::Invalid);
            return;
        }
        if p.sink_index.is_some_and(|i| i != SINK_INDEX) || p.sink_name.as_deref().is_some_and(|n| !is_sink(None, Some(n))) {
            c.error(seq, PulseError::NoEntity);
            return;
        }
        let map = if p.channel_map.num_channels() == spec.channels { p.channel_map } else { default_map(spec.channels) };
        let Some(conv) = Converter::new(&spec, &map) else {
            c.error(seq, PulseError::NotSupported);
            return;
        };
        let mut attr = p.buffer_attr;
        fix_buffer_attr(&mut attr, &spec);
        let volume = match p.cvolume {
            Some(v) if v.channels().len() == spec.channels as usize => v,
            _ => pa::ChannelVolume::norm(spec.channels),
        };
        let id = self.next_stream;
        self.next_stream += 1;
        let mut props = p.props;
        if prop_string(&props, pa::Prop::ApplicationName).is_none() {
            if let Some(app) = c.props.get(pa::Prop::ApplicationName) {
                props.set_bytes(pa::Prop::ApplicationName.to_c_str(), app);
            }
        }
        let s = Stream {
            id,
            spec,
            map,
            attr,
            props,
            conv,
            volume,
            muted: p.flags.start_muted.unwrap_or(false),
            queue: VecDeque::new(),
            requested: attr.target_length as u64,
            written: 0,
            pacer: Pacer::new(spec.sample_rate, LEAD_MS),
            corked: p.flags.start_corked,
            prebuffering: attr.pre_buffering > 0,
            announce_start: true,
            underflowed: false,
            drain: None,
            in_app: 0,
            progress: Instant::now(),
            deaf: false,
        };
        info!(
            "sound stream {id}: {} plays {:?} {} Hz {} ch (tlength {} prebuf {} minreq {}){}",
            c.name(),
            spec.format,
            spec.sample_rate,
            spec.channels,
            attr.target_length,
            attr.pre_buffering,
            attr.minimum_request_length,
            if s.corked { ", corked" } else { "" }
        );
        shared().post(Inbox::ToApp(s.open_msg()));
        if s.corked {
            shared().post(Inbox::ToApp(ToApp::AudioCork { stream: id, corked: true }));
        }
        let reply = pa::CreatePlaybackStreamReply {
            channel: id,
            stream_index: id,
            requested_bytes: attr.target_length,
            buffer_attr: attr,
            sample_spec: spec,
            channel_map: map,
            stream_latency: LEAD_MS as u64 * 1000,
            sink_index: SINK_INDEX,
            sink_name: Some(SINK_NAME.to_owned()),
            suspended: false,
            format: pa::FormatInfo::new(pa::FormatEncoding::Pcm),
        };
        c.streams.insert(id, s);
        c.reply(seq, &reply);
        self.events.push((F::SinkInput, T::New, id));
    }

    fn server_info(&self) -> pa::ServerInfo {
        let user = std::env::var("USER").ok().filter(|u| !u.is_empty()).unwrap_or_else(|| "lindroids".into());
        pa::ServerInfo {
            server_name: Some(c"pulseaudio".to_owned()),
            server_version: Some(c"17.0 (lindroids)".to_owned()),
            user_name: CString::new(user).ok(),
            host_name: Some(hostname()),
            sample_spec: sink_spec(),
            cookie: self.cookie,
            default_sink_name: Some(SINK_NAME.to_owned()),
            default_source_name: Some(MONITOR_NAME.to_owned()),
            channel_map: pa::ChannelMap::stereo(),
        }
    }

    fn sink_info(&self) -> pa::SinkInfo {
        let running = self.clients.iter().any(|c| c.streams.values().any(|s| s.pacer.running()));
        let mut props = pa::Props::new();
        props.set(pa::Prop::DeviceDescription, SINK_DESCRIPTION);
        props.set_bytes(c"device.class", c"sound".to_bytes_with_nul());
        props.set_bytes(c"device.api", c"lindroids".to_bytes_with_nul());
        props.set_bytes(c"device.icon_name", c"audio-speakers".to_bytes_with_nul());
        let mut ports = pa::SinkInfo::new_dummy(SINK_INDEX).ports;
        if let Some(p) = ports.first_mut() {
            p.name = c"speaker".to_owned();
            p.description = Some(c"Speaker".to_owned());
        }
        pa::SinkInfo {
            index: SINK_INDEX,
            name: SINK_NAME.to_owned(),
            description: Some(SINK_DESCRIPTION.to_owned()),
            props,
            state: if running { pa::SinkState::Running } else { pa::SinkState::Idle },
            sample_spec: sink_spec(),
            channel_map: pa::ChannelMap::stereo(),
            owner_module_index: None,
            cvolume: self.sink_volume,
            muted: self.sink_muted,
            monitor_source_index: Some(MONITOR_INDEX),
            monitor_source_name: Some(MONITOR_NAME.to_owned()),
            flags: pa::SinkFlags::DECIBEL_VOLUME | pa::SinkFlags::LATENCY,
            actual_latency: LEAD_MS as u64 * 1000,
            configured_latency: LEAD_MS as u64 * 1000,
            driver: Some(c"lindroids".to_owned()),
            base_volume: pa::Volume::NORM,
            volume_steps: None,
            card_index: None,
            ports,
            active_port: 0,
            formats: vec![pa::FormatInfo::new(pa::FormatEncoding::Pcm)],
        }
    }
}

/// A monitor source, so programs that look for a default source find one. Recording from it is
/// refused.
fn monitor_info() -> pa::SourceInfo {
    let mut props = pa::Props::new();
    props.set(pa::Prop::DeviceDescription, MONITOR_DESCRIPTION);
    props.set_bytes(c"device.class", c"monitor".to_bytes_with_nul());
    pa::SourceInfo {
        index: MONITOR_INDEX,
        name: MONITOR_NAME.to_owned(),
        description: Some(MONITOR_DESCRIPTION.to_owned()),
        props,
        state: pa::SourceState::Suspended,
        sample_spec: sink_spec(),
        channel_map: pa::ChannelMap::stereo(),
        owner_module_index: None,
        cvolume: pa::ChannelVolume::norm(2),
        base_volume: pa::Volume::NORM,
        volume_steps: None,
        muted: false,
        monitor_of_sink_index: Some(SINK_INDEX),
        monitor_of_sink_name: Some(SINK_NAME.to_owned()),
        flags: pa::SourceFlags::DECIBEL_VOLUME | pa::SourceFlags::LATENCY,
        actual_latency: 0,
        configured_latency: 0,
        driver: Some(c"lindroids".to_owned()),
        card_index: None,
        ports: Vec::new(),
        active_port: 0,
        formats: vec![pa::FormatInfo::new(pa::FormatEncoding::Pcm)],
    }
}

fn client_info(c: &Client) -> pa::ClientInfo {
    pa::ClientInfo {
        index: c.index,
        name: CString::new(c.name()).unwrap_or_default(),
        owner_module_index: None,
        driver: Some(c"lindroids".to_owned()),
        props: c.props.clone(),
    }
}

fn is_sink(index: Option<u32>, name: Option<&CStr>) -> bool {
    match (index, name) {
        (Some(i), _) => i == SINK_INDEX,
        (None, Some(n)) => n == SINK_NAME || n == pa::DEFAULT_SINK || n.is_empty(),
        (None, None) => true,
    }
}

fn is_source(index: Option<u32>, name: Option<&CStr>) -> bool {
    match (index, name) {
        (Some(i), _) => i == MONITOR_INDEX,
        (None, Some(n)) => n == MONITOR_NAME || n == pa::DEFAULT_SOURCE || n.is_empty(),
        (None, None) => true,
    }
}

fn sink_spec() -> pa::SampleSpec {
    pa::SampleSpec { format: pa::SampleFormat::S16Le, channels: 2, sample_rate: 48000 }
}

/// PulseAudio's default layouts (ALSA order) for a channel count the client gave no map for.
fn default_map(channels: u8) -> pa::ChannelMap {
    use pa::ChannelPosition::*;
    let surround = [FrontLeft, FrontRight, RearLeft, RearRight, FrontCenter, Lfe, SideLeft, SideRight];
    match channels {
        1 => pa::ChannelMap::mono(),
        2 => pa::ChannelMap::stereo(),
        n @ 3..=8 => pa::ChannelMap::new(surround.into_iter().take(n as usize)),
        n => pa::ChannelMap::new((0..n).map(|i| if i % 2 == 0 { FrontLeft } else { FrontRight })),
    }
}

fn hostname() -> CString {
    let mut buf = [0u8; 256];
    if unsafe { libc::gethostname(buf.as_mut_ptr() as *mut _, buf.len()) } == 0 {
        if let Ok(s) = CStr::from_bytes_until_nul(&buf) {
            if !s.is_empty() {
                return s.to_owned();
            }
        }
    }
    c"localhost".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(rate: u32, channels: u8) -> pa::SampleSpec {
        pa::SampleSpec { format: pa::SampleFormat::S16Le, channels, sample_rate: rate }
    }

    #[test]
    fn buffer_defaults_are_filled_and_frame_aligned() {
        let mut a = pa::stream::BufferAttr::default();
        fix_buffer_attr(&mut a, &spec(48000, 2));
        assert_eq!(a.target_length, 48000, "250 ms");
        assert_eq!(a.minimum_request_length, 3840, "20 ms");
        assert_eq!(a.pre_buffering, a.target_length + 4 - a.minimum_request_length);
        assert_eq!(a.max_length, pa::MAX_MEMBLOCKQ_LENGTH as u32);

        let mut b = pa::stream::BufferAttr { max_length: 1001, target_length: 5003, pre_buffering: 0, minimum_request_length: 7, fragment_size: 0 };
        fix_buffer_attr(&mut b, &spec(44100, 1));
        assert_eq!((b.max_length, b.target_length, b.pre_buffering, b.minimum_request_length), (1000, 1000, 0, 6));
    }

    /// 1000 Hz mono s16: two bytes a millisecond, 20-frame chunks, 100-frame lead.
    fn stream(prebuf: u32) -> Stream {
        let s = spec(1000, 1);
        let mut attr = pa::stream::BufferAttr { max_length: u32::MAX, target_length: 400, pre_buffering: prebuf, minimum_request_length: 40, fragment_size: 0 };
        fix_buffer_attr(&mut attr, &s);
        Stream {
            id: 9,
            spec: s,
            map: pa::ChannelMap::mono(),
            attr,
            props: pa::Props::new(),
            conv: Converter::new(&s, &pa::ChannelMap::mono()).unwrap(),
            volume: pa::ChannelVolume::norm(1),
            muted: false,
            queue: VecDeque::new(),
            requested: 400,
            written: 0,
            pacer: Pacer::new(1000, LEAD_MS),
            corked: false,
            prebuffering: attr.pre_buffering > 0,
            announce_start: true,
            underflowed: false,
            drain: None,
            in_app: 0,
            progress: Instant::now(),
            // Keeps the tests off the app link.
            deaf: true,
        }
    }

    fn commands(buf: &[u8]) -> Vec<Command> {
        let mut r = Cursor::new(buf);
        let mut v = Vec::new();
        while (r.position() as usize) < buf.len() {
            v.push(pa::read_command_message(&mut r, pa::MAX_VERSION).unwrap().1);
        }
        v
    }

    #[test]
    fn prebuffers_then_plays_at_real_time_and_drains() {
        let t0 = Instant::now();
        let ms = Duration::from_millis(1);
        let v = pa::MAX_VERSION;
        let mut s = stream(u32::MAX);
        let mut out = Vec::new();
        s.write(&[0u8; 200], &mut out, v);
        s.pump(t0, 1.0, &mut out, v);
        assert!(!s.pacer.running(), "still prebuffering");
        assert!(commands(&out).is_empty());
        s.write(&[0u8; 200], &mut out, v);
        s.pump(t0, 1.0, &mut out, v);
        assert!(s.pacer.running());
        assert_eq!(s.pacer.sent(), 100, "the lead goes straight away");
        let cmds = commands(&out);
        assert!(matches!(cmds[0], Command::Started(9)), "{cmds:?}");
        assert!(matches!(cmds[1], Command::Request(pa::Request { channel: 9, length: 200 })), "{cmds:?}");
        out.clear();

        s.drain = Some(77);
        for t in (10..=400).step_by(10) {
            s.pump(t0 + t * ms, 1.0, &mut out, v);
            if t == 40 {
                assert_eq!(s.pacer.sent(), 140, "real time after the lead, in whole chunks");
            }
        }
        assert_eq!(s.pacer.sent(), 200, "everything played");
        assert!(s.drain.is_none(), "drained once played out");
        let cmds = commands(&out);
        assert!(cmds.iter().any(|c| matches!(c, Command::Reply)), "the drain is acknowledged");
        assert!(!cmds.iter().any(|c| matches!(c, Command::Underflow(_))), "no underflow while draining");
    }

    #[test]
    fn underflow_is_reported_once_and_rearms_prebuffering() {
        let t0 = Instant::now();
        let ms = Duration::from_millis(1);
        let v = pa::MAX_VERSION;
        let mut s = stream(100);
        let mut out = Vec::new();
        s.write(&[0u8; 100], &mut out, v);
        s.pump(t0, 1.0, &mut out, v);
        assert!(s.pacer.running());
        out.clear();
        s.pump(t0 + 60 * ms, 1.0, &mut out, v);
        s.pump(t0 + 70 * ms, 1.0, &mut out, v);
        let n = commands(&out).iter().filter(|c| matches!(c, Command::Underflow(_))).count();
        assert_eq!(n, 1);
        assert!(s.prebuffering && !s.pacer.running());
    }

    #[test]
    fn corked_streams_do_not_move() {
        let t0 = Instant::now();
        let v = pa::MAX_VERSION;
        let mut s = stream(0);
        s.corked = true;
        let mut out = Vec::new();
        s.write(&[0u8; 400], &mut out, v);
        s.pump(t0, 1.0, &mut out, v);
        s.pump(t0 + Duration::from_secs(1), 1.0, &mut out, v);
        assert_eq!(s.pacer.sent(), 0);
        s.corked = false;
        s.pump(t0 + Duration::from_secs(1), 1.0, &mut out, v);
        assert_eq!(s.pacer.sent(), 100);
    }

    #[test]
    fn credit_window_holds_back_a_slow_app() {
        let t0 = Instant::now();
        let mut s = stream(0);
        s.deaf = false;
        // The app already holds a full window (250 ms of 2-byte frames).
        s.in_app = 500;
        s.progress = t0;
        s.pacer.start(t0);
        s.write(&[0u8; 400], &mut Vec::new(), pa::MAX_VERSION);
        s.send_due(t0 + Duration::from_millis(10), 1.0);
        assert_eq!(s.pacer.sent(), 0);
        assert!(s.deaf_now(t0 + Duration::from_millis(1500)), "an app that never answers is given up on");
        s.credit(500);
        assert_eq!(s.in_app, 0);
    }

    #[test]
    fn flush_drops_queued_data() {
        let t0 = Instant::now();
        let v = pa::MAX_VERSION;
        let mut s = stream(0);
        let mut out = Vec::new();
        s.write(&[0u8; 400], &mut out, v);
        s.pump(t0, 1.0, &mut out, v);
        s.flush(t0);
        assert!(s.queue.is_empty());
        assert_eq!(s.written, s.read_index());
        assert_eq!(s.latency(t0, SystemTime::now()).write_offset, 200);
    }

    #[test]
    fn raw_seq_reads_the_header() {
        let mut buf = Vec::new();
        pa::write_command_message(&mut buf, 1234, &Command::GetServerInfo, pa::MAX_VERSION).unwrap();
        assert_eq!(raw_seq(&buf[pa::DESCRIPTOR_SIZE..]), 1234);
        assert_eq!(raw_seq(&[1, 2]), u32::MAX);
    }
}
