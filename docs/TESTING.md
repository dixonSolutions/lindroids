# Testing

Three layers, from fastest to closest to a phone.

## 1. Unit tests

```sh
cargo test --workspace --exclude lindroids-android
```

Protocol framing and authentication, tile diffing and packing, compositing and blending, popup
placement, keymaps, the dpkg closure reader, Flatpak metadata and installation lookup, bundle
import, the PulseAudio protocol subset, pacing and format conversion.

## 2. The server against real clients, on any Linux machine

`lindroids debug-app` stands in for the Android app: it pairs with a local server, gives every
window a size, writes each window's newest frame to `<dir>/window-<n>.png`, counts sound bytes in
`<dir>/audio-<stream>.bytes`, and takes input commands on stdin:

```
tap <win> <x> <y>        drag <win> <x0> <y0> <x1> <y1>      click <win> <x> <y>
type <win> <text...>     key <win> <android keycode>         resize <win> <w> <h>
close <win>              clip <text...>
```

`tests/host-integration.sh` runs the whole loop (server, debug-app, `lindroids run foot`, typing,
and checks that the frame changed); CI runs it on every push. By hand:

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
provision (Termux, sshd), and a test that installs the APK and CLI, pairs, opens a window, types,
and checks reconnection, with SELinux permissive or enforcing. Results: [lab/README.md](../lab/README.md).

A permissive device is still useful for SELinux work: it logs every access an enforcing one would
refuse (`logcat | grep "avc: denied"`) — that is how the cross-app `connectto` denial behind the
TCP link was found.

Screens can be recorded without a hardware encoder by pulling raw `screencap` frames over adb and
encoding them on the host (the demo videos in `media/` were made that way).
