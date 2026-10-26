//! Playing the server's sound streams through Android.
//!
//! Each stream gets a thread of its own that owns its Android stream and feeds it from a bounded
//! queue. The link's reader only queues and returns: a stalled sound device must never hold up
//! frames and input on the same link. The server paces streams at real time and keeps about
//! 100 ms ahead, so the queue normally holds that much; if the device falls behind for good, the
//! oldest sound is dropped rather than letting the delay grow.
//!
//! Output is AAudio where it is solid (Android 8.1+), loaded with dlopen because the app also
//! runs on Android 7 where libaaudio.so does not exist; elsewhere, or when AAudio cannot open the
//! stream as asked, android.media.AudioTrack through JNI. A device that goes away (headphones
//! unplugged, Bluetooth gone) shows up as a failed write; the stream is then opened again on
//! whatever Android routes to now.

use std::collections::{HashMap, VecDeque};
use std::ffi::{c_char, c_void, CStr};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use jni::objects::{GlobalRef, JValue};
use jni::JNIEnv;
use lindroids_proto::AudioFormat;
use log::{info, warn};

/// Most sound a stream may have queued before the oldest is dropped.
const QUEUE_MS: u32 = 500;
/// A stream with nothing to play for this long pauses its device (it resumes with the next data).
const IDLE_PAUSE: Duration = Duration::from_secs(3);
/// Device buffer asked for: room for a late wakeup of the feeding thread.
const DEVICE_BUFFER_MS: u32 = 40;

/// Open streams, by (link, stream).
type Streams = HashMap<(i32, u32), Arc<Feed>>;
static STREAMS: Mutex<Option<Streams>> = Mutex::new(None);

#[derive(Clone, Copy)]
struct Spec {
    rate: u32,
    channels: u32,
    format: AudioFormat,
}

impl Spec {
    fn frame(&self) -> usize {
        (self.channels * self.format.bytes()) as usize
    }
}

/// What the link hands a stream's thread.
struct Feed {
    state: Mutex<Queue>,
    cv: Condvar,
}

struct Queue {
    chunks: VecDeque<Vec<u8>>,
    bytes: usize,
    cap: usize,
    corked: bool,
    closed: bool,
    dropped: bool,
}

pub fn open(link: i32, stream: u32, rate: u32, channels: u32, format: AudioFormat) {
    close(link, stream);
    if rate == 0 || !(1..=2).contains(&channels) {
        warn!("sound {link}/{stream}: cannot play {rate} Hz {channels} ch");
        return;
    }
    let spec = Spec { rate, channels, format };
    let cap = (rate as usize * spec.frame() * QUEUE_MS as usize / 1000).max(spec.frame());
    let feed = Arc::new(Feed {
        state: Mutex::new(Queue { chunks: VecDeque::new(), bytes: 0, cap, corked: false, closed: false, dropped: false }),
        cv: Condvar::new(),
    });
    let f = feed.clone();
    let spawned = std::thread::Builder::new().name(format!("lindroids-sound-{stream}")).spawn(move || play(link, stream, spec, f));
    if let Err(e) = spawned {
        warn!("sound {link}/{stream}: no thread: {e}");
        return;
    }
    STREAMS.lock().unwrap().get_or_insert_with(HashMap::new).insert((link, stream), feed);
}

pub fn write(link: i32, stream: u32, pcm: Vec<u8>) {
    let Some(feed) = feed(link, stream) else { return };
    let mut q = feed.state.lock().unwrap();
    q.bytes += pcm.len();
    q.chunks.push_back(pcm);
    while q.bytes > q.cap && q.chunks.len() > 1 {
        let old = q.chunks.pop_front().map_or(0, |c| c.len());
        q.bytes -= old;
        if !q.dropped {
            q.dropped = true;
            warn!("sound {link}/{stream}: the device is behind, dropping sound");
        }
    }
    feed.cv.notify_one();
}

pub fn cork(link: i32, stream: u32, corked: bool) {
    let Some(feed) = feed(link, stream) else { return };
    feed.state.lock().unwrap().corked = corked;
    feed.cv.notify_one();
}

pub fn close(link: i32, stream: u32) {
    let feed = STREAMS.lock().unwrap().as_mut().and_then(|m| m.remove(&(link, stream)));
    if let Some(f) = feed {
        finish(&f);
    }
}

pub fn close_link(link: i32) {
    let gone: Vec<Arc<Feed>> = match STREAMS.lock().unwrap().as_mut() {
        Some(m) => {
            let keys: Vec<(i32, u32)> = m.keys().filter(|(l, _)| *l == link).copied().collect();
            keys.iter().filter_map(|k| m.remove(k)).collect()
        }
        None => Vec::new(),
    };
    gone.iter().for_each(|f| finish(f));
}

fn feed(link: i32, stream: u32) -> Option<Arc<Feed>> {
    STREAMS.lock().unwrap().as_ref().and_then(|m| m.get(&(link, stream)).cloned())
}

/// The thread notices and closes its stream; nobody waits for it.
fn finish(f: &Feed) {
    f.state.lock().unwrap().closed = true;
    f.cv.notify_one();
}

// ------------------------------------------------------------------ the stream's thread

enum Step {
    Play(Vec<u8>),
    Cork(bool),
    Idle,
    Close,
}

fn play(link: i32, stream: u32, spec: Spec, feed: Arc<Feed>) {
    let mut out: Option<Output> = None;
    let mut corked = false;
    let mut paused = false;
    let mut last_try: Option<Instant> = None;
    let mut last_data = Instant::now();
    loop {
        let step = {
            let mut q = feed.state.lock().unwrap();
            loop {
                if q.closed {
                    break Step::Close;
                }
                if q.corked != corked {
                    break Step::Cork(q.corked);
                }
                if !q.corked {
                    if let Some(c) = q.chunks.pop_front() {
                        q.bytes -= c.len();
                        break Step::Play(c);
                    }
                }
                let (g, t) = feed.cv.wait_timeout(q, Duration::from_millis(500)).unwrap();
                q = g;
                if t.timed_out() && !paused && out.is_some() && last_data.elapsed() > IDLE_PAUSE {
                    break Step::Idle;
                }
            }
        };
        match step {
            Step::Close => break,
            Step::Cork(c) => {
                corked = c;
                if let Some(o) = &mut out {
                    if c && !paused {
                        o.pause();
                        paused = true;
                    }
                }
            }
            Step::Idle => {
                if let Some(o) = &mut out {
                    o.pause();
                    paused = true;
                }
            }
            Step::Play(pcm) => {
                last_data = Instant::now();
                if out.is_none() {
                    // A device that will not open is tried again at most once a second; the
                    // sound in between is dropped.
                    if last_try.is_some_and(|t| t.elapsed() < Duration::from_secs(1)) {
                        continue;
                    }
                    last_try = Some(Instant::now());
                    out = Output::open(spec);
                    paused = false;
                    match &out {
                        Some(o) => info!("sound {link}/{stream}: {} Hz {} ch {:?} on {}", spec.rate, spec.channels, spec.format, o.kind()),
                        None => warn!("sound {link}/{stream}: no output for {} Hz {} ch {:?}", spec.rate, spec.channels, spec.format),
                    }
                }
                let Some(o) = &mut out else { continue };
                if paused {
                    o.resume();
                    paused = false;
                }
                if !o.write(&pcm, spec) {
                    warn!("sound {link}/{stream}: {} failed, opening the stream again", o.kind());
                    // Release the old device before asking for a new one.
                    drop(out.take());
                    last_try = Some(Instant::now());
                    out = Output::open(spec);
                    if let Some(o) = &mut out {
                        o.write(&pcm, spec);
                    }
                }
            }
        }
    }
}

enum Output {
    AAudio(AAudioOut),
    Track(TrackOut),
}

impl Output {
    fn open(spec: Spec) -> Option<Output> {
        if let Some(o) = AAudioOut::open(spec) {
            return Some(Output::AAudio(o));
        }
        TrackOut::open(spec).map(Output::Track)
    }

    fn kind(&self) -> &'static str {
        match self {
            Output::AAudio(_) => "AAudio",
            Output::Track(_) => "AudioTrack",
        }
    }

    /// Blocks until the device took everything. False if the device is gone.
    fn write(&mut self, pcm: &[u8], spec: Spec) -> bool {
        match self {
            Output::AAudio(o) => o.write(pcm, spec),
            Output::Track(o) => o.write(pcm, spec),
        }
    }

    fn pause(&mut self) {
        match self {
            Output::AAudio(o) => o.pause(),
            Output::Track(o) => o.call("pause"),
        }
    }

    fn resume(&mut self) {
        match self {
            Output::AAudio(o) => o.resume(),
            Output::Track(o) => o.call("play"),
        }
    }
}

// ------------------------------------------------------------------ AAudio

type Builder = *mut c_void;
type AStream = *mut c_void;

const AAUDIO_OK: i32 = 0;
const AAUDIO_DIRECTION_OUTPUT: i32 = 0;
const AAUDIO_FORMAT_PCM_I16: i32 = 1;
const AAUDIO_FORMAT_PCM_FLOAT: i32 = 2;
const AAUDIO_SHARING_MODE_SHARED: i32 = 1;
const AAUDIO_PERFORMANCE_MODE_LOW_LATENCY: i32 = 12;
const AAUDIO_USAGE_MEDIA: i32 = 1;

/// AAudio as it shipped in Android 8.0 had enough rough edges that players (Oboe among them)
/// only use it from 8.1, API 27.
const AAUDIO_MIN_API: i32 = 27;

/// The AAudio functions used, from libaaudio.so.
struct AAudio {
    create_builder: unsafe extern "C" fn(*mut Builder) -> i32,
    set_rate: unsafe extern "C" fn(Builder, i32),
    set_channels: unsafe extern "C" fn(Builder, i32),
    set_format: unsafe extern "C" fn(Builder, i32),
    set_direction: unsafe extern "C" fn(Builder, i32),
    set_sharing: unsafe extern "C" fn(Builder, i32),
    set_performance: unsafe extern "C" fn(Builder, i32),
    /// Android 9+.
    set_usage: Option<unsafe extern "C" fn(Builder, i32)>,
    open: unsafe extern "C" fn(Builder, *mut AStream) -> i32,
    delete_builder: unsafe extern "C" fn(Builder) -> i32,
    start: unsafe extern "C" fn(AStream) -> i32,
    pause: unsafe extern "C" fn(AStream) -> i32,
    stop: unsafe extern "C" fn(AStream) -> i32,
    close: unsafe extern "C" fn(AStream) -> i32,
    write: unsafe extern "C" fn(AStream, *const c_void, i32, i64) -> i32,
    get_rate: unsafe extern "C" fn(AStream) -> i32,
    get_channels: unsafe extern "C" fn(AStream) -> i32,
    get_format: unsafe extern "C" fn(AStream) -> i32,
    set_buffer_size: unsafe extern "C" fn(AStream, i32) -> i32,
}

fn aaudio() -> Option<&'static AAudio> {
    static API: OnceLock<Option<AAudio>> = OnceLock::new();
    API.get_or_init(|| {
        if api_level() < AAUDIO_MIN_API {
            return None;
        }
        let lib = unsafe { libc::dlopen(c"libaaudio.so".as_ptr(), libc::RTLD_NOW) };
        if lib.is_null() {
            return None;
        }
        // SAFETY: each field's type is the C signature of the function of that name.
        unsafe {
            Some(AAudio {
                create_builder: func(lib, c"AAudio_createStreamBuilder")?,
                set_rate: func(lib, c"AAudioStreamBuilder_setSampleRate")?,
                set_channels: func(lib, c"AAudioStreamBuilder_setChannelCount")?,
                set_format: func(lib, c"AAudioStreamBuilder_setFormat")?,
                set_direction: func(lib, c"AAudioStreamBuilder_setDirection")?,
                set_sharing: func(lib, c"AAudioStreamBuilder_setSharingMode")?,
                set_performance: func(lib, c"AAudioStreamBuilder_setPerformanceMode")?,
                set_usage: func(lib, c"AAudioStreamBuilder_setUsage"),
                open: func(lib, c"AAudioStreamBuilder_openStream")?,
                delete_builder: func(lib, c"AAudioStreamBuilder_delete")?,
                start: func(lib, c"AAudioStream_requestStart")?,
                pause: func(lib, c"AAudioStream_requestPause")?,
                stop: func(lib, c"AAudioStream_requestStop")?,
                close: func(lib, c"AAudioStream_close")?,
                write: func(lib, c"AAudioStream_write")?,
                get_rate: func(lib, c"AAudioStream_getSampleRate")?,
                get_channels: func(lib, c"AAudioStream_getChannelCount")?,
                get_format: func(lib, c"AAudioStream_getFormat")?,
                set_buffer_size: func(lib, c"AAudioStream_setBufferSizeInFrames")?,
            })
        }
    })
    .as_ref()
}

/// A function from a dlopened library, as the function pointer type `T`.
unsafe fn func<T: Copy>(lib: *mut c_void, name: &CStr) -> Option<T> {
    let p = libc::dlsym(lib, name.as_ptr());
    (!p.is_null()).then(|| std::mem::transmute_copy::<*mut c_void, T>(&p))
}

fn api_level() -> i32 {
    let mut buf = [0 as c_char; 92];
    let n = unsafe { libc::__system_property_get(c"ro.build.version.sdk".as_ptr(), buf.as_mut_ptr()) };
    if n <= 0 {
        return 0;
    }
    unsafe { CStr::from_ptr(buf.as_ptr()) }.to_str().ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

struct AAudioOut {
    api: &'static AAudio,
    stream: AStream,
}

impl AAudioOut {
    fn open(spec: Spec) -> Option<AAudioOut> {
        let api = aaudio()?;
        let format = match spec.format {
            AudioFormat::S16le => AAUDIO_FORMAT_PCM_I16,
            AudioFormat::F32le => AAUDIO_FORMAT_PCM_FLOAT,
        };
        unsafe {
            let mut b: Builder = std::ptr::null_mut();
            if (api.create_builder)(&mut b) != AAUDIO_OK || b.is_null() {
                return None;
            }
            (api.set_direction)(b, AAUDIO_DIRECTION_OUTPUT);
            (api.set_rate)(b, spec.rate as i32);
            (api.set_channels)(b, spec.channels as i32);
            (api.set_format)(b, format);
            (api.set_sharing)(b, AAUDIO_SHARING_MODE_SHARED);
            (api.set_performance)(b, AAUDIO_PERFORMANCE_MODE_LOW_LATENCY);
            if let Some(set_usage) = api.set_usage {
                set_usage(b, AAUDIO_USAGE_MEDIA);
            }
            let mut s: AStream = std::ptr::null_mut();
            let r = (api.open)(b, &mut s);
            (api.delete_builder)(b);
            if r != AAUDIO_OK || s.is_null() {
                return None;
            }
            let out = AAudioOut { api, stream: s };
            // AAudio may hand back something other than what was asked; AudioTrack converts
            // anything, so it plays those instead.
            if (api.get_rate)(s) != spec.rate as i32 || (api.get_channels)(s) != spec.channels as i32 || (api.get_format)(s) != format {
                return None;
            }
            (api.set_buffer_size)(s, (spec.rate * DEVICE_BUFFER_MS / 1000) as i32);
            if (api.start)(s) != AAUDIO_OK {
                return None;
            }
            Some(out)
        }
    }

    fn write(&mut self, pcm: &[u8], spec: Spec) -> bool {
        let frame = spec.frame();
        let total = (pcm.len() / frame) as i32;
        let mut done = 0;
        let mut stuck = 0;
        while done < total {
            let p = unsafe { pcm.as_ptr().add(done as usize * frame) } as *const c_void;
            let n = unsafe { (self.api.write)(self.stream, p, total - done, 200_000_000) };
            if n < 0 {
                return false;
            }
            done += n;
            // A device that takes nothing for a second has stopped for good.
            stuck = if n == 0 { stuck + 1 } else { 0 };
            if stuck >= 5 {
                return false;
            }
        }
        true
    }

    fn pause(&mut self) {
        unsafe { (self.api.pause)(self.stream) };
    }

    fn resume(&mut self) {
        unsafe { (self.api.start)(self.stream) };
    }
}

impl Drop for AAudioOut {
    fn drop(&mut self) {
        unsafe {
            (self.api.stop)(self.stream);
            (self.api.close)(self.stream);
        }
    }
}

// ------------------------------------------------------------------ AudioTrack

const STREAM_MUSIC: i32 = 3;
const CHANNEL_OUT_MONO: i32 = 4;
const CHANNEL_OUT_STEREO: i32 = 12;
const ENCODING_PCM_16BIT: i32 = 2;
const ENCODING_PCM_FLOAT: i32 = 4;
const MODE_STREAM: i32 = 1;
const STATE_INITIALIZED: i32 = 1;
const WRITE_BLOCKING: i32 = 0;

struct TrackOut {
    track: GlobalRef,
}

/// Run `f` with this thread's JNIEnv, inside a local frame so nothing piles up on a thread that
/// never returns to Java. A Java exception is cleared and counts as failure.
fn with_env<T>(f: impl FnOnce(&mut JNIEnv) -> jni::errors::Result<T>) -> Option<T> {
    let vm = crate::android::VM.get()?;
    let mut env = vm.attach_current_thread_permanently().ok()?;
    let r = env.with_local_frame(8, |env| f(env));
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
        return None;
    }
    r.ok()
}

impl TrackOut {
    fn open(spec: Spec) -> Option<TrackOut> {
        let channels = if spec.channels == 1 { CHANNEL_OUT_MONO } else { CHANNEL_OUT_STEREO };
        let encoding = match spec.format {
            AudioFormat::S16le => ENCODING_PCM_16BIT,
            AudioFormat::F32le => ENCODING_PCM_FLOAT,
        };
        let rate = spec.rate as i32;
        with_env(|env| {
            let cls = "android/media/AudioTrack";
            let min = env.call_static_method(cls, "getMinBufferSize", "(III)I", &[rate.into(), channels.into(), encoding.into()])?.i()?;
            if min <= 0 {
                return Ok(None);
            }
            let want = (spec.rate as usize * spec.frame() * DEVICE_BUFFER_MS as usize / 1000) as i32;
            let args = [STREAM_MUSIC.into(), rate.into(), channels.into(), encoding.into(), min.max(want).into(), MODE_STREAM.into()];
            let track = env.new_object(cls, "(IIIIII)V", &args)?;
            if env.call_method(&track, "getState", "()I", &[])?.i()? != STATE_INITIALIZED {
                env.call_method(&track, "release", "()V", &[])?;
                return Ok(None);
            }
            env.call_method(&track, "play", "()V", &[])?;
            Ok(Some(TrackOut { track: env.new_global_ref(&track)? }))
        })
        .flatten()
    }

    fn write(&mut self, pcm: &[u8], spec: Spec) -> bool {
        let pcm = &pcm[..pcm.len() - pcm.len() % spec.frame()];
        let track = self.track.as_obj();
        let written = with_env(|env| match spec.format {
            AudioFormat::S16le => {
                let arr = env.new_byte_array(pcm.len() as i32)?;
                let bytes: &[i8] = unsafe { std::slice::from_raw_parts(pcm.as_ptr() as *const i8, pcm.len()) };
                env.set_byte_array_region(&arr, 0, bytes)?;
                let args = [JValue::Object(&arr), 0.into(), (pcm.len() as i32).into()];
                Ok(env.call_method(track, "write", "([BII)I", &args)?.i()? as usize == pcm.len())
            }
            AudioFormat::F32le => {
                let floats: Vec<f32> = pcm.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
                let arr = env.new_float_array(floats.len() as i32)?;
                env.set_float_array_region(&arr, 0, &floats)?;
                let args = [JValue::Object(&arr), 0.into(), (floats.len() as i32).into(), WRITE_BLOCKING.into()];
                Ok(env.call_method(track, "write", "([FIII)I", &args)?.i()? as usize == floats.len())
            }
        });
        written == Some(true)
    }

    fn call(&self, name: &str) {
        let track = self.track.as_obj();
        with_env(|env| env.call_method(track, name, "()V", &[]).map(|_| ()));
    }
}

impl Drop for TrackOut {
    fn drop(&mut self) {
        self.call("stop");
        self.call("release");
    }
}
