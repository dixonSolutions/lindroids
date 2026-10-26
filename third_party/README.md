# Third-party components

Lindroids itself is MIT OR Apache-2.0. The APK can also carry the programs below, which keep
their own licences. Nothing in this directory except this file is committed: `third_party/proot/`
is produced by `scripts/fetch-proot.sh`, which downloads Termux's prebuilt packages at pinned
versions (sha256-checked), and is gitignored.

## Why they are in the APK

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

## Modifications

The binaries are not rebuilt. `scripts/fetch-proot.sh` only edits their dynamic sections with
`patchelf`:

- `libproot.so`: `NEEDED libtalloc.so.2` → `libtalloc.so`; the Termux `RUNPATH`
  (`/data/data/com.termux/files/usr/lib`) is removed.
- `libtalloc.so`: `SONAME` set to `libtalloc.so`; `RUNPATH` removed.
- `libandroid-shmem.so`: `RUNPATH` removed.

## Source code offer

proot is GPL-2.0 and talloc is LGPL-3.0. The corresponding source for the binaries distributed
in a Lindroids release is the upstream source at the versions pinned in
`scripts/fetch-proot.sh`, plus the Termux patches in the recipes linked above (Termux also
publishes source on its package pages). If those links stop working, open an issue at
https://github.com/dixonSolutions/lindroids/issues and the source will be provided.
