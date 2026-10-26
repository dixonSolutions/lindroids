//! Shared memory the app can map: frames go to the app as fds, never as copies through a socket.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;

/// An fd of `len` bytes, from memfd_create where the kernel has it (3.17+), else an unlinked
/// file in `dir` (Android 7 devices on 3.10 kernels).
pub fn create(len: usize, dir: &Path) -> std::io::Result<OwnedFd> {
    let fd = unsafe {
        let name = b"lindroids-frame\0";
        libc::syscall(libc::SYS_memfd_create, name.as_ptr(), libc::MFD_CLOEXEC as libc::c_uint) as i32
    };
    let fd = if fd >= 0 {
        unsafe { OwnedFd::from_raw_fd(fd) }
    } else {
        let path = dir.join(format!("shm-{}-{}", std::process::id(), rand_u64()));
        let f = std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&path)?;
        let _ = std::fs::remove_file(&path);
        OwnedFd::from(f)
    };
    if unsafe { libc::ftruncate(fd.as_raw_fd(), len as libc::off_t) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(fd)
}

pub struct Mapping {
    pub ptr: *mut u8,
    pub len: usize,
}

unsafe impl Send for Mapping {}

impl Mapping {
    pub fn new(fd: &OwnedFd, len: usize, writable: bool) -> std::io::Result<Mapping> {
        let prot = if writable { libc::PROT_READ | libc::PROT_WRITE } else { libc::PROT_READ };
        let p = unsafe { libc::mmap(std::ptr::null_mut(), len, prot, libc::MAP_SHARED, fd.as_raw_fd(), 0) };
        if p == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Mapping { ptr: p as *mut u8, len })
    }

    pub fn as_u32_mut(&mut self) -> &mut [u32] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr as *mut u32, self.len / 4) }
    }

    pub fn as_u32(&self) -> &[u32] {
        unsafe { std::slice::from_raw_parts(self.ptr as *const u32, self.len / 4) }
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.ptr as *mut _, self.len) };
    }
}

pub fn rand_u64() -> u64 {
    let mut b = [0u8; 8];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = std::io::Read::read_exact(&mut f, &mut b);
    }
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    u64::from_le_bytes(b) ^ t.rotate_left(17) ^ (std::process::id() as u64)
}

/// A random string of `n` characters from `alphabet`.
pub fn random_string(n: usize, alphabet: &[u8]) -> String {
    let mut out = String::with_capacity(n);
    let mut pool = 0u64;
    let mut bits = 0;
    while out.len() < n {
        if bits < 8 {
            pool = rand_u64();
            bits = 64;
        }
        let c = alphabet[(pool & 0xff) as usize % alphabet.len()];
        pool >>= 8;
        bits -= 8;
        out.push(c as char);
    }
    out
}
