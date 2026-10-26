# Third-party components

Lindroids itself is MIT OR Apache-2.0. The APK and the packages also carry the programs below,
which keep their own licences:

- [proot, talloc, libandroid-shmem](#proot-in-the-apk), in the APK;
- [xwayland-satellite](#xwayland-satellite), in the `.deb`s and the static tarballs.

Only this file and `xwayland-satellite/*.patch` are committed. `third_party/proot/` is produced
by `scripts/fetch-proot.sh`, which downloads Termux's prebuilt packages at pinned versions
(sha256-checked), and is gitignored; xwayland-satellite is built under `target/xwls/`.

## proot in the APK

### Why they are in the APK

An app exported with `lindroids export --bundle` runs from the Lindroids app's own storage, with
Termux closed or not installed at all. It runs under proot, so the APK carries proot and the two
libraries Termux's proot build links. They are installed as `lib/<abi>/lib*.so` so that Android
extracts them into the app's native library directory, from where the app executes them.

| File in `lib/<abi>/`   | From Termux package (file)            | Upstream                                         | Licence             |
| ---------------------- | ------------------------------------- | ------------------------------------------------ | ------------------- |
| `libproot.so`          | `proot` (`bin/proot`)                 | https://github.com/termux/proot                  | GPL-2.0-or-later    |
| `libproot-loader.so`   | `proot` (`libexec/proot/loader`)      | https://github.com/termux/proot                  | GPL-2.0-or-later    |
| `libproot-loader32.so` | `proot` (`libexec/proot/loader32`)    | https://github.com/termux/proot                  | GPL-2.0-or-later    |
| `libtalloc.so`         | `libtalloc` (`lib/libtalloc.so.2.x`)  | https://talloc.samba.org/ (part of Samba)        | LGPL-3.0-or-later   |
| `libandroid-shmem.so`  | `libandroid-shmem`                    | https://github.com/termux/libandroid-shmem       | BSD-3-Clause        |

`libproot-loader32.so` exists only for the 64-bit ABIs (it runs 32-bit guests).

The exact versions are the `PINS` in `scripts/fetch-proot.sh`. The Termux build recipes, with
the patches Termux applies, are at
https://github.com/termux/termux-packages/tree/master/packages/proot,
https://github.com/termux/termux-packages/tree/master/packages/libtalloc and
https://github.com/termux/termux-packages/tree/master/packages/libandroid-shmem; the binaries
come from https://packages.termux.dev/apt/termux-main.

### Modifications

The binaries are not rebuilt. `scripts/fetch-proot.sh` only edits their dynamic sections with
`patchelf`:

- `libproot.so`: `NEEDED libtalloc.so.2` → `libtalloc.so`; the Termux `RUNPATH`
  (`/data/data/com.termux/files/usr/lib`) is removed.
- `libtalloc.so`: `SONAME` set to `libtalloc.so`; `RUNPATH` removed.
- `libandroid-shmem.so`: `RUNPATH` removed.

### Source code offer

proot is GPL-2.0 and talloc is LGPL-3.0. The corresponding source for the binaries distributed
in a Lindroids release is the upstream source at the versions pinned in
`scripts/fetch-proot.sh`, plus the Termux patches in the recipes linked above (Termux also
publishes source on its package pages). If those links stop working, open an issue at
https://github.com/dixonSolutions/lindroids/issues and the source will be provided.

## xwayland-satellite

[xwayland-satellite](https://github.com/Supreeeme/xwayland-satellite) by Shawn Wallace and
contributors runs Xwayland rootless and turns each X11 window into its own Wayland toplevel;
`lindroids run --x11` uses it, so every X11 window becomes an Android window. Licence:
**MPL-2.0** (Mozilla Public License 2.0). Its text ships next to the binary's documentation as
`LICENSE.xwayland-satellite`.

| | |
| --- | --- |
| Version | 0.8.2 (tag `v0.8.2`, commit `8d135d3b2854b30fd01ea6cd6c27e523dd50a839`) |
| Source | https://github.com/Supreeeme/xwayland-satellite/archive/refs/tags/v0.8.2.tar.gz |
| sha256 | `cb50bb6948582d5ec3aa511d2d66ad622989bb14bef94e3bb81bae8b64c120b1` |
| Changes | none: built from the unmodified release source, default features |
| Built by | [`scripts/build-xwayland-satellite.sh`](../scripts/build-xwayland-satellite.sh) (the pins are at its top) |

Where it is installed:

| Package | File | Build |
| --- | --- | --- |
| Termux `.deb` | `$PREFIX/libexec/lindroids/xwayland-satellite` | bionic, API 24 |
| Termux `.deb` | `$PREFIX/libexec/lindroids/glibc/xwayland-satellite` | glibc, for bundles of Debian-family proot distros |
| Debian `.deb` | `/usr/libexec/lindroids/xwayland-satellite` | glibc (built on Debian 12, needs glibc 2.34+) |
| static tarball | `libexec/lindroids/xwayland-satellite` | glibc, as for Debian |

It links the system's libxcb and libxcb-cursor (MIT); Termux's `libxcb` and `xcb-util-cursor`,
Debian's `libxcb1` and `libxcb-cursor0`, recommended by the packages. Only to build the Termux
binaries, Termux's own `.deb`s of libxcb and its dependencies (pinned in the script) are unpacked
into a temporary sysroot; nothing from them is shipped.

The Rust crates it is built from are statically linked (all permissive licences: MIT,
Apache-2.0, BSD, Zlib, Unlicense and similar); the list, with each crate's version and licence,
ships as `xwayland-satellite-crates.txt`. The binary embeds the Open Sans font (Copyright 2020
The Open Sans Project Authors, SIL Open Font License 1.1; the licence notice is in the font's
metadata) for its window decorations.

### Patches for Android

Two of those crates do not compile for Android as published, so the **Termux** build (only)
uses copies with a small patch, applied by the build script through Cargo's `[patch]` override;
xwayland-satellite's own source is unchanged. Both crates are MIT-licensed; the patches are in
this directory:

- [`smithay-client-toolkit-0.20.0-android-memfd.patch`](xwayland-satellite/smithay-client-toolkit-0.20.0-android-memfd.patch)
- [`wayland-cursor-0.31.11-android-no-shm.patch`](xwayland-satellite/wayland-cursor-0.31.11-android-no-shm.patch)

Both crates allocate Wayland shared-memory buffers with `memfd_create`, falling back to
`shm_open` through `rustix::shm`, which rustix does not provide on Android (bionic has no
`shm_open`). The patches remove that fallback on Android, where `memfd_create` is the only way
(smithay-client-toolkit only tried memfd on `target_os = "linux"`; the patch enables it on
Android too). Nothing changes on other systems. Upstream releases (smithay-client-toolkit
0.21.1, wayland-cursor 0.31.14) still have both problems.

### Source code

MPL-2.0 requires that the source of the distributed binaries be available: it is the tarball
above (sha256-checked by the build script), plus, for the Termux build, the two patches here
applied to the crates at the versions in its `Cargo.lock` (from crates.io). If those links stop
working, open an issue at https://github.com/dixonSolutions/lindroids/issues and the source
will be provided.
