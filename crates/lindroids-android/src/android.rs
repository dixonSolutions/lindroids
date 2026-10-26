//! JNI entry points (see `Native.java`): the app's end of each link, and the in-app server.
//!
//! A link is one connection to a server (one sandbox). The server sends the tiles of each frame
//! that changed; this side keeps every window's last frame, applies the tiles, copies the frame
//! into the window's Surface (ANativeWindow) and acknowledges it. Input goes back the same way,
//! sound streams go to `sound`, bundles to `bundle`. The Android integration (system state,
//! opening URIs and files, notifications, the file picker) is wired here; what it decides is in
//! `portal`.

use std::collections::HashMap;
use std::ffi::c_void;
use std::io::{Read, Write};
use std::os::fd::FromRawFd;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine;
use jni::objects::{GlobalRef, JClass, JLongArray, JObject, JObjectArray, JString, JValue};
use jni::sys::{jboolean, jfloat, jint, jlong, jstring, JNI_VERSION_1_6};
use jni::{JNIEnv, JavaVM};
use lindroids_proto::{self as proto, BatteryState, Framer, MemoryState, NetworkState, PickedFile, PointerAction, SystemState, ToApp, ToServer, TouchAction};
use lindroids_server::applink::{self, Conn, Outcome};
use log::{error, info, warn};

use crate::portal::{self, NoteIds};

pub(crate) static VM: OnceLock<JavaVM> = OnceLock::new();
static LINK_SERVICE: OnceLock<GlobalRef> = OnceLock::new();
static LINKS: Mutex<Option<HashMap<i32, Arc<LinkTx>>>> = Mutex::new(None);
static WINDOWS: Mutex<Option<HashMap<(i32, u32), Win>>> = Mutex::new(None);
/// Bundles being received: the write end of the pipe feeding the unpacking thread.
static IMPORTS: Mutex<Option<HashMap<(i32, String), std::fs::File>>> = Mutex::new(None);
static FILES_DIR: OnceLock<PathBuf> = OnceLock::new();
static CACHE_DIR: OnceLock<PathBuf> = OnceLock::new();
static DENSITY: Mutex<f32> = Mutex::new(1.0);
static SCREEN: Mutex<Option<(u32, u32)>> = Mutex::new(None);
/// The system state, and what each link last got of it.
static SYSTEM: Mutex<Option<System>> = Mutex::new(None);
/// Android notification ids of the servers' notifications.
static NOTES: Mutex<Option<NoteIds>> = Mutex::new(None);
/// Documents the user picked, by (link, request), in FilesPicked order: FetchFile names them by
/// index, and WriteFile may only write to these.
static PICKED: Mutex<Option<HashMap<(i32, u32), Vec<String>>>> = Mutex::new(None);
/// Writes in progress (WriteFile chunks of one request), by (link, request).
static WRITES: Mutex<Option<HashMap<(i32, u32), WriteState>>> = Mutex::new(None);

/// Run `f` on the map (or other state) behind one of the statics above, creating it if needed.
fn with<T: Default, R>(m: &Mutex<Option<T>>, f: impl FnOnce(&mut T) -> R) -> R {
    f(m.lock().unwrap().get_or_insert_with(T::default))
}

/// The sending half of a link. Writers take turns on `w`, one whole message (with its payload)
/// at a time; each link has its own, so a big payload (a fetched file) on one link never holds
/// up another. `ctl` is a second handle on the same socket, to shut it down even while a writer
/// is blocked in `w`.
struct LinkTx {
    w: Mutex<Conn>,
    ctl: Conn,
}

fn link_ids() -> Vec<i32> {
    LINKS.lock().unwrap().as_ref().map(|m| m.keys().copied().collect()).unwrap_or_default()
}

#[derive(Default)]
struct System {
    current: Option<SystemState>,
    sent: HashMap<i32, SystemState>,
    /// When the pending change goes out (debounce); a flusher thread waits for it.
    deadline: Option<Instant>,
    flusher: bool,
}

enum WriteState {
    Active(SyncSender<WriteChunk>),
    /// Already answered with an error: swallow the rest of its chunks.
    Failed,
}

struct WriteChunk {
    offset: u64,
    last: bool,
    data: Vec<u8>,
}

/// The app side of one Linux window: the last frame, kept whole so changed tiles can be applied.
#[derive(Default)]
struct Win {
    surface: Option<NativeWindow>,
    width: u32,
    height: u32,
    frame: Vec<u32>,
    /// A frame arrived while there was no Surface; acknowledged once it is shown.
    unacked: bool,
}

struct NativeWindow {
    ptr: *mut ndk_sys::ANativeWindow,
    geometry: (u32, u32),
}

unsafe impl Send for NativeWindow {}

impl Drop for NativeWindow {
    fn drop(&mut self) {
        unsafe { ndk_sys::ANativeWindow_release(self.ptr) };
    }
}

impl Win {
    /// Copy the frame into the Surface. Returns false if there is no Surface to draw into.
    fn present(&mut self) -> bool {
        let Some(nw) = &mut self.surface else { return false };
        let (w, h) = (self.width, self.height);
        if w == 0 || h == 0 || self.frame.len() < (w * h) as usize {
            return true;
        }
        unsafe {
            if nw.geometry != (w, h) {
                // WINDOW_FORMAT_RGBA_8888
                ndk_sys::ANativeWindow_setBuffersGeometry(nw.ptr, w as i32, h as i32, 1);
                nw.geometry = (w, h);
            }
            let mut buf: ndk_sys::ANativeWindow_Buffer = std::mem::zeroed();
            if ndk_sys::ANativeWindow_lock(nw.ptr, &mut buf, std::ptr::null_mut()) != 0 {
                return false;
            }
            let rows = (buf.height.max(0) as u32).min(h) as usize;
            let cols = (buf.width.max(0) as u32).min(w) as usize;
            let dst = buf.bits as *mut u32;
            for y in 0..rows {
                std::ptr::copy_nonoverlapping(self.frame.as_ptr().add(y * w as usize), dst.add(y * buf.stride as usize), cols);
            }
            ndk_sys::ANativeWindow_unlockAndPost(nw.ptr);
        }
        true
    }
}

#[no_mangle]
pub extern "system" fn JNI_OnLoad(vm: *mut jni::sys::JavaVM, _: *mut c_void) -> jint {
    android_logger::init_once(android_logger::Config::default().with_tag("lindroids").with_max_level(log::LevelFilter::Info));
    std::panic::set_hook(Box::new(|p| error!("panic: {p}")));
    if let Ok(vm) = unsafe { JavaVM::from_raw(vm) } {
        let _ = VM.set(vm);
    }
    JNI_VERSION_1_6
}

fn strings(env: &mut JNIEnv, arr: &JObjectArray) -> Vec<String> {
    let n = env.get_array_length(arr).unwrap_or(0);
    (0..n)
        .filter_map(|i| {
            let o = env.get_object_array_element(arr, i).ok()?;
            env.get_string(&JString::from(o)).ok().map(|s| s.into())
        })
        .collect()
}

fn jstr(env: &mut JNIEnv, s: &JString) -> String {
    if s.is_null() {
        return String::new();
    }
    env.get_string(s).map(|s| s.into()).unwrap_or_default()
}

fn opt_jstr(env: &mut JNIEnv, s: &JString) -> Option<String> {
    if s.is_null() {
        None
    } else {
        Some(jstr(env, s)).filter(|s| !s.is_empty())
    }
}

fn ret_string(env: &mut JNIEnv, s: &str) -> jstring {
    env.new_string(s).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut())
}

// ------------------------------------------------------------------ in-app server

#[no_mangle]
pub extern "system" fn Java_io_github_dixonsolutions_lindroids_Native_serverRun(mut env: JNIEnv, _: JClass, args: JObjectArray) -> jint {
    let args = strings(&mut env, &args);
    let cfg = lindroids_server::Config::from_args(&args);
    info!("in-app server starting: {:?}", cfg.runtime_dir);
    lindroids_server::run(cfg)
}

// ------------------------------------------------------------------ links

#[no_mangle]
pub extern "system" fn Java_io_github_dixonsolutions_lindroids_Native_appInit(
    mut env: JNIEnv,
    _: JClass,
    files: JString,
    cache: JString,
    density: jfloat,
    width: jint,
    height: jint,
) {
    let files = jstr(&mut env, &files);
    let _ = FILES_DIR.set(PathBuf::from(files));
    let cache = PathBuf::from(jstr(&mut env, &cache));
    if CACHE_DIR.set(cache.clone()).is_ok() {
        // Files handed to other apps by OpenFile; a day later they have been read.
        std::thread::spawn(move || portal::clean_old(&cache.join("open"), Duration::from_secs(86_400)));
    }
    *DENSITY.lock().unwrap() = density;
    if width > 0 && height > 0 {
        *SCREEN.lock().unwrap() = Some((width as u32, height as u32));
    }
    if LINK_SERVICE.get().is_none() {
        if let Ok(cls) = env.find_class("io/github/dixonsolutions/lindroids/LinkService") {
            if let Ok(g) = env.new_global_ref(cls) {
                let _ = LINK_SERVICE.set(g);
            }
        }
    }
}

fn send(link: i32, msg: &ToServer) {
    send_with(link, msg, &[]);
}

/// Send a message and its payload (the message's `len` must be `payload.len()`). Returns false
/// if the link is gone.
fn send_with(link: i32, msg: &ToServer, payload: &[u8]) -> bool {
    let Some(tx) = LINKS.lock().unwrap().as_ref().and_then(|m| m.get(&link).cloned()) else { return false };
    let buf = proto::encode_with(msg, payload);
    let mut w = tx.w.lock().unwrap();
    match w.write_all(&buf) {
        Ok(()) => true,
        Err(e) => {
            warn!("link {link}: {e}");
            false
        }
    }
}

/// Connect to a server and authenticate. With `link` < 0 only probe (nothing is kept).
///
/// Returns "ok <transport>", "paired <transport> <secret>" (pairing by code worked; keep the
/// secret), "denied <open> <why>" (not paired / wrong code; open = pairing still possible),
/// "wrong-server" (it could not prove it knows our secret) or "absent <why>".
#[no_mangle]
pub extern "system" fn Java_io_github_dixonsolutions_lindroids_Native_linkConnect(
    mut env: JNIEnv,
    _: JClass,
    link: jint,
    socket: JString,
    uid: jint,
    secret: JString,
    code: JString,
) -> jstring {
    let socket = opt_jstr(&mut env, &socket);
    let secret = opt_jstr(&mut env, &secret);
    let code = opt_jstr(&mut env, &code);
    let display = applink::Display { density: *DENSITY.lock().unwrap(), screen: *SCREEN.lock().unwrap() };
    let r = match applink::connect(uid as u32, socket.as_deref(), secret.as_deref(), code.as_deref(), display, crate::BUILD) {
        Outcome::Linked { conn, framer, paired, sandbox, backlog } => {
            let kind = conn.kind();
            if link >= 0 {
                match conn.try_clone().and_then(|w| Ok((w, conn.try_clone()?))) {
                    Ok((w, ctl)) => {
                        LINKS.lock().unwrap().get_or_insert_with(HashMap::new).insert(link, Arc::new(LinkTx { w: Mutex::new(w), ctl }));
                        info!("link {link}: linked over {kind} to {sandbox}");
                        for (m, p) in backlog {
                            dispatch(link, m, p);
                        }
                        start_reader(link, conn, framer);
                    }
                    Err(e) => return ret_string(&mut env, &format!("absent {e}")),
                }
            } else {
                conn.shutdown();
            }
            match paired {
                Some(s) => format!("paired {kind} {s}"),
                None => format!("ok {kind}"),
            }
        }
        Outcome::Denied { open, reason } => format!("denied {open} {reason}"),
        Outcome::WrongServer => "wrong-server".into(),
        Outcome::Absent(e) => format!("absent {e}"),
    };
    ret_string(&mut env, &r)
}

fn start_reader(link: i32, mut conn: Conn, mut framer: Framer) {
    std::thread::Builder::new()
        .name(format!("lindroids-link-{link}"))
        .spawn(move || {
            let mut buf = vec![0u8; 1 << 16];
            loop {
                match conn.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => framer.push(&buf[..n]),
                }
                while let Some(m) = framer.next::<ToApp>() {
                    match m {
                        Ok((m, p)) => dispatch(link, m, p),
                        Err(e) => warn!("link {link}: bad message: {e}"),
                    }
                }
            }
            info!("link {link}: closed");
            LINKS.lock().unwrap().as_mut().map(|m| m.remove(&link));
            WINDOWS.lock().unwrap().as_mut().map(|m| m.retain(|(l, _), _| *l != link));
            IMPORTS.lock().unwrap().as_mut().map(|m| m.retain(|(l, _), _| *l != link));
            crate::sound::close_link(link);
            with(&PICKED, |m| m.retain(|(l, _), _| *l != link));
            // Dropping a write's sender ends its thread without an answer: there is no one to tell.
            with(&WRITES, |m| m.retain(|(l, _), _| *l != link));
            with(&SYSTEM, |s| s.sent.remove(&link));
            // Their taps would go nowhere now.
            for note in with(&NOTES, |n| n.remove_link(link)) {
                call("onCancelNotification", "(I)V", |_| vec![JValueOwned::Int(note)]);
            }
            call("onLinkClosed", "(I)V", |_| vec![JValueOwned::Int(link)]);
        })
        .ok();
}

/// Call a static void method on LinkService from any thread.
fn call(name: &str, sig: &str, args: impl FnOnce(&mut JNIEnv) -> Vec<JValueOwned>) {
    invoke(name, sig, args, false);
}

/// Call a static method on LinkService that returns a String.
fn call_string(name: &str, sig: &str, args: impl FnOnce(&mut JNIEnv) -> Vec<JValueOwned>) -> Option<String> {
    invoke(name, sig, args, true)
}

/// The arguments and the call run in a local frame of their own: the link threads stay attached
/// for good, and local references they made would otherwise pile up until the VM's table
/// overflows (512 entries on Android 7).
fn invoke(name: &str, sig: &str, args: impl FnOnce(&mut JNIEnv) -> Vec<JValueOwned>, string: bool) -> Option<String> {
    let vm = VM.get()?;
    let cls = LINK_SERVICE.get()?;
    let mut env = vm.attach_current_thread_permanently().ok()?;
    let r = env.with_local_frame(32, |env| -> jni::errors::Result<Option<String>> {
        let owned = args(env);
        let vals: Vec<JValue> = owned.iter().map(|v| v.borrow()).collect();
        let class: &JClass = cls.as_obj().into();
        let r = env.call_static_method(class, name, sig, &vals).and_then(|v| {
            if !string {
                return Ok(None);
            }
            let o = v.l()?;
            if o.is_null() {
                return Ok(None);
            }
            Ok(Some(env.get_string(&JString::from(o))?.into()))
        });
        if r.is_err() {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
        }
        r
    });
    match r {
        Ok(v) => v,
        Err(e) => {
            error!("LinkService.{name}: {e}");
            None
        }
    }
}

enum JValueOwned {
    Int(i32),
    Bool(bool),
    Obj(JObject<'static>),
}

impl JValueOwned {
    fn borrow(&self) -> JValue<'_, '_> {
        match self {
            JValueOwned::Int(i) => JValue::Int(*i),
            JValueOwned::Bool(b) => JValue::Bool(*b as u8),
            JValueOwned::Obj(o) => JValue::Object(o),
        }
    }
}

fn jstring_owned(env: &mut JNIEnv, s: &str) -> JValueOwned {
    match env.new_string(s) {
        Ok(js) => JValueOwned::Obj(unsafe { JObject::from_raw(js.into_raw()) }),
        Err(_) => JValueOwned::Obj(JObject::null()),
    }
}

/// A String, or null.
fn jstring_opt(env: &mut JNIEnv, s: Option<&str>) -> JValueOwned {
    match s {
        Some(s) => jstring_owned(env, s),
        None => JValueOwned::Obj(JObject::null()),
    }
}

fn jbytes(env: &mut JNIEnv, b: &[u8]) -> JValueOwned {
    JValueOwned::Obj(env.byte_array_from_slice(b).map(|a| unsafe { JObject::from_raw(a.into_raw()) }).unwrap_or(JObject::null()))
}

/// A String[].
fn jstrings(env: &mut JNIEnv, items: &[String]) -> JValueOwned {
    let arr = match env.new_object_array(items.len() as i32, "java/lang/String", JObject::null()) {
        Ok(a) => a,
        Err(_) => return JValueOwned::Obj(JObject::null()),
    };
    for (i, s) in items.iter().enumerate() {
        if let Ok(js) = env.new_string(s) {
            let _ = env.set_object_array_element(&arr, i as i32, &js);
            let _ = env.delete_local_ref(js);
        }
    }
    JValueOwned::Obj(unsafe { JObject::from_raw(arr.into_raw()) })
}

/// A String[] from Java, keeping nulls in place (the arrays of one call line up by index).
fn strings_opt(env: &mut JNIEnv, arr: &JObjectArray) -> Vec<Option<String>> {
    if arr.is_null() {
        return Vec::new();
    }
    let n = env.get_array_length(arr).unwrap_or(0);
    (0..n)
        .map(|i| {
            let o = env.get_object_array_element(arr, i).ok()?;
            if o.is_null() {
                return None;
            }
            env.get_string(&JString::from(o)).ok().map(|s| s.into())
        })
        .collect()
}

fn dispatch(link: i32, msg: ToApp, payload: Vec<u8>) {
    use JValueOwned::*;
    match msg {
        ToApp::Hello { .. } | ToApp::Challenge { .. } | ToApp::Paired { .. } | ToApp::AuthFailed { .. } => {}
        ToApp::WindowNew { win, title, app_id, icon_png_b64 } => {
            WINDOWS.lock().unwrap().get_or_insert_with(HashMap::new).insert((link, win), Win::default());
            let icon = icon_png_b64.and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok()).unwrap_or_default();
            call("onWindowNew", "(IILjava/lang/String;Ljava/lang/String;[B)V", |env| {
                let arr = env.byte_array_from_slice(&icon).map(|a| unsafe { JObject::from_raw(a.into_raw()) }).unwrap_or(JObject::null());
                vec![Int(link), Int(win as i32), jstring_owned(env, &title), jstring_owned(env, &app_id), Obj(arr)]
            });
        }
        ToApp::WindowTitle { win, title } => {
            call("onWindowTitle", "(IILjava/lang/String;)V", |env| vec![Int(link), Int(win as i32), jstring_owned(env, &title)]);
        }
        ToApp::WindowClose { win } => {
            WINDOWS.lock().unwrap().as_mut().map(|m| m.remove(&(link, win)));
            call("onWindowClose", "(II)V", |_| vec![Int(link), Int(win as i32)]);
        }
        ToApp::Tiles { win, width, height, rects, .. } => {
            let ack = {
                let mut g = WINDOWS.lock().unwrap();
                let w = g.get_or_insert_with(HashMap::new).entry((link, win)).or_default();
                if (w.width, w.height) != (width, height) || w.frame.len() != (width * height) as usize {
                    w.width = width;
                    w.height = height;
                    w.frame = vec![0xff00_0000; (width * height) as usize];
                }
                if let Err(e) = lindroids_server::frames::unpack(&mut w.frame, width, &rects, &payload) {
                    warn!("window {win}: {e}");
                }
                // No Surface right now (the activity is in the background): hold the
                // acknowledgement, which pauses this window until it is visible again.
                let shown = w.present();
                w.unacked = !shown;
                shown
            };
            if ack {
                send(link, &ToServer::FrameDone { win });
            }
        }
        ToApp::Keyboard { win, show } => call("onKeyboard", "(IIZ)V", |_| vec![Int(link), Int(win as i32), Bool(show)]),
        ToApp::Clipboard { text } => call("onClipboard", "(ILjava/lang/String;)V", |env| vec![Int(link), jstring_owned(env, &text)]),
        ToApp::Export(req) => {
            let json = serde_json::to_string(&req).unwrap_or_default();
            call("onExport", "(ILjava/lang/String;)V", |env| vec![Int(link), jstring_owned(env, &json)]);
        }
        ToApp::PickFolder { request } => call("onPickFolder", "(II)V", |_| vec![Int(link), Int(request as i32)]),
        ToApp::Unexport { id } => call("onUnexport", "(ILjava/lang/String;)V", |env| vec![Int(link), jstring_owned(env, &id)]),
        ToApp::Log { level, msg } => info!("server {level}: {msg}"),
        ToApp::AudioOpen { stream, rate, channels, format } => crate::sound::open(link, stream, rate, channels, format),
        ToApp::AudioData { stream, .. } => {
            let n = payload.len() as u32;
            crate::sound::write(link, stream, payload);
            send(link, &ToServer::AudioCredit { stream, bytes: n });
        }
        ToApp::AudioCork { stream, corked } => crate::sound::cork(link, stream, corked),
        ToApp::AudioClose { stream } => crate::sound::close(link, stream),
        ToApp::BundleBegin { id } => {
            let Some(files) = FILES_DIR.get() else { return };
            let dest = crate::bundle::bundle_dir(&files.join("bundles"), &id);
            let mut fds = [0; 2];
            if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
                send(link, &ToServer::ImportResult { id, ok: false, message: "no pipe".into() });
                return;
            }
            let (r, w) = unsafe { (std::fs::File::from_raw_fd(fds[0]), std::fs::File::from_raw_fd(fds[1])) };
            IMPORTS.lock().unwrap().get_or_insert_with(HashMap::new).insert((link, id.clone()), w);
            std::thread::spawn(move || {
                let _ = std::fs::create_dir_all(dest.parent().unwrap());
                let msg = match crate::bundle::import(std::io::BufReader::with_capacity(1 << 20, r), &dest) {
                    Ok(m) => ToServer::ImportResult { id: id.clone(), ok: true, message: format!("{} copied into Lindroids", m.name) },
                    Err(e) => ToServer::ImportResult { id: id.clone(), ok: false, message: e },
                };
                info!("bundle {id}: {msg:?}");
                send(link, &msg);
            });
        }
        ToApp::BundleChunk { id, .. } => {
            let mut g = IMPORTS.lock().unwrap();
            if let Some(f) = g.as_mut().and_then(|m| m.get_mut(&(link, id))) {
                let _ = f.write_all(&payload);
            }
        }
        ToApp::BundleEnd { id, .. } => {
            // Closing the pipe ends the tar stream.
            IMPORTS.lock().unwrap().as_mut().map(|m| m.remove(&(link, id)));
        }

        // --- Android integration
        ToApp::OpenUri { request, uri } => {
            call("onOpenUri", "(IILjava/lang/String;)V", |env| vec![Int(link), Int(request as i32), jstring_owned(env, &uri)]);
        }
        ToApp::OpenFile { request, name, mime, .. } => open_file(link, request, &name, mime, payload),
        ToApp::Notify(n) => {
            let note = with(&NOTES, |ids| ids.assign(link, &n.id));
            let icon = n.icon_png_b64.and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok()).unwrap_or_default();
            // (key, label) pairs, flattened: key0, label0, key1, label1...
            let actions: Vec<String> = n.actions.into_iter().flat_map(|(k, l)| [k, l]).collect();
            let channel = portal::channel_for(n.urgency);
            call(
                "onNotify",
                "(IILjava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;[B[Ljava/lang/String;)V",
                |env| {
                    vec![
                        Int(link),
                        Int(note),
                        jstring_owned(env, channel),
                        jstring_owned(env, &n.title),
                        jstring_owned(env, &n.body),
                        jstring_owned(env, &n.app),
                        jbytes(env, &icon),
                        jstrings(env, &actions),
                    ]
                },
            );
        }
        ToApp::CloseNotification { id } => {
            let note = with(&NOTES, |ids| {
                let n = ids.get(link, &id)?;
                ids.remove(n);
                Some(n)
            });
            if let Some(note) = note {
                call("onCancelNotification", "(I)V", |_| vec![Int(note)]);
                send(link, &ToServer::NotificationClosed { id, reason: 3 });
            }
        }
        ToApp::PickFiles(p) => {
            call("onPickFiles", "(IIZZZ[Ljava/lang/String;Ljava/lang/String;)V", |env| {
                vec![
                    Int(link),
                    Int(p.request as i32),
                    Bool(p.save),
                    Bool(p.multiple),
                    Bool(p.directory),
                    jstrings(env, &p.mime_types),
                    jstring_opt(env, p.name.as_deref()),
                ]
            });
        }
        ToApp::FetchFile { request, index } => fetch_file(link, request, index),
        ToApp::WriteFile { request, uri, offset, last, .. } => write_file(link, request, uri, offset, last, payload),
    }
}

// ------------------------------------------------------------------ Android integration

/// Open a Linux file with an Android app: keep it in the cache, where the app's FileProvider
/// serves it, and let Java offer it to the apps that open its type.
fn open_file(link: i32, request: u32, name: &str, mime: Option<String>, data: Vec<u8>) {
    let Some(cache) = CACHE_DIR.get() else { return };
    // Per link: two servers may both be at request 1.
    let dir = cache.join("open").join(format!("{link}-{request}"));
    let path = dir.join(portal::safe_file_name(name));
    std::thread::spawn(move || {
        let written = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, &data));
        if let Err(e) = written {
            send(link, &ToServer::OpenResult { request, ok: false, message: Some(format!("cannot keep the file: {e}")) });
            return;
        }
        let path = path.to_string_lossy().into_owned();
        call("onOpenFile", "(IILjava/lang/String;Ljava/lang/String;)V", |env| {
            vec![JValueOwned::Int(link), JValueOwned::Int(request as i32), jstring_owned(env, &path), jstring_opt(env, mime.as_deref())]
        });
    });
}

/// Open a picked document (a content URI) through Java's ContentResolver; the fd is ours.
fn open_document(uri: &str, mode: &str) -> Result<std::fs::File, String> {
    let r = call_string("openDocument", "(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;", |env| {
        vec![jstring_owned(env, uri), jstring_owned(env, mode)]
    })
    .unwrap_or_else(|| "!the app could not open it".into());
    match r.strip_prefix('!') {
        Some(why) => Err(why.to_string()),
        None => match r.parse::<i32>() {
            Ok(fd) if fd >= 0 => Ok(unsafe { std::fs::File::from_raw_fd(fd) }),
            _ => Err(format!("bad descriptor {r}")),
        },
    }
}

/// Send a picked document's content as FileChunk payloads, from a thread of its own (a slow
/// provider must not hold up the link's reader).
fn fetch_file(link: i32, request: u32, index: u32) {
    let fail = move |e: String| {
        send(link, &ToServer::FileChunk { request, index, last: true, error: Some(e), len: 0 });
    };
    let Some(uri) = with(&PICKED, |m| m.get(&(link, request)).and_then(|v| v.get(index as usize)).cloned()) else {
        fail(format!("nothing was picked as file {index} of request {request}"));
        return;
    };
    std::thread::Builder::new()
        .name(format!("lindroids-fetch-{link}-{request}"))
        .spawn(move || {
            let f = match open_document(&uri, "r") {
                Ok(f) => f,
                Err(e) => return fail(e),
            };
            let r = portal::stream_chunks(f, portal::CHUNK, |c, last| {
                send_with(link, &ToServer::FileChunk { request, index, last, error: None, len: c.len() as u32 }, c)
            });
            if let Err(e) = r {
                fail(e.to_string());
            }
        })
        .ok();
}

/// Most WriteFile chunks queued for a document's writer thread before the link's reader waits.
const WRITE_QUEUE: usize = 16;

/// A chunk of a write into a picked document. The first chunk of a request starts a thread that
/// opens the document and writes the chunks in order; it answers WriteResult once, after the
/// last chunk or at the first error.
fn write_file(link: i32, request: u32, uri: String, offset: u64, last: bool, data: Vec<u8>) {
    let key = (link, request);
    let chunk = WriteChunk { offset, last, data };
    let state = with(&WRITES, |m| match m.get(&key) {
        None => None,
        Some(WriteState::Active(tx)) => Some(Some(tx.clone())),
        Some(WriteState::Failed) => Some(None),
    });
    let done = |m: &mut HashMap<(i32, u32), WriteState>| {
        if last {
            m.remove(&key);
        }
    };
    match state {
        Some(Some(tx)) => {
            // An error means the writer stopped (and answered): drop the rest.
            let failed = tx.send(chunk).is_err();
            with(&WRITES, |m| {
                if failed && !last {
                    m.insert(key, WriteState::Failed);
                }
                done(m);
            });
        }
        Some(None) => with(&WRITES, done),
        None => {
            // Only documents the user picked on this link: the URI comes from the Linux side,
            // and the app could otherwise be told to write into its own files.
            let picked = with(&PICKED, |m| m.iter().any(|((l, _), v)| *l == link && v.contains(&uri)));
            if !picked {
                send(link, &ToServer::WriteResult { request, ok: false, message: Some("not a document picked on this link".into()) });
                if !last {
                    with(&WRITES, |m| m.insert(key, WriteState::Failed));
                }
                return;
            }
            let (tx, rx) = sync_channel(WRITE_QUEUE);
            let _ = tx.send(chunk);
            if !last {
                with(&WRITES, |m| m.insert(key, WriteState::Active(tx)));
            }
            std::thread::Builder::new().name(format!("lindroids-write-{link}-{request}")).spawn(move || write_document(link, request, &uri, rx)).ok();
        }
    }
}

fn write_document(link: i32, request: u32, uri: &str, rx: Receiver<WriteChunk>) {
    let reply = |ok: bool, message: Option<String>| send(link, &ToServer::WriteResult { request, ok, message });
    let Ok(mut c) = rx.recv() else { return };
    let mode = if c.offset == 0 { "wt" } else { "wa" };
    let mut f = match open_document(uri, mode) {
        Ok(f) => f,
        Err(e) => return reply(false, Some(e)),
    };
    let mut at = c.offset;
    loop {
        if c.offset != at {
            return reply(false, Some(format!("chunk at offset {} after {at} bytes", c.offset)));
        }
        if let Err(e) = f.write_all(&c.data) {
            return reply(false, Some(e.to_string()));
        }
        at += c.data.len() as u64;
        if c.last {
            // Providers often hand out a pipe: the data is theirs once it is closed.
            drop(f);
            return reply(true, None);
        }
        c = match rx.recv() {
            Ok(c) => c,
            // The link closed mid-write.
            Err(_) => return,
        };
    }
}

/// Take a new system state: links that never had one get it now, the others once the change
/// has settled ([`portal::SYSTEM_DEBOUNCE`]) and only if it is worth it ([`portal::worth_sending`]).
fn system_update(state: SystemState) {
    let links = link_ids();
    let mut now = Vec::new();
    let spawn = with(&SYSTEM, |s| {
        s.sent.retain(|l, _| links.contains(l));
        let mut later = false;
        for l in &links {
            match s.sent.get(l) {
                None => {
                    s.sent.insert(*l, state.clone());
                    now.push(*l);
                }
                Some(last) => later |= portal::worth_sending(Some(last), &state),
            }
        }
        s.current = Some(state.clone());
        if !later {
            return false;
        }
        s.deadline = Some(Instant::now() + portal::SYSTEM_DEBOUNCE);
        !std::mem::replace(&mut s.flusher, true)
    });
    for l in now {
        send(l, &ToServer::System(state.clone()));
    }
    if spawn {
        std::thread::Builder::new().name("lindroids-system".into()).spawn(system_flush).ok();
    }
}

/// Wait out the debounce (each change pushes the deadline back), then send the state to the
/// links it differs for.
fn system_flush() {
    loop {
        let wait = with(&SYSTEM, |s| {
            let now = Instant::now();
            match s.deadline {
                Some(d) if d > now => Some(d - now),
                _ => {
                    s.deadline = None;
                    s.flusher = false;
                    None
                }
            }
        });
        match wait {
            Some(w) => std::thread::sleep(w),
            None => break,
        }
    }
    let links = link_ids();
    let out: Vec<(i32, SystemState)> = with(&SYSTEM, |s| {
        let Some(cur) = s.current.clone() else { return Vec::new() };
        let due: Vec<i32> =
            links.iter().copied().filter(|l| portal::worth_sending(s.sent.get(l), &cur)).collect();
        due.into_iter()
            .map(|l| {
                s.sent.insert(l, cur.clone());
                (l, cur.clone())
            })
            .collect()
    });
    for (l, st) in out {
        send(l, &ToServer::System(st));
    }
}

/// What `SystemMonitor.java` read from Android. Sent on to every link (see [`system_update`]).
#[no_mangle]
pub extern "system" fn Java_io_github_dixonsolutions_lindroids_Native_systemState(
    mut env: JNIEnv,
    _: JClass,
    dark: jboolean,
    has_accent: jboolean,
    accent: jint,
    high_contrast: jboolean,
    reduced_motion: jboolean,
    font_scale: jfloat,
    power_saver: jboolean,
    net_available: jboolean,
    net_internet: jboolean,
    net_validated: jboolean,
    net_captive: jboolean,
    net_metered: jboolean,
    mem_total: jlong,
    mem_available: jlong,
    mem_low: jboolean,
    battery_level: jint,
    charging: jboolean,
    locale: JString,
    timezone: JString,
) {
    let available = net_available != 0;
    let state = SystemState {
        dark: dark != 0,
        accent: (has_accent != 0).then(|| portal::accent_from_argb(accent)),
        high_contrast: high_contrast != 0,
        reduced_motion: reduced_motion != 0,
        font_scale,
        power_saver: power_saver != 0,
        network: NetworkState {
            available,
            metered: available && net_metered != 0,
            connectivity: portal::connectivity(available, net_internet != 0, net_validated != 0, net_captive != 0),
        },
        memory: MemoryState { total: mem_total.max(0) as u64, available: mem_available.max(0) as u64, low: mem_low != 0 },
        battery: (battery_level >= 0).then(|| BatteryState { level: battery_level.min(100) as u8, charging: charging != 0 }),
        locale: jstr(&mut env, &locale),
        timezone: jstr(&mut env, &timezone),
    };
    system_update(state);
}

/// ComponentCallbacks2.onTrimMemory, for every link.
#[no_mangle]
pub extern "system" fn Java_io_github_dixonsolutions_lindroids_Native_memoryWarning(_: JNIEnv, _: JClass, level: jint) {
    let Some(level) = portal::trim_level(level) else { return };
    for l in link_ids() {
        send(l, &ToServer::MemoryWarning { level });
    }
}

#[no_mangle]
pub extern "system" fn Java_io_github_dixonsolutions_lindroids_Native_linkOpenResult(mut env: JNIEnv, _: JClass, link: jint, request: jint, ok: jboolean, message: JString) {
    let message = opt_jstr(&mut env, &message);
    send(link, &ToServer::OpenResult { request: request as u32, ok: ok != 0, message });
}

/// The notification was tapped (`action` "default") or one of its buttons pressed. It is gone
/// afterwards (the freedesktop default for notifications that are not resident), so that is
/// reported too. Returns the link it belongs to, or -1.
#[no_mangle]
pub extern "system" fn Java_io_github_dixonsolutions_lindroids_Native_notificationAction(mut env: JNIEnv, _: JClass, note: jint, action: JString) -> jint {
    let action = jstr(&mut env, &action);
    let Some((link, id)) = with(&NOTES, |n| n.remove(note)) else { return -1 };
    send(link, &ToServer::NotificationAction { id: id.clone(), action });
    send(link, &ToServer::NotificationClosed { id, reason: 2 });
    link
}

/// The user swiped the notification away.
#[no_mangle]
pub extern "system" fn Java_io_github_dixonsolutions_lindroids_Native_notificationDismissed(_: JNIEnv, _: JClass, note: jint) {
    if let Some((link, id)) = with(&NOTES, |n| n.remove(note)) {
        send(link, &ToServer::NotificationClosed { id, reason: 2 });
    }
}

/// The picker's answer: one entry per document in every array (null where unknown). An empty
/// set means the user cancelled.
#[no_mangle]
pub extern "system" fn Java_io_github_dixonsolutions_lindroids_Native_linkFilesPicked(
    mut env: JNIEnv,
    _: JClass,
    link: jint,
    request: jint,
    uris: JObjectArray,
    names: JObjectArray,
    sizes: JLongArray,
    mimes: JObjectArray,
    authorities: JObjectArray,
    doc_ids: JObjectArray,
    primary_root: JString,
) {
    let uris = strings_opt(&mut env, &uris);
    let names = strings_opt(&mut env, &names);
    let mimes = strings_opt(&mut env, &mimes);
    let authorities = strings_opt(&mut env, &authorities);
    let doc_ids = strings_opt(&mut env, &doc_ids);
    let root = jstr(&mut env, &primary_root);
    let mut size = vec![0 as jlong; uris.len()];
    if !sizes.is_null() && env.get_array_length(&sizes).unwrap_or(0) as usize == size.len() {
        let _ = env.get_long_array_region(&sizes, 0, &mut size);
    }
    let at = |v: &Vec<Option<String>>, i: usize| v.get(i).cloned().flatten();
    let files: Vec<PickedFile> = (0..uris.len())
        .filter_map(|i| {
            let uri = at(&uris, i)?;
            let path = match (at(&authorities, i), at(&doc_ids, i)) {
                (Some(a), Some(d)) => portal::document_path(&a, &d, &root),
                _ => None,
            };
            let name = at(&names, i).unwrap_or_else(|| path.as_deref().and_then(|p| p.rsplit('/').next()).unwrap_or("document").to_string());
            Some(PickedFile { uri, name, path, size: size[i].max(0) as u64, mime: at(&mimes, i) })
        })
        .collect();
    let request = request as u32;
    with(&PICKED, |m| m.insert((link, request), files.iter().map(|f| f.uri.clone()).collect()));
    send(link, &ToServer::FilesPicked { request, files });
}

/// The window's Surface appeared (non-null) or went away (null).
#[no_mangle]
pub extern "system" fn Java_io_github_dixonsolutions_lindroids_Native_windowSurface(env: JNIEnv, _: JClass, link: jint, win: jint, surface: JObject) {
    let nw = if surface.is_null() {
        None
    } else {
        let p = unsafe { ndk_sys::ANativeWindow_fromSurface(env.get_raw() as *mut _, surface.as_raw() as *mut _) };
        (!p.is_null()).then_some(NativeWindow { ptr: p, geometry: (0, 0) })
    };
    let ack = {
        let mut g = WINDOWS.lock().unwrap();
        let w = g.get_or_insert_with(HashMap::new).entry((link, win as u32)).or_default();
        w.surface = nw;
        if w.surface.is_some() {
            w.present();
            std::mem::take(&mut w.unacked)
        } else {
            false
        }
    };
    if ack {
        send(link, &ToServer::FrameDone { win: win as u32 });
    }
}

#[no_mangle]
pub extern "system" fn Java_io_github_dixonsolutions_lindroids_Native_linkResize(_: JNIEnv, _: JClass, link: jint, win: jint, w: jint, h: jint, density: jfloat) {
    send(link, &ToServer::Resize { win: win as u32, width: w as u32, height: h as u32, density });
}

#[no_mangle]
pub extern "system" fn Java_io_github_dixonsolutions_lindroids_Native_linkTouch(_: JNIEnv, _: JClass, link: jint, win: jint, action: jint, id: jint, x: jfloat, y: jfloat, t: jint) {
    let action = match action {
        0 => TouchAction::Down,
        1 => TouchAction::Move,
        2 => TouchAction::Up,
        _ => TouchAction::Cancel,
    };
    send(link, &ToServer::Touch { win: win as u32, action, id, x, y, time_ms: t as u32 });
}

#[no_mangle]
pub extern "system" fn Java_io_github_dixonsolutions_lindroids_Native_linkPointer(_: JNIEnv, _: JClass, link: jint, win: jint, action: jint, x: jfloat, y: jfloat, button: jint, t: jint) {
    let action = match action {
        0 => PointerAction::Enter,
        1 => PointerAction::Leave,
        2 => PointerAction::Motion,
        3 => PointerAction::Press,
        _ => PointerAction::Release,
    };
    send(link, &ToServer::Pointer { win: win as u32, action, x, y, button: button as u32, time_ms: t as u32 });
}

#[no_mangle]
pub extern "system" fn Java_io_github_dixonsolutions_lindroids_Native_linkScroll(_: JNIEnv, _: JClass, link: jint, win: jint, dx: jfloat, dy: jfloat, t: jint) {
    send(link, &ToServer::Scroll { win: win as u32, dx, dy, time_ms: t as u32 });
}

#[no_mangle]
pub extern "system" fn Java_io_github_dixonsolutions_lindroids_Native_linkKey(_: JNIEnv, _: JClass, link: jint, win: jint, code: jint, down: jboolean, t: jint) {
    send(link, &ToServer::Key { win: win as u32, code: code as u32, down: down != 0, time_ms: t as u32 });
}

#[no_mangle]
pub extern "system" fn Java_io_github_dixonsolutions_lindroids_Native_linkText(mut env: JNIEnv, _: JClass, link: jint, win: jint, text: JString) {
    let text = jstr(&mut env, &text);
    send(link, &ToServer::Text { win: win as u32, text });
}

#[no_mangle]
pub extern "system" fn Java_io_github_dixonsolutions_lindroids_Native_linkFocus(_: JNIEnv, _: JClass, link: jint, win: jint, focused: jboolean) {
    send(link, &ToServer::Focus { win: win as u32, focused: focused != 0 });
}

#[no_mangle]
pub extern "system" fn Java_io_github_dixonsolutions_lindroids_Native_linkClose(_: JNIEnv, _: JClass, link: jint, win: jint) {
    send(link, &ToServer::Close { win: win as u32 });
}

#[no_mangle]
pub extern "system" fn Java_io_github_dixonsolutions_lindroids_Native_linkClipboard(mut env: JNIEnv, _: JClass, link: jint, text: JString) {
    let text = jstr(&mut env, &text);
    send(link, &ToServer::Clipboard { text });
}

#[no_mangle]
pub extern "system" fn Java_io_github_dixonsolutions_lindroids_Native_linkFolderPicked(mut env: JNIEnv, _: JClass, link: jint, request: jint, path: JString) {
    let path = opt_jstr(&mut env, &path);
    send(link, &ToServer::FolderPicked { request: request as u32, path });
}

#[no_mangle]
pub extern "system" fn Java_io_github_dixonsolutions_lindroids_Native_linkExportResult(mut env: JNIEnv, _: JClass, link: jint, id: JString, ok: jboolean, message: JString) {
    let id = jstr(&mut env, &id);
    let message = jstr(&mut env, &message);
    send(link, &ToServer::ExportResult { id, ok: ok != 0, message });
}

#[no_mangle]
pub extern "system" fn Java_io_github_dixonsolutions_lindroids_Native_linkDisconnect(_: JNIEnv, _: JClass, link: jint) {
    if let Some(s) = LINKS.lock().unwrap().as_mut().and_then(|m| m.remove(&link)) {
        s.ctl.shutdown();
    }
}

#[no_mangle]
pub extern "system" fn Java_io_github_dixonsolutions_lindroids_Native_bundleSpawn(
    mut env: JNIEnv,
    _: JClass,
    lib_dir: JString,
    bundle: JString,
    runtime: JString,
    argv: JObjectArray,
    envs: JObjectArray,
    log_path: JString,
) -> jint {
    let lib_dir = jstr(&mut env, &lib_dir);
    let bundle = jstr(&mut env, &bundle);
    let runtime = jstr(&mut env, &runtime);
    let log_path = jstr(&mut env, &log_path);
    let argv = strings(&mut env, &argv);
    let extra: Vec<(String, String)> = strings(&mut env, &envs)
        .into_iter()
        .filter_map(|kv| kv.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect();
    let paths = crate::bundle::SpawnPaths {
        native_lib_dir: Path::new(&lib_dir),
        bundle: Path::new(&bundle),
        runtime: Path::new(&runtime),
        log: Path::new(&log_path),
    };
    match crate::bundle::spawn(&paths, &argv, &extra) {
        Ok(pid) => pid as jint,
        Err(e) => {
            error!("bundle {bundle}: {e}");
            -1
        }
    }
}
