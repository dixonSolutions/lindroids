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

`scripts/build.sh` is **for development only**: it builds your checkout into a `lindroids`
package, version `<release>~dev.<commit>` (`lindroids version` says `+<commit>-dev`), that
installs over the release. Everyone else installs Lindroids with `install.sh` (README).
`scripts/use-release.sh` puts a release back. Both have `--help`.

`build.sh` checks for everything the build needs and offers to install what is missing (Rust,
clang, cargo-ndk, the NDK, dpkg-deb, adb); `--yes` installs it without asking.

### Build it in Termux

```sh
pkg install git
git clone https://github.com/dixonSolutions/lindroids && cd lindroids
scripts/build.sh --install             # offers Rust and clang, builds, installs, stops an idle server
lindroids run foot
```

The first build compiles every dependency (under two minutes on an x86 lab VM, longer on a
phone, and about 1 GB with Rust); later ones only what changed. Clone the whole history (no
`--depth`): the version is counted from it.

To try a build without installing it, `./target/release/lindroids run foot` after
`lindroids stop`: the server is started from the binary that asked for it.

### Or cross-build on a PC

With the phone on USB debugging:

```sh
scripts/build.sh --device --push       # the attached device's CPU; prints the command for Termux
```

The first run offers what a cross build needs: Rust (rustup) and the Android target,
[cargo-ndk](https://github.com/bbqsrc/cargo-ndk), the Android NDK r27c (the 650 MB zip alone,
checked, into `~/.local/share/lindroids-dev`: no SDK, no Java; an NDK you have is used if
`ANDROID_NDK_HOME` names it), and `dpkg-deb` and adb from your package manager. On an immutable
system (Fedora Silverblue and the like), run it in a toolbox or distrobox.

In Termux (after `termux-setup-storage` once, so it can read Download), the command it prints:

```sh
apt-get install --reinstall --allow-downgrades -y /sdcard/Download/lindroids_<version>_aarch64.deb
```

`--target aarch64|arm|x86_64|i686` builds a Termux package without a device, and
`--target arm64|armhf|amd64|i386` a Debian/Ubuntu one (proot distros, desktops). Packages go to
`target/dev-build/`. API 24 (Android 7) is the oldest supported.

### Back to a release

```sh
scripts/use-release.sh                 # the latest release
scripts/use-release.sh 0.1.130         # a given one (--list shows them)
```

A version still in the package repository (the newest three) comes from it; an older one from
its GitHub release, checked against its `SHA256SUMS`. Your dev package also gives way by itself
to the release with its number, on a later `pkg upgrade`.

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
