---
title: "Architecture"
description: "The parts of Lindroids, why the server is a plain Linux program, and why the display is the browser."
---

Lindroids has three parts, and only the first is installed:

| Part | Where it runs | What it is |
|---|---|---|
| `lindroids` | inside the sandbox (Termux, a proot distro, a desktop) | one binary: the CLI and, as `lindroids server`, the server |
| server | the same place, started on demand | a Wayland compositor and a PulseAudio-protocol sound server, which also serves the page (`crates/lindroids-server`) |
| the page | a window of its own on Android (a browser's Custom Tab, opened by Termux), a browser tab elsewhere | windows, input, sound output, clipboard, files, notifications (`crates/lindroids-server/data/web`, embedded in the binary) |

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
Termux's `allow-external-apps` setting, and broke whenever the two fell out of step. Without an
app of its own, the only thing on a phone that can draw a window for it is the browser: every
window is an activity of an installed app, and Termux's one window is its terminal. The browser
can do everything the app did that matters: a canvas at the screen's resolution, multi-touch,
pen and hardware keyboard events, the soft keyboard, Web Audio, the clipboard, file pickers and
full screen.

On Android each program gets **a window of its own**. Termux's `app_process` helper opens it as a
browser **Custom Tab**: its own task in recents, no tabs, no address bar, and full screen from
the first tap. Closing it ends the program. [DISPLAY.md](DISPLAY.md) has the design, everything
that was tried and why it was rejected (installed web apps, overlays, the terminal), and the lab
tests behind each step.

The page is served by the server it shows, from the binary, so the two never disagree about the
protocol.

## The link

The server's loopback port (`server_tcp_port`: 47000 + the uid's offset, fixed per uid) speaks
both HTTP and the link protocol; the first bytes tell which (`web.rs`).

- `GET /` serves the page; `GET /link` upgrades to a WebSocket that carries the link.
- Every server message is one WebSocket message, so the page needs no framer. What the page sends
  is a byte stream of the same lines.
- Messages are JSON lines; a message with a `len` field is followed by that many bytes of payload
  (`crates/lindroids-proto`).
- Raw TCP and the abstract socket `@lindroids.server.<uid>` carry the same bytes for
  `lindroids debug-app`, the stand-in display the tests use.

**One origin.** Pages are served only at `http://<label>.localhost:<port>/`, where the label is
130 random bits kept in `~/.local/share/lindroids/origin`. Browsers treat `*.localhost` as
loopback and as a secure context. Requests with any other `Host`, and WebSockets with any other
`Origin`, get nothing (`421`), not even the label. So an app that takes the port while the server
is down cannot serve a page into our origin, and a web site cannot reach the link.

**Who may connect.** Both ends hold ECDSA P-256 keys (`lindroids-proto/src/auth.rs`; WebCrypto in
the page):

- **The relay's key** is `~/.local/share/lindroids/relay.key`.
- **Each display** (a browser profile, `debug-app`) made its own key pair when it paired. The
  page's private key is a non-extractable CryptoKey in IndexedDB. The relay keeps only public keys
  (`displays/<id>.json`; `lindroids displays [forget <id>]`).
- **The handshake.** Each side signs the other's nonce together with the page's origin. The page
  accepts only the relay key it pinned when it paired.
- **Pairing.** A display pairs with a *ticket* that the CLI puts in the URL fragment of the window
  it opens (`#t=…`, never sent over HTTP), or with the 6-digit code `lindroids pair` shows. Five
  wrong codes close pairing until the server restarts.
- **Attaching to a run.** A run's window must also present that run's ticket.

There is no TLS and no message encryption: on loopback they would protect against nobody that
cannot already read the keys. DISPLAY.md has the messages and the threat model.

**Pages and runs.** What `lindroids run` starts on Android is a *run* (the pid it forked), with its
own page, `/?window&run=<pid>`:

- **Routing.** The server routes each window to its run's page: Wayland clients by their process
  ancestry, X11 windows by their X client's pid (X-Resource).
- **One page per run.** A run has one page; a second one for the same run stands down.
- **The shared page.** It (`/`, for desktops and browsers without Custom Tabs) shows the rest;
  the newest wins there.
- **Lifecycle.** When a run's page closes, the run ends 5 s later; if its link is just lost, 60 s
  later (`LINDROIDS_LINGER`). The whole process tree goes. When the program ends, the page shows
  its icon and "closed".
- **Hidden pages wait.** A page nobody can see does not link until it is shown.
- **Updates.** A page from an older build reloads itself.

**Opening the page.**

- **On Android:** the helper (`shortcut.dex open`, run with `app_process` as Termux) sends a VIEW
  intent with a Custom Tabs session extra to the first installed browser that has Custom Tabs
  (`LINDROIDS_BROWSER` first). The same URL again brings back the same window.
- **Elsewhere:** Termux's `termux-am`, else `am`, else `xdg-open` open the shared page.

No permission is involved.

## Frames

The compositor implements wl_compositor, wl_subcompositor, wl_shm, wp_viewporter, xdg-shell
(toplevels, popups with positioner constraints), xdg-decoration (client-side), wl_seat
(pointer/keyboard/touch), text-input-v3, wl_data_device (clipboard), wl_output and xdg-output.

Client buffers are copied (and converted to RGBA) at commit, so clients get their buffers back at
once: only the rectangles the client damaged, into the memory the surface already has (a cursor's
buffer is not converted at all: the page shows its own arrow). Each window is composited on the
CPU into a canvas the size of the page's drawing area, in physical pixels — the root surface
copied, subsurfaces and popups blended — then compared with the last frame sent in 64×64 tiles.
When no surface moved, only the union of what they damaged is composed and compared; when one did
(a menu, a tooltip) the whole window is composed again but still only the tiles that differ go.
Only a new size, or a page that lost its copy (a lost GPU context), gets a whole frame.

The changed tiles travel in one of two encodings, chosen per frame (`frames.rs`, `Tiles.raw`):
**LZ4** for content it shrinks well (flat UI: 20–250×), **RAW** for content it hardly does (dense
text, photos), where compressing here and decoding in JavaScript cost far more than the bytes
saved on a loopback socket. Frames under 128 KiB are always LZ4; for larger ones, 64 KiB of the
pixels in eight slices are compressed first (about 0.07 ms) and below 4× (the whole frame then
does about a third better) the frame goes RAW. `LINDROIDS_FRAMES=lz4|raw` forces one, to compare
them on a device. A RAW frame's pixels are written from the window's canvas straight into the
link's output buffer; an LZ4 frame is packed in buffers the window keeps, so a frame allocates
nothing. Measured on a desktop (x86-64, release build, 1280×800 full frames):

| content | LZ4 ratio | server: pack (before → now) | page: JS decode | RAW instead |
|---|---|---|---|---|
| flat UI | 249× | 0.58 → 0.50 ms | 0.13 ms | — (stays LZ4) |
| dense text (synthetic) | 4.0× | 8.3 → 5.8 ms | 6.6 ms | 0.2 ms copy, no decode |

(`cargo test --release -p lindroids-server frames::tests::bench -- --ignored --nocapture`; the
page's decoder timed in node on the payloads it writes.) The page draws on the GPU: a worker that
owns the window's canvas (`OffscreenCanvas`, `data/web/draw.js`) decodes an LZ4 frame, or takes a
RAW one as it came (the WebSocket message's buffer, moved to the worker), and uploads each tile
into a WebGL texture (`texSubImage2D`) that one triangle draws, so decoding and drawing never
delay input on the page's main thread. The context asks for the default GPU (`powerPreference:
'default'`), not the fast one. Without a GPU (a software GL such as SwiftShader or llvmpipe) it
uses `putImageData`; without `OffscreenCanvas` the same code runs in the page. A lost GPU context
asks the server for a whole frame (a resize always gets one).

One frame per window is in flight; the page acknowledges it once drawn, and only then do the
clients' frame callbacks fire, so clients render at the speed the page takes frames. A window
whose tab is not shown (or a page in the background) holds its acknowledgement and its clients
idle. A window no page shows at all (none linked yet, or Android killed the page in the
background) and a pointer's cursor surface get their callbacks about once a second, not every
16 ms: a program animating for nobody draws once a second instead of sixty times, yet one that
waits for a callback never hangs. Their buffers are still taken at each commit, so the page that
shows the window again gets a whole, current frame at once. Clients that speak xdg_toplevel v6
(GTK 4, Qt 6, Chromium) are also told they are `suspended` while no page shows their window or its
page is out of sight, and stop their animations and timers.

**Wakeups.** On a phone every wakeup costs battery, so nothing in the server runs on a timer of
its own while it is idle. The compositor's loop sleeps until the first thing due (a frame
callback, a grace period, a handshake's deadline); its own housekeeping (is the control socket
still there, did a run end without a word) runs every 10 s, and every second only for a few
seconds after the CLI said something. Xwayland-satellite's exit is noticed through SIGCHLD rather
than a 500 ms timer; running programs' notifications go when the loop notices their run ended,
not from a thread per run looking every second; the battery is read (which starts Android's
helper, a Java VM) at most every two minutes and only while a page is on screen. With
`LINDROIDS_STATS=1` the log says how often each loop woke (`wakeups: …/s compositor, …/s sound`).
Measured here with `debug-app` as the page (voluntary context switches of the server's threads):

| situation | before | now |
|---|---|---|
| idle, a page linked, with or without an idle window | 4.0 wakeups/s | 0.1/s |
| a program animating, its page gone (killed in the background) | 78.5 wakeups/s, the program at ~40 fps | 2.2/s, the program at 1 fps |

Before, the compositor woke every second for nothing, the sound thread too, and an idle thread
of the D-Bus library's pool (the `blocking` crate 1.7 never lets its idle threads go) twice a
second; `Cargo.lock` keeps `blocking` at 1.6.1, whose idle threads end after half a second.

**Scale.** The output scale is an integer a little below the screen density (`density × 0.75`,
rounded): desktop apps get ~540 logical pixels across a phone instead of 360. A client that is
still larger than its window is scaled down to fit (2×2 averaging keeps text legible) and centred;
one that is smaller (a dialog with a size of its own) is centred. Input is mapped back through the
same transform.

## Input

Input is sent the moment it happens: the lines one event makes (a key with its Shift) leave in
one WebSocket message when its handler ends; a mouse or pen moving uses Chromium's
`pointerrawupdate`, which does not wait for the next animation frame, and sends one motion per
event (where the pointer is now, not its coalesced points), none while the link is backed up; the
canvas position is kept between events rather than read (a layout) on each. The server hands the
programs what a page sent as soon as it has read it, before it composes the next frame. The window app asks Android
for unbuffered pointer events (`requestUnbufferedDispatch`, Android 11+) and the screen's fastest
refresh rate.

The page's pointer events become `wl_touch` for touches on clients that bound it, and a
left-button pointer for those that did not (or for everyone with `LINDROIDS_TOUCH=pointer`). A pen
is a pointer with its button held while it touches, so it draws in note and paint programs. Mice
give `wl_pointer` with buttons and wheel. Physical keys (`KeyboardEvent.code`) map to Android
keycodes and those to evdev codes of the embedded US XKB keymap. The soft keyboard types into a
hidden text area; the page diffs what changed there into text and backspaces (predictive keyboards
rewrite whole words), and text goes through text-input-v3 when the focused client has a text field
(any script, emoji), as key presses otherwise. Ctrl+V lets the browser's paste through first, so
the Linux side has the clipboard before the keys arrive.

Nothing stays held. The server owns letting go: when the focused window loses the focus (the page
blurs or hides, another window is activated), when its page hides, and when its link drops (the
page cannot say anything then), it releases every key and mouse button that window holds; the page
only forgets what it held. Only a key going down moves the focus; a key-up for a window without it
is dropped. Repeats have one source, the page's keyboard: clients are told not to repeat
(`repeat_info` 0), and the browser's auto-repeat reaches them as the key released and pressed
again, never a second press. Only the browser knows when a key is really up, so a lost or late
key-up can never make a program type on by itself, and Android's own repeat settings apply.
A key-up in a composition (keyCode 229) still releases the key it names. Shift, Ctrl, Alt and
Meta follow each key event's flags both ways (a modifier whose key-up was lost goes up, one whose
flag is on with no key held goes down; a mouse press or wheel only lets go, and not Meta). Caps
Lock and Num Lock come from hardware key events' `getModifierState` (locked Lock and Mod2). Mouse
buttons follow each event's `buttons`, so chorded buttons and a cancel leave nothing held; the
surface a button went down on keeps the pointer until the last one is up (an implicit grab). A
wheel's notches are discrete steps; a touchpad's deltas are a continuous axis in pixels.

Sizes: a ResizeObserver on the stage sends a window's size when it changed, with the screen's
(which turns with the device, and the output with it). A frame already on its way keeps its
acknowledgement; the page drops it if it is the old size, and the server sends the whole frame at
the new size next, so a rotation sends each size once and never has more than one frame out.

## Sound

`<runtime>/pulse/native` speaks the PulseAudio native protocol (playback; no shm). Each stream is
sent to the page as PCM chunks paced at real time, about 100 ms ahead, with credits from the page
bounding what is outstanding; the page schedules each chunk as a Web Audio buffer and credits it
once played. The sound thread looks at its clock every 20 ms while something plays (each look
sends at least a 20 ms chunk, so the page stays 60–100 ms ahead) and sleeps without a timeout
while nothing does. Browsers start sound only after a tap on the page; until then chunks are
credited and dropped. The page suspends its `AudioContext` when nothing has played for 2 s (no
stream open, all paused, or none sending), which closes Android's audio output; the next chunk
resumes it and is scheduled meanwhile, so nothing is lost at a stream's start. Formats other than
s16/f32 are converted, more than two channels are folded to stereo, stream and sink volumes are
applied in the server.

Recording comes the other way: the source `android-mic` (the default) asks the program's page for
the microphone; the page takes it with `getUserMedia` (the browser asks the user once), sends
~20 ms of s16 PCM at a time, and the server converts it to what the stream asked for. With no page
or no microphone, recorders get silence in real time rather than stalling.

Streams go to the page of the program that opened them (its pid → its run) and follow it when that
page changes or relinks.

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
on it ([portal.rs](https://github.com/dixonSolutions/lindroids/blob/main/crates/lindroids-server/src/portal.rs)). The page answers over the link,
with what a browser knows:

| Interface | The page |
|---|---|
| Settings (`org.freedesktop.appearance`, GNOME's `gtk-theme`/`color-scheme`/`icon-theme`) | `prefers-color-scheme`, the CSS `AccentColor` (Android's accent), `prefers-contrast`, `prefers-reduced-motion`; changes signalled live. Answered from the start (the last state is kept on disk) with GNOME's defaults for the rest: libadwaita gives up on a portal that fails at startup, and GTK 4 fills keys a namespace lacks with stand-ins (icon theme "gnome") |
| OpenURI | links open in a new tab (after a tap, if the browser wants one); files are offered to open or save |
| FileChooser | A closable sheet first asks where the file is: the Linux container's own files (the page browses them through `GET /ls`, a listing of one directory by the secret token in the page, and the program gets the picked path directly — no copy), the home directory, or Android. Android's picker (Files, Camera…) comes at once after the tap that asked; contents are fetched into the runtime dir. With File System Access (Chromium 132+): the copy of a picked file is written back into it when it changes (the browser asks "Save changes?" once), *Save As* writes where the user chose, and a picked folder is copied and its changed and new files written back. Without it (Firefox): copies, saves become downloads, no folders |
| Notification, fdo Notifications | Android notifications, posted by the `app_process` helper as Termux (program name and icon, a tap brings back its window), and notices in the page with their buttons |
| NetworkMonitor | `navigator.onLine` |
| Inhibit (idle, suspend) | a screen wake lock while the program's request lives; `CreateMonitor` answers "running" |

Requests go to the page of the program that asked (the caller's pid → its run), sound included.
Files come the other way too: Android's *Open with → Termux* (or Share) saves the file in
`~/downloads` and runs `~/bin/termux-file-editor`, which Lindroids installs; it runs
`lindroids open --linux`, which picks the program by MIME type from exported programs and the
.desktop files of Termux, Flatpaks and distros (`~/.config/mimeapps.list` remembers a choice).

Programs get `DBUS_SESSION_BUS_ADDRESS`, `GTK_USE_PORTAL=1` (GLib and GTK 3 use the portal outside
a Flatpak only when asked), `GDK_DEBUG=portals` (GTK 4), `QT_QPA_PLATFORMTHEME=xdgdesktopportal`,
and an `xdg-open` first on PATH that runs `lindroids open`. Programs under proot run as a faked
root, so EXTERNAL authentication cannot work for them; the bus also takes ANONYMOUS, which is safe
because its socket is in the runtime dir only this uid can enter. Distro and Flatpak runs see the
runtime dir at the server's path too, so the paths the FileChooser hands out are valid there.
Because the Linux side of the picker is the server's own filesystem, Lindroids installed in a
proot distro serves that distro's files, and the programs it starts run there too: the shell and
the data belong in the same container as the server. Android's files stay outside it, through the
browser, as before.
Each portal interface publishes its `version` property under that name (zbus would say `Version`):
Chromium and Electron read it to decide whether there is a portal, and without it showed their own
GTK dialogs instead of Android's picker. FileChooser is version 4. Older Electron paths probe
once at startup; the bus must already be ready. In the tested stock Electron 43.0.0, Chromium's
on-demand path uses portal 4 even with `--xdg-portal-required-version=999`, so that override is
ineffective. Fake-root GIO clients also need correct kernel credentials before they can reach
the bus. See [the run regression notes](RUN_REGRESSIONS.md) for the reproduction and fix.

GTK 3 asks the portal only for a GtkFileChooserNative; programs that build a
GtkFileChooserDialog (Mousepad, Xournal++, GIMP, Inkscape, LibreOffice...) showed GTK's own
"Open File" window on the Linux side. `liblindroids-gtk.so`
([third_party/gtkpicker](https://github.com/dixonSolutions/lindroids/blob/main/third_party/gtkpicker/lindroids-gtk.c)), which every run gets in
`LD_PRELOAD` (written to `<runtime>/lib`), turns such a dialog into a native one with the same
action, title, filters, multiple selection and file name, and hands its answer back as the
dialog's (`gtk_dialog_run`'s result or its `response`, and `gtk_file_chooser_get_filename(s)` and
friends); `LINDROIDS_GTK_DIALOGS=1` keeps GTK's. It has no libc and finds GTK with `dlsym`, so
one file per CPU serves bionic and glibc programs. A Response always carries `uris` (empty when
cancelled): GTK 3 reads it unchecked and crashed on a cancelled Save As without it. Globs become
the picker's MIME types (Chromium's `*.[pP][nN][gG]` too); a glob of a type Android does not know
(`*.xopp`) lets every file through rather than hiding the program's own.

*Tested on b16 with `lab/test-file-picker.sh` (2026-10-28): VSCodium (Flatpak, Electron 43) Open
File, Save Workspace As and Open Folder; Mousepad (Termux, GTK 3 dialog) Open and Save As; GNOME
Text Editor (Flatpak, GTK 4) Open and Save As: each a FileChooser call and Android's picker, no
GTK window; a picked Documents/meeting.txt opened in VSCodium, Mousepad and Text Editor.
Xournal++ in a Debian proot (glibc, GTK 3 dialog): Open went to Android's picker. Not tested:
several files picked at once, GIMP, a 32-bit or ARM device.*

The soft keyboard follows taps as on Android (`seat.rs`). It never comes up without a tap; apps
enable text input for a field that has the focus at start, and it used to pop up then. An app that
asks for it (GTK enables text input again on a tap into a field) gets it when a tap is answered
that way, and loses it on a tap that is not *(tested: Mousepad)*. An app that never asks
(Chromium, Electron: nothing on a tap) gets it on a tap while its text input is enabled
*(tested: VSCodium)*. The keyboard button shows it whenever wanted.

## Flatpak

Flatpak's own runner needs bubblewrap, which needs namespaces an Android app does not have.
`lindroids run <app-id>` finds the installation (in Termux's view, inside a proot-distro rootfs,
or a toolbox host), reads the app's and runtime's metadata, resolves extensions (GL, locale,
codecs), and builds the same filesystem view with proot: runtime at `/usr`, app at `/app`,
extensions at their mount points, a generated `/etc/passwd`, `~/.var/app/<id>` as the XDG dirs,
and `$XDG_RUNTIME_DIR/app/<id>`, where apps keep their sockets. `flatpak run [options] <app-id>`
works the same, also with a remote name before the app (`flatpak run flathub <app-id>`), which
`flatpak run` itself does not take. Any other `flatpak` command, or `flatpak run` without an
installed app, is an error in the terminal, with no window: Flatpak's own runner cannot work
here, and its command line is no program with a window.

What else differs from `flatpak run` *(tested on b16: Calculator, VSCodium)*:

- **The run lasts as long as anything in it.** proot runs without `--kill-on-exit`, so a launcher
  that starts the app and returns (VSCodium's command is VS Code's CLI, which starts the editor
  detached and exits) does not take the app with it. A process left with init as its parent
  counts as a child of the one tracing it, the run's proot (`TracerPid`): its windows show in the
  run's window, and it ends with the run.
- **Electron and Chromium apps run unsandboxed.** They start through zypak, which puts each
  Chromium process in Flatpak's sandbox through Flatpak's portal; neither exists here, and
  Chromium's own sandbox cannot work under ptrace (the zygote dies: `write: Broken pipe`). A
  stand-in for zypak's helper starts the program with `--no-sandbox`, and
  `ELECTRON_DISABLE_SANDBOX=1` covers Electron started any other way.

## Home screen shortcuts

With `lindroids config browser app`, the same shortcut's `lindroids launch` opens the run in the
shared window app instead of a Custom Tab (BROWSER_AGNOSTIC_APK.md); the shortcut itself still
belongs to Termux.

`lindroids export <command>` writes `<state>/apps/<id>.json` (name, the `lindroids run`
arguments, with the distro found pinned) and pins a shortcut — with no app of its own:

- **Who pins it.** `shortcut.dex` (from `crates/lindroids-cli/shortcut/Shortcut.java`, built by
  `scripts/build-shortcut-dex.sh` and checked in, so releases need no Java or Android SDK; it is
  embedded in the binary) runs under Android's `/system/bin/app_process`, as Termux's own `am`
  does. The process has Termux's uid, so Android's shortcut service accepts it as the package
  `com.termux` (`IShortcutService.requestPinShortcut`, reached by reflection: its signature
  changes between releases), and the launcher asks the user to add it. Termux has a foreground
  activity or service at that moment, which the service requires. `lindroids remove` disables it
  (an app cannot take a pinned shortcut off the home screen); exporting the same id again
  enables it first, since a disabled shortcut can be neither updated nor pinned.
- **What it does.** Its intent is a shared URL, `http://127.0.0.1:<port>/launch/termux/<id>`,
  sent to Termux's share receiver (`TermuxFileReceiverActivity`; `.app.api.file.FileShareReceiverActivity` since Termux 0.119: the helper picks the one the installed Termux has, since a shortcut to a class that is gone shows "Can't open this app"): no permission or Termux setting
  is involved, Termux starts if it is closed, and it runs `~/bin/termux-url-opener <url>` in a
  session. `lindroids export` installs that opener; it runs `lindroids launch <id>` for these URLs
  and hands every other URL to the opener the user had (kept in
  `~/.local/share/lindroids/termux-url-opener.before`). `lindroids launch` is `lindroids run` with
  the recorded arguments: it starts the server if needed and opens the program's window. If the
  program is already running (the server's `Instances` of its app), it brings back the window
  used last instead; see [Several instances](#several-instances).
- **Where it does not work.** On x86 images with an ARM translation layer, `app_process` is
  killed by seccomp (see above), so no shortcut is made; `lindroids launch <id>` still works. On
  a desktop, an exported program gets a `.desktop` file instead.

## Several instances

Any number of programs run at once, each in a window of its own (a Custom Tab per run, as above),
and so can several instances of one app.

- **Which app.** `lindroids run` registers each run (`CliRequest::Launch`, protocol 6) with the app
  it is an instance of: the Flatpak's id, or `<place>.<program>` (`termux.foot`,
  `debian.xournalpp`), as `export` names it. The server numbers an app's instances with the lowest
  free number (`VSCodium`, `VSCodium 2`); the second and later show their number in their window's
  title (`VSCodium 2 — main.rs`), name and notification. A launch remembers its process's start
  time (field 22 of `/proc/<pid>/stat`), so a pid used again by another process is not it.
- **Tapping an icon** (`lindroids launch <id>`) asks the server for the app's instances, most
  recently used first (a window focused, its page come on screen), and opens that one's window
  again; it starts a program only if none runs. Another window: `lindroids launch --new <id>`,
  `lindroids run --new <command>`, a second icon from `lindroids export --new-window <id>`
  ("`<name>` (new window)"), or the ＋ in a window's tools (`ToServer::NewInstance`: the server runs
  what the run registered as its way to start one more). `lindroids instances` lists what runs;
  `lindroids show <pid|app>` brings one back. Long-pressing the icon is Termux's (the shortcut is
  Termux's), so it offers nothing of Lindroids'.
- **Instances share everything** of their app by default: its `/tmp`, the runtime dir, the
  session bus, the display and its data, as on a desktop. The app's own single-instance mechanism
  (Electron's singleton socket, GApplication on the session bus, GIMP) then works, and two copies
  never write the same files at once. With `--new`, the app is asked for a new window: its desktop
  entry's `new-window` / `new-empty-window` / `new-instance` / `new` action, else a known option
  (`crates/lindroids-cli/src/instances.rs`: VSCodium `--new-window`, GIMP `--new-instance`,
  Mousepad `--opening-mode=window`), else it is just started again.
- **Handed-over windows.** A second VSCodium or Mousepad passes its arguments to the first and
  exits 0 at once; the new window belongs to the first one's process. A run that ends cleanly
  within 5 s while its app has another instance sends `CliRequest::Claim`: the server gives it
  the newest toplevel of that app made since the run started (not a dialog: no parent; not one
  claimed already; `claim_target`), waiting up to 3 s (`LINDROIDS_CLAIM_WAIT`, ms). That window
  moves to the claiming run's own page, and the run lives as long as it holds windows: when the
  last one closes, its page gets `RunEnded` and closes. If no window comes (an app that only
  raises its one window), the run's page closes and the instance that runs is brought forward.
  Programs that start a process per window (foot, GNOME Calculator 51 under Lindroids) are simply
  two runs.
- **Closing.** Closing a claimed run's window asks its windows to close (`xdg_toplevel.close`); it
  never ends the process, which is another instance's. Closing the first instance's window while
  its process also shows a claimed window does the same for its own windows. A program of its own
  ends as before (SIGTERM, SIGKILL 5 s later). VSCodium ignores `xdg_toplevel.close` (tested on
  b16): closing *VSCodium 2*'s window hides it, and the run stays (its notification, its icon or
  `lindroids show` bring it back); VSCodium's own ✕ or File > Close Window closes it for good.
- **Lost runs.** Every 2 s the server looks for runs whose process is gone without `RunEnded`
  (their `lindroids run` was killed): after 3 s their page is told the program ended (code -1) and
  their number is free again.
- **Limits.** Before starting a process, `lindroids run` reads `MemAvailable`: below 700 MB it
  warns, below 300 MB it refuses (Android would close something for it, most likely the browser
  showing the windows). At most 8 programs (`LINDROIDS_MAX_RUNS`), 6 instances of one app
  (`LINDROIDS_MAX_INSTANCES`) and 3 processes of one app (`LINDROIDS_MAX_APP_RUNS`); `--force`
  passes all of these. Android 12 and later also kill an app's child processes beyond 32 in all
  apps (the "phantom process" limit, `max_phantom_processes`), and a program here is several
  (proot, the program, its helpers: VSCodium is about 15): that limit, not these caps, is what
  stops many programs on a phone. It can be lifted with `adb shell device_config put
  activity_manager max_phantom_processes 2147483647` (or Android 14's developer option "Disable
  child process restrictions"). The lab images have it lifted already, so the 32 is not tested.
- **Protocol 6.** The CLI and the server must speak the same protocol; a CLI finding a server of
  another one restarts it when it shows no window, and otherwise says to run `lindroids stop`.

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
  [third_party/compat](https://github.com/dixonSolutions/lindroids/blob/main/third_party/compat/lindroids-compat.c) and embedded in the CLI, is
  preloaded into glibc programs on x86 Android and implements `alarm()` with `setitimer`.

## Where it runs: host detection

One detection, `lindroids_server::host`, used by the CLI and the server, works out the host from
what the system shows rather than from a list of app names, so hosts nobody listed still get the
right behaviour. `lindroids doctor` prints it (`host ...`).

- **Layer**, how this Linux reaches the kernel: Termux's prefix (`$PREFIX`, `TERMUX_VERSION`);
  proot, found by its tracer process (`TracerPid` in `/proc/self/status`) or its loader
  variables, whichever app started it (proot-distro, UserLAnd, Andronix, AnLinux); a root mapped
  by a preloaded library: Cortex Terminal's `libcortex-hook.so` by name, or any other `LD_PRELOAD`
  library when a static binary on Android finds no `/usr` (the root is the library's nearest
  ancestor with an `etc/os-release`); a chroot (`/` is not init's root); a container (docker,
  podman, LXC, distrobox, toolbox, systemd-nspawn, from the marks they leave); or the system.
  Android itself is told by `/system`, binder devices or the kernel's version string, which a
  chroot still shows.
- **Fake root**: `getuid()` is 0 while the kernel's uid is not (proot -0, Cortex). Peers that
  compare the two fail: Xwayland then runs with `-ac` and listens only on its socket file in the
  app's private storage, and the portal signs in to D-Bus with ANONYMOUS.
- **Bridges** to Android, tried in order when a page has to open: Termux's am (socket server,
  then app_process), Cortex's `cortex-api open`, Android's own `am` (only for root and shell, or
  through Termux), then `xdg-open`.
- **App**: the Android package whose files this runs in (`/data/data/<package>` in `$PREFIX`,
  `$HOME`, proot's binary...). The server's sandbox name carries it ("Debian GNU/Linux 13 in
  Termux", "Ubuntu 24.04.5 LTS in Cortex"), and the browser page remembers it to say which app to
  open while it waits for the server.

In a mapped root the static CLI cannot work at all (it never loads the mapping library), so it
execs the glibc build the arm packages carry through the root's own loader
(`crates/lindroids-cli/src/cortex.rs`), which also sets `LOCPATH` for programs and rebuilds the
MIME cache Cortex skips.
