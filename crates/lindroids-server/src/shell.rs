//! xdg-shell (toplevels and popups) and xdg-decoration.
//!
//! Every toplevel becomes its own Android window. Popups (menus, tooltips, dropdowns) are drawn
//! into the window of the toplevel they belong to.

use std::sync::{Arc, Mutex};

use log::info;
use wayland_protocols::xdg::decoration::zv1::server::{zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1};
use wayland_protocols::xdg::shell::server::{xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base};
use wayland_server::backend::{ClientId, ObjectId};
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum};

use lindroids_proto::ToApp;

use super::render::Placed;
use super::surface::Role;
use super::{State, Window};

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Per wl_surface xdg state.
pub struct XdgInfo {
    pub xdg_surface: xdg_surface::XdgSurface,
    pub geometry: Option<Rect>,
    pub pending_geometry: Option<Rect>,
    /// The initial configure has been sent (after the first commit).
    pub initial_sent: bool,
    pub acked: bool,
}

pub struct Popup {
    pub popup: xdg_popup::XdgPopup,
    pub surface: ObjectId,
    pub parent: ObjectId,
    pub win: u32,
    /// Position of the popup's window geometry, relative to the parent's window geometry.
    pub rel: (i32, i32),
    pub size: (i32, i32),
    pub mapped: bool,
    pub grab: bool,
}

pub fn register_globals(dh: &DisplayHandle) {
    dh.create_global::<State, xdg_wm_base::XdgWmBase, ()>(5, ());
    dh.create_global::<State, zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, ()>(1, ());
}

pub fn send_toplevel_configure(w: &mut Window, size: (i32, i32), serial: u32) {
    // A client that never asked for a decoration mode gets client-side: GNOME-style apps keep
    // their menus in the header bar.
    if let (Some(d), None) = (&w.decoration, w.decoration_mode) {
        d.configure(zxdg_toplevel_decoration_v1::Mode::ClientSide);
        w.decoration_mode = Some(zxdg_toplevel_decoration_v1::Mode::ClientSide);
    }
    let mut states: Vec<u8> = Vec::new();
    let mut push = |s: xdg_toplevel::State| states.extend_from_slice(&(s as u32).to_ne_bytes());
    if size.0 > 0 && size.1 > 0 {
        if w.fullscreen {
            push(xdg_toplevel::State::Fullscreen);
        } else {
            push(xdg_toplevel::State::Maximized);
        }
        if w.toplevel.version() >= 2 {
            push(xdg_toplevel::State::TiledLeft);
            push(xdg_toplevel::State::TiledRight);
            push(xdg_toplevel::State::TiledTop);
            push(xdg_toplevel::State::TiledBottom);
        }
    }
    if w.activated {
        push(xdg_toplevel::State::Activated);
    }
    if w.toplevel.version() >= 4 && size.0 > 0 {
        w.toplevel.configure_bounds(size.0, size.1);
    }
    w.toplevel.configure(size.0, size.1, states);
    w.xdg_surface.configure(serial);
}

// ------------------------------------------------------------------ geometry helpers

fn xdg_geometry(state: &State, sid: &ObjectId) -> Rect {
    let s = state.surfaces.get(sid);
    let g = s.and_then(|s| s.xdg.as_ref()).and_then(|x| x.geometry);
    match g {
        Some(g) => g,
        None => {
            let (w, h) = s.map(|s| s.logical_size()).unwrap_or((0, 0));
            Rect { x: 0, y: 0, w, h }
        }
    }
}

/// Where a toplevel's or popup's window geometry sits in its Android window (logical pixels).
fn geometry_origin(state: &State, sid: &ObjectId) -> (i32, i32) {
    let mut x = 0;
    let mut y = 0;
    let mut cur = sid.clone();
    for _ in 0..32 {
        match state.popups.get(&cur) {
            Some(p) => {
                x += p.rel.0;
                y += p.rel.1;
                cur = p.parent.clone();
            }
            None => break,
        }
    }
    (x, y)
}

pub struct Layers {
    pub placed: Vec<Placed>,
    pub surfaces: Vec<ObjectId>,
    /// Logical rectangles for hit testing, bottom to top: (surface, x, y, w, h).
    pub rects: Vec<(ObjectId, i32, i32, i32, i32)>,
}

fn push_tree(state: &State, sid: &ObjectId, x: i32, y: i32, root: bool, out: &mut Layers) {
    let Some(s) = state.surfaces.get(sid) else { return };
    let scale = state.scale as i32;
    let child_pos = |c: &ObjectId| state.surfaces.get(c).and_then(|cs| cs.sub.as_ref().map(|sub| (sub.x, sub.y)));
    for (c, below) in &s.children {
        if *below {
            if let Some((cx, cy)) = child_pos(c) {
                push_tree(state, c, x + cx, y + cy, false, out);
            }
        }
    }
    if let Some(img) = &s.image {
        let (lw, lh) = s.logical_size();
        out.placed.push(Placed {
            image: img.clone(),
            x: x * scale,
            y: y * scale,
            w: (lw * scale).max(0) as u32,
            h: (lh * scale).max(0) as u32,
            root,
        });
        out.rects.push((sid.clone(), x, y, lw, lh));
    }
    out.surfaces.push(sid.clone());
    for (c, below) in &s.children {
        if !*below {
            if let Some((cx, cy)) = child_pos(c) {
                push_tree(state, c, x + cx, y + cy, false, out);
            }
        }
    }
}

pub fn layers_for_window(state: &State, win: u32) -> Layers {
    let mut out = Layers { placed: Vec::new(), surfaces: Vec::new(), rects: Vec::new() };
    let Some(w) = state.windows.get(&win) else { return out };
    let g = xdg_geometry(state, &w.surface);
    push_tree(state, &w.surface, -g.x, -g.y, true, &mut out);
    for p in &w.popups {
        if !state.popups.get(p).map_or(false, |p| p.mapped) {
            continue;
        }
        let (ox, oy) = geometry_origin(state, p);
        let pg = xdg_geometry(state, p);
        push_tree(state, p, ox - pg.x, oy - pg.y, false, &mut out);
    }
    out
}

/// The logical size of a window's content (its window geometry).
pub fn content_size(state: &State, win: u32) -> (i32, i32) {
    state.windows.get(&win).map(|w| {
        let g = xdg_geometry(state, &w.surface);
        (g.w, g.h)
    }).unwrap_or((0, 0))
}

/// Topmost surface under a logical point of a window, with surface-local coordinates.
pub fn surface_at(state: &State, win: u32, x: f64, y: f64) -> Option<(ObjectId, f64, f64)> {
    let layers = layers_for_window(state, win);
    for (sid, sx, sy, w, h) in layers.rects.iter().rev() {
        let (lx, ly) = (x - *sx as f64, y - *sy as f64);
        if lx >= 0.0 && ly >= 0.0 && lx < *w as f64 && ly < *h as f64 {
            return Some((sid.clone(), lx, ly));
        }
    }
    // Outside every surface (e.g. the client is smaller than the Android window): the root.
    let w = state.windows.get(&win)?;
    let g = xdg_geometry(state, &w.surface);
    Some((w.surface.clone(), x + g.x as f64, y + g.y as f64))
}

/// True if (x, y) is inside one of the window's mapped popups.
pub fn in_popup(state: &State, win: u32, x: f64, y: f64) -> bool {
    let Some(w) = state.windows.get(&win) else { return false };
    w.popups.iter().any(|p| {
        let Some(pp) = state.popups.get(p) else { return false };
        let (ox, oy) = geometry_origin(state, p);
        pp.mapped && x >= ox as f64 && y >= oy as f64 && x < (ox + pp.size.0) as f64 && y < (oy + pp.size.1) as f64
    })
}

/// Close every grabbing popup of a window (a tap landed outside them).
pub fn dismiss_popups(state: &mut State, win: u32) {
    let Some(w) = state.windows.get(&win) else { return };
    for p in w.popups.iter().rev() {
        if let Some(pp) = state.popups.get(p) {
            if pp.grab {
                pp.popup.popup_done();
            }
        }
    }
}

// ------------------------------------------------------------------ commits

pub fn surface_committed(state: &mut State, sid: &ObjectId, _first: bool) {
    let Some(s) = state.surfaces.get_mut(sid) else { return };
    let has_image = s.image.is_some();
    let role = s.role.clone();
    let Some(x) = s.xdg.as_mut() else { return };
    if let Some(g) = x.pending_geometry.take() {
        x.geometry = Some(g);
    }
    let initial = !x.initial_sent;
    x.initial_sent = true;
    let acked = x.acked;
    match role {
        Role::Toplevel(win) => {
            if initial {
                state.configure_window(win, true);
                return;
            }
            let mapped = state.windows.get(&win).map_or(false, |w| w.mapped);
            if has_image && acked && !mapped {
                state.window_mapped(win);
            } else if !has_image && mapped {
                // Unmapped (a null buffer): the Android window goes away until it maps again.
                log::info!("window {win} unmapped by its program");
                let was_announced = state.windows.get_mut(&win).map_or(false, |w| {
                    w.mapped = false;
                    w.announced
                });
                if was_announced {
                    state.send_win(win, &ToApp::WindowClose { win });
                    if let Some(w) = state.windows.get_mut(&win) {
                        w.announced = false;
                        w.link = None;
                        w.shown.clear();
                        w.shown_size = (0, 0);
                        w.in_flight = false;
                    }
                }
            }
        }
        Role::Popup => {
            if initial {
                let serial = state.next_serial();
                if let Some(p) = state.popups.get(sid) {
                    p.popup.configure(p.rel.0, p.rel.1, p.size.0, p.size.1);
                }
                if let Some(x) = state.surfaces.get(sid).and_then(|s| s.xdg.as_ref()) {
                    x.xdg_surface.configure(serial);
                }
                return;
            }
            let win = state.popups.get(sid).map(|p| p.win);
            if let Some(p) = state.popups.get_mut(sid) {
                p.mapped = has_image;
            }
            if let Some(win) = win {
                if let Some(w) = state.windows.get_mut(&win) {
                    if has_image && !w.popups.contains(sid) {
                        w.popups.push(sid.clone());
                    }
                    w.dirty = true;
                }
            }
        }
        _ => {}
    }
}

pub fn surface_destroyed(state: &mut State, sid: &ObjectId) {
    remove_popup(state, sid);
    let role = state.surfaces.get(sid).map(|s| s.role.clone());
    if let Some(Role::Toplevel(win)) = role {
        state.window_gone(win);
    }
}

fn remove_popup(state: &mut State, sid: &ObjectId) {
    if let Some(p) = state.popups.remove(sid) {
        if let Some(w) = state.windows.get_mut(&p.win) {
            w.popups.retain(|x| x != sid);
            w.dirty = true;
        }
    }
}

// ------------------------------------------------------------------ xdg_wm_base

impl GlobalDispatch<xdg_wm_base::XdgWmBase, ()> for State {
    fn bind(_: &mut State, _: &DisplayHandle, _: &Client, r: New<xdg_wm_base::XdgWmBase>, _: &(), di: &mut DataInit<'_, State>) {
        di.init(r, ());
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for State {
    fn request(
        state: &mut State,
        _: &Client,
        _: &xdg_wm_base::XdgWmBase,
        request: xdg_wm_base::Request,
        _: &(),
        _: &DisplayHandle,
        di: &mut DataInit<'_, State>,
    ) {
        match request {
            xdg_wm_base::Request::CreatePositioner { id } => {
                di.init(id, Mutex::new(Positioner::default()));
            }
            xdg_wm_base::Request::GetXdgSurface { id, surface } => {
                let sid = surface.id();
                let xs = di.init(id, sid.clone());
                if let Some(s) = state.surfaces.get_mut(&sid) {
                    s.xdg = Some(XdgInfo { xdg_surface: xs, geometry: None, pending_geometry: None, initial_sent: false, acked: false });
                }
            }
            _ => {}
        }
    }
}

// ------------------------------------------------------------------ positioner

#[derive(Debug, Clone, Default)]
pub struct Positioner {
    size: (i32, i32),
    anchor_rect: Rect,
    anchor: u32,
    gravity: u32,
    offset: (i32, i32),
    constraint: u32,
}

impl Dispatch<xdg_positioner::XdgPositioner, Mutex<Positioner>> for State {
    fn request(
        _: &mut State,
        _: &Client,
        _: &xdg_positioner::XdgPositioner,
        request: xdg_positioner::Request,
        data: &Mutex<Positioner>,
        _: &DisplayHandle,
        _: &mut DataInit<'_, State>,
    ) {
        let mut p = data.lock().unwrap();
        match request {
            xdg_positioner::Request::SetSize { width, height } => p.size = (width, height),
            xdg_positioner::Request::SetAnchorRect { x, y, width, height } => p.anchor_rect = Rect { x, y, w: width, h: height },
            xdg_positioner::Request::SetAnchor { anchor } => {
                p.anchor = match anchor {
                    WEnum::Value(a) => a as u32,
                    WEnum::Unknown(u) => u,
                }
            }
            xdg_positioner::Request::SetGravity { gravity } => {
                p.gravity = match gravity {
                    WEnum::Value(g) => g as u32,
                    WEnum::Unknown(u) => u,
                }
            }
            xdg_positioner::Request::SetOffset { x, y } => p.offset = (x, y),
            xdg_positioner::Request::SetConstraintAdjustment { constraint_adjustment } => {
                p.constraint = match constraint_adjustment {
                    WEnum::Value(c) => c.bits(),
                    WEnum::Unknown(u) => u,
                }
            }
            _ => {}
        }
    }
}

// anchor / gravity enum values: none=0 top=1 bottom=2 left=3 right=4 top_left=5 bottom_left=6 top_right=7 bottom_right=8
fn edges(v: u32) -> (i32, i32) {
    // (-1 = left/top, 0 = center, 1 = right/bottom)
    match v {
        1 => (0, -1),
        2 => (0, 1),
        3 => (-1, 0),
        4 => (1, 0),
        5 => (-1, -1),
        6 => (-1, 1),
        7 => (1, -1),
        8 => (1, 1),
        _ => (0, 0),
    }
}

/// Place a popup relative to its parent's window geometry, keeping it inside `bounds`
/// (the Android window, in the parent's coordinates).
pub fn place(p: &Positioner, bounds: Rect) -> Rect {
    let compute = |anchor: (i32, i32), gravity: (i32, i32), offset: (i32, i32)| {
        let ar = p.anchor_rect;
        let ax = match anchor.0 { -1 => ar.x, 1 => ar.x + ar.w, _ => ar.x + ar.w / 2 };
        let ay = match anchor.1 { -1 => ar.y, 1 => ar.y + ar.h, _ => ar.y + ar.h / 2 };
        let x = match gravity.0 { -1 => ax - p.size.0, 1 => ax, _ => ax - p.size.0 / 2 };
        let y = match gravity.1 { -1 => ay - p.size.1, 1 => ay, _ => ay - p.size.1 / 2 };
        Rect { x: x + offset.0, y: y + offset.1, w: p.size.0, h: p.size.1 }
    };
    let (a, g) = (edges(p.anchor), edges(p.gravity));
    let mut r = compute(a, g, p.offset);
    let out_x = |r: &Rect| r.x < bounds.x || r.x + r.w > bounds.x + bounds.w;
    let out_y = |r: &Rect| r.y < bounds.y || r.y + r.h > bounds.y + bounds.h;
    const SLIDE_X: u32 = 1;
    const SLIDE_Y: u32 = 2;
    const FLIP_X: u32 = 4;
    const FLIP_Y: u32 = 8;
    if out_x(&r) && p.constraint & FLIP_X != 0 {
        let f = compute((-a.0, a.1), (-g.0, g.1), (-p.offset.0, p.offset.1));
        if !out_x(&f) {
            r.x = f.x;
        }
    }
    if out_y(&r) && p.constraint & FLIP_Y != 0 {
        let f = compute((a.0, -a.1), (g.0, -g.1), (p.offset.0, -p.offset.1));
        if !out_y(&f) {
            r.y = f.y;
        }
    }
    // Slide whatever still sticks out. On a phone every pixel counts, so slide even when the
    // client did not ask for it: a menu half off-screen is worse than a moved one.
    if out_x(&r) && (p.constraint & SLIDE_X != 0 || bounds.w > 0) {
        r.x = r.x.min(bounds.x + bounds.w - r.w).max(bounds.x);
    }
    if out_y(&r) && (p.constraint & SLIDE_Y != 0 || bounds.h > 0) {
        r.y = r.y.min(bounds.y + bounds.h - r.h).max(bounds.y);
    }
    r
}

// ------------------------------------------------------------------ xdg_surface

impl Dispatch<xdg_surface::XdgSurface, ObjectId> for State {
    fn request(
        state: &mut State,
        client: &Client,
        res: &xdg_surface::XdgSurface,
        request: xdg_surface::Request,
        sid: &ObjectId,
        _: &DisplayHandle,
        di: &mut DataInit<'_, State>,
    ) {
        match request {
            xdg_surface::Request::GetToplevel { id } => {
                let win = state.new_window_id();
                let tl = di.init(id, (sid.clone(), win));
                if tl.version() >= 5 {
                    // Windows are always maximized into their Android task and may go fullscreen.
                    // An empty list would tell libdecor (SDL's decorations) that fullscreen is
                    // impossible, and SDL's fullscreen windows would then never get a size.
                    let caps: Vec<u8> = [xdg_toplevel::WmCapabilities::Maximize, xdg_toplevel::WmCapabilities::Fullscreen]
                        .iter()
                        .flat_map(|c| (*c as u32).to_ne_bytes())
                        .collect();
                    tl.wm_capabilities(caps);
                }
                if let Some(s) = state.surfaces.get_mut(sid) {
                    s.role = Role::Toplevel(win);
                }
                state.windows.insert(
                    win,
                    Window {
                        id: win,
                        surface: sid.clone(),
                        client: client.id(),
                        toplevel: tl,
                        xdg_surface: res.clone(),
                        title: String::new(),
                        app_id: String::new(),
                        phys: state.outputs.screen.map(|(w, h)| (w, h)),
                        configured: (-1, -1),
                        activated: true,
                        announced: false,
                        mapped: false,
                        dirty: false,
                        shown: Vec::new(),
                        shown_size: (0, 0),
                        canvas: Vec::new(),
                        in_flight: false,
                        waiting: Vec::new(),
                        popups: Vec::new(),
                        fullscreen: false,
                        fit: 1.0,
                        fit_offset: (0, 0),
                        decoration: None,
                        decoration_mode: None,
                        run: None,
                        link: None,
                    },
                );
            }
            xdg_surface::Request::GetPopup { id, parent, positioner } => {
                let pos = positioner.data::<Mutex<Positioner>>().map(|m| m.lock().unwrap().clone()).unwrap_or_default();
                let popup = di.init(id, sid.clone());
                let Some(parent) = parent else { return };
                let parent_sid: ObjectId = parent.data::<ObjectId>().cloned().unwrap_or_else(|| parent.id());
                let Some(win) = state.window_of(&parent_sid) else { return };
                // Bounds: the Android window, expressed relative to the parent's geometry.
                let (ox, oy) = geometry_origin(state, &parent_sid);
                let (bw, bh) = state.windows.get(&win).map(|w| w.configured).unwrap_or((0, 0));
                let bounds = if bw > 0 { Rect { x: -ox, y: -oy, w: bw, h: bh } } else { Rect { x: -100000, y: -100000, w: 200000, h: 200000 } };
                let r = place(&pos, bounds);
                if let Some(s) = state.surfaces.get_mut(sid) {
                    s.role = Role::Popup;
                }
                state.popups.insert(
                    sid.clone(),
                    Popup { popup, surface: sid.clone(), parent: parent_sid, win, rel: (r.x, r.y), size: (r.w, r.h), mapped: false, grab: false },
                );
            }
            xdg_surface::Request::SetWindowGeometry { x, y, width, height } => {
                log::debug!("window geometry {sid:?}: {x},{y} {width}x{height}");
                if let Some(x_) = state.surfaces.get_mut(sid).and_then(|s| s.xdg.as_mut()) {
                    x_.pending_geometry = Some(Rect { x, y, w: width, h: height });
                }
            }
            xdg_surface::Request::AckConfigure { .. } => {
                if let Some(x) = state.surfaces.get_mut(sid).and_then(|s| s.xdg.as_mut()) {
                    x.acked = true;
                }
            }
            _ => {}
        }
    }

    fn destroyed(state: &mut State, _: ClientId, _: &xdg_surface::XdgSurface, sid: &ObjectId) {
        if let Some(s) = state.surfaces.get_mut(sid) {
            s.xdg = None;
        }
    }
}

// ------------------------------------------------------------------ xdg_toplevel

impl Dispatch<xdg_toplevel::XdgToplevel, (ObjectId, u32)> for State {
    fn request(
        state: &mut State,
        _: &Client,
        _: &xdg_toplevel::XdgToplevel,
        request: xdg_toplevel::Request,
        data: &(ObjectId, u32),
        _: &DisplayHandle,
        _: &mut DataInit<'_, State>,
    ) {
        let win = data.1;
        match request {
            xdg_toplevel::Request::SetTitle { title } => {
                let announced = state.windows.get_mut(&win).map(|w| {
                    w.title = title.clone();
                    // Xwayland's "Xwayland on :1" says nothing; the launched name stays.
                    w.announced && w.app_id != "org.freedesktop.Xwayland"
                });
                if announced == Some(true) {
                    state.send_win(win, &ToApp::WindowTitle { win, title });
                }
            }
            xdg_toplevel::Request::SetAppId { app_id } => {
                if let Some(w) = state.windows.get_mut(&win) {
                    info!("window {win} app id {app_id}");
                    w.app_id = app_id;
                }
            }
            xdg_toplevel::Request::SetFullscreen { .. } => {
                if let Some(w) = state.windows.get_mut(&win) {
                    w.fullscreen = true;
                }
                state.configure_window(win, true);
            }
            xdg_toplevel::Request::UnsetFullscreen => {
                if let Some(w) = state.windows.get_mut(&win) {
                    w.fullscreen = false;
                }
                state.configure_window(win, true);
            }
            // Android windows are always "maximized" into their task; re-send the size.
            xdg_toplevel::Request::SetMaximized | xdg_toplevel::Request::UnsetMaximized => state.configure_window(win, true),
            _ => {}
        }
    }

    fn destroyed(state: &mut State, _: ClientId, _: &xdg_toplevel::XdgToplevel, data: &(ObjectId, u32)) {
        if let Some(s) = state.surfaces.get_mut(&data.0) {
            if s.role == Role::Toplevel(data.1) {
                s.role = Role::None;
            }
        }
        state.window_gone(data.1);
    }
}

// ------------------------------------------------------------------ xdg_popup

impl Dispatch<xdg_popup::XdgPopup, ObjectId> for State {
    fn request(
        state: &mut State,
        _: &Client,
        _: &xdg_popup::XdgPopup,
        request: xdg_popup::Request,
        sid: &ObjectId,
        _: &DisplayHandle,
        _: &mut DataInit<'_, State>,
    ) {
        match request {
            xdg_popup::Request::Grab { .. } => {
                if let Some(p) = state.popups.get_mut(sid) {
                    p.grab = true;
                }
            }
            xdg_popup::Request::Reposition { positioner, token } => {
                let pos = positioner.data::<Mutex<Positioner>>().map(|m| m.lock().unwrap().clone()).unwrap_or_default();
                let Some(p) = state.popups.get(sid) else { return };
                let (ox, oy) = geometry_origin(state, &p.parent);
                let (bw, bh) = state.windows.get(&p.win).map(|w| w.configured).unwrap_or((0, 0));
                let bounds = if bw > 0 { Rect { x: -ox, y: -oy, w: bw, h: bh } } else { Rect { x: -100000, y: -100000, w: 200000, h: 200000 } };
                let r = place(&pos, bounds);
                let serial = state.next_serial();
                if let Some(p) = state.popups.get_mut(sid) {
                    p.rel = (r.x, r.y);
                    p.size = (r.w, r.h);
                    if p.popup.version() >= 3 {
                        p.popup.repositioned(token);
                    }
                    p.popup.configure(r.x, r.y, r.w, r.h);
                }
                if let Some(x) = state.surfaces.get(sid).and_then(|s| s.xdg.as_ref()) {
                    x.xdg_surface.configure(serial);
                }
            }
            _ => {}
        }
    }

    fn destroyed(state: &mut State, _: ClientId, _: &xdg_popup::XdgPopup, sid: &ObjectId) {
        remove_popup(state, sid);
        if let Some(s) = state.surfaces.get_mut(sid) {
            if s.role == Role::Popup {
                s.role = Role::None;
            }
        }
    }
}

// ------------------------------------------------------------------ decorations

impl GlobalDispatch<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, ()> for State {
    fn bind(_: &mut State, _: &DisplayHandle, _: &Client, r: New<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1>, _: &(), di: &mut DataInit<'_, State>) {
        di.init(r, ());
    }
}

impl Dispatch<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, ()> for State {
    fn request(
        state: &mut State,
        _: &Client,
        _: &zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
        request: zxdg_decoration_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        di: &mut DataInit<'_, State>,
    ) {
        if let zxdg_decoration_manager_v1::Request::GetToplevelDecoration { id, toplevel } = request {
            let win = toplevel.data::<(ObjectId, u32)>().map(|d| d.1).unwrap_or(0);
            let d = di.init(id, win);
            // The mode is told with the first configure, once the client had a chance to ask.
            if let Some(w) = state.windows.get_mut(&win) {
                w.decoration = Some(d);
                w.decoration_mode = None;
            }
        }
    }
}

impl Dispatch<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, u32> for State {
    fn request(
        state: &mut State,
        _: &Client,
        res: &zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
        request: zxdg_toplevel_decoration_v1::Request,
        win: &u32,
        _: &DisplayHandle,
        _: &mut DataInit<'_, State>,
    ) {
        use zxdg_toplevel_decoration_v1::Mode;
        let wanted = match request {
            // What the client asks for, it gets: server-side means no title bar at all (Android
            // shows the name), which suits SDL games; otherwise the client draws its own.
            zxdg_toplevel_decoration_v1::Request::SetMode { mode: WEnum::Value(Mode::ServerSide) } => Mode::ServerSide,
            zxdg_toplevel_decoration_v1::Request::SetMode { .. } | zxdg_toplevel_decoration_v1::Request::UnsetMode => Mode::ClientSide,
            _ => return,
        };
        let configured = state.windows.get(win).map_or(false, |w| w.configured != (-1, -1));
        if let Some(w) = state.windows.get_mut(win) {
            if w.decoration_mode == Some(wanted) {
                return;
            }
            w.decoration_mode = Some(wanted);
        }
        res.configure(wanted);
        // A new mode takes effect with the next xdg_surface.configure.
        if configured {
            state.configure_window(*win, true);
        }
    }

    fn destroyed(state: &mut State, _: ClientId, _: &zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, win: &u32) {
        if let Some(w) = state.windows.get_mut(win) {
            w.decoration = None;
        }
    }
}

#[allow(dead_code)]
fn _assert_send(_: Arc<Positioner>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_below_button_stays_on_screen() {
        // A 200x300 menu under a button near the bottom-right of an 800x600 window.
        let p = Positioner {
            size: (200, 300),
            anchor_rect: Rect { x: 700, y: 500, w: 80, h: 30 },
            anchor: 6,  // bottom_left
            gravity: 8, // bottom_right
            offset: (0, 0),
            constraint: 1 | 2 | 4 | 8,
        };
        let r = place(&p, Rect { x: 0, y: 0, w: 800, h: 600 });
        assert!(r.x >= 0 && r.x + r.w <= 800, "{r:?}");
        assert!(r.y >= 0 && r.y + r.h <= 600, "{r:?}");
        // Flipped above the button.
        assert_eq!(r.y + r.h, 500);
    }
}
