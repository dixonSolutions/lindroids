//! wl_compositor, wl_surface, wl_region, wl_subcompositor and wl_shm.

use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::sync::{Arc, Mutex};

use log::warn;
use wayland_protocols::wp::viewporter::server::{wp_viewport, wp_viewporter};
use wayland_server::backend::{ClientId, ObjectId};
use wayland_server::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_region, wl_shm, wl_shm_pool, wl_subcompositor, wl_subsurface,
    wl_surface,
};
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum};

use super::render::{convert_row, Image};
use super::State;

#[derive(Debug, Clone, PartialEq)]
pub enum Role {
    None,
    Toplevel(u32),
    Popup,
    Subsurface,
    Cursor,
}

#[derive(Default)]
pub struct Pending {
    /// Some(None) detaches the buffer.
    pub buffer: Option<Option<wl_buffer::WlBuffer>>,
    pub scale: Option<i32>,
    pub frame: Vec<wl_callback::WlCallback>,
    pub viewport_dst: Option<Option<(i32, i32)>>,
}

pub struct SubState {
    pub parent: ObjectId,
    pub x: i32,
    pub y: i32,
    pub pending_pos: Option<(i32, i32)>,
    pub sync: bool,
    /// State committed while synchronized, applied with the parent's next commit.
    pub cached: Option<Pending>,
}

pub struct Surface {
    pub res: wl_surface::WlSurface,
    pub client: ClientId,
    pub pending: Pending,
    pub image: Option<Arc<Image>>,
    pub buffer_scale: i32,
    pub viewport_dst: Option<(i32, i32)>,
    pub frame_cbs: Vec<wl_callback::WlCallback>,
    pub role: Role,
    /// Subsurfaces, bottom to top. Entries flagged `below` are drawn under this surface.
    pub children: Vec<(ObjectId, bool)>,
    pub sub: Option<SubState>,
    pub committed_once: bool,
    pub xdg: Option<super::shell::XdgInfo>,
}

impl Surface {
    /// Size in surface-local (logical) coordinates.
    pub fn logical_size(&self) -> (i32, i32) {
        if let Some(d) = self.viewport_dst {
            return d;
        }
        match &self.image {
            Some(img) => {
                let s = self.buffer_scale.max(1);
                (img.width as i32 / s, img.height as i32 / s)
            }
            None => (0, 0),
        }
    }
}

pub fn register_globals(dh: &DisplayHandle) {
    dh.create_global::<State, wl_compositor::WlCompositor, ()>(5, ());
    dh.create_global::<State, wl_subcompositor::WlSubcompositor, ()>(1, ());
    dh.create_global::<State, wl_shm::WlShm, ()>(1, ());
    dh.create_global::<State, wp_viewporter::WpViewporter, ()>(1, ());
}

// ------------------------------------------------------------------ wl_compositor

impl GlobalDispatch<wl_compositor::WlCompositor, ()> for State {
    fn bind(_: &mut State, _: &DisplayHandle, _: &Client, r: New<wl_compositor::WlCompositor>, _: &(), di: &mut DataInit<'_, State>) {
        di.init(r, ());
    }
}

impl Dispatch<wl_compositor::WlCompositor, ()> for State {
    fn request(
        state: &mut State,
        client: &Client,
        _: &wl_compositor::WlCompositor,
        request: wl_compositor::Request,
        _: &(),
        _: &DisplayHandle,
        di: &mut DataInit<'_, State>,
    ) {
        match request {
            wl_compositor::Request::CreateSurface { id } => {
                let s = di.init(id, ());
                state.surfaces.insert(
                    s.id(),
                    Surface {
                        res: s,
                        client: client.id(),
                        pending: Pending::default(),
                        image: None,
                        buffer_scale: 1,
                        viewport_dst: None,
                        frame_cbs: Vec::new(),
                        role: Role::None,
                        children: Vec::new(),
                        sub: None,
                        committed_once: false,
                        xdg: None,
                    },
                );
            }
            wl_compositor::Request::CreateRegion { id } => {
                di.init(id, ());
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_region::WlRegion, ()> for State {
    fn request(_: &mut State, _: &Client, _: &wl_region::WlRegion, _: wl_region::Request, _: &(), _: &DisplayHandle, _: &mut DataInit<'_, State>) {
        // Input and opaque regions are not used: windows take input everywhere.
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for State {
    fn request(_: &mut State, _: &Client, _: &wl_callback::WlCallback, _: wl_callback::Request, _: &(), _: &DisplayHandle, _: &mut DataInit<'_, State>) {}
}

// ------------------------------------------------------------------ wl_surface

impl Dispatch<wl_surface::WlSurface, ()> for State {
    fn request(
        state: &mut State,
        _: &Client,
        res: &wl_surface::WlSurface,
        request: wl_surface::Request,
        _: &(),
        _: &DisplayHandle,
        di: &mut DataInit<'_, State>,
    ) {
        let id = res.id();
        match request {
            wl_surface::Request::Attach { buffer, .. } => {
                if let Some(s) = state.surfaces.get_mut(&id) {
                    s.pending.buffer = Some(buffer);
                }
            }
            wl_surface::Request::Frame { callback } => {
                let cb = di.init(callback, ());
                if let Some(s) = state.surfaces.get_mut(&id) {
                    s.pending.frame.push(cb);
                }
            }
            wl_surface::Request::SetBufferScale { scale } => {
                if let Some(s) = state.surfaces.get_mut(&id) {
                    s.pending.scale = Some(scale.max(1));
                }
            }
            wl_surface::Request::Commit => commit(state, &id),
            wl_surface::Request::Destroy => {}
            // Damage, regions, transform and offset: every commit redraws the whole window.
            _ => {}
        }
    }

    fn destroyed(state: &mut State, _: ClientId, res: &wl_surface::WlSurface, _: &()) {
        let id = res.id();
        super::shell::surface_destroyed(state, &id);
        if let Some(s) = state.surfaces.remove(&id) {
            if let Some(sub) = &s.sub {
                if let Some(p) = state.surfaces.get_mut(&sub.parent) {
                    p.children.retain(|(c, _)| c != &id);
                }
                state.mark_dirty(&sub.parent);
            }
            for (child, _) in s.children {
                if let Some(c) = state.surfaces.get_mut(&child) {
                    c.sub = None;
                    c.role = Role::None;
                }
            }
        }
        super::seat::surface_destroyed(state, &id);
    }
}

fn commit(state: &mut State, id: &ObjectId) {
    let Some(s) = state.surfaces.get_mut(id) else { return };
    // A synchronized subsurface keeps its state until the parent commits.
    if let Some(sub) = &mut s.sub {
        if sub.sync {
            let p = std::mem::take(&mut s.pending);
            match &mut sub.cached {
                Some(c) => merge(c, p),
                None => sub.cached = Some(p),
            }
            return;
        }
    }
    let p = std::mem::take(&mut s.pending);
    apply(state, id, p);
}

fn merge(into: &mut Pending, p: Pending) {
    if p.buffer.is_some() {
        into.buffer = p.buffer;
    }
    if p.scale.is_some() {
        into.scale = p.scale;
    }
    if p.viewport_dst.is_some() {
        into.viewport_dst = p.viewport_dst;
    }
    into.frame.extend(p.frame);
}

fn apply(state: &mut State, id: &ObjectId, p: Pending) {
    let Some(s) = state.surfaces.get_mut(id) else { return };
    if let Some(b) = p.buffer {
        match b {
            Some(buf) => {
                match read_buffer(&buf) {
                    Ok(img) => s.image = Some(Arc::new(img)),
                    Err(e) => warn!("unreadable buffer: {e}"),
                }
                buf.release();
            }
            None => s.image = None,
        }
    }
    if let Some(sc) = p.scale {
        s.buffer_scale = sc;
    }
    if let Some(v) = p.viewport_dst {
        s.viewport_dst = v;
    }
    log::debug!("commit {:?}: buffer={} frame_cbs+{}", id, s.image.is_some(), p.frame.len());
    s.frame_cbs.extend(p.frame);
    let first = !s.committed_once;
    s.committed_once = true;
    let role = s.role.clone();
    let children: Vec<ObjectId> = s.children.iter().map(|(c, _)| c.clone()).collect();

    // Children: pending positions and cached synchronized state land with this commit.
    for c in children {
        let mut cached = None;
        if let Some(cs) = state.surfaces.get_mut(&c) {
            if let Some(sub) = &mut cs.sub {
                if let Some((x, y)) = sub.pending_pos.take() {
                    sub.x = x;
                    sub.y = y;
                }
                cached = sub.cached.take();
            }
        }
        if let Some(p) = cached {
            apply(state, &c, p);
        }
    }

    match role {
        Role::Toplevel(_) | Role::Popup | Role::None => super::shell::surface_committed(state, id, first),
        _ => {}
    }
    state.mark_dirty(id);
}

fn read_buffer(buf: &wl_buffer::WlBuffer) -> Result<Image, String> {
    let data = buf.data::<Arc<BufferData>>().ok_or("not a wl_shm buffer")?;
    let pool = data.pool.lock().unwrap();
    let needed = data.offset as usize + data.stride as usize * data.height as usize;
    // A client that shrinks the file under us would crash us with SIGBUS; check first.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(pool.fd.as_raw_fd(), &mut st) } == 0 && (st.st_size as usize) < needed {
        return Err("buffer is past the end of its pool".into());
    }
    if needed > pool.len {
        return Err("buffer is past the end of its pool".into());
    }
    let mut img = Image { width: data.width as u32, height: data.height as u32, data: vec![0; (data.width * data.height) as usize], opaque: true };
    let base = unsafe { std::slice::from_raw_parts(pool.ptr as *const u8, pool.len) };
    let w = data.width as usize;
    for y in 0..data.height as usize {
        let off = data.offset as usize + y * data.stride as usize;
        let row = &base[off..off + w * 4];
        let o = convert_row(data.format, row, &mut img.data[y * w..(y + 1) * w]);
        img.opaque &= o;
    }
    Ok(img)
}

// ------------------------------------------------------------------ subsurfaces

impl GlobalDispatch<wl_subcompositor::WlSubcompositor, ()> for State {
    fn bind(_: &mut State, _: &DisplayHandle, _: &Client, r: New<wl_subcompositor::WlSubcompositor>, _: &(), di: &mut DataInit<'_, State>) {
        di.init(r, ());
    }
}

impl Dispatch<wl_subcompositor::WlSubcompositor, ()> for State {
    fn request(
        state: &mut State,
        _: &Client,
        _: &wl_subcompositor::WlSubcompositor,
        request: wl_subcompositor::Request,
        _: &(),
        _: &DisplayHandle,
        di: &mut DataInit<'_, State>,
    ) {
        if let wl_subcompositor::Request::GetSubsurface { id, surface, parent } = request {
            let sid = surface.id();
            let pid = parent.id();
            di.init(id, sid.clone());
            if let Some(s) = state.surfaces.get_mut(&sid) {
                s.role = Role::Subsurface;
                s.sub = Some(SubState { parent: pid.clone(), x: 0, y: 0, pending_pos: None, sync: true, cached: None });
            }
            if let Some(p) = state.surfaces.get_mut(&pid) {
                p.children.push((sid, false));
            }
        }
    }
}

impl Dispatch<wl_subsurface::WlSubsurface, ObjectId> for State {
    fn request(
        state: &mut State,
        _: &Client,
        _: &wl_subsurface::WlSubsurface,
        request: wl_subsurface::Request,
        sid: &ObjectId,
        _: &DisplayHandle,
        _: &mut DataInit<'_, State>,
    ) {
        match request {
            wl_subsurface::Request::SetPosition { x, y } => {
                if let Some(sub) = state.surfaces.get_mut(sid).and_then(|s| s.sub.as_mut()) {
                    sub.pending_pos = Some((x, y));
                }
            }
            wl_subsurface::Request::SetSync => {
                if let Some(sub) = state.surfaces.get_mut(sid).and_then(|s| s.sub.as_mut()) {
                    sub.sync = true;
                }
            }
            wl_subsurface::Request::SetDesync => {
                let mut cached = None;
                if let Some(sub) = state.surfaces.get_mut(sid).and_then(|s| s.sub.as_mut()) {
                    sub.sync = false;
                    cached = sub.cached.take();
                }
                if let Some(p) = cached {
                    apply(state, sid, p);
                }
            }
            wl_subsurface::Request::PlaceAbove { sibling } => restack(state, sid, &sibling.id(), true),
            wl_subsurface::Request::PlaceBelow { sibling } => restack(state, sid, &sibling.id(), false),
            wl_subsurface::Request::Destroy => {
                let parent = state.surfaces.get(sid).and_then(|s| s.sub.as_ref().map(|x| x.parent.clone()));
                if let Some(pid) = parent {
                    if let Some(p) = state.surfaces.get_mut(&pid) {
                        p.children.retain(|(c, _)| c != sid);
                    }
                    state.mark_dirty(&pid);
                }
                if let Some(s) = state.surfaces.get_mut(sid) {
                    s.sub = None;
                    s.role = Role::None;
                }
            }
            _ => {}
        }
    }
}

fn restack(state: &mut State, sid: &ObjectId, sibling: &ObjectId, above: bool) {
    let Some(pid) = state.surfaces.get(sid).and_then(|s| s.sub.as_ref().map(|x| x.parent.clone())) else { return };
    let Some(p) = state.surfaces.get_mut(&pid) else { return };
    let Some(pos) = p.children.iter().position(|(c, _)| c == sid) else { return };
    let me = p.children.remove(pos);
    if sibling == &pid {
        // Relative to the parent itself: right above it is the first slot after every child drawn
        // below it; right below it is the last of those.
        let first_above = p.children.iter().position(|(_, b)| !*b).unwrap_or(p.children.len());
        p.children.insert(first_above, (me.0, !above));
        return;
    }
    let Some(spos) = p.children.iter().position(|(c, _)| c == sibling) else {
        p.children.push(me);
        return;
    };
    let below_flag = p.children[spos].1;
    let at = if above { spos + 1 } else { spos };
    p.children.insert(at, (me.0, below_flag));
}

// ------------------------------------------------------------------ wl_shm

pub struct Pool {
    pub fd: OwnedFd,
    pub ptr: *mut libc::c_void,
    pub len: usize,
}

// The mapping is only touched from the server thread.
unsafe impl Send for Pool {}

impl Drop for Pool {
    fn drop(&mut self) {
        if !self.ptr.is_null() && self.len > 0 {
            unsafe { libc::munmap(self.ptr, self.len) };
        }
    }
}

fn map(fd: &OwnedFd, len: usize) -> *mut libc::c_void {
    let p = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ, libc::MAP_SHARED, fd.as_raw_fd(), 0) };
    if p == libc::MAP_FAILED { std::ptr::null_mut() } else { p }
}

pub struct BufferData {
    pub pool: Arc<Mutex<Pool>>,
    pub offset: i32,
    pub width: i32,
    pub height: i32,
    pub stride: i32,
    pub format: u32,
}

impl GlobalDispatch<wl_shm::WlShm, ()> for State {
    fn bind(_: &mut State, _: &DisplayHandle, _: &Client, r: New<wl_shm::WlShm>, _: &(), di: &mut DataInit<'_, State>) {
        let shm = di.init(r, ());
        for f in [wl_shm::Format::Argb8888, wl_shm::Format::Xrgb8888, wl_shm::Format::Abgr8888, wl_shm::Format::Xbgr8888] {
            shm.format(f);
        }
    }
}

impl Dispatch<wl_shm::WlShm, ()> for State {
    fn request(_: &mut State, _: &Client, shm: &wl_shm::WlShm, request: wl_shm::Request, _: &(), _: &DisplayHandle, di: &mut DataInit<'_, State>) {
        if let wl_shm::Request::CreatePool { id, fd, size } = request {
            if size <= 0 {
                shm.post_error(wl_shm::Error::InvalidStride, "pool size must be positive");
                return;
            }
            let ptr = map(&fd, size as usize);
            if ptr.is_null() {
                shm.post_error(wl_shm::Error::InvalidFd, "cannot map the pool");
                return;
            }
            di.init(id, Arc::new(Mutex::new(Pool { fd, ptr, len: size as usize })));
        }
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, Arc<Mutex<Pool>>> for State {
    fn request(
        _: &mut State,
        _: &Client,
        res: &wl_shm_pool::WlShmPool,
        request: wl_shm_pool::Request,
        pool: &Arc<Mutex<Pool>>,
        _: &DisplayHandle,
        di: &mut DataInit<'_, State>,
    ) {
        match request {
            wl_shm_pool::Request::CreateBuffer { id, offset, width, height, stride, format } => {
                let fmt = match format {
                    WEnum::Value(f) => f as u32,
                    WEnum::Unknown(u) => u,
                };
                let ok_fmt = [wl_shm::Format::Argb8888, wl_shm::Format::Xrgb8888, wl_shm::Format::Abgr8888, wl_shm::Format::Xbgr8888]
                    .iter()
                    .any(|f| *f as u32 == fmt);
                let len = pool.lock().unwrap().len;
                if !ok_fmt {
                    res.post_error(wl_shm::Error::InvalidFormat, "unsupported format");
                    return;
                }
                if width <= 0 || height <= 0 || stride < width * 4 || offset < 0
                    || offset as usize + stride as usize * height as usize > len
                {
                    res.post_error(wl_shm::Error::InvalidStride, "buffer does not fit its pool");
                    return;
                }
                di.init(id, Arc::new(BufferData { pool: pool.clone(), offset, width, height, stride, format: fmt }));
            }
            wl_shm_pool::Request::Resize { size } => {
                let mut p = pool.lock().unwrap();
                if size as usize > p.len {
                    let fd = p.fd.as_fd().try_clone_to_owned();
                    if let Ok(fd) = fd {
                        let ptr = map(&fd, size as usize);
                        if !ptr.is_null() {
                            unsafe { libc::munmap(p.ptr, p.len) };
                            p.ptr = ptr;
                            p.len = size as usize;
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, Arc<BufferData>> for State {
    fn request(_: &mut State, _: &Client, _: &wl_buffer::WlBuffer, _: wl_buffer::Request, _: &Arc<BufferData>, _: &DisplayHandle, _: &mut DataInit<'_, State>) {}
}

// ------------------------------------------------------------------ wp_viewporter

impl GlobalDispatch<wp_viewporter::WpViewporter, ()> for State {
    fn bind(_: &mut State, _: &DisplayHandle, _: &Client, r: New<wp_viewporter::WpViewporter>, _: &(), di: &mut DataInit<'_, State>) {
        di.init(r, ());
    }
}

impl Dispatch<wp_viewporter::WpViewporter, ()> for State {
    fn request(_: &mut State, _: &Client, _: &wp_viewporter::WpViewporter, request: wp_viewporter::Request, _: &(), _: &DisplayHandle, di: &mut DataInit<'_, State>) {
        if let wp_viewporter::Request::GetViewport { id, surface } = request {
            di.init(id, surface.id());
        }
    }
}

impl Dispatch<wp_viewport::WpViewport, ObjectId> for State {
    fn request(state: &mut State, _: &Client, _: &wp_viewport::WpViewport, request: wp_viewport::Request, sid: &ObjectId, _: &DisplayHandle, _: &mut DataInit<'_, State>) {
        match request {
            wp_viewport::Request::SetDestination { width, height } => {
                if let Some(s) = state.surfaces.get_mut(sid) {
                    s.pending.viewport_dst = Some(if width > 0 && height > 0 { Some((width, height)) } else { None });
                }
            }
            wp_viewport::Request::Destroy => {
                if let Some(s) = state.surfaces.get_mut(sid) {
                    s.pending.viewport_dst = Some(None);
                }
            }
            // Cropping (set_source) is not applied; the whole buffer is scaled to the destination.
            _ => {}
        }
    }
}
