//! A tiny X11 window manager for X11 programs under a rootful Xwayland in one Android window:
//! normal windows fill the screen (and follow it when the Android window resizes), dialogs are
//! centred, and the newest window gets the focus. Override-redirect windows (menus, tooltips)
//! are not managed, as with any window manager.

use std::os::unix::net::UnixStream;
use std::path::Path;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::{DefaultStream, RustConnection};

fn atom(conn: &RustConnection, name: &str) -> Result<Atom, String> {
    Ok(conn.intern_atom(false, name.as_bytes()).map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?.atom)
}

pub fn run(socket: &Path) -> Result<(), String> {
    let stream = UnixStream::connect(socket).map_err(|e| e.to_string())?;
    let (stream, _) = DefaultStream::from_unix_stream(stream).map_err(|e| e.to_string())?;
    let conn = RustConnection::connect_to_stream(stream, 0).map_err(|e| e.to_string())?;
    let screen = conn.setup().roots[0].clone();
    let root = screen.root;
    conn.change_window_attributes(
        root,
        &ChangeWindowAttributesAux::new().event_mask(EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY | EventMask::STRUCTURE_NOTIFY),
    )
    .map_err(|e| e.to_string())?
    .check()
    .map_err(|_| "another window manager is running".to_string())?;
    let wm_type = atom(&conn, "_NET_WM_WINDOW_TYPE")?;
    let dialog_types = [atom(&conn, "_NET_WM_WINDOW_TYPE_DIALOG")?, atom(&conn, "_NET_WM_WINDOW_TYPE_UTILITY")?, atom(&conn, "_NET_WM_WINDOW_TYPE_SPLASH")?];
    let (mut sw, mut sh) = (screen.width_in_pixels, screen.height_in_pixels);
    let mut managed: Vec<(Window, bool)> = Vec::new();

    let is_dialog = |w: Window| -> bool {
        let transient = conn
            .get_property(false, w, AtomEnum::WM_TRANSIENT_FOR, AtomEnum::WINDOW, 0, 1)
            .ok()
            .and_then(|c| c.reply().ok())
            .map_or(false, |r| r.value_len > 0);
        let typed = conn
            .get_property(false, w, wm_type, AtomEnum::ATOM, 0, 8)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().map(|v| v.collect::<Vec<u32>>()))
            .map_or(false, |v| v.iter().any(|a| dialog_types.contains(a)));
        transient || typed
    };
    let place = |w: Window, dialog: bool, sw: u16, sh: u16| {
        if dialog {
            if let Ok(Ok(g)) = conn.get_geometry(w).map(|c| c.reply()) {
                let gw = g.width.min(sw);
                let gh = g.height.min(sh);
                let aux = ConfigureWindowAux::new().x(((sw - gw) / 2) as i32).y(((sh - gh) / 2) as i32).width(gw as u32).height(gh as u32);
                let _ = conn.configure_window(w, &aux);
            }
        } else {
            let aux = ConfigureWindowAux::new().x(0).y(0).width(sw as u32).height(sh as u32).border_width(0);
            let _ = conn.configure_window(w, &aux);
        }
    };
    let focus = |w: Window| {
        let _ = conn.configure_window(w, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE));
        let _ = conn.set_input_focus(InputFocus::POINTER_ROOT, w, x11rb::CURRENT_TIME);
    };

    loop {
        let ev = conn.wait_for_event().map_err(|e| e.to_string())?;
        match ev {
            Event::MapRequest(e) => {
                let dialog = is_dialog(e.window);
                if !managed.iter().any(|(w, _)| *w == e.window) {
                    managed.push((e.window, dialog));
                }
                place(e.window, dialog, sw, sh);
                let _ = conn.map_window(e.window);
                focus(e.window);
            }
            Event::ConfigureRequest(e) => match managed.iter().find(|(w, _)| *w == e.window) {
                Some((w, false)) => place(*w, false, sw, sh),
                _ => {
                    // Not ours yet, or a dialog: grant what it asks, within the screen.
                    let mut aux = ConfigureWindowAux::from_configure_request(&e);
                    if let Some(width) = aux.width {
                        aux.width = Some(width.min(sw as u32));
                    }
                    if let Some(height) = aux.height {
                        aux.height = Some(height.min(sh as u32));
                    }
                    let _ = conn.configure_window(e.window, &aux);
                }
            },
            Event::ConfigureNotify(e) if e.window == root => {
                if (e.width, e.height) != (sw, sh) {
                    sw = e.width;
                    sh = e.height;
                    for (w, d) in &managed {
                        place(*w, *d, sw, sh);
                    }
                }
            }
            Event::UnmapNotify(e) => {
                managed.retain(|(w, _)| *w != e.window);
                if let Some((w, _)) = managed.last() {
                    focus(*w);
                }
            }
            Event::DestroyNotify(e) => managed.retain(|(w, _)| *w != e.window),
            _ => {}
        }
        let _ = conn.flush();
    }
}
