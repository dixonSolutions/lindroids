---
name: android-test
description: Test Lindroids on Android for real - on the user's tablet over USB adb when it is attached and usable, otherwise on a local Bliss OS lab VM (b15/b16). Use whenever a change to the window app, the page (crates/lindroids-server/data/web), export/install, keyboard, touch or performance has to be checked on Android, or the user asks to test "on the tablet", "on Android" or "in the VM".
---

# Testing on Android: tablet first, lab VM otherwise

Lindroids is only done when it works on Android. Pick the device, put the dev build on it, drive
it over adb, look at the screen, and write what was found into `docs/APPS.md` (or the doc the
change belongs to).

## 1. Which device

```sh
ADB=~/Android/Sdk/platform-tools/adb; [ -x $ADB ] || ADB=~/.local/opt/platform-tools/adb
$ADB devices -l
```

- **`R52X201DM2P`** listed as `device` (not `unauthorized`/`offline`): the user's Galaxy Tab
  SM-X306B, Android 16, One UI, Termux from Google Play, 1200x1920. Use it, `-s R52X201DM2P` on
  every call. Its Termux cannot install apps itself (no REQUEST_INSTALL_PACKAGES): exports go
  through Download + My Files; Play Protect may ask (*More details -> Install without scanning*,
  then the PIN: the ClaudeKeys secret `TABLET_PIN`, used only as a placeholder inside a Bash
  command, never printed or written to a file). Never choose "Scan app" (it uploads the APK).
  Before long sessions note and later RESTORE its screen settings (the `real-tablet-adb` memory):
  `screen_off_timeout`, `stay_on`, lock timeout, doze.
- Otherwise (no device, unauthorized, offline): **a lab VM**. Do not wait for the tablet.

## 2. The lab VM

`lab/vm.sh` runs headless QEMU/KVM Bliss OS VMs; each has its own adb port and an ssh into Termux.

| VM | Android | adb | Termux | Use |
| --- | --- | --- | --- | --- |
| b15 | 12L (sdk 32), 1280x800 | 127.0.0.1:5605 | F-Droid 0.118, installs apps itself | default for window-app tests |
| b16 | 13, browsers (Cromite, Firefox, Jellyfish) | 127.0.0.1:5607 | F-Droid 0.118.3 | shared with other agents: check before wiping or taking over the screen |

Never touch the libvirt VM `xtbliss` (127.0.0.1:5555): it is the lead's.

```sh
lab/vm.sh list                      # which run
lab/vm.sh b15 status                # booted? Android version, SELinux
# not running: start detached (QEMU dies with the tool call otherwise), then wait and provision
setsid -f bash -c 'lab/vm.sh b15 start > .scratch/b15-start.log 2>&1 < /dev/null'
lab/vm.sh b15 wait-boot && lab/provision.sh b15     # provision only if ssh does not answer
timeout 20 lab/vm.sh b15 ssh 'lindroids version'
```

`lab/vm.sh b15 adb <args>` is adb for that VM; `lab/vm.sh b15 ssh '<cmd>'` runs in Termux (wrap in
`timeout`); `lab/vm.sh b15 scp <src> <dst>` copies into Termux's home.

## 3. Put the dev build on it

```sh
export PATH=$HOME/.cargo/bin:$PATH ANDROID_NDK_HOME=$(ls -d ~/Android/Sdk/ndk/27* | tail -1)
cargo ndk -t x86_64 -P 24 build -p lindroids --release        # tablet: -t arm64-v8a
lab/vm.sh b15 scp target/x86_64-linux-android/release/lindroids lindroids.new
timeout 60 lab/vm.sh b15 ssh 'lindroids stop; mv ~/lindroids.new $PREFIX/bin/lindroids; chmod 755 $PREFIX/bin/lindroids; lindroids version'
```

The CLI embeds the page (`crates/lindroids-server/data/web`) and the window app's template
(`crates/lindroids-apk/template/window-app.apk`). After changing `packaging/webview-app`, rebuild
the template first: `PATH=<jdk>/bin:$PATH ANDROID_HOME=~/Android/Sdk packaging/build-app-template.sh`
(needs `javac`; commit the new template).

Export and install a program's app (foot is on b15; Calculator needs Flatpak):

```sh
timeout 60 lab/vm.sh b15 ssh 'lindroids export foot'      # builds ~/.local/share/lindroids/apk/termux.foot.apk
# the installer may not come up from a background ssh: install the same APK with adb
timeout 30 lab/vm.sh b15 ssh 'cat ~/.local/share/lindroids/apk/termux.foot.apk' > .scratch/foot.apk
lab/vm.sh b15 adb install -r .scratch/foot.apk
lab/vm.sh b15 adb shell monkey -p org.lindroids.app.termux.foot -c android.intent.category.LAUNCHER 1
```

To test the real install flow, drive the installer's buttons (below) instead of `adb install`.

## 4. Drive and look

```sh
A="lab/vm.sh b15 adb"
$A shell dumpsys window | grep -m1 mCurrentFocus                 # what is in front
$A exec-out screencap -p > .scratch/shot.png                      # then Read the PNG
$A shell uiautomator dump /sdcard/ui.xml && $A shell cat /sdcard/ui.xml   # buttons and bounds
$A shell input tap X Y ; $A shell input swipe X1 Y1 X2 Y2 300
$A shell 'input text "echo%shello"; input keyevent 66'             # hardware-style keys
```

- **Soft keyboard on the VM**: it has a "hardware" keyboard, so Android hides the soft one. To
  test like a tablet: `$A shell settings put secure show_ime_with_hard_keyboard 1` (put back `0`
  when done). Then a tap in a text field shows LatinIME; tap its keys by their bounds.
- **Frame rate**: start the server with `LINDROIDS_STATS=1` (`lindroids stop; LINDROIDS_STATS=1
  nohup lindroids launch <id> &`): the page shows fps, KiB/s and draw time in a corner. A full
  redraw load: `base64 /dev/urandom` in foot (stop it with `pkill -x base64` over ssh; `yes` is no
  load, its lines are identical).
- **Inside the page (DevTools)**: start the app with debugging, then forward its socket:
  ```sh
  $A shell am force-stop <pkg>
  $A shell am start -n <pkg>/org.lindroids.Launch --ez org.lindroids.DEBUG true
  pid=$($A shell pidof <pkg> | tr -d '\r'); $A forward tcp:9444 localabstract:webview_devtools_remote_$pid
  curl -s localhost:9444/json      # webSocketDebuggerUrl -> Runtime.evaluate (Node 22 has WebSocket)
  ```
  Page functions are globals (classic script): wrap them to trace (`showKeyboard`, `tiles`, ...).
- **Video**: the VM has no hardware encoder (`screenrecord` fails): `lab/record.sh`. The tablet
  records with `adb shell screenrecord`.
- `pkill -f <pattern>` over ssh can kill the ssh shell itself when the pattern is in its command
  line: use `pkill -x <name>` or `[/]path` patterns.

## 5. Finish

- Write what was tested, on which device and Android version, before/after numbers and anything
  still open into `docs/APPS.md` (or the relevant doc).
- Put back what you changed on the device: `show_ime_with_hard_keyboard`, the tablet's screen
  settings, `adb forward --remove-all`, `adb reverse --remove-all`.
- Leave the dev build in place or restore `$PREFIX/bin/lindroids` from its backup, and say which.
