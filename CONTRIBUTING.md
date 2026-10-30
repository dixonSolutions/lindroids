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
| `scripts/` | dev builds (`build.sh`, `use-release.sh`), the release build, versions, xwayland-satellite |
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

## Test a dev build

`scripts/build.sh` is **for development only**: it builds your checkout into a `lindroids`
package that installs over the release, so you can try a change before it is released. Everyone
else installs Lindroids with `install.sh` (README). `scripts/use-release.sh` goes back to an
official release. Both have `--help`.

| Where you test | Run there | It makes |
|---|---|---|
| Termux, on the phone | `scripts/build.sh --install` | Termux's `.deb` (or pacman package, in a Termux with pacman) |
| A Linux distro: a container, a proot distro, a desktop | `scripts/build.sh --install` | that system's package: `.deb` (apt), pacman, `.rpm` (dnf, yum, zypper) or `.apk` (Alpine) |
| A phone, building on a PC (USB debugging on) | `scripts/build.sh --device --push` | the phone's Termux `.deb`, copied to its Download folder |

`build.sh` finds the package manager of the system it runs in (or takes `--format
deb|pacman|rpm|apk`), checks for everything the build needs and **offers to install what is
missing** with that package manager: Rust (rustup, or Termux's `rust`), a C compiler, clang,
the Rust target, cargo-ndk and the Android NDK for phone builds on a PC, `dpkg-deb`, `bsdtar`,
adb. `--yes` installs it without asking; without a terminal it stops and prints the command.
The first build compiles every dependency (two minutes on a fast machine, longer on a phone;
about 1 GB with Rust), later ones only what changed. Clone the whole history (no `--depth`):
the version is counted from it.

A dev package is the release this commit would be, marked as a dev build: it installs over the
releases before it, and the release itself replaces it on a later upgrade.

| Package | Version | `lindroids version` |
|---|---|---|
| deb, rpm | `0.1.136~dev.<commit>` (`.dirty` with uncommitted changes) | `0.1.136+<commit>-dev` |
| pacman | `0.1.136dev.<commit>` | the same |
| apk | `0.1.136_pre<commit time>` | the same |

A running server is kept until it is stopped: `--install` stops an idle one, and one with
windows open says so. **Run `lindroids stop` before trying a new build** you installed another
way, or the old server keeps serving.

### In Termux

```sh
pkg install git
git clone https://github.com/dixonSolutions/lindroids && cd lindroids
scripts/build.sh --install             # offers Rust and clang, builds, installs, stops an idle server
lindroids run foot
```

To try a build without installing it: `lindroids stop`, then `./target/release/lindroids run
foot` (the server is started from the binary that asked for it).

### In a Linux distro: a container, a proot distro, a desktop

```sh
git clone https://github.com/dixonSolutions/lindroids && cd lindroids   # Alpine: apk add bash git first
scripts/build.sh --install             # this system's package; offers Rust and the rest
```

rpm and apk packages are made in a Fedora or Alpine container when podman or docker is there,
else natively on an rpm system or on Alpine (`packaging/container.sh`), so they build in proot
distros too. Void (xbps) has no dev packages. To make another system's package,
`--format` and `--target arm64|armhf|amd64|i386`.

### From a PC, for a phone

With the phone (or any Android device) in developer mode, USB debugging on, and plugged in:

```sh
scripts/build.sh --device --push       # the attached device's CPU; prints the command for Termux
```

The first run offers what a cross build needs: Rust and the Android target,
[cargo-ndk](https://github.com/bbqsrc/cargo-ndk), the Android NDK r27c (the 650 MB zip alone,
checked, into `~/.local/share/lindroids-dev`: no SDK, no Java; an NDK of yours is used if
`ANDROID_NDK_HOME` names it), adb and `dpkg-deb` from your package manager. On an immutable
system (Fedora Silverblue and the like) run it in a toolbox or distrobox. With several devices
attached, `--device <serial>` (from `adb devices`).

Then in Termux (after `termux-setup-storage` once, so it can read Download), the command it
printed:

```sh
apt-get install --reinstall --allow-downgrades -y /sdcard/Download/lindroids_<version>_aarch64.deb
```

`--target aarch64|arm|x86_64|i686` builds a Termux package without a device. For a proot distro
on the phone, `--target arm64 --format pacman --device --push` (for example): proot-distro
shares the Download folder as `/sdcard/Download`. Packages go to `target/dev-build/`. Android 7
(API 24) is the oldest supported.

### Back to a release

```sh
scripts/use-release.sh                 # the latest release
scripts/use-release.sh 0.1.130         # a given one
scripts/use-release.sh --list          # the releases, and how lindroids is installed here
```

It goes through whatever installed `lindroids` (apt, pacman, dnf, yum, zypper, apk or a static
install), downgrading from a dev build or a newer release as needed. The repositories keep the
newest three releases; older ones come from their GitHub release (a `.deb` checked against its
`SHA256SUMS`, an `.rpm`). pacman has only those three. Alpine's repository is not published yet:
there the release's static binary is put in `/usr/local/bin` instead.

### What to try, and what to report

- `lindroids run foot` (`pkg install foot`): a window, typing, the soft keyboard, closing it.
- `lindroids export foot`, then the app's icon with Termux closed.
- A GTK program (`pkg install mousepad`), a Flatpak, or an X11 program (`lindroids setup x11`)
  if your change touches them.
- `lindroids doctor` when something does not start; the server's log is `server.log` in the
  directory `lindroids status` shows.

In the pull request or on Discord, say what you ran it on: `lindroids version`, the device and
Android version, which Termux (F-Droid, GitHub or Google Play) or which distro, and the browser
if a window opened in one.

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
  containers of a dozen distros. Run the first two yourself before pushing, and
  `tests/install-repos.sh` (podman or docker) when you change `packaging/` or `install.sh`.
- Change the docs with the code: a new command in the README's usage, a design decision in
  `docs/`, with what was tried and what was found.
- Commit messages say what changed for the user, in a short sentence (see `git log`).
- Every merge to `main` is a release (`0.1.<commit count>`), published to the package
  repositories, so `main` must always work.
- Contributions are licensed MIT OR Apache-2.0, as the project is.
