//! JNI entry points (see `Native.java`): the app's end of each link, and the in-app server.
//!
//! A link is one connection to a server (one sandbox). The server sends the tiles of each frame
//! that changed; this side keeps every window's last frame, applies the tiles, copies the frame
//! into the window's Surface (ANativeWindow) and acknowledges it. Input goes back the same way,
//! sound streams go to `sound`, bundles to `bundle`.

use std::collections::HashMap;
use std::ffi::c_void;
use std::io::{Read, Write};
use std::os::fd::FromRawFd;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use base64::Engine;
use jni::objects::{GlobalRef, JClass, JObject, JObjectArray, JString, JValue};
use jni::sys::{jboolean, jfloat, jint, jstring, JNI_VERSION_1_6};
use jni::{JNIEnv, JavaVM};
use lindroids_proto::{self as proto, Framer, PointerAction, ToApp, ToServer, TouchAction};
use lindroids_server::applink::{self, Conn, Outcome};
use log::{error, info, warn};

pub(crate) static VM: OnceLock<JavaVM> = OnceLock::new();
static LINK_SERVICE: OnceLock<GlobalRef> = OnceLock::new();
static LINKS: Mutex<Option<HashMap<i32, Conn>>> = Mutex::new(None);
static WINDOWS: Mutex<Option<HashMap<(i32, u32), Win>>> = Mutex::new(None);
/// Bundles being received: the write end of the pipe feeding the unpacking thread.
static IMPORTS: Mutex<Option<HashMap<(i32, String), std::fs::File>>> = Mutex::new(None);
static FILES_DIR: OnceLock<PathBuf> = OnceLock::new();
static DENSITY: Mutex<f32> = Mutex::new(1.0);

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
pub extern "system" fn Java_io_github_dixonsolutions_lindroids_Native_appInit(mut env: JNIEnv, _: JClass, files: JString, density: jfloat) {
    let files = jstr(&mut env, &files);
    let _ = FILES_DIR.set(PathBuf::from(files));
    *DENSITY.lock().unwrap() = density;
    if LINK_SERVICE.get().is_none() {
        if let Ok(cls) = env.find_class("io/github/dixonsolutions/lindroids/LinkService") {
            if let Ok(g) = env.new_global_ref(cls) {
                let _ = LINK_SERVICE.set(g);
            }
        }
    }
}

fn send(link: i32, msg: &ToServer) {
    let mut g = LINKS.lock().unwrap();
    if let Some(s) = g.as_mut().and_then(|m| m.get_mut(&link)) {
        if let Err(e) = s.write_all(&proto::encode(msg)) {
            warn!("link {link}: {e}");
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
    let density = *DENSITY.lock().unwrap();
    let r = match applink::connect(uid as u32, socket.as_deref(), secret.as_deref(), code.as_deref(), density, crate::BUILD) {
        Outcome::Linked { conn, framer, paired, sandbox, backlog } => {
            let kind = conn.kind();
            if link >= 0 {
                match conn.try_clone() {
                    Ok(w) => {
                        LINKS.lock().unwrap().get_or_insert_with(HashMap::new).insert(link, w);
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
            call("onLinkClosed", "(I)V", |_| vec![JValueOwned::Int(link)]);
        })
        .ok();
}

/// Call a static void method on LinkService from any thread.
fn call(name: &str, sig: &str, args: impl FnOnce(&mut JNIEnv) -> Vec<JValueOwned>) {
    let Some(vm) = VM.get() else { return };
    let Some(cls) = LINK_SERVICE.get() else { return };
    let Ok(mut env) = vm.attach_current_thread_permanently() else { return };
    let owned = args(&mut env);
    let vals: Vec<JValue> = owned.iter().map(|v| v.borrow()).collect();
    let class: &JClass = cls.as_obj().into();
    if let Err(e) = env.call_static_method(class, name, sig, &vals) {
        error!("LinkService.{name}: {e}");
        let _ = env.exception_describe();
        let _ = env.exception_clear();
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
    }
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
        s.shutdown();
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
