//! Builds the libraries the CLI embeds and LD_PRELOADs into programs, both without libc, so clang
//! alone builds them for any target (LINDROIDS_CLANG names the clang to use; another CPU than the
//! host's needs lld):
//!
//! - liblindroids-compat.so (third_party/compat), x86 only: programs in glibc distros under proot
//!   on x86 Android need its alarm() (see the C file for why);
//! - liblindroids-gtk.so (third_party/gtkpicker): GTK 3's own file dialogs go to the desktop
//!   portal, which answers with Android's picker (see the C file).
//!
//! Without clang the CLI is built without them, and says so.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Build `src` into `out` as a shared library named `soname` for `triple`.
fn build(src: &Path, out: &Path, soname: &str, triple: &str) -> bool {
    let clang = std::env::var("LINDROIDS_CLANG").unwrap_or_else(|_| "clang".into());
    let args = [&format!("--target={triple}"), "-O2", "-fPIC", "-shared", "-nostdlib", "-Wl,--hash-style=both", &format!("-Wl,-soname,{soname}"), "-o"];
    // lld first (any CPU), then clang's default linker (the host's CPU).
    [Some("-fuse-ld=lld"), None].into_iter().any(|ld| {
        Command::new(&clang)
            .args(ld)
            .args(args)
            .arg(out)
            .arg(src)
            .stderr(std::process::Stdio::null())
            .status()
            .map_or(false, |s| s.success())
    })
}

fn main() {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../third_party");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    println!("cargo:rerun-if-env-changed=LINDROIDS_CLANG");
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();

    let compat = dir.join("compat/lindroids-compat.c");
    println!("cargo:rerun-if-changed={}", compat.display());
    let out = out_dir.join("liblindroids-compat.so");
    let triple = match arch.as_str() {
        "x86_64" => Some("x86_64-linux-gnu"),
        "x86" => Some("i686-linux-gnu"),
        _ => None,
    };
    let built = triple.map_or(false, |t| build(&compat, &out, "liblindroids-compat.so", t));
    if triple.is_some() && !built {
        println!("cargo:warning=clang could not build liblindroids-compat.so; xterm and other alarm() users in glibc distros will fail on x86 Android");
    }
    if !built {
        std::fs::write(&out, b"").unwrap();
    }

    let gtk = dir.join("gtkpicker/lindroids-gtk.c");
    println!("cargo:rerun-if-changed={}", gtk.display());
    let out = out_dir.join("liblindroids-gtk.so");
    // The programs' ABI, by glibc's names: the library has no libc, and bionic loads it as well.
    let triple = match arch.as_str() {
        "x86_64" => Some("x86_64-linux-gnu"),
        "x86" => Some("i686-linux-gnu"),
        "aarch64" => Some("aarch64-linux-gnu"),
        "arm" => Some("armv7-linux-gnueabihf"),
        _ => None,
    };
    let built = triple.map_or(false, |t| build(&gtk, &out, "liblindroids-gtk.so", t));
    if triple.is_some() && !built {
        println!("cargo:warning=clang could not build liblindroids-gtk.so; GTK 3 programs' own file dialogs will not show Android's picker");
    }
    if !built {
        std::fs::write(&out, b"").unwrap();
    }
}
