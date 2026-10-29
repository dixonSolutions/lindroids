# Testing

Three layers, from fastest to closest to a phone.

## 1. Unit tests

```sh
cargo test --workspace
```

Protocol framing and authentication, the HTTP/WebSocket side of the loopback port (Host and
Origin checks, framing), tile diffing and packing, compositing and blending, popup placement,
keymaps, Flatpak metadata, installation and running-instance lookup, export ids and the
termux-url-opener script, proot's signal reports, the PulseAudio protocol subset, pacing and
format conversion.

## 2. The server against real clients, on any Linux machine

The display is the browser page the server serves: on a desktop, `lindroids run foot` opens it
with `xdg-open`. For scripted tests, `lindroids debug-app` stands in for the page: it pairs with a local server, gives every
window a size, writes each window's newest frame to `<dir>/window-<n>.png`, counts sound bytes in
`<dir>/audio-<stream>.bytes`, and takes input commands on stdin:

```
tap <win> <x> <y>        drag <win> <x0> <y0> <x1> <y1>      click <win> <x> <y>
type <win> <text...>     key <win> <android keycode>         resize <win> <w> <h>
close <win>              clip <text...>
```

`tests/host-integration.sh` runs the whole loop (server, debug-app, `lindroids run foot`, typing,
and checks that the frame changed); CI runs it on every push. The page itself is tested in a real
browser: headless Chromium (Playwright) with phone emulation, driving taps, typing, the
soft-keyboard text path, the clipboard and cursors against GTK 3 and GTK 4 programs. By hand:

```sh
lindroids start
mkfifo cmds; tail -f cmds | lindroids debug-app --dir frames --size 900x600 &
XDG_RUNTIME_DIR=$TMPDIR/lindroids-$(id -u) WAYLAND_DISPLAY=wayland-0 gtk4-widget-factory &
echo "tap 1 258 101" > cmds        # open a dropdown
```

Useful switches: `LINDROIDS_LOG=debug` (commits, configures, frames), `LINDROIDS_TRANSPORT=tcp`
(loopback only, as on enforcing devices), `debug-app --tcp`.

## 3. Android

`lab/` holds a QEMU lab of Android-x86 and BlissOS images (Android 7 to 13): fetch, start,
provision (Termux, sshd), and `lab/test-lindroids.sh`: with no app and no Termux setting, it
installs the CLI, has `lindroids run foot` open the page in the device's browser, types, checks
that the page relinks after a server restart, then exports foot, adds its home screen shortcut,
stops Termux and taps it. `lab/test-window-app.sh` installs the default window app, runs foot in
it (`config browser app`), checks one full-screen task, Back, the program's end removing it, the
offline page, and the bridge over WebView DevTools. Results: [lab/README.md](../lab/README.md).

## 4. Packages

`tests/install-repos.sh` builds every package kind (deb, pacman, rpm, apk, xbps) and the signed
repositories with throwaway keys, serves them locally, and runs `install.sh` twice (install, then
update) in containers of 15 distros, checking the method it picks and `lindroids version`. CI runs
it with docker.

A permissive device is still useful for SELinux work: it logs every access an enforcing one would
refuse (`logcat | grep "avc: denied"`) — that is how the cross-app `connectto` denial behind the
TCP link was found.

Screens are recorded without a hardware encoder (the VMs have none) by `lab/record.sh`, which
pulls raw `screencap` frames over adb and encodes an MP4 and an animated WebP for the README.
A server started with `LINDROIDS_STATS=1` puts the frame rate and drawing time in a corner of
each window and the page's layout in `server.log`; pages also log their errors there.
[lab/CHECKLIST.md](../lab/CHECKLIST.md) lists what "fully working" means and where each item
stands.
