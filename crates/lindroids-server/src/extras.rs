//! Smaller protocols: primary selection (middle-click paste between Linux programs),
//! xdg-activation (a program brings a window forward: its tab is shown) and cursor-shape (the
//! pointer's shape, as the page's CSS cursor).

use std::os::fd::AsFd;
use std::sync::Mutex;

use lindroids_proto::ToApp;
use wayland_protocols::wp::cursor_shape::v1::server::{wp_cursor_shape_device_v1, wp_cursor_shape_manager_v1};
use wayland_protocols::wp::primary_selection::zv1::server::{
    zwp_primary_selection_device_manager_v1 as psm, zwp_primary_selection_device_v1 as psd, zwp_primary_selection_offer_v1 as pso,
    zwp_primary_selection_source_v1 as pss,
};
use wayland_protocols::xdg::activation::v1::server::{xdg_activation_token_v1, xdg_activation_v1};
use wayland_server::protocol::wl_surface;
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum};

use super::State;

#[derive(Default)]
pub struct ExtrasState {
    pub primary_devices: Vec<psd::ZwpPrimarySelectionDeviceV1>,
    pub primary: Option<pss::ZwpPrimarySelectionSourceV1>,
    /// Activation tokens handed out and not used yet.
    tokens: Vec<String>,
    /// The CSS cursor last sent, and the page it went to.
    cursor: (Option<u64>, String),
}

pub fn register_globals(dh: &DisplayHandle) {
    dh.create_global::<State, psm::ZwpPrimarySelectionDeviceManagerV1, ()>(1, ());
    dh.create_global::<State, xdg_activation_v1::XdgActivationV1, ()>(1, ());
    dh.create_global::<State, wp_cursor_shape_manager_v1::WpCursorShapeManagerV1, ()>(1, ());
}

fn same_client<R: Resource>(r: &R, s: &wl_surface::WlSurface) -> bool {
    matches!((r.client(), s.client()), (Some(a), Some(b)) if a.id() == b.id())
}

// ------------------------------------------------------------------ primary selection

fn primary_offer_to(state: &State, dev: &psd::ZwpPrimarySelectionDeviceV1) {
    let Some(src) = &state.extras.primary else {
        dev.selection(None);
        return;
    };
    let Some(client) = dev.client() else { return };
    let Ok(offer) = client.create_resource::<pso::ZwpPrimarySelectionOfferV1, pss::ZwpPrimarySelectionSourceV1, State>(&state.dh, dev.version(), src.clone()) else {
        return;
    };
    dev.data_offer(&offer);
    for m in src.data::<Mutex<Vec<String>>>().map(|m| m.lock().unwrap().clone()).unwrap_or_default() {
        offer.offer(m);
    }
    dev.selection(Some(&offer));
}

/// As with the clipboard, only the client with keyboard focus hears about the primary selection.
pub fn focus_entering(state: &State, surface: &wl_surface::WlSurface) {
    for d in state.extras.primary_devices.iter().filter(|d| same_client(*d, surface)) {
        primary_offer_to(state, d);
    }
}

impl GlobalDispatch<psm::ZwpPrimarySelectionDeviceManagerV1, ()> for State {
    fn bind(_: &mut State, _: &DisplayHandle, _: &Client, r: New<psm::ZwpPrimarySelectionDeviceManagerV1>, _: &(), di: &mut DataInit<'_, State>) {
        di.init(r, ());
    }
}

impl Dispatch<psm::ZwpPrimarySelectionDeviceManagerV1, ()> for State {
    fn request(state: &mut State, _: &Client, _: &psm::ZwpPrimarySelectionDeviceManagerV1, request: psm::Request, _: &(), _: &DisplayHandle, di: &mut DataInit<'_, State>) {
        match request {
            psm::Request::CreateSource { id } => {
                di.init(id, Mutex::new(Vec::<String>::new()));
            }
            psm::Request::GetDevice { id, .. } => {
                let d = di.init(id, ());
                if state.seat.kb_focus.as_ref().map_or(false, |s| same_client(&d, s)) {
                    primary_offer_to(state, &d);
                }
                state.extras.primary_devices.push(d);
            }
            _ => {}
        }
    }
}

impl Dispatch<pss::ZwpPrimarySelectionSourceV1, Mutex<Vec<String>>> for State {
    fn request(state: &mut State, _: &Client, res: &pss::ZwpPrimarySelectionSourceV1, request: pss::Request, data: &Mutex<Vec<String>>, _: &DisplayHandle, _: &mut DataInit<'_, State>) {
        match request {
            pss::Request::Offer { mime_type } => data.lock().unwrap().push(mime_type),
            pss::Request::Destroy => {
                if state.extras.primary.as_ref() == Some(res) {
                    state.extras.primary = None;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<psd::ZwpPrimarySelectionDeviceV1, ()> for State {
    fn request(state: &mut State, _: &Client, _: &psd::ZwpPrimarySelectionDeviceV1, request: psd::Request, _: &(), _: &DisplayHandle, _: &mut DataInit<'_, State>) {
        if let psd::Request::SetSelection { source, .. } = request {
            if let Some(old) = state.extras.primary.take() {
                if Some(&old) != source.as_ref() {
                    old.cancelled();
                }
            }
            state.extras.primary = source;
            for d in state.extras.primary_devices.iter().filter(|d| state.seat.kb_focus.as_ref().map_or(false, |s| same_client(*d, s))) {
                primary_offer_to(state, d);
            }
        }
    }

    fn destroyed(state: &mut State, _: wayland_server::backend::ClientId, r: &psd::ZwpPrimarySelectionDeviceV1, _: &()) {
        state.extras.primary_devices.retain(|d| d != r);
    }
}

impl Dispatch<pso::ZwpPrimarySelectionOfferV1, pss::ZwpPrimarySelectionSourceV1> for State {
    fn request(_: &mut State, _: &Client, _: &pso::ZwpPrimarySelectionOfferV1, request: pso::Request, src: &pss::ZwpPrimarySelectionSourceV1, _: &DisplayHandle, _: &mut DataInit<'_, State>) {
        if let pso::Request::Receive { mime_type, fd } = request {
            src.send(mime_type, fd.as_fd());
        }
    }
}

// ------------------------------------------------------------------ xdg-activation

impl GlobalDispatch<xdg_activation_v1::XdgActivationV1, ()> for State {
    fn bind(_: &mut State, _: &DisplayHandle, _: &Client, r: New<xdg_activation_v1::XdgActivationV1>, _: &(), di: &mut DataInit<'_, State>) {
        di.init(r, ());
    }
}

impl Dispatch<xdg_activation_v1::XdgActivationV1, ()> for State {
    fn request(state: &mut State, _: &Client, _: &xdg_activation_v1::XdgActivationV1, request: xdg_activation_v1::Request, _: &(), _: &DisplayHandle, di: &mut DataInit<'_, State>) {
        match request {
            xdg_activation_v1::Request::GetActivationToken { id } => {
                di.init(id, ());
            }
            xdg_activation_v1::Request::Activate { token, surface } => {
                // Every token we handed out is good once: there is one user and one screen, and a
                // program asking to show its window is what the user started it for.
                let Some(i) = state.extras.tokens.iter().position(|t| *t == token) else { return };
                state.extras.tokens.remove(i);
                if let Some(win) = state.window_of(&surface.id()) {
                    state.send_win(win, &ToApp::WindowActivate { win });
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<xdg_activation_token_v1::XdgActivationTokenV1, ()> for State {
    fn request(state: &mut State, _: &Client, res: &xdg_activation_token_v1::XdgActivationTokenV1, request: xdg_activation_token_v1::Request, _: &(), _: &DisplayHandle, _: &mut DataInit<'_, State>) {
        if let xdg_activation_token_v1::Request::Commit = request {
            let token = super::shm::random_string(32, b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789");
            if state.extras.tokens.len() > 64 {
                state.extras.tokens.remove(0);
            }
            state.extras.tokens.push(token.clone());
            res.done(token);
        }
    }
}

// ------------------------------------------------------------------ cursor shape

impl GlobalDispatch<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1, ()> for State {
    fn bind(_: &mut State, _: &DisplayHandle, _: &Client, r: New<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1>, _: &(), di: &mut DataInit<'_, State>) {
        di.init(r, ());
    }
}

impl Dispatch<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1, ()> for State {
    fn request(_: &mut State, _: &Client, _: &wp_cursor_shape_manager_v1::WpCursorShapeManagerV1, request: wp_cursor_shape_manager_v1::Request, _: &(), _: &DisplayHandle, di: &mut DataInit<'_, State>) {
        match request {
            wp_cursor_shape_manager_v1::Request::GetPointer { cursor_shape_device, .. } => {
                di.init(cursor_shape_device, ());
            }
            wp_cursor_shape_manager_v1::Request::GetTabletToolV2 { cursor_shape_device, .. } => {
                di.init(cursor_shape_device, ());
            }
            _ => {}
        }
    }
}

impl Dispatch<wp_cursor_shape_device_v1::WpCursorShapeDeviceV1, ()> for State {
    fn request(state: &mut State, _: &Client, res: &wp_cursor_shape_device_v1::WpCursorShapeDeviceV1, request: wp_cursor_shape_device_v1::Request, _: &(), _: &DisplayHandle, _: &mut DataInit<'_, State>) {
        if let wp_cursor_shape_device_v1::Request::SetShape { shape, .. } = request {
            // Only the client the pointer is over may set it.
            if !state.seat.ptr_focus.as_ref().map_or(false, |s| same_client(res, s)) {
                return;
            }
            if let WEnum::Value(shape) = shape {
                set_cursor(state, css_cursor(shape));
            }
        }
    }
}

/// The page's pointer over Linux windows (a CSS cursor name). wl_pointer.set_cursor with a
/// surface (a picture of its own) shows the default arrow; with none, no pointer.
/// It goes to the page showing the window the pointer is over.
pub fn set_cursor(state: &mut State, css: &str) {
    let Some(win) = state.seat.ptr_focus.as_ref().and_then(|s| state.window_of(&s.id())) else { return };
    let page = state.windows.get(&win).and_then(|w| w.link);
    if page.is_some() && (state.extras.cursor.0 != page || state.extras.cursor.1 != css) {
        state.extras.cursor = (page, css.to_string());
        state.send_win(win, &ToApp::Cursor { css: css.to_string() });
    }
}

/// cursor-shape-v1 is modelled on CSS: the names map one to one.
fn css_cursor(shape: wp_cursor_shape_device_v1::Shape) -> &'static str {
    use wp_cursor_shape_device_v1::Shape as S;
    match shape {
        S::Default => "default",
        S::ContextMenu => "context-menu",
        S::Help => "help",
        S::Pointer => "pointer",
        S::Progress => "progress",
        S::Wait => "wait",
        S::Cell => "cell",
        S::Crosshair => "crosshair",
        S::Text => "text",
        S::VerticalText => "vertical-text",
        S::Alias => "alias",
        S::Copy => "copy",
        S::Move => "move",
        S::NoDrop => "no-drop",
        S::NotAllowed => "not-allowed",
        S::Grab => "grab",
        S::Grabbing => "grabbing",
        S::EResize => "e-resize",
        S::NResize => "n-resize",
        S::NeResize => "ne-resize",
        S::NwResize => "nw-resize",
        S::SResize => "s-resize",
        S::SeResize => "se-resize",
        S::SwResize => "sw-resize",
        S::WResize => "w-resize",
        S::EwResize => "ew-resize",
        S::NsResize => "ns-resize",
        S::NeswResize => "nesw-resize",
        S::NwseResize => "nwse-resize",
        S::ColResize => "col-resize",
        S::RowResize => "row-resize",
        S::AllScroll => "all-scroll",
        S::ZoomIn => "zoom-in",
        S::ZoomOut => "zoom-out",
        _ => "default",
    }
}
