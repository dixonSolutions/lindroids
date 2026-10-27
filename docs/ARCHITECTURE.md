# Architecture

Lindroids has three parts, and only the first is installed:

| Part | Where it runs | What it is |
|---|---|---|
| `lindroids` | inside the sandbox (Termux, a proot distro, a desktop) | one binary: the CLI and, as `lindroids server`, the server |
| server | the same place, started on demand | a Wayland compositor and a PulseAudio-protocol sound server, which also serves the page (`crates/lindroids-server`) |
| the page | the phone's web browser | windows, input, sound output, clipboard, files, notifications (`crates/lindroids-server/data/web`, embedded in the binary) |

## Why the server is a plain Linux program

The obvious design is the one Termux:X11 uses: start a Java process inside the terminal app with
`app_process`, use hidden Android APIs to broadcast a Binder to the display app, and hand Surfaces
over Binder. It works on most phones, but not everywhere: on x86 images with an ARM translation
layer (BlissOS, Android-x86, many Chromebook builds) `app_process` loads the native bridge, which
calls `mount()` and is killed by the app seccomp filter. And `am`, used to wake other apps, needs
`app_process` too on recent Android, or is refused to apps outright.

So the server touches no Android API at all. It runs wherever Linux programs run, and the display
reaches it.

## Why the display is the browser

Lindroids used to ship a companion app for the display, which needed installing, pairing and
Termux's `allow-external-apps` setting, and broke whenever the two fell out of step. Showing Linux
windows on Android without any app of its own leaves two places: the terminal itself, or the web
browser every phone already has. The browser can do everything the app did that matters — a
canvas at the screen's resolution, multi-touch, pen and hardware keyboard events, the soft
keyboard, Web Audio, the clipboard, file pickers, full screen and home-screen icons — and asking it
to open a page is an ordinary intent any app may send. (Drawing overlay windows from a process
started with `app_process` in Termux was tried too: Android 13's system server crashes on the
"displaying over other apps" notification of a window whose session has no package.)

The page is served by the server it shows, from the binary, so the two never disagree about the
protocol.

## The link

The server's loopback port, `127.0.0.1:<47000 + uid offset>` (fixed per uid, so a page on the
home screen finds it again), speaks both HTTP and the link protocol; the first bytes tell which
(`web.rs`). `GET /` serves the page, `GET /link` upgrades to a WebSocket that carries the link:
every server message is one WebSocket message, so the page needs no framer; what the page sends is
a byte stream of the same lines. Messages are JSON lines; a message with a `len` field is followed
by that many bytes of payload (`crates/lindroids-proto`). Raw TCP and the abstract socket
`@lindroids.server.<uid>` still carry the same bytes for `lindroids debug-app`, the stand-in display
the tests use.

**Who may connect.** Any app can reach a loopback port, and a web site could try to through the
browser. The server answers only requests whose `Host` is `127.0.0.1:<port>` or `localhost:<port>`
(a site that rebinds its name to 127.0.0.1 still sends its own name) and upgrades only WebSockets
whose `Origin` is the page's own; responses forbid framing. Then the page authenticates like the
app did, with WebCrypto:

```
page   → Hello { nonce_a }
server → Challenge { nonce_s, HMAC(secret, "lindroids-server" | nonce_a) }
page   : verifies the server (no proof, no secret sent)
page   → Auth { HMAC(secret, "lindroids-app" | nonce_s) }      or  Auth { code }
server → Hello            (or AuthFailed; Paired { secret } after a code)
```

The secret lives in `~/.local/share/lindroids/secret`. `lindroids run` opens
`http://127.0.0.1:<port>/#k=<secret>`: the part after `#` never leaves the browser, and the page
moves it into its origin's storage and out of the address bar at once. A page opened by hand asks
for the 6-digit code `lindroids pair` shows; five wrong codes close pairing until the server
restarts.

**One display at a time.** A newly linked page replaces the old one, which is told so (`Bye`) and
waits rather than taking the link back; a tab with one history entry (one the browser opened for
`lindroids run`) closes itself, so browsers that open a new tab per intent do not pile them up.

**Opening the page.** The CLI sends a VIEW intent from the terminal: Termux's am socket server
(`termux-am`), else Termux's `am` (app_process; skipped on x86 images with an ARM translation
layer, where seccomp kills it), else Android's `am`; `xdg-open` on a desktop. No permission is
involved. The intent names the browser application id `lindroids`, which Chrome uses to bring back
the same tab; when the page is already linked, only its fragment changes, so it does not reload.
On a desktop a linked page is left where it is.

## Frames

The compositor implements wl_compositor, wl_subcompositor, wl_shm, wp_viewporter, xdg-shell
(toplevels, popups with positioner constraints), xdg-decoration (client-side), wl_seat
(pointer/keyboard/touch), text-input-v3, wl_data_device (clipboard), wl_output and xdg-output.

Client buffers are copied (and converted to RGBA) at commit, so clients get their buffers back at
once. Each window is composited on the CPU into a canvas the size of the page's drawing area, in
physical pixels — the root surface copied, subsurfaces and popups blended — then compared with the
last frame sent in 64×64 tiles; the changed tiles go to the page LZ4-compressed, and the page puts
them into its canvas. One frame per window is in flight; the page acknowledges it on the next
animation frame, and only then do the clients' frame callbacks fire, so clients render at the
speed the screen takes frames. A window whose tab is not shown (or a page in the background)
holds its acknowledgement and its clients idle.

**Scale.** The output scale is an integer a little below the screen density (`density × 0.75`,
rounded): desktop apps get ~540 logical pixels across a phone instead of 360. A client that is
still larger than its window is scaled down to fit (2×2 averaging keeps text legible) and centred;
one that is smaller (a dialog with a size of its own) is centred. Input is mapped back through the
same transform.

## Input

The page's pointer events become `wl_touch` for touches on clients that bound it, and a
left-button pointer for those that did not (or for everyone with `LINDROIDS_TOUCH=pointer`). A pen
is a pointer with its button held while it touches, so it draws in note and paint programs. Mice
give `wl_pointer` with buttons and wheel. Physical keys (`KeyboardEvent.code`) map to Android
keycodes and those to evdev codes of the embedded US XKB keymap. The soft keyboard types into a
hidden text area; the page diffs what changed there into text and backspaces (predictive keyboards
rewrite whole words), and text goes through text-input-v3 when the focused client has a text field
(any script, emoji), as key presses otherwise. Ctrl+V lets the browser's paste through first, so
the Linux side has the clipboard before the keys arrive.

## Sound

`<runtime>/pulse/native` speaks the PulseAudio native protocol (playback; no shm). Each stream is
sent to the page as PCM chunks paced at real time, about 100 ms ahead, with credits from the page
bounding what is outstanding; the page schedules each chunk as a Web Audio buffer and credits it
once played. Browsers start sound only after a tap on the page; until then chunks are credited
and dropped. Formats other than s16/f32 are converted, more than two channels
are folded to stereo, stream and sink volumes are applied in the server.

## X11

X11 is integrated: every session has an X display, and each X11 window becomes its own Wayland
window, and so its own tab in the page, exactly like a Wayland program's.

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
- **Fallback.** Without satellite, `--x11` still starts a rootful Xwayland with a small window
  manager in the `lindroids` binary (x11rb): one window for the whole X screen, normal
  windows filling it, dialogs centred.

## Android's side of the desktop

Linux programs keep asking what they ask on any desktop, and Android answers. The server starts
the environment's own `dbus-daemon` as the session's bus (`<runtime>/bus`) and registers, with
zbus, the XDG desktop portal (`org.freedesktop.portal.Desktop`) and `org.freedesktop.Notifications`
on it ([portal.rs](../crates/lindroids-server/src/portal.rs)). The page answers over the link,
with what a browser knows:

| Interface | The page |
|---|---|
| Settings (`org.freedesktop.appearance`, GNOME's `gtk-theme`/`color-scheme`) | `prefers-color-scheme`, `prefers-contrast`, `prefers-reduced-motion`; changes signalled live |
| OpenURI | links open in a new tab (after a tap, if the browser wants one); files are offered to open or save |
| FileChooser | the browser's file chooser (a tap starts it), contents fetched into the runtime dir; saves become downloads |
| Notification, fdo Notifications | notices in the page with their buttons; taps come back as actions |
| NetworkMonitor | `navigator.onLine` |

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

## Launchers

`lindroids export <command>` writes `<state>/apps/<id>.json` (name, PNG icon, the `lindroids run`
arguments, with the distro found pinned) and opens `/app/<id>`: the same page, whose manifest and
favicon are the program's, so the browser's "Add to Home screen" (or "Install app") makes an icon
for it. Once linked, that page sends `Launch { id }`; the server starts `lindroids launch <id>`
detached, which runs the record (a repeat of the same id within seconds is ignored: some browsers
load the page twice for one tap). A launcher works while the server runs; without it the page says
so and offers to open Termux.

## Programs under proot

Two things that work on an ordinary Linux system do not under proot on Android, and Lindroids
fixes both for `--distro` and for the CLI running inside a distro (UserLAnd, Andronix):

- **bubblewrap.** proot fakes the namespace calls and then cannot do the mounts, so `bwrap`
  "succeeds" and the sandboxed program has no filesystem. glycin, which loads GTK's images since
  gdk-pixbuf 2.44 (Alpine, Arch), would abort GTK. A `bwrap` first on PATH reports what is true
  there, that namespaces are unavailable, which is the case glycin falls back on by design.
- **alarm() on x86 Android.** Android's seccomp filter for apps on x86/x86_64 refuses the legacy
  `alarm` syscall, which glibc calls directly there (ARM64 has no such syscall, so phones are not
  affected): xterm, for one, fails. `liblindroids-compat.so`, a libc-free shim built from
  [third_party/compat](../third_party/compat/lindroids-compat.c) and embedded in the CLI, is
  preloaded into glibc programs on x86 Android and implements `alarm()` with `setitimer`.
