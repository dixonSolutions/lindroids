# Contributing

Lindroids is one Rust binary, `lindroids`: the CLI and, as `lindroids server`, a Wayland
compositor and PulseAudio-protocol sound server that also serves the page that draws its windows.
You can build it, test it and run your build on a phone with **Rust alone**: no Java, no Android
SDK, no NDK. Those are needed only for the few files listed under
[Files that need other tools](#files-that-need-other-tools), and their outputs are committed.

Questions: [Discord](https://discord.gg/gBbW9mNr9P) or an issue.

## The code

| Path | What |
|---|---|
| `crates/lindroids-cli` | the `lindroids` binary: commands, sandboxes (Termux, proot distros, Flatpak), export, setup |
| `crates/lindroids-server` | the compositor, the sound server and the HTTP/WebSocket link; `data/web` is the page, embedded in the binary |
| `crates/lindroids-proto` | the messages between them, and the authentication |
| `crates/lindroids-apk` | builds a program's own app from the committed template, on the phone (`lindroids export`) |
| `packaging/` | packages (deb, pacman, rpm, apk, xbps), the window app's Java source, Flatpak for Termux |
| `scripts/` | the release build, versions, xwayland-satellite |
| `tests/` | the host integration test and the package/install test |
| `lab/` | QEMU Android VMs and the end-to-end tests run in them |
| `site/` | the website (SvelteKit) |

Read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) first, then [docs/DISPLAY.md](docs/DISPLAY.md)
for the page and the link's security, or [docs/APPS.md](docs/APPS.md) for exported apps.

## Build and test on a Linux PC

Rust stable from [rustup](https://rustup.rs). `clang` is optional: without it the build leaves
out two small preload libraries (the GTK 3 file picker and an x86 `alarm()` fix) and says so.

```sh
git clone https://github.com/dixonSolutions/lindroids && cd lindroids
cargo build -p lindroids               # target/debug/lindroids
cargo test --workspace                 # the unit tests
tests/host-integration.sh              # the real server, a stand-in page and foot (needs foot, python3)
```

Your build runs on the desktop too: `target/debug/lindroids run foot` starts the server and
opens the page in your browser (`xdg-open`). `LINDROIDS_LOG=debug` logs more; the server's log is
`server.log` in its runtime directory (`lindroids status` shows it). More, including
`lindroids debug-app` for scripted input: [docs/TESTING.md](docs/TESTING.md).

## Run your build on a phone

A running server is kept when the protocol version has not changed, so **always
`lindroids stop` before trying a new build**, or the old server keeps serving.

### Build it in Termux

```sh
pkg install rust clang git
git clone https://github.com/dixonSolutions/lindroids && cd lindroids
cargo build -p lindroids --release
lindroids stop; ./target/release/lindroids run foot
```

The first build compiles every dependency (under two minutes on an x86 lab VM, longer on a
phone, and about 1 GB with Rust); later ones only what changed. The server is started from the binary that asked for it, so this runs your build
without touching the installed one.

### Or cross-build on a PC

With the Android NDK (the zip alone, unpacked: no SDK) and
[cargo-ndk](https://github.com/bbqsrc/cargo-ndk):

```sh
rustup target add aarch64-linux-android        # x86_64-linux-android for x86 devices and VMs
cargo install cargo-ndk
export ANDROID_NDK_HOME=~/android-ndk-r27c
cargo ndk -t arm64-v8a -P 24 build -p lindroids --release
adb push target/aarch64-linux-android/release/lindroids /sdcard/Download/
```

API 24 is Android 7, the oldest supported. In Termux (after `termux-setup-storage` once, so it
can read Download), copy it in and run it:

```sh
cp /sdcard/Download/lindroids ~/lindroids && chmod 755 ~/lindroids
lindroids stop; ~/lindroids run foot
```

To use your build for everything (exports, home screen icons), put it in place of the
installed one: `cp $PREFIX/bin/lindroids $PREFIX/bin/lindroids.orig` first. The next
`pkg upgrade` puts a release back.

### What to try

- `lindroids run foot` (`pkg install foot`): a window, typing, the soft keyboard, closing it.
- `lindroids export foot`, then the app's icon with Termux closed.
- A GTK program (`pkg install mousepad`), a Flatpak, or an X11 program (`lindroids setup x11`)
  if your change touches them.
- `lindroids doctor` when something does not start.

Say in the pull request what you ran it on: device, Android version, which Termux (F-Droid,
GitHub or Google Play), and the browser if a window opened in one.

## Files that need other tools

These are built rarely and committed, so nobody else needs the tools:

| Changed | Rebuild with | Needs |
|---|---|---|
| `packaging/webview-app/**` | `packaging/build-app-template.sh` (writes `crates/lindroids-apk/template/window-app.apk`) | JDK, Android SDK (`android.jar`, aapt2, d8, zipalign) |
| `crates/lindroids-cli/shortcut/Shortcut.java` | `scripts/build-shortcut-dex.sh` | JDK, Android SDK (`android.jar`, d8) |
| a full release: every ABI, every package | `scripts/build-release.sh` | NDK r27c, podman or docker |
| `site/` | `npm ci && npm run dev` in `site/` | Node |

## Pull requests

- Branch from `main`; one change per pull request.
- CI runs on every pull request: `cargo test --workspace`, `tests/host-integration.sh`,
  shellcheck on the scripts, the window app build, and every package installed by `install.sh` in
  containers of a dozen distros. Run the first two yourself before pushing.
- Change the docs with the code: a new command in the README's usage, a design decision in
  `docs/`, with what was tried and what was found.
- Commit messages say what changed for the user, in a short sentence (see `git log`).
- Every merge to `main` is a release (`0.1.<commit count>`), published to the package
  repositories, so `main` must always work.
- Contributions are licensed MIT OR Apache-2.0, as the project is.
