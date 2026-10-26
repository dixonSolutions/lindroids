//! Builds liblindroids-compat.so (third_party/compat) for x86 targets and embeds it: programs in
//! glibc distros under proot on x86 Android need its alarm() (see the C file for why). It has no
//! libc, so clang alone builds it for any x86 target; without clang the CLI is built without it,
//! and says so.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    let src = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../third_party/compat/lindroids-compat.c");
    println!("cargo:rerun-if-changed={}", src.display());
    println!("cargo:rerun-if-env-changed=LINDROIDS_CLANG");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("liblindroids-compat.so");
    let triple = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86_64") => Some("x86_64-linux-gnu"),
        Ok("x86") => Some("i686-linux-gnu"),
        _ => None,
    };
    let built = triple.map_or(false, |t| {
        let clang = std::env::var("LINDROIDS_CLANG").unwrap_or_else(|_| "clang".into());
        Command::new(clang)
            .args([&format!("--target={t}"), "-O2", "-fPIC", "-shared", "-nostdlib", "-Wl,--hash-style=both", "-Wl,-soname,liblindroids-compat.so", "-o"])
            .arg(&out)
            .arg(&src)
            .status()
            .map_or(false, |s| s.success())
    });
    if triple.is_some() && !built {
        println!("cargo:warning=clang could not build liblindroids-compat.so; xterm and other alarm() users in glibc distros will fail on x86 Android");
    }
    if !built {
        std::fs::write(&out, b"").unwrap();
    }
}
