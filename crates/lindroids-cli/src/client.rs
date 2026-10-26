//! Talking to the server's control socket.

use std::io::{BufRead, BufReader, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use lindroids_proto::{self as proto, CliReply, CliRequest};

pub fn socket_path(runtime: &Path) -> PathBuf {
    runtime.join("lindroids.sock")
}

pub fn connect(runtime: &Path) -> std::io::Result<UnixStream> {
    UnixStream::connect(socket_path(runtime))
}

/// Send one request and wait for its reply (up to `timeout`).
pub fn request(runtime: &Path, req: &CliRequest, timeout: Duration) -> Result<CliReply, String> {
    request_with_fds(runtime, req, &[], timeout)
}

pub fn request_with_fds(runtime: &Path, req: &CliRequest, fds: &[RawFd], timeout: Duration) -> Result<CliReply, String> {
    let mut s = connect(runtime).map_err(|e| format!("the Lindroids server is not running ({e})"))?;
    s.set_read_timeout(Some(timeout)).ok();
    let line = proto::encode(req);
    if fds.is_empty() {
        s.write_all(&line).map_err(|e| e.to_string())?;
    } else {
        proto::fdpass::send_with_fds(s.as_raw_fd(), &line, fds).map_err(|e| e.to_string())?;
    }
    let mut r = BufReader::new(s);
    let mut reply = String::new();
    r.read_line(&mut reply).map_err(|e| format!("no answer from the server: {e}"))?;
    if reply.is_empty() {
        return Err("the server closed the connection".into());
    }
    proto::decode(reply.trim_end().as_bytes()).map_err(|e| format!("bad answer from the server: {e}"))
}

pub fn status(runtime: &Path) -> Result<CliReply, String> {
    request(runtime, &CliRequest::Status, Duration::from_secs(5))
}
