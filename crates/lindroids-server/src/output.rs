//! wl_output and xdg-output: one output, the size of the Android screen.

use wayland_protocols::xdg::xdg_output::zv1::server::{zxdg_output_manager_v1, zxdg_output_v1};
use wayland_server::protocol::wl_output;
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};

use super::State;

#[derive(Default)]
pub struct Outputs {
    pub resources: Vec<wl_output::WlOutput>,
    pub xdg: Vec<zxdg_output_v1::ZxdgOutputV1>,
    /// Largest window size seen, in physical pixels: the best guess at the screen.
    pub screen: Option<(u32, u32)>,
}

pub fn register_globals(dh: &DisplayHandle) {
    dh.create_global::<State, wl_output::WlOutput, ()>(4, ());
    dh.create_global::<State, zxdg_output_manager_v1::ZxdgOutputManagerV1, ()>(3, ());
}

fn size(state: &State) -> (i32, i32) {
    let (w, h) = state.outputs.screen.unwrap_or((1280, 800));
    (w as i32, h as i32)
}

fn send_all(state: &State, o: &wl_output::WlOutput) {
    let (w, h) = size(state);
    // ~160 dpi per scale step, so physical size is plausible for toolkits that read it.
    let mm = |px: i32| (px as f64 / (160.0 * state.scale as f64) * 25.4) as i32;
    o.geometry(0, 0, mm(w), mm(h), wl_output::Subpixel::Unknown, "Android".into(), "Lindroids".into(), wl_output::Transform::Normal);
    o.mode(wl_output::Mode::Current | wl_output::Mode::Preferred, w, h, 60000);
    if o.version() >= 2 {
        o.scale(state.scale as i32);
    }
    if o.version() >= 4 {
        o.name("ANDROID-1".into());
        o.description("Android screen".into());
    }
    for x in state.outputs.xdg.iter().filter(|x| x.id().same_client_as(&o.id())) {
        send_xdg(state, x);
    }
    if o.version() >= 2 {
        o.done();
    }
}

fn send_xdg(state: &State, x: &zxdg_output_v1::ZxdgOutputV1) {
    let (w, h) = size(state);
    x.logical_position(0, 0);
    x.logical_size(w / state.scale as i32, h / state.scale as i32);
    if x.version() >= 2 {
        x.name("ANDROID-1".into());
    }
    if x.version() < 3 {
        x.done();
    }
}

pub fn scale_changed(state: &mut State) {
    for o in &state.outputs.resources {
        send_all(state, o);
    }
    let wins: Vec<u32> = state.windows.keys().copied().collect();
    for w in wins {
        state.configure_window(w, true);
    }
}

pub fn screen_size_hint(state: &mut State, w: u32, h: u32) {
    let bigger = match state.outputs.screen {
        None => true,
        Some((sw, sh)) => w * h > sw * sh,
    };
    if bigger {
        state.outputs.screen = Some((w, h));
        for o in &state.outputs.resources {
            send_all(state, o);
        }
    }
}

impl GlobalDispatch<wl_output::WlOutput, ()> for State {
    fn bind(state: &mut State, _: &DisplayHandle, _: &Client, r: New<wl_output::WlOutput>, _: &(), di: &mut DataInit<'_, State>) {
        let o = di.init(r, ());
        send_all(state, &o);
        state.outputs.resources.push(o);
    }
}

impl Dispatch<wl_output::WlOutput, ()> for State {
    fn request(_: &mut State, _: &Client, _: &wl_output::WlOutput, _: wl_output::Request, _: &(), _: &DisplayHandle, _: &mut DataInit<'_, State>) {}

    fn destroyed(state: &mut State, _: wayland_server::backend::ClientId, r: &wl_output::WlOutput, _: &()) {
        state.outputs.resources.retain(|o| o != r);
    }
}

impl GlobalDispatch<zxdg_output_manager_v1::ZxdgOutputManagerV1, ()> for State {
    fn bind(_: &mut State, _: &DisplayHandle, _: &Client, r: New<zxdg_output_manager_v1::ZxdgOutputManagerV1>, _: &(), di: &mut DataInit<'_, State>) {
        di.init(r, ());
    }
}

impl Dispatch<zxdg_output_manager_v1::ZxdgOutputManagerV1, ()> for State {
    fn request(
        state: &mut State,
        _: &Client,
        _: &zxdg_output_manager_v1::ZxdgOutputManagerV1,
        request: zxdg_output_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        di: &mut DataInit<'_, State>,
    ) {
        if let zxdg_output_manager_v1::Request::GetXdgOutput { id, output } = request {
            let x = di.init(id, ());
            send_xdg(state, &x);
            if x.version() >= 3 {
                output.done();
            }
            state.outputs.xdg.push(x);
        }
    }
}

impl Dispatch<zxdg_output_v1::ZxdgOutputV1, ()> for State {
    fn request(_: &mut State, _: &Client, _: &zxdg_output_v1::ZxdgOutputV1, _: zxdg_output_v1::Request, _: &(), _: &DisplayHandle, _: &mut DataInit<'_, State>) {}

    fn destroyed(state: &mut State, _: wayland_server::backend::ClientId, r: &zxdg_output_v1::ZxdgOutputV1, _: &()) {
        state.outputs.xdg.retain(|x| x != r);
    }
}
