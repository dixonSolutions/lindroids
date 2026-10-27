//! wl_seat (pointer, keyboard, touch) and text-input-v3, and the routing of Android input into them.

use std::collections::HashMap;
use std::io::Write;
use std::os::fd::{AsFd, FromRawFd, OwnedFd};
use std::sync::Mutex;

use lindroids_proto::{PointerAction, ToApp, TouchAction};
use wayland_protocols::wp::text_input::zv3::server::{zwp_text_input_manager_v3, zwp_text_input_v3};
use wayland_server::backend::{ClientId, ObjectId};
use wayland_server::protocol::{wl_keyboard, wl_pointer, wl_seat, wl_surface, wl_touch};
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};

use crate::keys;

use super::surface::Role;
use super::{shell, State};

const BTN_LEFT: u32 = 0x110;

#[derive(Default)]
pub struct Seat {
    pub pointers: Vec<wl_pointer::WlPointer>,
    pub keyboards: Vec<wl_keyboard::WlKeyboard>,
    pub touches: Vec<wl_touch::WlTouch>,
    pub text_inputs: Vec<zwp_text_input_v3::ZwpTextInputV3>,
    /// The Linux window whose Android window has focus.
    pub focus_win: Option<u32>,
    pub(crate) kb_focus: Option<wl_surface::WlSurface>,
    pub ptr_focus: Option<wl_surface::WlSurface>,
    /// Touch point -> (surface, surface origin in window logical coordinates).
    touch_targets: HashMap<i32, (wl_surface::WlSurface, f64, f64)>,
    /// Touch point driving the emulated pointer, for clients without wl_touch.
    emulating: Option<i32>,
    mods_depressed: u32,
    mods_locked: u32,
}

#[derive(Default)]
pub struct TextInputState {
    enabled: bool,
    pending: Option<bool>,
    commits: u32,
}

pub fn register_globals(dh: &DisplayHandle) {
    dh.create_global::<State, wl_seat::WlSeat, ()>(7, ());
    dh.create_global::<State, zwp_text_input_manager_v3::ZwpTextInputManagerV3, ()>(1, ());
}

fn same_client<R: Resource>(r: &R, s: &wl_surface::WlSurface) -> bool {
    r.id().same_client_as(&s.id())
}

fn force_pointer() -> bool {
    std::env::var("LINDROIDS_TOUCH").map_or(false, |v| v == "pointer")
}

/// A sealed-ish file holding the keymap, for wl_keyboard.keymap.
fn keymap_fd() -> Option<(OwnedFd, u32)> {
    let text = keys::KEYMAP_US.as_bytes();
    let size = text.len() + 1;
    let fd = unsafe {
        let name = b"lindroids-keymap\0";
        let fd = libc::syscall(libc::SYS_memfd_create, name.as_ptr(), libc::MFD_CLOEXEC as libc::c_uint) as i32;
        if fd >= 0 {
            Some(OwnedFd::from_raw_fd(fd))
        } else {
            None
        }
    };
    let fd = match fd {
        Some(fd) => fd,
        None => {
            // Kernels without memfd (old Android 7 devices): an unlinked temporary file.
            let dir = std::env::var_os("LINDROIDS_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(super::default_runtime_dir);
            let path = dir.join(format!("keymap-{}", std::process::id()));
            let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&path).ok()?;
            let _ = std::fs::remove_file(&path);
            OwnedFd::from(f)
        }
    };
    let mut f = std::fs::File::from(fd);
    f.write_all(text).ok()?;
    f.write_all(&[0]).ok()?;
    Some((OwnedFd::from(f), size as u32))
}

// ------------------------------------------------------------------ globals

impl GlobalDispatch<wl_seat::WlSeat, ()> for State {
    fn bind(_: &mut State, _: &DisplayHandle, _: &Client, r: New<wl_seat::WlSeat>, _: &(), di: &mut DataInit<'_, State>) {
        let seat = di.init(r, ());
        seat.capabilities(wl_seat::Capability::Pointer | wl_seat::Capability::Keyboard | wl_seat::Capability::Touch);
        if seat.version() >= 2 {
            seat.name("lindroids".into());
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn request(state: &mut State, _: &Client, _: &wl_seat::WlSeat, request: wl_seat::Request, _: &(), _: &DisplayHandle, di: &mut DataInit<'_, State>) {
        match request {
            wl_seat::Request::GetPointer { id } => {
                let p = di.init(id, ());
                state.seat.pointers.push(p);
            }
            wl_seat::Request::GetKeyboard { id } => {
                let k = di.init(id, ());
                if let Some((fd, size)) = keymap_fd() {
                    k.keymap(wl_keyboard::KeymapFormat::XkbV1, fd.as_fd(), size);
                }
                if k.version() >= 4 {
                    k.repeat_info(30, 400);
                }
                // A keyboard bound by the focused client must learn about the focus.
                if let Some(s) = state.seat.kb_focus.clone() {
                    if same_client(&k, &s) {
                        let serial = state.next_serial();
                        k.enter(serial, &s, Vec::new());
                        k.modifiers(serial, state.seat.mods_depressed, 0, state.seat.mods_locked, 0);
                    }
                }
                state.seat.keyboards.push(k);
            }
            wl_seat::Request::GetTouch { id } => {
                let t = di.init(id, ());
                state.seat.touches.push(t);
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn request(state: &mut State, _: &Client, p: &wl_pointer::WlPointer, request: wl_pointer::Request, _: &(), _: &DisplayHandle, _: &mut DataInit<'_, State>) {
        if let wl_pointer::Request::SetCursor { surface, .. } = request {
            // Only the client the pointer is over may set it.
            if !state.seat.ptr_focus.as_ref().map_or(false, |s| same_client(p, s)) {
                return;
            }
            match surface {
                // A picture of its own: the page shows its default arrow (cursor surfaces are
                // accepted and never drawn).
                Some(s) => {
                    if let Some(cs) = state.surfaces.get_mut(&s.id()) {
                        if cs.role == Role::None {
                            cs.role = Role::Cursor;
                        }
                    }
                    super::extras::set_cursor(state, "default");
                }
                None => super::extras::set_cursor(state, "none"),
            }
        }
    }

    fn destroyed(state: &mut State, _: ClientId, r: &wl_pointer::WlPointer, _: &()) {
        state.seat.pointers.retain(|p| p != r);
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn request(_: &mut State, _: &Client, _: &wl_keyboard::WlKeyboard, _: wl_keyboard::Request, _: &(), _: &DisplayHandle, _: &mut DataInit<'_, State>) {}

    fn destroyed(state: &mut State, _: ClientId, r: &wl_keyboard::WlKeyboard, _: &()) {
        state.seat.keyboards.retain(|k| k != r);
    }
}

impl Dispatch<wl_touch::WlTouch, ()> for State {
    fn request(_: &mut State, _: &Client, _: &wl_touch::WlTouch, _: wl_touch::Request, _: &(), _: &DisplayHandle, _: &mut DataInit<'_, State>) {}

    fn destroyed(state: &mut State, _: ClientId, r: &wl_touch::WlTouch, _: &()) {
        state.seat.touches.retain(|t| t != r);
    }
}

impl GlobalDispatch<zwp_text_input_manager_v3::ZwpTextInputManagerV3, ()> for State {
    fn bind(_: &mut State, _: &DisplayHandle, _: &Client, r: New<zwp_text_input_manager_v3::ZwpTextInputManagerV3>, _: &(), di: &mut DataInit<'_, State>) {
        di.init(r, ());
    }
}

impl Dispatch<zwp_text_input_manager_v3::ZwpTextInputManagerV3, ()> for State {
    fn request(
        state: &mut State,
        _: &Client,
        _: &zwp_text_input_manager_v3::ZwpTextInputManagerV3,
        request: zwp_text_input_manager_v3::Request,
        _: &(),
        _: &DisplayHandle,
        di: &mut DataInit<'_, State>,
    ) {
        if let zwp_text_input_manager_v3::Request::GetTextInput { id, .. } = request {
            let ti = di.init(id, Mutex::new(TextInputState::default()));
            if let Some(s) = state.seat.kb_focus.clone() {
                if same_client(&ti, &s) {
                    ti.enter(&s);
                }
            }
            state.seat.text_inputs.push(ti);
        }
    }
}

impl Dispatch<zwp_text_input_v3::ZwpTextInputV3, Mutex<TextInputState>> for State {
    fn request(
        state: &mut State,
        _: &Client,
        res: &zwp_text_input_v3::ZwpTextInputV3,
        request: zwp_text_input_v3::Request,
        data: &Mutex<TextInputState>,
        _: &DisplayHandle,
        _: &mut DataInit<'_, State>,
    ) {
        let mut st = data.lock().unwrap();
        match request {
            zwp_text_input_v3::Request::Enable => st.pending = Some(true),
            zwp_text_input_v3::Request::Disable => st.pending = Some(false),
            zwp_text_input_v3::Request::Commit => {
                st.commits = st.commits.wrapping_add(1);
                if let Some(en) = st.pending.take() {
                    if en != st.enabled {
                        st.enabled = en;
                        drop(st);
                        let focused = state.seat.kb_focus.as_ref().map_or(false, |s| same_client(res, s));
                        if focused {
                            if let Some(win) = state.seat.focus_win {
                                state.send_win(win, &ToApp::Keyboard { win, show: en });
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn destroyed(state: &mut State, _: ClientId, r: &zwp_text_input_v3::ZwpTextInputV3, _: &Mutex<TextInputState>) {
        state.seat.text_inputs.retain(|t| t != r);
    }
}

// ------------------------------------------------------------------ focus

pub fn surface_destroyed(state: &mut State, id: &ObjectId) {
    let seat = &mut state.seat;
    if seat.kb_focus.as_ref().map_or(false, |s| &s.id() == id) {
        seat.kb_focus = None;
    }
    if seat.ptr_focus.as_ref().map_or(false, |s| &s.id() == id) {
        seat.ptr_focus = None;
    }
    seat.touch_targets.retain(|_, (s, _, _)| &s.id() != id);
}

fn set_keyboard_focus(state: &mut State, surface: Option<wl_surface::WlSurface>) {
    if state.seat.kb_focus == surface {
        return;
    }
    if let Some(old) = state.seat.kb_focus.take() {
        let serial = state.next_serial();
        for k in state.seat.keyboards.iter().filter(|k| same_client(*k, &old)) {
            k.leave(serial, &old);
        }
        for t in state.seat.text_inputs.iter().filter(|t| same_client(*t, &old)) {
            t.leave(&old);
        }
    }
    if let Some(new) = &surface {
        super::data::focus_entering(state, new);
        super::extras::focus_entering(state, new);
        let serial = state.next_serial();
        for k in state.seat.keyboards.iter().filter(|k| same_client(*k, new)) {
            k.enter(serial, new, Vec::new());
            k.modifiers(serial, state.seat.mods_depressed, 0, state.seat.mods_locked, 0);
        }
        for t in state.seat.text_inputs.iter().filter(|t| same_client(*t, new)) {
            t.enter(new);
        }
    }
    state.seat.kb_focus = surface;
}

pub fn window_focus(state: &mut State, win: u32, focused: bool) {
    if focused {
        state.seat.focus_win = Some(win);
        let surface = state.windows.get(&win).and_then(|w| state.surfaces.get(&w.surface)).map(|s| s.res.clone());
        set_keyboard_focus(state, surface);
    } else if state.seat.focus_win == Some(win) {
        state.seat.focus_win = None;
        set_keyboard_focus(state, None);
    }
    if let Some(w) = state.windows.get_mut(&win) {
        if w.activated != focused {
            w.activated = focused;
            state.configure_window(win, true);
        }
    }
}

// ------------------------------------------------------------------ pointer

fn to_logical(state: &State, win: u32, x: f32, y: f32) -> (f64, f64) {
    let (fit, (ox, oy)) = state.windows.get(&win).map_or((1.0, (0, 0)), |w| (w.fit, w.fit_offset));
    let k = state.scale as f64 * fit as f64;
    ((x as f64 - ox as f64) / k, (y as f64 - oy as f64) / k)
}

fn pointer_motion(state: &mut State, win: u32, lx: f64, ly: f64, time: u32) {
    let Some((sid, sx, sy)) = shell::surface_at(state, win, lx, ly) else { return };
    let Some(surface) = state.surfaces.get(&sid).map(|s| s.res.clone()) else { return };
    if state.seat.ptr_focus.as_ref() != Some(&surface) {
        if let Some(old) = state.seat.ptr_focus.take() {
            let serial = state.next_serial();
            for p in state.seat.pointers.iter().filter(|p| same_client(*p, &old)) {
                p.leave(serial, &old);
                if p.version() >= 5 {
                    p.frame();
                }
            }
        }
        let serial = state.next_serial();
        for p in state.seat.pointers.iter().filter(|p| same_client(*p, &surface)) {
            p.enter(serial, &surface, sx, sy);
        }
        state.seat.ptr_focus = Some(surface.clone());
    }
    for p in state.seat.pointers.iter().filter(|p| same_client(*p, &surface)) {
        p.motion(time, sx, sy);
        if p.version() >= 5 {
            p.frame();
        }
    }
}

fn pointer_button(state: &mut State, button: u32, pressed: bool, time: u32) {
    let Some(surface) = state.seat.ptr_focus.clone() else { return };
    let serial = state.next_serial();
    let st = if pressed { wl_pointer::ButtonState::Pressed } else { wl_pointer::ButtonState::Released };
    for p in state.seat.pointers.iter().filter(|p| same_client(*p, &surface)) {
        p.button(serial, time, button, st);
        if p.version() >= 5 {
            p.frame();
        }
    }
}

pub fn pointer(state: &mut State, win: u32, action: PointerAction, x: f32, y: f32, button: u32, time: u32) {
    let (lx, ly) = to_logical(state, win, x, y);
    match action {
        PointerAction::Enter | PointerAction::Motion => pointer_motion(state, win, lx, ly, time),
        PointerAction::Leave => {
            if let Some(old) = state.seat.ptr_focus.take() {
                let serial = state.next_serial();
                for p in state.seat.pointers.iter().filter(|p| same_client(*p, &old)) {
                    p.leave(serial, &old);
                }
            }
        }
        PointerAction::Press | PointerAction::Release => {
            let pressed = action == PointerAction::Press;
            if pressed && !shell::in_popup(state, win, lx, ly) {
                shell::dismiss_popups(state, win);
            }
            pointer_motion(state, win, lx, ly, time);
            let code = match button {
                1 => BTN_LEFT,
                2 => 0x111, // right
                3 => 0x112, // middle
                4 => 0x113, // side (back)
                5 => 0x114, // extra (forward)
                _ => return,
            };
            pointer_button(state, code, pressed, time);
        }
    }
}

pub fn scroll(state: &mut State, _win: u32, dx: f32, dy: f32, time: u32) {
    let Some(surface) = state.seat.ptr_focus.clone() else { return };
    for p in state.seat.pointers.iter().filter(|p| same_client(*p, &surface)) {
        if p.version() >= 5 {
            p.axis_source(wl_pointer::AxisSource::Wheel);
        }
        for (axis, v) in [(wl_pointer::Axis::VerticalScroll, dy), (wl_pointer::Axis::HorizontalScroll, dx)] {
            if v == 0.0 {
                continue;
            }
            if p.version() >= 5 {
                p.axis_discrete(axis, v.round() as i32);
            }
            p.axis(time, axis, v as f64 * 15.0);
        }
        if p.version() >= 5 {
            p.frame();
        }
    }
}

// ------------------------------------------------------------------ touch

pub fn touch(state: &mut State, win: u32, action: TouchAction, id: i32, x: f32, y: f32, time: u32) {
    let (lx, ly) = to_logical(state, win, x, y);
    if action == TouchAction::Down && !shell::in_popup(state, win, lx, ly) {
        shell::dismiss_popups(state, win);
    }
    // Pick the target at touch-down; it keeps the whole gesture.
    let target = match action {
        TouchAction::Down => {
            let Some((sid, sx, sy)) = shell::surface_at(state, win, lx, ly) else { return };
            let Some(surface) = state.surfaces.get(&sid).map(|s| s.res.clone()) else { return };
            let origin = (lx - sx, ly - sy);
            state.seat.touch_targets.insert(id, (surface.clone(), origin.0, origin.1));
            Some((surface, origin))
        }
        TouchAction::Cancel => None,
        _ => state.seat.touch_targets.get(&id).map(|(s, ox, oy)| (s.clone(), (*ox, *oy))),
    };
    let wants_touch = |state: &State, s: &wl_surface::WlSurface| {
        !force_pointer() && state.seat.touches.iter().any(|t| same_client(t, s))
    };

    if action == TouchAction::Cancel {
        for t in &state.seat.touches {
            t.cancel();
        }
        state.seat.touch_targets.clear();
        if state.seat.emulating.take().is_some() {
            pointer_button(state, BTN_LEFT, false, time);
        }
        return;
    }
    let Some((surface, (ox, oy))) = target else { return };

    if wants_touch(state, &surface) {
        let (sx, sy) = (lx - ox, ly - oy);
        match action {
            TouchAction::Down => {
                let serial = state.next_serial();
                for t in state.seat.touches.iter().filter(|t| same_client(*t, &surface)) {
                    t.down(serial, time, &surface, id, sx, sy);
                    t.frame();
                }
            }
            TouchAction::Move => {
                for t in state.seat.touches.iter().filter(|t| same_client(*t, &surface)) {
                    t.motion(time, id, sx, sy);
                    t.frame();
                }
            }
            TouchAction::Up => {
                let serial = state.next_serial();
                for t in state.seat.touches.iter().filter(|t| same_client(*t, &surface)) {
                    t.up(serial, time, id);
                    t.frame();
                }
                state.seat.touch_targets.remove(&id);
            }
            TouchAction::Cancel => {}
        }
        return;
    }

    // No wl_touch on this client: the first finger drives a left-button pointer.
    match action {
        TouchAction::Down if state.seat.emulating.is_none() => {
            state.seat.emulating = Some(id);
            pointer_motion(state, win, lx, ly, time);
            pointer_button(state, BTN_LEFT, true, time);
        }
        TouchAction::Move if state.seat.emulating == Some(id) => pointer_motion(state, win, lx, ly, time),
        TouchAction::Up if state.seat.emulating == Some(id) => {
            pointer_motion(state, win, lx, ly, time);
            pointer_button(state, BTN_LEFT, false, time);
            state.seat.emulating = None;
            state.seat.touch_targets.remove(&id);
        }
        _ => {}
    }
}

// ------------------------------------------------------------------ keyboard

fn send_key(state: &mut State, code: u32, down: bool, time: u32) {
    let Some(surface) = state.seat.kb_focus.clone() else { return };
    let m = keys::modifier_of(code);
    let mut mods_changed = false;
    if m != 0 {
        if down {
            state.seat.mods_depressed |= m;
        } else {
            state.seat.mods_depressed &= !m;
        }
        mods_changed = true;
    }
    if code == keys::KEY_CAPSLOCK && down {
        state.seat.mods_locked ^= keys::MOD_LOCK;
        mods_changed = true;
    }
    let serial = state.next_serial();
    let st = if down { wl_keyboard::KeyState::Pressed } else { wl_keyboard::KeyState::Released };
    for k in state.seat.keyboards.iter().filter(|k| same_client(*k, &surface)) {
        k.key(serial, time, code, st);
        if mods_changed {
            k.modifiers(serial, state.seat.mods_depressed, 0, state.seat.mods_locked, 0);
        }
    }
}

pub fn key(state: &mut State, win: u32, android_code: u32, down: bool, time: u32) {
    if state.seat.focus_win != Some(win) {
        window_focus(state, win, true);
    }
    if let Some(code) = keys::android_to_evdev(android_code as i32) {
        send_key(state, code, down, time);
    }
}

pub fn text(state: &mut State, win: u32, text: &str) {
    if state.seat.focus_win != Some(win) {
        window_focus(state, win, true);
    }
    let Some(surface) = state.seat.kb_focus.clone() else { return };
    // A text field that speaks text-input-v3 takes the string as is (any script, emoji...).
    let ti = state.seat.text_inputs.iter().find(|t| {
        same_client(*t, &surface) && t.data::<Mutex<TextInputState>>().map_or(false, |d| d.lock().unwrap().enabled)
    });
    if let Some(ti) = ti {
        let commits = ti.data::<Mutex<TextInputState>>().map_or(0, |d| d.lock().unwrap().commits);
        ti.commit_string(Some(text.to_string()));
        ti.done(commits);
        return;
    }
    // Otherwise type it on the US layout, one key at a time.
    let time = state.now_ms();
    for c in text.chars() {
        let Some((code, shift)) = keys::char_to_key(c) else { continue };
        let had_shift = state.seat.mods_depressed & keys::MOD_SHIFT != 0;
        if shift && !had_shift {
            send_key(state, keys::KEY_LEFTSHIFT, true, time);
        }
        send_key(state, code, true, time);
        send_key(state, code, false, time);
        if shift && !had_shift {
            send_key(state, keys::KEY_LEFTSHIFT, false, time);
        }
    }
}
