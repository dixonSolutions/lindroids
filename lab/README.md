# Lindroids Android test lab

A persistent set of QEMU/KVM Android VMs (7.1 → 13) to check Lindroids end to end on every version,
permissive and with the app sandbox enforcing. The scripts are here; everything big (ISOs, data
disks, sockets, logs) lives outside the repo in `~/VMs/lindroids-lab` (`$LAB_HOME`).

The official Android emulator segfaults on this host's kernel, hence QEMU with Android-x86 and
BlissOS live images, booted directly with `-kernel/-initrd` from the ISO (attached as a read-only
cdrom; the initrd finds `system.sfs` on it with `SRC=`) and an 8 GB sparse ext4 data disk as
`/data` (`DATA=vda`).

| VM | Android | Image | adb | ssh (Termux) | Notes |
|----|---------|-------|-----|--------------|-------|
| a7 | 7.1.2 (25) | Android-x86 7.1-r5 | 127.0.0.1:5601 | 127.0.0.1:8601 | `VIRT_WIFI=0` (plain ethernet), `buildvariant=userdebug` |
| a9 | 9 (28) | Android-x86 9.0-r2 | 127.0.0.1:5603 | 127.0.0.1:8603 | `VIRT_WIFI=0` |
| b15 | 12L (32) | BlissOS 15.9.2 foss | 127.0.0.1:5605 | 127.0.0.1:8605 | `HWC=drm_minigbm GRALLOC=minigbm_arcvm SETUPWIZARD=0` |
| b16 | 13 (33) | BlissOS 16.9.7 foss (the ISO in `~/Downloads`, read-only) | 127.0.0.1:5607 | 127.0.0.1:8607 | as b15 |

Never touch the lead's VM (libvirt `xtbliss`, adb 127.0.0.1:5555 / emulator-5554): every adb call
in these scripts names its own `-s 127.0.0.1:<port>`.

## Use

```sh
lab/fetch-images.sh [a7 a9 b15 b16]   # resumable, md5-checked (SourceForge), + kernel/initrd, + magiskpolicy
lab/vm.sh b16 start                   # headless, KVM, 3 GB / 4 vCPU; max 2 lab VMs; checks MemAvailable
lab/vm.sh b16 wait-boot
lab/provision.sh b16                  # setup wizard off, Termux + openssh + foot + lab key, sshd
lab/test-lindroids.sh b16             # CLI + window + typing + closing + second run + shortcut -> lab/results/b16/<mode>/
lab/test-windows.sh b16               # a Flatpak's window, run and from its exported icon: Recents icon, kept in the background, ✕ ends it, its end closes it
lab/test-fullscreen.sh b16            # each browser: is the window full screen when it opens (no), and after one tap
lab/test-window-app.sh b16            # the window app: task, full screen, Back, end, offline, bridge
lab/test-recents.sh b16               # whose icon each Custom Tab browser's window can get in Recents (TaskDescription, root activity), per intent shape; ROUTES_ONLY=1: only what the terminal app itself can do about a task (Route.java)
lab/test-file-picker.sh b16 [app]     # Open File, Save As, Open Folder: FileChooser on the bus and Android's picker; a GTK window of the program's own fails (VSCodium, mousepad, org.gnome.TextEditor)
lab/vm.sh b16 stop                    # adb `reboot -p`, then QMP quit
```

Other `vm.sh` actions: `status`, `list`, `adb <args>`, `ssh <cmd>` (into Termux), `scp <src> <dst>`,
`screenshot <png>` (adb screencap, QMP screendump fallback), `console '<cmd>'` (root shell on the
serial console of the Android-x86 images — the way in when adb is down), `qmp '<json>'`, `ports`.

Start knobs: `SELINUX=enforcing|permissive|late`, `GPU=gl|virtio|std` (gl = `virtio-vga-gl` +
`egl-headless` from GNOME Boxes' flatpak QEMU, which has virgl; linuxbrew's QEMU does not),
`EXTRA="k=v"` and `DROP="word"` to edit the image's cmdline, `--fresh` to recreate the data disk.

`provision.sh` serves its setup script and the public key from the host
(`python3 -m http.server` on 127.0.0.1:<ssh port + 100>, i.e. `10.0.2.2` in the guest), and types
`curl -fsS 10.0.2.2:<port>/s|sh` into Termux with `adb shell "input text '...'"` (host double
quotes, device single quotes, so the `|` reaches `input text` instead of the root device shell).
The script does `apt-get full-upgrade` — a plain `upgrade` of the 0.118.0 bootstrap leaves apt and
curl without `liblz4`/OpenSSL 3. It writes `~/lab-setup.log` ending in `LAB-SETUP-DONE` or
`LAB-SETUP-FAILED`, which the host reads over root adb. After a reboot, `provision.sh` only types
`sshd` again. QEMU forwards the ssh port straight to the guest's 8022, so no `adb forward`.

## SELinux

No image boots with `androidboot.selinux=enforcing` here: Android-x86 7.1/9.0 boot-loop in init
(the live root is a tmpfs-labelled ramdisk), BlissOS 15/16 never reach `sys.boot_completed`, and a
bare late `setenforce 1` on Bliss kills SurfaceFlinger (virgl ioctls, or llvmpipe `execmem` with
software rendering) until rescue-party reboots the VM.

`SELINUX=late` boots the image default, then (in `wait-boot`) pushes `magiskpolicy`, applies
`lab/selinux-shim.rules` live and runs `setenforce 1`. The shim makes every domain permissive and
then re-enforces the `untrusted_app*` domains — Termux and Lindroids — so everything they do is
decided by the image's own, enforced policy; it only adds emulator-only rules for those domains
(Mesa enumerating `/dev/dri` via sysfs, the DRM nodes relabelled `gpu_device`, Bliss' Codec2 HAL
running in the init domain). For a7/a9, `late` also switches to software rendering
(`GPU=std HWACCEL=0`): with virgl their apps still cannot initialise EGL under enforcing.
`getenforce` reports `Enforcing`; `run/<vm>/shim.txt` lists what was applied.

## Results with windows of their own (2026-10-27)

Each program in a Custom Tab of its own, opened by Termux's `app_process` helper; no Lindroids app,
no Termux setting, no browser install. Termux 0.118.3 (F-Droid), Firefox 158 (the first browser
with Custom Tabs on the image; the default browser is Jellyfish). The test turns the images' crash
dialogs off (`hide_error_dialogs`): Media Storage keeps crashing on the nearly full data disk, and
its dialog swallowed the test's taps.

| Android | Image | SELinux | Paired (ticket) | Window | Typing | Closing ends the program | Second run, same pairing | Shortcut, Termux stopped |
|---------|-------|---------|-----------------|--------|--------|--------------------------|--------------------------|--------------------------|
| 13 | BlissOS 16.9.7 | permissive | yes | yes | yes | 5 s | yes | 2 s |
| 13 | BlissOS 16.9.7 | enforcing (`late`) | yes | yes | yes | 5 s | yes | 1 s |

Checked by hand on the same VM: X11 (xclock), a Flatpak (GNOME Calculator), a proot-distro
program (Xournal++ in Debian; its whole process tree ends with the window), two programs at once in
two windows (closing one leaves the other), tapping a running program's icon (its window comes
back, no second copy), and the security cases in [docs/DISPLAY.md](../docs/DISPLAY.md): `127.0.0.1`
gets `421`, a run URL without its ticket attaches to nothing, a forgotten browser re-pairs with
the ticket, a replaced relay key is refused until the pairing code is entered.

## The window app: no browser (2026-10-29)

`lab/test-window-app.sh` installs `org.lindroids` and runs a program with `config browser app`
(now the default export).
PASS on b16 (Android 13, SELinux enforcing, no Chrome, microG; `lab/results/b16/window-app/`):
one immersive document task, Back kept, the program's end removed it, the app's own offline page,
the bridge over WebView DevTools (clipboard round trip), and by hand the open/save pickers. Also
checked by hand on b15 (Android 12L): the APK installed and foot opened in
`org.lindroids/.WindowActivity`, full screen, display linked.

## Browser windows: what Android allows (2026-10-27)

Custom Tab and installed web app windows, intents, close detection, Termux's share receiver and wake lock, windows from `app_process`: [browser-window/](browser-window/README.md), behind [docs/DISPLAY.md](../docs/DISPLAY.md).

## Results with the browser as the display (2026-10-27)

No Lindroids app on the device, `allow-external-apps` removed from Termux's settings, Termux
0.118.3 (F-Droid), the image's own browser (Jellyfish, WebView-based):

| Android | Image | SELinux | Linked | Window | Typing | Relink after server restart | Shortcut, Termux stopped |
|---------|-------|---------|--------|--------|--------|-----------------------------|--------------------------|
| 13 | BlissOS 16.9.7 | permissive | yes | yes | yes | 1.0 s | yes |

`lindroids run` opens the page with Termux's `am`; on these x86 images that needs the ARM
translation layer off (`resetprop ro.dalvik.vm.native.bridge 0`, which the test does when
`$LAB_HOME/tools/magisk` — Magisk's x86_64 `libmagisk.so` — is there), as on a phone, which has no
translation layer.

## Results with the old Lindroids app (2026-10-26)

`lab/results/SUMMARY.md` is the table `test-lindroids.sh` maintains; per-run evidence
(screenshots `home/pair-dialog/pair-code/window/typed/window-after-reconnect.png`, `logcat.txt`,
`avc-denied*.txt`, `server.log`, `result.txt`) stays in `lab/results/<vm>/<mode>/` (gitignored).

| Android | Image | SELinux | Boot | Termux | Paired | Transport | Window | Typing | Reconnect (no pairing) |
|---------|-------|---------|------|--------|--------|-----------|--------|--------|------------------------|
| 7.1.2 | Android-x86 7.1-r5 | permissive | ok | ok | yes | unix | yes | yes | 0.8 s |
| 7.1.2 | Android-x86 7.1-r5 | enforcing (late, app sandbox) | boot-time enforcing loops | ok | yes | unix | yes | yes | 1.1 s |
| 9 | Android-x86 9.0-r2 | permissive | ok | ok | yes | unix | yes | yes | 1.4 s |
| 9 | Android-x86 9.0-r2 | enforcing (late, app sandbox) | boot-time enforcing loops | ok | yes | tcp | yes | yes | 0.9 s |
| 12L | BlissOS 15.9.2 | permissive | ok | ok | yes | unix | yes | yes | 0.9 s |
| 12L | BlissOS 15.9.2 | enforcing (late, app sandbox) | boot-time enforcing hangs | ok | yes | tcp | yes | yes | 0.9 s |
| 13 | BlissOS 16.9.7 | permissive | ok | ok | yes | unix | yes | yes | 1.8 s |
| 13 | BlissOS 16.9.7 | enforcing (late; platform enforcing too, except SurfaceFlinger + graphics HALs) | boot-time enforcing hangs | ok | yes | tcp | yes | yes | 1.5 s |

Transport: from Android 9 every app has its own MLS categories, so the Lindroids app's `connectto`
on Termux's abstract socket `@lindroids.server.<uid>` is denied
(`scontext=untrusted_app…:c30… tcontext=untrusted_app…:c26… tclass=unix_stream_socket`) and the
link falls back to TCP; permissive images log the same denial with `permissive=1` and link over
unix — so a permissive run does not show the transport a real device uses. Android 7.1 gives all
apps `s0:c512,c768`, so unix works there enforcing too.

## Caveats

- Disk: the ISOs (0.9–2 GB each) and data disks (1.5–3 GB used each) were deleted after the
  2026-10-26 runs to free `/var/home`; `fetch-images.sh` and `vm.sh start` recreate them, and
  `provision.sh` then takes ~2–3 minutes per VM.
- Termux 0.118.0 has no `termux-am` socket server, and on b16 even 0.118.3's does not start; the
  CLI then opens the browser with Termux's `am` (app_process), which needs the ARM translation
  layer off on these x86 images (the test does it with `resetprop`).
- Bliss is a `user` build but `adb root` works; the test uses it for `pm clear`, `resetprop` and
  uiautomator, never for anything Lindroids itself does.
- When the shortcut test stops Termux, sshd goes with it; the test starts a new Termux session
  (Ctrl+Alt+C) and types `sshd`.
