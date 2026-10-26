//! wl_data_device: the clipboard, bridged both ways with Android's.
//!
//! When a Linux app copies, its text is read and put on the Android clipboard. When Android's
//! clipboard changes (seen when a Linux window gains focus), Linux apps are offered that text.

use std::io::{Read, Write};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::sync::Mutex;

use lindroids_proto::ToApp;
use wayland_server::protocol::{wl_data_device, wl_data_device_manager, wl_data_offer, wl_data_source, wl_seat};
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};

use super::{Inbox, State};

const TEXT_MIMES: [&str; 5] = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING", "TEXT", "STRING"];

#[derive(Default)]
pub struct DataState {
    pub devices: Vec<wl_data_device::WlDataDevice>,
    pub selection: Option<Selection>,
}

#[derive(Clone)]
pub enum Selection {
    Client(wl_data_source::WlDataSource),
    Android(String),
}

pub fn register_globals(dh: &DisplayHandle) {
    dh.create_global::<State, wl_data_device_manager::WlDataDeviceManager, ()>(3, ());
}

fn pipe() -> Option<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return None;
    }
    Some(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

fn mimes_of(sel: &Selection) -> Vec<String> {
    match sel {
        Selection::Client(src) => src.data::<Mutex<Vec<String>>>().map(|m| m.lock().unwrap().clone()).unwrap_or_default(),
        Selection::Android(_) => TEXT_MIMES.iter().map(|s| s.to_string()).collect(),
    }
}

/// Tell one data device about the current selection.
fn offer_to(state: &State, dev: &wl_data_device::WlDataDevice) {
    let Some(sel) = &state.data.selection else {
        dev.selection(None);
        return;
    };
    let Some(client) = dev.client() else { return };
    let Ok(offer) = client.create_resource::<wl_data_offer::WlDataOffer, Selection, State>(&state.dh, dev.version(), sel.clone()) else {
        return;
    };
    dev.data_offer(&offer);
    for m in mimes_of(sel) {
        offer.offer(m);
    }
    dev.selection(Some(&offer));
}

fn broadcast(state: &State) {
    for d in &state.data.devices {
        offer_to(state, d);
    }
}

pub fn android_clipboard(state: &mut State, text: String) {
    if let Some(Selection::Android(t)) = &state.data.selection {
        if *t == text {
            return;
        }
    }
    if let Some(Selection::Client(src)) = state.data.selection.take() {
        src.cancelled();
    }
    state.data.selection = Some(Selection::Android(text));
    broadcast(state);
}

/// A Linux app set the selection: read its text and hand it to Android.
fn fetch_text(src: &wl_data_source::WlDataSource) {
    let mimes = src.data::<Mutex<Vec<String>>>().map(|m| m.lock().unwrap().clone()).unwrap_or_default();
    let Some(mime) = TEXT_MIMES.iter().find(|m| mimes.iter().any(|x| x == *m)) else { return };
    let Some((r, w)) = pipe() else { return };
    src.send(mime.to_string(), w.as_fd());
    drop(w);
    std::thread::spawn(move || {
        let mut f = std::fs::File::from(r);
        let mut buf = Vec::new();
        let _ = Read::by_ref(&mut f).take(4 << 20).read_to_end(&mut buf);
        if let Ok(text) = String::from_utf8(buf) {
            if !text.is_empty() {
                super::shared().post(Inbox::ToApp(ToApp::Clipboard { text }));
            }
        }
    });
}

impl GlobalDispatch<wl_data_device_manager::WlDataDeviceManager, ()> for State {
    fn bind(_: &mut State, _: &DisplayHandle, _: &Client, r: New<wl_data_device_manager::WlDataDeviceManager>, _: &(), di: &mut DataInit<'_, State>) {
        di.init(r, ());
    }
}

impl Dispatch<wl_data_device_manager::WlDataDeviceManager, ()> for State {
    fn request(
        state: &mut State,
        _: &Client,
        _: &wl_data_device_manager::WlDataDeviceManager,
        request: wl_data_device_manager::Request,
        _: &(),
        _: &DisplayHandle,
        di: &mut DataInit<'_, State>,
    ) {
        match request {
            wl_data_device_manager::Request::CreateDataSource { id } => {
                di.init(id, Mutex::new(Vec::<String>::new()));
            }
            wl_data_device_manager::Request::GetDataDevice { id, seat } => {
                let _: wl_seat::WlSeat = seat;
                let d = di.init(id, ());
                offer_to(state, &d);
                state.data.devices.push(d);
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_data_source::WlDataSource, Mutex<Vec<String>>> for State {
    fn request(
        state: &mut State,
        _: &Client,
        res: &wl_data_source::WlDataSource,
        request: wl_data_source::Request,
        data: &Mutex<Vec<String>>,
        _: &DisplayHandle,
        _: &mut DataInit<'_, State>,
    ) {
        match request {
            wl_data_source::Request::Offer { mime_type } => data.lock().unwrap().push(mime_type),
            wl_data_source::Request::Destroy => {
                if matches!(&state.data.selection, Some(Selection::Client(s)) if s == res) {
                    state.data.selection = None;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_data_device::WlDataDevice, ()> for State {
    fn request(
        state: &mut State,
        _: &Client,
        _: &wl_data_device::WlDataDevice,
        request: wl_data_device::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, State>,
    ) {
        if let wl_data_device::Request::SetSelection { source, .. } = request {
            if let Some(Selection::Client(old)) = state.data.selection.take() {
                if Some(&old) != source.as_ref() {
                    old.cancelled();
                }
            }
            state.data.selection = source.clone().map(Selection::Client);
            if let Some(src) = &source {
                fetch_text(src);
            }
            broadcast(state);
        }
    }

    fn destroyed(state: &mut State, _: wayland_server::backend::ClientId, r: &wl_data_device::WlDataDevice, _: &()) {
        state.data.devices.retain(|d| d != r);
    }
}

impl Dispatch<wl_data_offer::WlDataOffer, Selection> for State {
    fn request(
        _: &mut State,
        _: &Client,
        _: &wl_data_offer::WlDataOffer,
        request: wl_data_offer::Request,
        sel: &Selection,
        _: &DisplayHandle,
        _: &mut DataInit<'_, State>,
    ) {
        if let wl_data_offer::Request::Receive { mime_type, fd } = request {
            match sel {
                Selection::Client(src) => src.send(mime_type, fd.as_fd()),
                Selection::Android(text) => {
                    let text = text.clone();
                    std::thread::spawn(move || {
                        let mut f = std::fs::File::from(fd);
                        let _ = f.write_all(text.as_bytes());
                    });
                }
            }
        }
    }
}

#[allow(dead_code)]
fn _unused(f: &OwnedFd) -> i32 {
    f.as_raw_fd()
}
