# Architecture

Lindroids has three parts:

| Part | Where it runs | What it is |
|---|---|---|
| `lindroids` | inside the sandbox (Termux, a proot distro, a desktop) | one binary: the CLI and, as `lindroids server`, the server |
| server | the same place, started on demand | a Wayland compositor and a PulseAudio-protocol sound server (`crates/lindroids-server`) |
| Lindroids app | Android | windows, input, sound output, shortcuts, and an in-app server for exported bundles (`android/`, `crates/lindroids-android`) |

## Why the server is a plain Linux program

The obvious design is the one Termux:X11 uses: start a Java process inside the terminal app with
`app_process`, use hidden Android APIs to broadcast a Binder to the display app, and hand Surfaces
over Binder. It works on most phones, but not everywhere: on x86 images with an ARM translation
layer (BlissOS, Android-x86, many Chromebook builds) `app_process` loads the native bridge, which
calls `mount()` and is killed by the app seccomp filter. And `am`, used to wake other apps, needs
`app_process` too on recent Android, or is refused to apps outright.

So the server touches no Android API at all. It runs wherever Linux programs run, and the app
reaches it.

## The link

Android's SELinux policy gives every app its own MLS categories and refuses `connectto` on another
app's Unix sockets (seen on a permissive device as
`avc: denied { connectto } ... tclass=unix_stream_socket`). Loopback TCP, on the other hand, is open
between any two apps with the INTERNET permission. The link is therefore a byte stream:

1. the app tries the server's abstract socket `@lindroids.server.<uid>` (works for the in-app
   server, and on devices whose policy allows it); the kernel's `SO_PEERCRED` says who listens;
2. otherwise it connects to `127.0.0.1:<47000 + uid offset>`.

Nothing depends on passing file descriptors. Messages are JSON lines; a message with a `len` field
is followed by that many bytes of payload (`crates/lindroids-proto`).

**Authentication.** Any app can connect to a loopback port, and any app could listen on one first,
so neither side trusts the other on sight. Both know a secret from an earlier pairing:

```
app    → Hello { nonce_a }
server → Challenge { nonce_s, HMAC(secret, "lindroids-server" | nonce_a) }
app    : verifies the server (no proof, no secret sent)
app    → Auth { HMAC(secret, "lindroids-app" | nonce_s) }      or  Auth { code }
server → Hello            (or AuthFailed; Paired { secret } after a code)
```

With Termux the first pairing can do without the code: the app asks Termux, through Termux's
RUN_COMMAND API, to run `lindroids app-secret`, and Termux returns the output through a
PendingIntent to a receiver that is not exported (`TermuxPairing`, `PairResultReceiver`), so only
Termux can have answered. That needs Android's "run commands in Termux" permission for the app
(asked for once) and `allow-external-apps = true` in `~/.termux/termux.properties`; when either is
missing, or Termux's secret does not fit the server (one inside a proot distro has its own), the app
asks for the code. Otherwise, the first time, the user types the 6-digit code the CLI shows; five wrong codes close pairing until
the server restarts. The secret lives in `~/.local/share/lindroids/secret` on the sandbox side and
in the app's private preferences. When the CLI can send intents (`termux-am`, `am`, `cmd`) it also
wakes the app with the secret, so pairing is not needed again; otherwise the app finds servers on its
own while its service runs, by trying the sockets of approved and installed apps.

## Frames

The compositor implements wl_compositor, wl_subcompositor, wl_shm, wp_viewporter, xdg-shell
(toplevels, popups with positioner constraints), xdg-decoration (client-side), wl_seat
(pointer/keyboard/touch), text-input-v3, wl_data_device (clipboard), wl_output and xdg-output.

Client buffers are copied (and converted to RGBA) at commit, so clients get their buffers back at
once. Each window is composited on the CPU into a canvas the size of its Android Surface — the root
surface copied, subsurfaces and popups blended — then compared with the last frame sent in 64×64
tiles; the changed tiles go to the app LZ4-compressed. One frame per window is in flight; the app
acknowledges it once it is in the Surface, and only then do the clients' frame callbacks fire, so
clients render at the speed the screen takes frames. A hidden window (no Surface) holds its
acknowledgement and its clients idle.

**Scale.** The output scale is an integer a little below the screen density (`density × 0.75`,
rounded): desktop apps get ~540 logical pixels across a phone instead of 360. A client that is
still larger than its window is scaled down to fit (2×2 averaging keeps text legible) and centred;
input is mapped back through the same transform.

## Input

Android `MotionEvent`s become `wl_touch` for clients that bound it, and a left-button pointer for
those that did not (or for everyone with `LINDROIDS_TOUCH=pointer`). Mice give `wl_pointer` with
buttons and wheel. Android keycodes map to evdev codes of the embedded US XKB keymap. Text from the
soft keyboard goes through text-input-v3 when the focused client has a text field (any script,
emoji), and is typed as key presses otherwise.

## Sound

`<runtime>/pulse/native` speaks the PulseAudio native protocol (playback; no shm). Each stream is
sent to the app as PCM chunks paced at real time, about 100 ms ahead, with credits from the app
bounding what is outstanding; the app plays each stream on its own AAudio stream (API 27+) or
`AudioTrack`, and Android mixes. Formats other than s16/f32 are converted, more than two channels
are folded to stereo, stream and sink volumes are applied in the server.

## X11

X11 is integrated: every session has an X display, and each X11 window becomes its own Wayland
window, and so its own Android window, exactly like a Wayland program's.

- **The display.** At startup the server reserves a free display `:N` the way an X server does
  (`/tmp/.X<N>-lock`), listens on its socket (`/tmp/.X11-unix/X<N>`, or `$PREFIX/tmp/...` in
  Termux, whose libraries are patched to look there) and on the abstract `@/tmp/.X11-unix/X<N>`,
  and writes `:N` to `<runtime>/x11-display`. The CLI sets `DISPLAY` from that file for everything
  it starts, so X11-only programs need no option. Toolkits that have a Wayland backend still use
  it (`GDK_BACKEND`, `QT_QPA_PLATFORM`… are set to Wayland); `--x11` flips them to X11.
- **Started on demand.** Nothing runs until an X11 program connects. Then the server starts
  [xwayland-satellite](https://github.com/Supreeeme/xwayland-satellite) (MPL-2.0) and hands it
  the listening sockets (`-listenfd`), the way the niri compositor integrates it. Satellite runs
  Xwayland *rootless* and turns every X11 toplevel into an `xdg_toplevel` (popups into
  `xdg_popup`) of ours, with titles and app ids, and bridges the clipboard. Satellite needs only
  what the server already has: the core interfaces, xdg-shell and viewporter. If it keeps dying,
  the server stops restarting it and refuses X connections, so clients fail instead of hanging.
- **Where it comes from.** `LINDROIDS_XWAYLAND_SATELLITE`, else the copy Lindroids ships next to
  its binary (`<prefix>/libexec/lindroids/xwayland-satellite`: a bionic build in the Termux
  package, a glibc one in the Debian package and the tarballs), else `xwayland-satellite` on PATH
  (Fedora, Arch, Alpine and openSUSE package it). Xwayland always comes from the distribution;
  `lindroids setup x11` installs what is missing with pkg, apt, dnf, pacman, apk or zypper and
  restarts the server.
- **proot distros** (`--distro`) share Termux's tmp as their /tmp, so their X11 programs reach the
  Termux-side display; **Flatpaks** reach it through the abstract socket (their /tmp is private,
  and the socket directory is bound in too).
- **Exported X11 programs** (an ELF that links Xlib or XCB and no Wayland-capable toolkit, or
  `--x11`) carry Xwayland and satellite in their bundle. On each launch the app reserves a display
  inside the bundle's /tmp and runs satellite on those sockets next to the program, under the
  bundle's proot.
- **Fallback.** Without satellite, `--x11` still starts a rootful Xwayland with a small window
  manager in the `lindroids` binary (x11rb): one Android window for the whole X screen, normal
  windows filling it, dialogs centred.

## Android's side of the desktop

Linux programs keep asking what they ask on any desktop, and Android answers. The server starts
the environment's own `dbus-daemon` as the session's bus (`<runtime>/bus`) and registers, with
zbus, the XDG desktop portal (`org.freedesktop.portal.Desktop`) and `org.freedesktop.Notifications`
on it ([portal.rs](../crates/lindroids-server/src/portal.rs)). The app answers over the link:

| Interface | Android |
|---|---|
| Settings (`org.freedesktop.appearance`, GNOME's `gtk-theme`/`color-scheme`) | night mode, Material You accent, contrast, animations; changes signalled live |
| OpenURI | `ACTION_VIEW` for links; files go through the app's FileProvider |
| FileChooser | the document picker; shared-storage documents are used in place, others fetched into the runtime dir; saves are written back to the picked document when they change |
| Notification, fdo Notifications | Android notifications with their buttons; taps come back as actions |
| MemoryMonitor | `onTrimMemory` |
| NetworkMonitor, PowerProfileMonitor | the default network; battery saver |

Programs get `DBUS_SESSION_BUS_ADDRESS`, `GTK_USE_PORTAL=1` (GLib and GTK 3 use the portal outside
a Flatpak only when asked), `GDK_DEBUG=portals` (GTK 4), `QT_QPA_PLATFORMTHEME=xdgdesktopportal`,
and an `xdg-open` first on PATH that runs `lindroids open`. Programs under proot run as a faked
root, so EXTERNAL authentication cannot work for them; the bus also takes ANONYMOUS, which is safe
because its socket is in the runtime dir only this uid can enter. Distro and Flatpak runs see the
runtime dir at the server's path too, so the paths the FileChooser hands out are valid there.

## Flatpak

Flatpak's own runner needs bubblewrap, which needs namespaces an Android app does not have.
`lindroids run <app-id>` finds the installation (in Termux's view, inside a proot-distro rootfs,
or a toolbox host), reads the app's and runtime's metadata, resolves extensions (GL, locale,
codecs), and builds the same filesystem view with proot: runtime at `/usr`, app at `/app`,
extensions at their mount points, a generated `/etc/passwd`, `~/.var/app/<id>` as the XDG dirs.

## Exported bundles

`lindroids export` resolves what to copy:

- Termux program: the owning package, its dependency closure from the dpkg database, the packages
  that put plugins into directories the closure owns (pixbuf loaders, GIO/GTK modules…), files
  that package scripts generated there (`loaders.cache`, `gschemas.compiled`), plus bash and
  coreutils. Inside the app it runs under its own proot root with Termux's prefix and home bound in.
- proot-distro program: the same closure from the distro's package database (dpkg for
  Debian/Ubuntu, apk for Alpine, pacman for Arch; rpm distros are refused with a message), with
  the base packages and `/etc`; it runs with `-r` on the copied root. apk's `install_if` is
  followed (Alpine's glycin image loaders arrive that way), and busybox's applet links are added.

## Programs under proot

Two things that work on an ordinary Linux system do not under proot on Android, and Lindroids
fixes both for `--distro`, for the CLI running inside a distro (UserLAnd, Andronix) and for
exported distro bundles:

- **bubblewrap.** proot fakes the namespace calls and then cannot do the mounts, so `bwrap`
  "succeeds" and the sandboxed program has no filesystem. glycin, which loads GTK's images since
  gdk-pixbuf 2.44 (Alpine, Arch), would abort GTK. A `bwrap` first on PATH reports what is true
  there, that namespaces are unavailable, which is the case glycin falls back on by design.
- **alarm() on x86 Android.** Android's seccomp filter for apps on x86/x86_64 refuses the legacy
  `alarm` syscall, which glibc calls directly there (ARM64 has no such syscall, so phones are not
  affected): xterm, for one, fails. `liblindroids-compat.so`, a libc-free shim built from
  [third_party/compat](../third_party/compat/lindroids-compat.c) and embedded in the CLI, is
  preloaded into glibc programs on x86 Android and implements `alarm()` with `setitimer`.
- Flatpak: runtime, app and extensions, with a skeleton root.

The CLI streams the tar over the server and the link into the app, which unpacks it, records the
launch spec and pins a shortcut (`ShortcutManager.requestPinShortcut`, adaptive icon; the legacy
`INSTALL_SHORTCUT` broadcast on Android 7). Tapping it opens a short splash (a visible activity is
what lets Android open the first window at once), starts the in-app server if needed, and spawns
the program under the APK's proot (`lib<abi>/libproot.so`, executable because native libraries
are extracted and the app targets SDK 28, like Termux).
