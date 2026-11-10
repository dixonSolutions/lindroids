//! wl_seat (pointer, keyboard, touch) and text-input-v3, and the routing of Android input into them.

use std::collections::{BTreeSet, HashMap};
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
    /// Where `ptr_focus` starts, in window logical coordinates: while a button is down the pointer
    /// stays on it (an implicit grab) and moves relative to this.
    ptr_origin: (f64, f64),
    /// Touch point -> (surface, surface origin in window logical coordinates).
    touch_targets: HashMap<i32, (wl_surface::WlSurface, f64, f64)>,
    /// Touch point driving the emulated pointer, for clients without wl_touch.
    emulating: Option<i32>,
    /// The keys and buttons the page holds down, and the locks.
    keys: KeyState,
    /// The on-screen keyboard follows taps, as on Android, never comes up without one (apps
    /// enable text input for a field focused at start: the keyboard came up on its own), and:
    /// - apps that ask for it (GTK enables text input again on a tap into a field: text-input-v3's
    ///   way to ask) get it when a tap is answered that way, and lose it on a tap that is not
    ///   (Calculator's buttons, which keep its entry focused);
    /// - apps that never ask (Chromium and Electron: nothing on a tap) get it on a tap while
    ///   their text input is enabled.
    tapped: Option<std::time::Instant>,
    /// When a tap with no text-input answer hides the keyboard, if it is still unanswered then.
    pub(crate) kb_check: Option<std::time::Instant>,
    tap_answered: bool,
    /// Where and when each touch went down: a tap is short and stays put (not a scroll).
    touch_down: HashMap<i32, (f32, f32, std::time::Instant)>,
}

/// What the page holds down on the Linux side: keys (evdev codes) and pointer buttons, and the lock
/// modifiers. The modifiers a client is sent are the ones held down now. The server owns letting
/// go: when the focused window loses the focus, its page hides or its link drops
/// (`release_input`, `release_link`), whatever key-ups and button releases never came.
#[derive(Default)]
pub struct KeyState {
    pressed: BTreeSet<u32>,
    buttons: BTreeSet<u32>,
    locked: u32,
}

impl KeyState {
    /// The modifiers held down: from the keys pressed, so releasing one Shift keeps the other.
    pub fn depressed(&self) -> u32 {
        self.pressed.iter().fold(0, |m, &c| m | keys::modifier_of(c))
    }

    pub fn locked(&self) -> u32 {
        self.locked
    }

    /// A key message from the page: what the client is sent for it, in order (true: pressed).
    /// - A key going down again while down (the browser's auto-repeat, or a key-up that never
    ///   came): released and pressed again. A client never gets a press for a key it has down,
    ///   and repeats come from one place, the page's keyboard (clients are told not to repeat).
    /// - A key-up for a key not down (the focus moved mid-hold, or it was let go already): nothing.
    pub fn key(&mut self, code: u32, down: bool) -> &'static [bool] {
        match (down, self.pressed.contains(&code)) {
            (true, false) => {
                self.pressed.insert(code);
                &[true]
            }
            (true, true) => &[false, true],
            (false, true) => {
                self.pressed.remove(&code);
                &[false]
            }
            (false, false) => &[],
        }
    }

    /// A pointer button went down or up: whether the client is told (a press of a button that is
    /// down, or a release of one that is not, is nothing new).
    pub fn button(&mut self, code: u32, pressed: bool) -> bool {
        if pressed {
            self.buttons.insert(code)
        } else {
            self.buttons.remove(&code)
        }
    }

    pub fn any_button(&self) -> bool {
        !self.buttons.is_empty()
    }

    /// The page's lock state: Caps Lock, and Num Lock when it says (unchanged otherwise). Returns
    /// whether it changed.
    pub fn set_locks(&mut self, caps: bool, num: Option<bool>) -> bool {
        let num = num.map_or(self.locked & keys::MOD_NUM, |n| if n { keys::MOD_NUM } else { 0 });
        let locked = if caps { keys::MOD_LOCK } else { 0 } | num;
        if self.locked == locked {
            return false;
        }
        self.locked = locked;
        true
    }

    /// Whether a key can change the modifiers a client sees (the lock keys too: the modifiers are
    /// sent so the client's own toggle is put back to the page's state).
    pub fn is_modifier_key(code: u32) -> bool {
        keys::modifier_of(code) != 0 || code == keys::KEY_CAPSLOCK || code == keys::KEY_NUMLOCK
    }

    /// Lets go of every key: the codes that were down, in order. The locks stay.
    pub fn release_keys(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.pressed).into_iter().collect()
    }

    /// Lets go of every button: the codes that were down, in order.
    pub fn release_buttons(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.buttons).into_iter().collect()
    }
}

/// How long after a tap the app's text-input answer counts as answering it.
const TAP_ANSWER: std::time::Duration = std::time::Duration::from_millis(1500);
/// How long a tap waits for that answer before the keyboard goes.
const TAP_WAIT: std::time::Duration = std::time::Duration::from_millis(700);

#[derive(Default)]
pub struct TextInputState {
    enabled: bool,
    pending: Option<bool>,
    commits: u32,
    /// It enabled text input again while enabled: it asks for the keyboard that way.
    asks: bool,
}

/// A tap (touch or button released) on a window: the keyboard waits for the app's answer.
fn tapped(state: &mut State) {
    let now = std::time::Instant::now();
    state.seat.tapped = Some(now);
    state.seat.tap_answered = false;
    // The focused app's text input: enabled, and whether it asks for the keyboard itself.
    let Some(surface) = state.seat.kb_focus.clone() else { return };
    let ti = state.seat.text_inputs.iter().filter(|t| same_client(*t, &surface)).find_map(|t| {
        t.data::<Mutex<TextInputState>>().map(|d| {
            let d = d.lock().unwrap();
            (d.enabled, d.asks)
        })
    });
    match ti {
        Some((true, false)) => {
            state.seat.tap_answered = true;
            if let Some(win) = state.seat.focus_win {
                state.send_win(win, &ToApp::Keyboard { win, show: true });
            }
        }
        _ => state.seat.kb_check = Some(now + TAP_WAIT),
    }
}

/// Called from the server's loop: a tap nothing answered hides the keyboard.
pub fn keyboard_due(state: &mut State) {
    let Some(at) = state.seat.kb_check else { return };
    if std::time::Instant::now() < at {
        return;
    }
    state.seat.kb_check = None;
    if !state.seat.tap_answered {
        if let Some(win) = state.seat.focus_win {
            state.send_win(win, &ToApp::Keyboard { win, show: false });
        }
    }
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
                // No repeating in the client: the page's keyboard repeats (the browser's auto-repeat
                // comes as the key released and pressed again, see `KeyState::key`). One source,
                // the one that knows when the key is really up: a key-up lost or held back on a
                // busy link can never make a client repeat on its own, and Android's own repeat
                // settings (delay, rate, off) apply.
                if k.version() >= 4 {
                    k.repeat_info(0, 0);
                }
                // A keyboard bound by the focused client must learn about the focus.
                if let Some(s) = state.seat.kb_focus.clone() {
                    if same_client(&k, &s) {
                        let serial = state.next_serial();
                        k.enter(serial, &s, Vec::new());
                        k.modifiers(serial, state.seat.keys.depressed(), 0, state.seat.keys.locked(), 0);
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
        log::debug!("text input: {request:?}");
        match request {
            zwp_text_input_v3::Request::Enable => st.pending = Some(true),
            zwp_text_input_v3::Request::Disable => st.pending = Some(false),
            zwp_text_input_v3::Request::Commit => {
                st.commits = st.commits.wrapping_add(1);
                // Text input answering a tap: enabled, or enabled again (GTK's way to ask for the
                // keyboard on a field that has the focus already).
                let mut answer = false;
                let mut gone = false;
                match st.pending.take() {
                    Some(true) => {
                        if st.enabled {
                            st.asks = true;
                        }
                        st.enabled = true;
                        answer = true;
                    }
                    Some(false) if st.enabled => {
                        st.enabled = false;
                        gone = true;
                    }
                    _ => {}
                }
                drop(st);
                let focused = state.seat.kb_focus.as_ref().map_or(false, |s| same_client(res, s));
                let Some(win) = state.seat.focus_win.filter(|_| focused) else { return };
                if gone {
                    state.send_win(win, &ToApp::Keyboard { win, show: false });
                } else if answer && state.seat.tapped.is_some_and(|t| t.elapsed() < TAP_ANSWER) {
                    state.seat.tap_answered = true;
                    state.send_win(win, &ToApp::Keyboard { win, show: true });
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
        // Nobody left to send the key-ups to: forget them, or the next focus gets them as held.
        seat.keys.release_keys();
    }
    if seat.ptr_focus.as_ref().map_or(false, |s| &s.id() == id) {
        seat.ptr_focus = None;
        seat.keys.release_buttons();
        seat.emulating = None;
    }
    seat.touch_targets.retain(|_, (s, _, _)| &s.id() != id);
}

/// Lets go of the keys the focused client holds, and of the Shift etc. on it: a key-up that never
/// comes (the window lost the focus, its page hid or went) must not leave them down there.
pub fn release_keys(state: &mut State) {
    let released = state.seat.keys.release_keys();
    if released.is_empty() {
        return;
    }
    let Some(surface) = state.seat.kb_focus.clone() else { return };
    let time = state.now_ms();
    for code in released {
        let serial = state.next_serial();
        for k in state.seat.keyboards.iter().filter(|k| same_client(*k, &surface)) {
            k.key(serial, time, code, wl_keyboard::KeyState::Released);
        }
    }
    let serial = state.next_serial();
    let locked = state.seat.keys.locked();
    for k in state.seat.keyboards.iter().filter(|k| same_client(*k, &surface)) {
        k.modifiers(serial, 0, 0, locked, 0);
    }
}

/// Lets go of the buttons held on the surface under the pointer (and the touch that drives it).
fn release_buttons(state: &mut State) {
    state.seat.emulating = None;
    let released = state.seat.keys.release_buttons();
    let Some(surface) = state.seat.ptr_focus.clone().filter(|_| !released.is_empty()) else { return };
    let time = state.now_ms();
    for code in released {
        let serial = state.next_serial();
        for p in state.seat.pointers.iter().filter(|p| same_client(*p, &surface)) {
            p.button(serial, time, code, wl_pointer::ButtonState::Released);
        }
    }
    for p in state.seat.pointers.iter().filter(|p| same_client(*p, &surface) && p.version() >= 5) {
        p.frame();
    }
}

/// A page stops getting input (it hid, or its link dropped): the keys go up when it has the focus,
/// the buttons when the pointer is in one of its windows (a click needs no keyboard focus).
pub fn release_page(state: &mut State, link: u64) {
    if focus_on_link(state.seat.focus_win, |w| state.windows.get(&w).and_then(|w| w.link), link) {
        release_keys(state);
    }
    if state.seat.ptr_focus.as_ref().is_some_and(|s| surface_link(state, s) == Some(link)) {
        release_buttons(state);
    }
}

/// Whether the focused window is one of `link`'s windows. `link_of` maps a window to its page.
pub fn focus_on_link(focus_win: Option<u32>, link_of: impl Fn(u32) -> Option<u64>, link: u64) -> bool {
    focus_win.and_then(link_of) == Some(link)
}

/// The page a surface is drawn for: the link of the window it is in.
fn surface_link(state: &State, surface: &wl_surface::WlSurface) -> Option<u64> {
    let win = state.window_of(&surface.id())?;
    state.windows.get(&win)?.link
}

/// A page is dropped: what it holds goes, and nothing of another page's. Its keys go only when it
/// has the focus (its key-ups will not come); the buttons and the touches go only when they are in
/// its windows, so a background page leaving does not cancel a live touch elsewhere.
pub fn release_link(state: &mut State, link: u64) {
    release_page(state, link);
    let gone: Vec<i32> = state.seat.touch_targets.iter().filter(|(_, (s, _, _))| surface_link(state, s) == Some(link)).map(|(id, _)| *id).collect();
    if state.seat.emulating.is_some_and(|id| gone.contains(&id)) {
        // The touch driving the emulated pointer went with its window: let go of the button.
        release_buttons(state);
    }
    let mut cancelled = Vec::new();
    for id in gone {
        if let Some((s, _, _)) = state.seat.touch_targets.remove(&id) {
            cancelled.push(s);
        }
    }
    for t in &state.seat.touches {
        if cancelled.iter().any(|s| same_client(t, s)) {
            t.cancel();
        }
    }
}

fn set_keyboard_focus(state: &mut State, surface: Option<wl_surface::WlSurface>) {
    if state.seat.kb_focus == surface {
        return;
    }
    // Before the leave: the old client gets its key-ups while it still has the focus.
    release_keys(state);
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
            k.modifiers(serial, state.seat.keys.depressed(), 0, state.seat.keys.locked(), 0);
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
    } else {
        // The window's buttons go with the focus, whether or not it had the keyboard's.
        let pointer_win = state.seat.ptr_focus.as_ref().and_then(|s| state.window_of(&s.id()));
        if state.seat.focus_win == Some(win) || pointer_win == Some(win) {
            release_buttons(state);
        }
        if state.seat.focus_win == Some(win) {
            state.seat.focus_win = None;
            set_keyboard_focus(state, None);
        }
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
    // While a button is down the surface it went down on keeps the pointer (an implicit grab): a
    // drag across a popup or a subsurface still ends, released, where it began.
    let grab = state.seat.ptr_focus.clone().filter(|_| state.seat.keys.any_button());
    let (surface, sx, sy) = match grab {
        Some(s) => (s, lx - state.seat.ptr_origin.0, ly - state.seat.ptr_origin.1),
        None => {
            let Some((sid, sx, sy)) = shell::surface_at(state, win, lx, ly) else { return };
            let Some(surface) = state.surfaces.get(&sid).map(|s| s.res.clone()) else { return };
            state.seat.ptr_origin = (lx - sx, ly - sy);
            (surface, sx, sy)
        }
    };
    if state.seat.ptr_focus.as_ref() != Some(&surface) {
        pointer_leave(state);
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

fn pointer_leave(state: &mut State) {
    let Some(old) = state.seat.ptr_focus.take() else { return };
    let serial = state.next_serial();
    for p in state.seat.pointers.iter().filter(|p| same_client(*p, &old)) {
        p.leave(serial, &old);
        if p.version() >= 5 {
            p.frame();
        }
    }
}

fn pointer_button(state: &mut State, button: u32, pressed: bool, time: u32) {
    let Some(surface) = state.seat.ptr_focus.clone() else { return };
    if !state.seat.keys.button(button, pressed) {
        return;
    }
    if !pressed {
        tapped(state);
    }
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
        // Not while a button is down: the surface keeps the pointer until it is released.
        PointerAction::Leave if !state.seat.keys.any_button() => pointer_leave(state),
        PointerAction::Leave => {}
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

/// Scrolling: wheel steps (`pixels` false) as discrete steps of 15, or a touchpad's distance in
/// the window's physical pixels (`pixels` true) as a continuous axis, in surface coordinates.
pub fn scroll(state: &mut State, win: u32, dx: f32, dy: f32, pixels: bool, time: u32) {
    let Some(surface) = state.seat.ptr_focus.clone() else { return };
    let fit = state.windows.get(&win).map_or(1.0, |w| w.fit as f64);
    let k = if pixels { 1.0 / (state.scale as f64 * fit) } else { 15.0 };
    let source = if pixels { wl_pointer::AxisSource::Continuous } else { wl_pointer::AxisSource::Wheel };
    for p in state.seat.pointers.iter().filter(|p| same_client(*p, &surface)) {
        if p.version() >= 5 {
            p.axis_source(source);
        }
        for (axis, v) in [(wl_pointer::Axis::VerticalScroll, dy), (wl_pointer::Axis::HorizontalScroll, dx)] {
            if v == 0.0 {
                continue;
            }
            let steps = v.round() as i32;
            if !pixels && steps != 0 && p.version() >= 5 {
                p.axis_discrete(axis, steps);
            }
            p.axis(time, axis, v as f64 * k);
        }
        if p.version() >= 5 {
            p.frame();
        }
    }
}

// ------------------------------------------------------------------ touch

pub fn touch(state: &mut State, win: u32, action: TouchAction, id: i32, x: f32, y: f32, time: u32) {
    match action {
        TouchAction::Down => {
            state.seat.touch_down.insert(id, (x, y, std::time::Instant::now()));
        }
        TouchAction::Up => {
            if let Some((x0, y0, t0)) = state.seat.touch_down.remove(&id) {
                if (x - x0).hypot(y - y0) < 24.0 && t0.elapsed() < std::time::Duration::from_millis(600) {
                    tapped(state);
                }
            }
        }
        TouchAction::Cancel => {
            state.seat.touch_down.remove(&id);
        }
        TouchAction::Move => {}
    }
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
    let steps = state.seat.keys.key(code, down);
    if steps.is_empty() {
        return;
    }
    // The locks come from the page (`locks`); a lock key alone does not toggle them. The modifiers
    // are still sent so a client's own toggle on the key is put back to the page's state.
    let mods_changed = KeyState::is_modifier_key(code);
    for &pressed in steps {
        let serial = state.next_serial();
        let st = if pressed { wl_keyboard::KeyState::Pressed } else { wl_keyboard::KeyState::Released };
        for k in state.seat.keyboards.iter().filter(|k| same_client(*k, &surface)) {
            k.key(serial, time, code, st);
        }
    }
    if mods_changed {
        let serial = state.next_serial();
        let (depressed, locked) = (state.seat.keys.depressed(), state.seat.keys.locked());
        for k in state.seat.keyboards.iter().filter(|k| same_client(*k, &surface)) {
            k.modifiers(serial, depressed, 0, locked, 0);
        }
    }
}

/// The page keyboard's Caps Lock and Num Lock.
pub fn locks(state: &mut State, caps: bool, num: Option<bool>) {
    if !state.seat.keys.set_locks(caps, num) {
        return;
    }
    let Some(surface) = state.seat.kb_focus.clone() else { return };
    let serial = state.next_serial();
    let (depressed, locked) = (state.seat.keys.depressed(), state.seat.keys.locked());
    for k in state.seat.keyboards.iter().filter(|k| same_client(*k, &surface)) {
        k.modifiers(serial, depressed, 0, locked, 0);
    }
}

/// Whether a key message is dropped (None), or goes to the focused window, taking the focus first
/// (Some(true)) or not (Some(false)). Only a key going down takes the focus: a key-up for a window
/// without it (sent as its page hid, or after another window took the focus) has nothing held
/// there, which was let go when the focus went.
pub fn key_route(focus_win: Option<u32>, win: u32, down: bool) -> Option<bool> {
    match (focus_win == Some(win), down) {
        (true, _) => Some(false),
        (false, true) => Some(true),
        (false, false) => None,
    }
}

pub fn key(state: &mut State, win: u32, android_code: u32, down: bool, time: u32) {
    match key_route(state.seat.focus_win, win, down) {
        None => return,
        Some(true) if !state.windows.contains_key(&win) => return,
        Some(true) => window_focus(state, win, true),
        Some(false) => {}
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
    // A text field that speaks text-input-v3 takes the string as is (any script, emoji...), but
    // not what keys can type: keys and text-input reach the program by different ways, and keys
    // pressed around it (Backspace, Enter, a keyboard that sends some letters as keys) would
    // overtake it (Firefox: ".lab-typed" arrived as ".lypedab-t").
    let typeable = text.chars().all(|c| keys::char_to_key(c).is_some());
    let ti = state.seat.text_inputs.iter().filter(|_| !typeable).find(|t| {
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
        let had_shift = state.seat.keys.depressed() & keys::MOD_SHIFT != 0;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const KEY_A: u32 = 30;

    #[test]
    fn a_key_down_again_is_released_and_pressed_never_pressed_twice() {
        let mut k = KeyState::default();
        assert_eq!(k.key(KEY_A, false), &[] as &[bool]);
        assert_eq!(k.key(KEY_A, true), &[true]);
        // The browser's auto-repeat (or a lost key-up, then the key again).
        assert_eq!(k.key(KEY_A, true), &[false, true]);
        assert_eq!(k.key(KEY_A, false), &[false]);
        assert_eq!(k.key(KEY_A, false), &[] as &[bool]);
        assert_eq!(k.depressed(), 0);
    }

    #[test]
    fn buttons_are_told_once() {
        let mut k = KeyState::default();
        assert!(!k.button(BTN_LEFT, false));
        assert!(k.button(BTN_LEFT, true));
        assert!(!k.button(BTN_LEFT, true));
        assert!(k.any_button());
        assert_eq!(k.release_buttons(), vec![BTN_LEFT]);
        assert!(!k.button(BTN_LEFT, false));
        assert!(!k.any_button());
    }

    #[test]
    fn is_modifier_key_covers_modifiers_and_the_lock_keys_only() {
        assert!(KeyState::is_modifier_key(keys::KEY_LEFTSHIFT));
        assert!(KeyState::is_modifier_key(keys::KEY_LEFTCTRL));
        assert!(KeyState::is_modifier_key(keys::KEY_RIGHTALT));
        assert!(KeyState::is_modifier_key(keys::KEY_CAPSLOCK));
        assert!(KeyState::is_modifier_key(keys::KEY_NUMLOCK));
        assert!(!KeyState::is_modifier_key(KEY_A));
    }

    #[test]
    fn release_keys_takes_a_held_shift_whose_key_up_never_came() {
        let mut k = KeyState::default();
        k.key(keys::KEY_LEFTSHIFT, true);
        k.key(KEY_A, true);
        k.key(KEY_A, false);
        assert_eq!(k.depressed(), keys::MOD_SHIFT);
        assert_eq!(k.release_keys(), vec![keys::KEY_LEFTSHIFT]);
        assert_eq!(k.depressed(), 0);
        assert!(k.release_keys().is_empty());
    }

    #[test]
    fn focus_on_link_is_true_only_for_the_focused_windows_page() {
        // Window 1 belongs to page 10, window 2 to page 20, window 3 to no page.
        let link_of = |w: u32| match w {
            1 => Some(10),
            2 => Some(20),
            _ => None,
        };
        assert!(focus_on_link(Some(1), link_of, 10));
        assert!(!focus_on_link(Some(1), link_of, 20));
        assert!(focus_on_link(Some(2), link_of, 20));
        assert!(!focus_on_link(Some(2), link_of, 10));
        assert!(!focus_on_link(None, link_of, 10));
        assert!(!focus_on_link(Some(3), link_of, 10));
        assert!(!focus_on_link(Some(9), link_of, 10));
    }

    #[test]
    fn releasing_one_shift_keeps_the_other_shift_depressed() {
        let mut k = KeyState::default();
        k.key(keys::KEY_LEFTSHIFT, true);
        k.key(keys::KEY_RIGHTSHIFT, true);
        k.key(keys::KEY_LEFTSHIFT, false);
        assert_eq!(k.depressed(), keys::MOD_SHIFT);
        k.key(keys::KEY_RIGHTSHIFT, false);
        assert_eq!(k.depressed(), 0);
    }

    #[test]
    fn locks_follow_the_page_and_outlive_a_release() {
        let mut k = KeyState::default();
        assert!(k.set_locks(true, None));
        assert_eq!(k.locked(), keys::MOD_LOCK);
        assert!(k.set_locks(true, Some(true)));
        assert_eq!(k.locked(), keys::MOD_LOCK | keys::MOD_NUM);
        // Num Lock unsaid: kept.
        assert!(k.set_locks(false, None));
        assert_eq!(k.locked(), keys::MOD_NUM);
        assert!(!k.set_locks(false, None));
        k.key(keys::KEY_LEFTCTRL, true);
        k.release_keys();
        assert_eq!(k.locked(), keys::MOD_NUM);
    }

    #[test]
    fn lock_keys_hold_no_modifier_bit() {
        let mut k = KeyState::default();
        k.key(keys::KEY_CAPSLOCK, true);
        k.key(keys::KEY_NUMLOCK, true);
        assert_eq!(k.depressed(), 0);
    }

    #[test]
    fn only_a_key_down_takes_the_focus() {
        assert_eq!(key_route(Some(1), 1, true), Some(false));
        assert_eq!(key_route(Some(1), 1, false), Some(false));
        assert_eq!(key_route(Some(1), 2, true), Some(true));
        assert_eq!(key_route(None, 2, true), Some(true));
        // A key-up sent while its page hid, or after another window took the focus: dropped.
        assert_eq!(key_route(Some(1), 2, false), None);
        assert_eq!(key_route(None, 2, false), None);
    }

    // ------------------------------------------------------------------ seeded storm

    /// splitmix64: a seeded generator, so a failing storm can be run again.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
        fn pick<T: Copy>(&mut self, of: &[T]) -> T {
            of[self.below(of.len() as u64) as usize]
        }
    }

    /// What one client (one window's program) has been told.
    #[derive(Default, Debug)]
    struct Client {
        keys: BTreeSet<u32>,
        buttons: BTreeSet<u32>,
        depressed: u32,
        locked: u32,
        focused: bool,
    }

    /// The seat's routing, as seat.rs does it (`key`, `window_focus`, `set_keyboard_focus`,
    /// `release_input`, `release_link`, `pointer_button`, `locks`) with the Wayland requests
    /// replaced by what each client is told, checked as it is told.
    struct Model {
        keys: KeyState,
        focus_win: Option<u32>,
        kb_focus: Option<u32>,
        ptr_focus: Option<u32>,
        clients: BTreeMap<u32, Client>,
        link_of: BTreeMap<u32, u64>,
        last_locks: (bool, Option<bool>),
        log: Vec<String>,
    }

    impl Model {
        fn new() -> Model {
            let link_of: BTreeMap<u32, u64> = [(1, 10), (2, 10), (3, 20)].into();
            let clients = link_of.keys().map(|w| (*w, Client::default())).collect();
            Model { keys: KeyState::default(), focus_win: None, kb_focus: None, ptr_focus: None, clients, link_of, last_locks: (false, None), log: Vec::new() }
        }
        fn fail(&self, what: String) -> ! {
            let tail: Vec<&String> = self.log.iter().rev().take(30).collect();
            panic!("{what}\nlast messages (newest first): {tail:#?}");
        }
        fn tell_key(&mut self, win: u32, code: u32, pressed: bool) {
            let c = self.clients.get_mut(&win).unwrap();
            let fresh = if pressed { c.keys.insert(code) } else { c.keys.remove(&code) };
            if !fresh {
                let what = format!("win {win}: key {code} {} while it {} down", if pressed { "pressed" } else { "released" }, if pressed { "was" } else { "was not" });
                self.fail(what);
            }
        }
        fn tell_mods(&mut self, win: u32) {
            let (d, l) = (self.keys.depressed(), self.keys.locked());
            let c = self.clients.get_mut(&win).unwrap();
            c.depressed = d;
            c.locked = l;
        }
        fn tell_button(&mut self, win: u32, code: u32, pressed: bool) {
            let c = self.clients.get_mut(&win).unwrap();
            let fresh = if pressed { c.buttons.insert(code) } else { c.buttons.remove(&code) };
            if !fresh {
                self.fail(format!("win {win}: button {code} told {pressed} twice"));
            }
        }
        fn release_keys(&mut self) {
            let released = self.keys.release_keys();
            let Some(win) = self.kb_focus else { return };
            if released.is_empty() {
                return;
            }
            for code in released {
                self.tell_key(win, code, false);
            }
            self.tell_mods(win);
        }
        fn release_buttons(&mut self) {
            let released = self.keys.release_buttons();
            let Some(win) = self.ptr_focus else { return };
            for code in released {
                self.tell_button(win, code, false);
            }
        }
        fn set_keyboard_focus(&mut self, win: Option<u32>) {
            if self.kb_focus == win {
                return;
            }
            self.release_keys();
            if let Some(old) = self.kb_focus.take() {
                self.clients.get_mut(&old).unwrap().focused = false;
            }
            if let Some(new) = win {
                self.clients.get_mut(&new).unwrap().focused = true;
                self.tell_mods(new);
            }
            self.kb_focus = win;
        }
        fn window_focus(&mut self, win: u32, focused: bool) {
            if focused {
                self.focus_win = Some(win);
                self.set_keyboard_focus(Some(win));
            } else {
                if self.focus_win == Some(win) || self.ptr_focus == Some(win) {
                    self.release_buttons();
                }
                if self.focus_win == Some(win) {
                    self.focus_win = None;
                    self.set_keyboard_focus(None);
                }
            }
        }
        fn key(&mut self, win: u32, code: u32, down: bool) {
            match key_route(self.focus_win, win, down) {
                None => return,
                Some(true) => self.window_focus(win, true),
                Some(false) => {}
            }
            let Some(target) = self.kb_focus else { return };
            for &pressed in self.keys.key(code, down) {
                self.tell_key(target, code, pressed);
            }
            if KeyState::is_modifier_key(code) {
                self.tell_mods(target);
            }
        }
        fn button(&mut self, win: u32, code: u32, pressed: bool) {
            // pointer_motion: no button down, the pointer goes to the window under it.
            if !self.keys.any_button() {
                self.ptr_focus = Some(win);
            }
            let Some(target) = self.ptr_focus else { return };
            if self.keys.button(code, pressed) {
                self.tell_button(target, code, pressed);
            }
        }
        fn locks(&mut self, caps: bool, num: Option<bool>) {
            self.last_locks = (caps, num.or(self.last_locks.1));
            if self.keys.set_locks(caps, num) {
                if let Some(win) = self.kb_focus {
                    self.tell_mods(win);
                }
            }
        }
        fn shown(&mut self, link: u64, shown: bool) {
            if !shown {
                self.drop_link(link);
            }
        }
        fn drop_link(&mut self, link: u64) {
            let focused_here = focus_on_link(self.focus_win, |w| self.link_of.get(&w).copied(), link);
            let pointer_here = self.ptr_focus.is_some_and(|w| self.link_of.get(&w) == Some(&link));
            if focused_here {
                self.release_keys();
            }
            if pointer_here {
                self.release_buttons();
            }
        }
        /// Nothing held of `link`'s windows, on either side.
        fn check_released(&self, link: u64, why: &str) {
            for (win, c) in &self.clients {
                if self.link_of[win] != link {
                    continue;
                }
                if !c.keys.is_empty() || !c.buttons.is_empty() || c.depressed != 0 {
                    self.fail(format!("after {why}: win {win} still holds {c:?}"));
                }
            }
            let focus_here = self.focus_win.is_some_and(|w| self.link_of[&w] == link);
            if focus_here && (!self.keys.pressed.is_empty() || self.keys.depressed() != 0) {
                self.fail(format!("after {why}: the seat still holds {:?}", self.keys.pressed));
            }
        }
        fn check_locks(&self) {
            let (caps, num) = self.last_locks;
            let want = if caps { keys::MOD_LOCK } else { 0 } | if num == Some(true) { keys::MOD_NUM } else { 0 };
            if self.keys.locked() != want {
                self.fail(format!("locks {:#x}, the page last said {want:#x}", self.keys.locked()));
            }
            if let Some(win) = self.kb_focus {
                if self.clients[&win].locked != want {
                    self.fail(format!("win {win} was told locks {:#x}, the page last said {want:#x}", self.clients[&win].locked));
                }
            }
        }
        fn check_mods(&self) {
            if let Some(win) = self.kb_focus {
                let c = &self.clients[&win];
                let from_keys = c.keys.iter().fold(0, |m, &k| m | keys::modifier_of(k));
                if c.depressed != from_keys || c.depressed != self.keys.depressed() {
                    self.fail(format!("win {win}: told depressed {:#x}, its keys say {from_keys:#x}", c.depressed));
                }
            }
        }
    }

    /// Seeded random input as the pages send it (keys with repeats and lost key-ups, modifier
    /// chords, buttons, focus moves, hidden pages, dropped links, lock changes): every client is
    /// told a press only for what it does not have down, and nothing is held once the focus, the
    /// page or the link that held it is gone. LINDROIDS_SEAT_SEED and LINDROIDS_SEAT_STEPS pick
    /// another storm.
    #[test]
    fn seeded_input_storm_leaves_nothing_held() {
        let env = |k: &str, d: u64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
        let first = env("LINDROIDS_SEAT_SEED", 1);
        let steps = env("LINDROIDS_SEAT_STEPS", 20_000);
        let codes = [KEY_A, 31, 32, keys::KEY_ENTER, keys::KEY_LEFTSHIFT, keys::KEY_RIGHTSHIFT, keys::KEY_LEFTCTRL, keys::KEY_LEFTALT, keys::KEY_RIGHTALT, keys::KEY_LEFTMETA, keys::KEY_CAPSLOCK, keys::KEY_NUMLOCK];
        let buttons = [BTN_LEFT, 0x111, 0x112];
        for seed in first..first + 8 {
            let mut rng = Rng(seed);
            let mut m = Model::new();
            for _ in 0..steps {
                let win = rng.pick(&[1u32, 2, 3]);
                let link = m.link_of[&win];
                match rng.below(100) {
                    0..=44 => {
                        let (code, down) = (rng.pick(&codes), rng.below(2) == 0);
                        m.log.push(format!("key win {win} code {code} down {down}"));
                        m.key(win, code, down);
                    }
                    45..=64 => {
                        let (code, pressed) = (rng.pick(&buttons), rng.below(2) == 0);
                        m.log.push(format!("button win {win} {code:#x} {pressed}"));
                        m.button(win, code, pressed);
                    }
                    65..=74 => {
                        let focused = rng.below(2) == 0;
                        m.log.push(format!("focus win {win} {focused}"));
                        let had = m.focus_win == Some(win);
                        m.window_focus(win, focused);
                        if !focused && (had || m.ptr_focus == Some(win)) {
                            if m.ptr_focus == Some(win) && m.keys.any_button() {
                                m.fail(format!("focus lost: win {win} still has buttons down"));
                            }
                            if had {
                                m.check_released(link, "focus lost");
                            }
                        }
                    }
                    75..=82 => {
                        let shown = rng.below(2) == 0;
                        m.log.push(format!("shown link {link} {shown}"));
                        m.shown(link, shown);
                        if !shown {
                            m.check_released(link, "page hidden");
                        }
                    }
                    83..=87 => {
                        m.log.push(format!("link {link} dropped"));
                        let focused_here = m.focus_win.is_some_and(|w| m.link_of[&w] == link);
                        m.drop_link(link);
                        if focused_here {
                            for (w, c) in &m.clients {
                                if m.link_of[w] == link && m.kb_focus == Some(*w) && (!c.keys.is_empty() || c.depressed != 0) {
                                    m.fail(format!("link dropped: win {w} still holds keys {c:?}"));
                                }
                            }
                        }
                        if m.ptr_focus.is_some_and(|w| m.link_of[&w] == link) && m.keys.any_button() {
                            m.fail("link dropped: buttons still held".into());
                        }
                    }
                    _ => {
                        let (caps, num) = (rng.below(2) == 0, [None, Some(false), Some(true)][rng.below(3) as usize]);
                        m.log.push(format!("locks caps {caps} num {num:?}"));
                        m.locks(caps, num);
                    }
                }
                m.check_mods();
                m.check_locks();
            }
            // The storm ends with the page hidden: nothing may stay held anywhere.
            for link in [10, 20] {
                m.shown(link, false);
                m.drop_link(link);
            }
            m.window_focus(m.focus_win.unwrap_or(1), false);
            for link in [10, 20] {
                m.check_released(link, "the end");
            }
        }
    }
}
