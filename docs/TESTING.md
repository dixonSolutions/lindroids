---
title: "Testing"
description: "Unit tests, the server against real clients, the Android lab and the package tests."
---

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

Stuck input in the page: `node tests/stuck-input.mjs [--js <lindroids.js>] [--evidence out.json]`,
headless Chromium through playwright-core (set `LINDROIDS_PLAYWRIGHT_CORE` if it is not in
`node_modules`). The link is a recorder; a model of the server's seat reads what the page sent.
The cases: blur, hidden page, lost link, window switch (the focus goes, the server lets go, the
key-up sends nothing); auto-repeat (a release and a press, never a second press; modifiers do not
repeat); key-ups with keyCode 229; modifiers from the flags both ways, Meta from keys only; Caps
and Num Lock from key events only; chorded mouse buttons and a page hidden mid-drag; one motion per
move; wheel notches as steps, touchpad deltas as pixels; a rotation (one resize, the screen
turned), a frame of the old size dropped and acknowledged, no resize when nothing changed.

```sh
node tests/stuck-input.mjs --stress [--seed N] [--storms N] [--iterations N]
```

Seeded random storms (default 5 storms of 300 events, seeds 1 to 5): keys down, auto-repeat and
up (stray ups too), IME key events, flags that disagree with the keys held, Caps and Num Lock,
mouse presses, moves and chorded releases, blur and focus, hidden and shown, rotations and resizes
(`Emulation.setDeviceMetricsOverride`), the link closing and linking again. After each storm
everything is let go and the page comes back: nothing may be held by the page or by the server
(model), no key or button pressed while down, the last size sent is the stage's with the
viewport's screen, the last lock state the keyboard's. A failure prints its seed and last steps.

The server's side of the same: `cargo test -p lindroids-server seeded_input_storm` feeds the
seat's routing (`KeyState`, `key_route`, focus, `release_page`, `release_link`) seeded random
key, button, focus, shown, link-drop and lock messages, checking each client is never pressed a
key or button it has down, holds nothing (keys, buttons, depressed modifiers) once the focus, the
page or the link that held it is gone, and has the locks the page said last
(`LINDROIDS_SEAT_SEED`, `LINDROIDS_SEAT_STEPS` pick another storm).

## 3. Android

`lab/` holds a QEMU lab of Android-x86 and BlissOS images (Android 7 to 13): fetch, start,
provision (Termux, sshd), and `lab/test-lindroids.sh`: with no app and no Termux setting, it
installs the CLI, has `lindroids run foot` open the page in the device's browser, types, checks
that the page relinks after a server restart, then exports foot, adds its home screen shortcut,
stops Termux and taps it. `lab/test-window-app.sh` installs the default window app, runs foot in
it (`config browser app`), checks one full-screen task, Back, the program's end removing it, the
offline page, and the bridge over WebView DevTools. Results: [lab/README.md](https://github.com/dixonSolutions/lindroids/blob/main/lab/README.md).

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
each window and the page's layout in `server.log`; pages also log their errors there. It also logs
how often the server woke up (`wakeups: …/s`); `tests/idle-wakeups.py` checks on any Linux machine
that an idle server hardly wakes and that a program whose page is gone draws about once a second.
[lab/CHECKLIST.md](https://github.com/dixonSolutions/lindroids/blob/main/lab/CHECKLIST.md) lists what "fully working" means and where each item
stands.
