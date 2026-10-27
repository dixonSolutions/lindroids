//! `lindroids bwrap`: bubblewrap for Flatpak on rooted Android.
//!
//! Flatpak always asks bubblewrap for PID (and usually IPC and UTS) namespaces. Many Android
//! kernels are built without them (`/proc/self/ns/pid` is missing), so bubblewrap fails with
//! "Creating new namespace failed: Invalid argument". This stands in front of the real bubblewrap
//! (Termux's root-repo package) and leaves out exactly the namespace requests the kernel cannot
//! do — and the options that need them — so the rest of the sandbox (mount and network
//! namespaces, seccomp, the filesystem view) still applies. On a kernel that has them, nothing
//! changes. The Flatpak that Lindroids builds for Termux runs this as its bubblewrap.
//!
//! As root (the only way bubblewrap works on Android: `tsu`), a user namespace adds nothing, and on
//! these kernels it hides the host's `/proc/self/fd`, through which Flatpak passes the runtime and
//! app trees ("Can't find source path /proc/self/fd/18"): user namespace requests are left out
//! too, with the two options that need one.
//!
//! Flatpak passes most arguments through a file descriptor (`--args <fd>`, NUL-separated); those
//! are read and passed on as ordinary arguments.

use std::io::Read;
use std::os::fd::FromRawFd;
use std::os::unix::process::CommandExt;
use std::path::Path;

/// Namespaces this kernel has, as bubblewrap names them.
fn kernel_has(ns: &str) -> bool {
    Path::new("/proc/self/ns").join(ns).exists()
}

/// The arguments with unsupported namespace requests left out. `has` says which namespaces the
/// kernel has; "user" also says whether to use one (not as root).
pub fn filter(args: &[String], has: &dyn Fn(&str) -> bool) -> Vec<String> {
    let (pid, ipc, uts, user) = (has("pid"), has("ipc"), has("uts"), has("user"));
    let mut out = Vec::with_capacity(args.len());
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        // Flatpak puts the command after "--": nothing from there on is bubblewrap's.
        if a == "--" {
            out.extend(args[i..].iter().cloned());
            break;
        }
        match a {
            "--unshare-pid" | "--as-pid-1" if !pid => {}
            "--unshare-ipc" if !ipc => {}
            "--unshare-uts" if !uts => {}
            "--hostname" if !uts => i += 1,
            "--unshare-user" | "--unshare-user-try" | "--disable-userns" | "--assert-userns-disabled" if !user => {}
            "--unshare-all" => {
                out.extend(["--unshare-net", "--unshare-cgroup-try"].map(String::from));
                for (yes, flag) in [(pid, "--unshare-pid"), (ipc, "--unshare-ipc"), (uts, "--unshare-uts"), (user, "--unshare-user-try")] {
                    if yes {
                        out.push(flag.into());
                    }
                }
            }
            _ => out.push(a.to_string()),
        }
        i += 1;
    }
    out
}

/// Android has no `/etc/resolv.conf` (and other host files Flatpak binds with `--ro-bind-try`,
/// skipping them when they are missing): use Termux's copy, `<prefix>/etc/...`, instead. Without
/// it, apps have no DNS.
pub fn termux_host_files(args: Vec<String>, exists: &dyn Fn(&str) -> bool) -> Vec<String> {
    let mut out = args;
    let mut i = 0;
    while i + 2 < out.len() {
        if out[i] == "--" {
            break;
        }
        if matches!(out[i].as_str(), "--ro-bind-try" | "--bind-try") && out[i + 1].starts_with("/etc/") && !exists(&out[i + 1]) {
            let termux = format!("{}{}", lindroids_proto::TERMUX_PREFIX, out[i + 1]);
            if exists(&termux) {
                out[i + 1] = termux;
            }
        }
        i += 1;
    }
    out
}

/// Replace every `--args <fd>` by the arguments read from that fd.
fn expand_args_fds(args: &[String]) -> Result<Vec<String>, String> {
    let mut out = Vec::with_capacity(args.len());
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--args" && i + 1 < args.len() {
            let fd: i32 = args[i + 1].parse().map_err(|_| format!("bad --args fd {:?}", args[i + 1]))?;
            let mut buf = Vec::new();
            unsafe { std::fs::File::from_raw_fd(fd) }.read_to_end(&mut buf).map_err(|e| format!("--args {fd}: {e}"))?;
            // Each argument ends with a NUL; empty arguments are real (`--setenv X ""`).
            if buf.last() == Some(&0) {
                buf.pop();
            }
            if !buf.is_empty() {
                out.extend(buf.split(|&b| b == 0).map(|s| String::from_utf8_lossy(s).into_owned()));
            }
            i += 2;
        } else {
            out.push(args[i].clone());
            i += 1;
        }
    }
    Ok(out)
}

/// The real bubblewrap: LINDROIDS_BWRAP, else Termux's.
fn real_bwrap() -> String {
    std::env::var("LINDROIDS_BWRAP").unwrap_or_else(|_| format!("{}/bin/bwrap", lindroids_proto::TERMUX_PREFIX))
}

pub fn cmd_bwrap(args: &[String]) -> i32 {
    let expanded = match expand_args_fds(args) {
        Ok(a) => a,
        Err(e) => return crate::fail(&e),
    };
    let root = unsafe { libc::geteuid() } == 0;
    let filtered = filter(&expanded, &|ns: &str| if ns == "user" { !root && kernel_has(ns) } else { kernel_has(ns) });
    let filtered = termux_host_files(filtered, &|p: &str| Path::new(p).exists());
    let err = std::process::Command::new(real_bwrap()).args(&filtered).exec();
    crate::fail(&format!("cannot run bubblewrap ({}): {err}", real_bwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn drops_only_what_the_kernel_lacks() {
        let a = s(&["--unshare-pid", "--unshare-ipc", "--as-pid-1", "--hostname", "box", "--unshare-user-try", "--disable-userns", "--bind", "/a", "/b", "--unshare-net", "--", "sh", "--unshare-pid"]);
        let none = |_: &str| false;
        assert_eq!(filter(&a, &none), s(&["--bind", "/a", "/b", "--unshare-net", "--", "sh", "--unshare-pid"]));
        let all = |_: &str| true;
        assert_eq!(filter(&a, &all), a);
    }

    #[test]
    fn option_values_pass_through() {
        let a = s(&["--setenv", "PATH", "/app/bin:/usr/bin", "--chdir", "/", "--", "true"]);
        assert_eq!(filter(&a, &|_: &str| false), a);
    }

    #[test]
    fn missing_host_etc_files_come_from_termux() {
        let a = s(&["--ro-bind-try", "/etc/resolv.conf", "/etc/resolv.conf", "--ro-bind-try", "/etc/hosts", "/etc/hosts", "--", "x"]);
        let exists = |p: &str| p == "/etc/hosts" || p.ends_with("/usr/etc/resolv.conf");
        let out = termux_host_files(a, &exists);
        assert_eq!(out[1], format!("{}/etc/resolv.conf", lindroids_proto::TERMUX_PREFIX));
        assert_eq!(out[2], "/etc/resolv.conf");
        assert_eq!(out[4], "/etc/hosts");
    }

    #[test]
    fn unshare_all_keeps_what_is_there() {
        let out = filter(&s(&["--unshare-all", "--", "true"]), &|ns: &str| ns == "ipc");
        assert!(out.contains(&"--unshare-ipc".to_string()) && !out.contains(&"--unshare-pid".to_string()));
        assert_eq!(out.last().unwrap(), "true");
    }
}
