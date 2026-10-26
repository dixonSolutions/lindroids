//! Passing file descriptors over Unix sockets (SCM_RIGHTS), with newline-delimited messages.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;

/// Send `data` with `fds` attached to its first byte.
pub fn send_with_fds(sock: RawFd, data: &[u8], fds: &[RawFd]) -> io::Result<()> {
    let mut iov = libc::iovec { iov_base: data.as_ptr() as *mut _, iov_len: data.len() };
    let space = unsafe { libc::CMSG_SPACE((fds.len() * std::mem::size_of::<RawFd>()) as u32) } as usize;
    let mut cbuf = vec![0u8; space];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    if !fds.is_empty() {
        msg.msg_control = cbuf.as_mut_ptr() as *mut _;
        msg.msg_controllen = space as _;
        unsafe {
            let c = libc::CMSG_FIRSTHDR(&msg);
            (*c).cmsg_level = libc::SOL_SOCKET;
            (*c).cmsg_type = libc::SCM_RIGHTS;
            (*c).cmsg_len = libc::CMSG_LEN((fds.len() * std::mem::size_of::<RawFd>()) as u32) as _;
            std::ptr::copy_nonoverlapping(fds.as_ptr(), libc::CMSG_DATA(c) as *mut RawFd, fds.len());
        }
    }
    let n = unsafe { libc::sendmsg(sock, &msg, libc::MSG_NOSIGNAL) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    // Whatever did not fit goes as plain data (the fds already travelled).
    let n = n as usize;
    if n < data.len() {
        let mut s = unsafe { std::mem::ManuallyDrop::new(UnixStream::from_raw_fd(sock)) };
        io::Write::write_all(&mut *s, &data[n..])?;
    }
    Ok(())
}

/// Receive what is available. Complete lines go to `lines`, received fds to `fds`.
/// Returns true when the peer closed the connection. Works on blocking and non-blocking sockets;
/// on a blocking socket it returns after one read.
pub fn read_lines_with_fds(stream: &UnixStream, buf: &mut Vec<u8>, lines: &mut Vec<Vec<u8>>, fds: &mut Vec<OwnedFd>) -> bool {
    let mut closed = false;
    loop {
        let mut data = [0u8; 16384];
        let mut iov = libc::iovec { iov_base: data.as_mut_ptr() as *mut _, iov_len: data.len() };
        let mut cbuf = [0u8; 256];
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cbuf.as_mut_ptr() as *mut _;
        msg.msg_controllen = cbuf.len() as _;
        let n = unsafe { libc::recvmsg(stream.as_raw_fd(), &mut msg, libc::MSG_CMSG_CLOEXEC | libc::MSG_DONTWAIT) };
        if n < 0 {
            let e = io::Error::last_os_error();
            match e.kind() {
                io::ErrorKind::WouldBlock => break,
                io::ErrorKind::Interrupted => continue,
                _ => {
                    closed = true;
                    break;
                }
            }
        }
        unsafe {
            let mut c = libc::CMSG_FIRSTHDR(&msg);
            while !c.is_null() {
                if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                    let count = ((*c).cmsg_len as usize - libc::CMSG_LEN(0) as usize) / std::mem::size_of::<RawFd>();
                    let p = libc::CMSG_DATA(c) as *const RawFd;
                    for i in 0..count {
                        fds.push(OwnedFd::from_raw_fd(std::ptr::read_unaligned(p.add(i))));
                    }
                }
                c = libc::CMSG_NXTHDR(&msg, c);
            }
        }
        if n == 0 {
            closed = true;
            break;
        }
        buf.extend_from_slice(&data[..n as usize]);
    }
    while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
        let line: Vec<u8> = buf.drain(..=pos).collect();
        if line.len() > 1 {
            lines.push(line[..line.len() - 1].to_vec());
        }
    }
    closed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn passes_a_pipe() {
        let (a, b) = UnixStream::pair().unwrap();
        let mut p = [0; 2];
        unsafe { libc::pipe(p.as_mut_ptr()) };
        send_with_fds(a.as_raw_fd(), b"{\"t\":\"x\"}\n", &[p[0]]).unwrap();
        unsafe { libc::write(p[1], b"hi".as_ptr() as *const _, 2) };
        unsafe { libc::close(p[1]) };
        let (mut buf, mut lines, mut fds) = (Vec::new(), Vec::new(), Vec::new());
        read_lines_with_fds(&b, &mut buf, &mut lines, &mut fds);
        assert_eq!(lines, vec![b"{\"t\":\"x\"}".to_vec()]);
        assert_eq!(fds.len(), 1);
        let mut s = String::new();
        std::fs::File::from(fds.pop().unwrap()).read_to_string(&mut s).unwrap();
        assert_eq!(s, "hi");
    }
}
