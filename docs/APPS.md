---
title: "A program's own app"
sidebarTitle: "Exported apps"
description: "`lindroids export` builds a program its own Android app on the phone, in Rust, in milliseconds."
---

`lindroids export <program>` gives the program **an Android app of its own**: its name and icon in
the app list, on the home screen and in Recents, and a window that is Android's own WebView, full
screen, with no browser. Lindroids builds the app **on the phone, in Rust, in milliseconds**, signs
it with a key made on the phone, and hands it to Android's installer. Nothing is downloaded and
nothing leaves the phone.

```sh
lindroids export com.vscodium.codium     # builds VSCodium's app and offers it for install
```

```
Built VSCodium's app (org.lindroids.app.com.vscodium.codium, 181 KB, 24 ms).
...tap VSCodium.apk, then Install...
Once it is installed, tap Open (or its icon in the app list): VSCodium starts through Termux the
first time, then by itself, in a window of its own with its name and icon in Recents.
```

## What the app is

The window app's code (`packaging/webview-app`, one activity, a WebView and a small JavaScript
bridge) under the program's own identity:

| | |
|---|---|
| Package | `org.lindroids.app.<id>` (`com.vscodium.codium` → `org.lindroids.app.com.vscodium.codium`) |
| Name and icon | the program's: an adaptive icon (the program's icon inside the safe zone, on a pale tint of its own colour) and a plain one |
| Start page | the program's page on this phone's Lindroids server, `http://<label>.localhost:<port>/app/<id>/` |
| Permissions | `INTERNET` only (the page is on the phone itself: cleartext is allowed for `*.localhost` and nothing else) |
| Size | about 180 KB |
| Signed by | a key made on this phone, once (`<state>/apk/signing.key`, private to Termux) |

Its icon opens the start page, which starts the program and becomes its window. When Termux is not
running, the app shows its own page, "*VSCodium* runs in Termux, which is not running now", whose
*Start* hands the program's launch URL to Termux (the same URL a Termux home screen shortcut
shares); Termux starts the program and its window opens in the app. Windows the server opens later
(`lindroids run`, a notification's tap) open in the same app, so every window of the program has
its name and icon.

## How it is built: no SDK on the phone

Android installs only APKs: a ZIP with a compiled (binary XML) manifest, compiled resources, Dalvik
code, and a signature. Building one normally takes the Android SDK (aapt2, d8, zipalign,
apksigner) and Java: hundreds of megabytes that Termux users should not need. So the project
builds **one template** and the phone only fills it in:

1. **The template** (`crates/lindroids-apk/template/window-app.apk`, unsigned, about 160 KB) is built by
   `packaging/build-app-template.sh` with the SDK, from `packaging/webview-app` and
   `AndroidManifest.program.xml`, whose package, label and meta-data are placeholder strings. It is
   committed, like the helper's `shortcut.dex`, and carried inside the `lindroids` binary.
2. **On the phone** (`crates/lindroids-apk`, pure Rust: `p256`, `sha2`, `flate2`, `crc32fast`):
   - the binary manifest's string pool is decoded, the placeholders replaced (package, name, start
     page, launch URL, terminal app, id) and written again; `versionCode` (minutes since 1970, so a
     new export is always an update) and `versionName` are set in place. An unchanged manifest
     comes out byte for byte (a test checks it);
   - the three icon PNGs are swapped;
   - the ZIP is written again, stored entries aligned to 4 bytes (`zipalign -p 4`: Android 11+
     refuses a compressed or unaligned `resources.arsc`);
   - it is signed with **APK Signature Scheme v2**, ECDSA P-256 with SHA-256, and a self-signed
     certificate "Lindroids on this phone". v2 is all Android 7 (the minimum) and later need.
     Google's `apksigner verify` accepts the result.

`cargo run -p lindroids-apk --example build-app -- out.apk <id> <name> <start> <launch> <icon.png>`
builds one on a computer.

## How it reaches Android's installer

Installing is always the user's tap in Android's installer. Getting the APK there depends on the
terminal app:

| Terminal app | Route | Taps |
|---|---|---|
| Termux from F-Droid or GitHub (declares `REQUEST_INSTALL_PACKAGES`) | Lindroids opens the installer itself, through Termux's file provider | *Install* (the first time: allow Termux to install apps, then export again) |
| **Termux from Google Play** (does not declare it) | Lindroids writes `<Name>.apk` to **Download** and opens the file manager there (My Files on Samsung) | tap `<Name>.apk`, *Install* (the first time: allow the file manager, go back, tap it again) |
| neither works (Download not writable) | the browser downloads it from the program's page (`/app/<id>/<Name>.apk`) | *Download*, *Open*, *Install* |

With Termux paired once with Android's Wireless debugging (`lindroids adb pair`), none of this is
needed: Termux's own `adb` installs the app with no taps ([No-tap installs](ADB.md)).

Then **Google Play Protect** may stop an app it has not seen: "App scan recommended". *Scan app*
uploads the APK to Google; *More details → Install without scanning* installs it after the phone's
PIN or fingerprint. Either is the user's choice; Lindroids cannot answer it.

## Tested on a real phone

Samsung Galaxy Tab Active5 (SM-X306B), Android 16, One UI, Termux from Google Play
(googleplay.2026.10.09, targetSdk 37), 2026-10-29:

| What | Result |
|---|---|
| Build VSCodium's app on the phone | 181 KB in 22–24 ms |
| Install over adb (a check of the APK itself) | installs; the launcher entry opens the app full screen with its name |
| Termux opens Android's installer (the 0.1.108 way) | **fails**: the installer logs `Requesting uid 10160 needs to declare permission android.permission.REQUEST_INSTALL_PACKAGES` and closes at once. This is why 0.1.108 never offered anything |
| Termux writes `/storage/emulated/0/Download/VSCodium.apk` | **works with no storage permission** (Android 11+ lets an app create its own files in Download) |
| My Files opens at Download (`samsung.myfiles.intent.action.LAUNCH_MY_FILES` + `START_PATH`) | works; the APK shows VSCodium's own icon |
| Tap the APK | "Permission required: My Files" → *Settings* → *Allow permission*; Android then returns to My Files, **not** the installer: tap the APK again |
| Installer | "Install this app? VSCodium", VSCodium's icon |
| Play Protect | "App scan recommended" → *More details* → *Install without scanning* → "Install app without scanning?" with *Use PIN* (a secure window: screenshots come out black; `uiautomator dump` reads it) |
| Open from the icon, Lindroids server stopped | the app's own "VSCodium runs in Termux, which is not running now" page |
| *Start VSCodium* | `SEND` to `com.termux/.app.TermuxFileReceiverActivity` → `~/bin/termux-url-opener` → `lindroids launch` → VSCodium's window in the app, full screen, editor usable |
| Recents and the taskbar | **VSCodium's own icon and name** (a browser's Custom Tab shows the browser's icon on One UI) |

Then, driven by a second agent from a written plan (round 1, `0.1.0+dev`):

| What | Result |
|---|---|
| Export again → *Update this app?* → *Update* | Play Protect showed "App scan recommended" again; *More details* opened a help page in the browser this time, and coming back left "App not installed". A second *Update* installed it with no Play Protect prompt (versionCode 29887615 → 29887635) and the installer opened the app |
| Icon with the server stopped → *Start VSCodium* | VSCodium's window, one card in Recents, the start page's card gone. The first tap was lost: the offline page reloaded itself every 4 s while the server was down (fixed: it reloads only once the server answers) |
| Card closed, icon tapped while VSCodium runs | its window comes back, no second instance |
| Back in the window | the window stays (Back belongs to the program) |
| `pkill` VSCodium | its card leaves Recents after about 6 s |
| Termux's card closed (its process stays, for its notification), icon tapped | VSCodium opens directly |

The installer's *Open* and the icon could each make a start-page task (two VSCodium cards before
any window opened): the icon was an alias of the window activity, whose tasks are per document.
The icon is now its own single-task activity (`org.lindroids.Launch`), and every window Lindroids
opens for the program loads in that task: one program, one window.

**Termux from F-Droid** (lab VM b15, Bliss OS, Android 12, Termux 0.118.0): `lindroids export
foot` opened Android's installer directly. The first time it asks to allow Termux ("For your
security, your tablet currently isn't allowed to install unknown apps from this source"); after
*Settings → Allow from this source → Back* Android returned to the home screen, not the installer,
so `lindroids export foot` again, then *Do you want to install this app? Foot → Install*. Foot's
icon then opened foot in `org.lindroids.app.termux.foot`, full screen, first in Recents.

Found and fixed on the way:

- **Termux cannot see the app it offered.** Termux from Google Play targets Android 15+ without
  `QUERY_ALL_PACKAGES`, so package lookups (`getPackageInfo`, `pm path`) do not see other apps;
  "is it installed?" always said no. Lindroids no longer asks.
- **Termux cannot open the app from behind the installer.** Opening it while My Files was in front
  was refused: `Abort background activity starts from 10160`, `BAL_BLOCK`, result 102 (which is
  not an error code: the helper now treats every result but 0–3 as a failure). So the app is not
  opened by `export`: its first start, from the user's tap, hands its launch to Termux.
- **The app's WebView has storage of its own**, not paired with this Lindroids: its start page
  would have asked for `lindroids pair`'s code. An unpaired start page in an app now starts the
  program through Termux instead; the window then opened in the app carries its run's ticket and
  pairs it, and later starts need nothing.
- **Two windows**: the start page ("not running") stayed in Recents as a second VSCodium card next
  to the program's window. Now the icon's activity is single-task and the program's windows load
  in it (see above).
- **MediaStore cannot be used from the helper**: `app_process` running as Termux is not an app
  Activity Manager knows ("Unable to find app for caller ... when getting content provider
  media"), and a Context made there calls itself `android` ("Given calling package android does
  not match caller's uid"). Writing the file path in Download directly works instead.
- The service worker the browser apps use for their offline page is not registered inside a
  Lindroids app (the app shows its own offline page).

## Clean starts and single runs (third round)

Two errors the user saw on the tablet, and what caused them:

- **"VSCodium runs in Termux, which is not running now", all the time.** The app's window still
  showed a run whose server had gone (the binary was replaced while it ran); when a new server
  answered, the offline page's retry loaded that dead run's page again. The retry now goes back to
  the program's start page, which finds the program running or starts it, and only while the app
  is in front.
- **"Another instance of VSCodium is running but not responding".** That VSCodium (under proot) had
  outlived its server: nothing ended the programs a stopping server had launched, and an orphan
  without a display hangs. It also made the next launch think VSCodium's app was busy, so that
  window went to Chrome. The server now ends every program it launched when it stops (the process
  tree, SIGTERM then SIGKILL), and a program's app takes a new run in place.

And two additions:

- **A clean first start.** Opening a program's icon with Lindroids not running starts it through
  Termux by itself (no *Start* tap): tested, the icon handed the launch to Termux within 0.2 s and
  VSCodium's window loaded in the same task.
- **Single runs in a window app of their own.** `lindroids app install` builds the windows app
  (`org.lindroids.windows`, "Lindroids") on the phone from the same template, with no page of its
  own. `lindroids run <program>` opens each run there as one task, with the program's name and icon
  on it (`setTaskDescription`), closed when the program ends and when the server stops (tested:
  GNOME Calculator's window opened in it and went with `lindroids stop`). One UI draws the windows
  app's icon on such a card, not the program's: it ignores task icons (tested earlier with Chrome's
  Custom Tabs). Only an installed app of the program's own (`export`) gets its icon there; installing
  one per run would mean an installer prompt per run.

Play Protect is not the same every time: the first install offered *More details → Install without
scanning* (then the PIN); one update scanned the app by itself and installed it; later installs of
a new app (Calculator, the windows app) offered only *Scan app* or *Don't install app*. For those
tests the same APK, built by Termux, was installed with `adb install` instead.

The demo (`media/demos/export-and-run.mp4`, 45 s) was recorded on the tablet with
`adb shell screenrecord`: `lindroids run org.gnome.Calculator`, then `lindroids export
org.gnome.Calculator` (built in 28 ms), the installer, the icon with Lindroids not running, and
Recents. File names in My Files and other apps' Recents cards are blurred.

## Touch, keyboard and speed (fourth round, lab VM)

Reported from the tablet: the app flashed blue on taps and froze; there was no way to type.
Tested on lab VM b15 (Android 12L, x86_64, foot exported, `LINDROIDS_STATS=1`), since the tablet
was away:

| What | Before | After |
| --- | --- | --- |
| Tap highlight | WebView's default (a blue box on the tapped element) | `-webkit-tap-highlight-color: transparent`, no focus highlight on the WebView (`setDefaultFocusHighlightEnabled(false)`), no long-press selection or callout, no overscroll glow |
| Soft keyboard | opened only by a tap in the hidden tools bar | opens when the program focuses a text field (the page asks the app: `lindroids.keyboard(true)`, `InputMethodManager.showSoftInput`), closes with it |
| Window and keyboard | adjustResize; nothing on Android 15+ edge to edge, where the keyboard covers the bottom | the window shrinks by the keyboard's height (`WindowInsets.Type.ime()`, Android 11+); checked: foot's prompt stays above the keyboard and the window grows back when it closes |
| Full-screen redraw (foot, `base64 /dev/urandom`, 1280x800) | ~30 fps | 40-45 fps: frames acknowledged once drawn instead of on the next animation frame; one decode buffer reused instead of a new one per frame (garbage collection pauses mid-drag); canvas without alpha, low latency (`desynchronized`) |
| Drawing a frame in the page | 6-7 ms | unchanged (LZ4 in JS + `putImageData`) |

Typing on the soft keyboard (LatinIME) and on a hardware keyboard reaches foot, fast taps
included. Open: twice, the first character typed right after the app started was lost
("base64" arrived as "ase64"); not reproduced after, with the page's events traced over
DevTools (the page had the character; the loss is on the server or program side).

## Input and the GPU (fifth round, lab VM)

Asked for: input as fast as it can be, drawing on the GPU. Changed:

- **The window app** asks Android for unbuffered pointer events (`requestUnbufferedDispatch`,
  Android 11+): touches reach the page as they come, not batched to the screen's next frame. It
  also asks for the screen's fastest refresh rate, and declares hardware acceleration.
- **The page** draws in a worker that owns the canvas (`OffscreenCanvas`, `data/web/draw.js`), on
  the GPU: each changed tile goes into a WebGL texture (`texSubImage2D`) that one triangle draws.
  Software GL (SwiftShader, llvmpipe) falls back to `putImageData`; browsers without
  `OffscreenCanvas` run the same code in the page. A lost GPU context gets a whole frame again.
  LZ4 decoding copies repeated runs in doubling chunks (2.4x faster on the same data, and checked
  byte for byte against `lz4_flex` on 60 generated payloads).
- **Input** leaves at once: the lines one event makes go in one WebSocket message; mouse, pen (and
  in WebView, touch) moves come from `pointerrawupdate`; the canvas position is kept, not read on
  every event.
- **The server** packs and sends frames with fewer copies (rows, not pixels; the payload straight
  into the link's buffer), and converts client buffers in loops the compiler vectorizes.
- **Only what changed** (second pass): a client's `damage`/`damage_buffer` rectangles are all that
  is converted from its new buffer, into the memory the surface already has (a new allocation per
  commit was megabytes of page faults); when no surface moved, only their union is composed and
  diffed again. A whole buffer is converted on up to four cores; `lz4_flex` compresses with its
  fast (non-`safe`) encoder. `LINDROIDS_STATS=1` now also logs the server's own time per frame.
- **Keys from the first one:** the window app gives its WebView Android's focus when the window
  opens (a WebView without it took the first D-pad or arrow key to gain focus, and moved the page's
  focus to a button or the hidden text area instead of passing the key on; reported on b15 with
  Tux Typing); the page's buttons and that text area are out of key focus; a window's canvas takes
  the focus when it is activated.

Measured on lab VM b15 (Android 12L, WebView 129, virgl GPU), foot in the windows app, the page's
own timestamps over DevTools (`event.timeStamp` is Android's event time). Medians of three runs,
before and after interleaved; the host was shared with other builds, so single runs vary:

| What | Before | After |
| --- | --- | --- |
| A touch move: Android's event to the page's handler | 14.4 ms (p95 15.2) | 1.4 ms (p95 3.4); `pointerrawupdate` 0.5 ms |
| The same during a full-screen redraw (`base64 /dev/urandom`) | 17.5 ms (p95 34.4) | 2.1 ms (p95 9.1) |
| Full-screen redraw, 1280x800 | 31 fps, drawn in 9 ms (p95 19) | 38.6 fps, 5.7 ms (p95 17.8); final build 47 fps, 4.7 ms |
| A key: press to the program's frame back | 14.5 ms (p95 86) | 6.1 ms (p95 9.2) |
| A mouse move (the VM's USB tablet over QMP) | 10.9 ms | 10.9-14.3 ms: unchanged within the noise |

Second pass, same VM and method, interleaved with the first pass's build (three runs each):

| What | First pass | Second pass |
| --- | --- | --- |
| A key: press to the program's frame back | 6.9 ms (p95 9.5) | 2.9 ms (p95 4.0); 2.8 ms on the final build |
| Full-screen redraw | 45.3 fps | 48.3 fps; 47.7 on the final build |
| Server, per full frame (desktop, foot 1280x800) | convert 2.0 ms, compose 0.6, diff 0.5, pack 2.3 | convert 0.64, compose 0.6, diff 0.4, pack 1.9 |
| Server, per typed character (desktop) | convert ~2 ms, compose ~1.1, diff ~0.6 | convert 0.3, compose 0.3, diff 0.01 |
| The first D-pad key after a window opens | lost when the WebView had no focus (moved the page's focus) | reaches the program (3 of 3) |

The GTK 3 widget factory run gives the same screens as before damage tracking (the only
differing pixels are its two spinners and the pulsing progress bar).

Touches now arrive at the digitizer's rate rather than once a frame, and each goes out at once
(here ~860 a second from adb's injected swipe; a real screen gives 120-480).

Checked by hand on b15: the soft keyboard (LatinIME) types into foot, with Backspace and Enter;
foot in a Firefox 158 Custom Tab draws with WebGL 2 in a worker, takes keys and goes full screen on
the first tap. In headless Chromium with GTK 3's widget factory, in each of the four ways to draw
(worker or page, WebGL or 2D): a click, a touch tap, typing, soft-keyboard text (`é✓`), a touch
drag on a slider, the wheel and a tab switch, with the same screens in all four; at
`devicePixelRatio` 2 too; and a lost and restored WebGL context redrew the whole window. Not
checked: a real phone or the tablet (not attached), a 90/120 Hz screen.

## Switching between apps (sixth round, lab VM)

Reported from the tablet: closing one program's app and opening another flashed the previous
app's screen. Tested on lab VM b15 (Android 12L) with foot's and xclock's apps, recording every
screen update from the VM's VNC display (25-50 ms apart) through each switch, with Termux's
"display over other apps" turned off (as on the tablet: it lets Termux start activities from the
background):

| Switch | Before | After |
| --- | --- | --- |
| An app's first start, its WebView not paired yet, Lindroids running | the page handed the launch to Termux: **Termux's terminal, with whatever it last showed, filled the screen for about half a second**, then the program's window | the page asks its server (`/app/<id>/start`); the program starts and its window comes to the app by a broadcast to its package: splash, "Starting Xclock…", xclock (1.8 s), no Termux |
| The icon of an app whose run is over (the server stopped it, or it ended, while the app was out of sight) | its old window (the last frame, then "Foot closed"), and the app closed itself: no program, tap again | the old frame from Android's snapshot, then its start page starts the program (foot's window 1.2 s after the tap) |
| Close one app (Recents, Home, or the program ending), open another, both paired | no frame of the other app in any of 4 switches | unchanged |

How the first start works now: Termux, in the background, may not start the app's activity
(`Abort background activity starts`), but an explicit broadcast is not an activity start. The
server runs `lindroids launch <id>` with `LINDROIDS_APP_BROADCAST=1`; the helper still tries the
activity, then sends `org.lindroids.WINDOW` with the run's URL to `org.lindroids.app.<id>` alone
(Android 12 answers 0 for the blocked start, Android 16 102, so it always sends it). The app's
start activity loads only a URL under its own start page on its own server. Starting a program
this way needs no pairing, as sharing its launch URL to Termux never did; the window's ticket
reaches only the program's own package. When the server cannot (an older one) or no window comes
within 15 s, the page hands the launch to Termux as before. With Lindroids not running at all,
the start still goes through Termux, which must come forward to start it.

Not checked: the tablet (not attached), Android 13+ (the receiver is registered with
`RECEIVER_EXPORTED` there), a Chrome WebAPK.

## A window and its program go together (seventh round)

Seen on the tablet: VSCodium's whole process tree running under Termux, with no Lindroids task in
Recents. A window swiped away after Android had dropped its page said nothing, and the server kept
programs lost in the background with no limit; a detached GUI also outlived its run, and a window
brought back after its run ended sat on a dead panel. Now (docs/DISPLAY.md, "Lifecycles"):

| | Before | Now |
|---|---|---|
| Task removed from Recents | nothing told the server once the page was gone | the app says so (`CloseWatch.onTaskRemoved`, or `onDestroy` when finishing): the program is asked to close, and ends 5 s later |
| Page lost in the background without a word | the program ran for good | it runs until its page links again, 10 minutes at most |
| The program ends while its page is frozen | the card stayed (only the page could close it) | the server sends the app `org.lindroids.END` with the run's ticket (`End` receiver), and the app removes that task; cards of runs that ended go when the next run's window opens |
| Task rebuilt from Recents after its run ended | "This window was not opened by `lindroids run`" | the end, then the window closes; a program's own app starts the program again |
| "New window" in a program's own app while a run is shown | the new run's page replaced the shown one, which then ended 5 s later | the new run gets a task of its own |
| Android's folder picker | listed on the main thread (Android's "not responding" on big folders), cut silently at 20,000 entries, and the folder itself left out of what the program got | listed on a thread; more than 20,000 entries is an error, unless the folder is in shared storage (`primary:…`), whose path (`/storage/emulated/0/…`) the program gets as it is when Termux can read it (storage access), with no copy; picked files there are used in place too |
| A big picked file | opened again and skipped to each 512 KB chunk (quadratic) | one stream, read on |
| A second picker while one is open | ignored: the program waited an hour | answered at once with an error |

**Out-of-date apps.** The app's code is in each APK, built from the template of the Lindroids that
built it: an app built before these fixes lacks them all, whatever Lindroids is installed now.
The template has a number (`WindowActivity.TEMPLATE`, `lindroids_proto::APP_TEMPLATE`, bumped
together; a test checks), which the app's bridge reports (`template()`; none before this round).
A page in an app older than its server says so once ("This app is out of date", with `lindroids
export <id>` or `lindroids app install`); `lindroids run` (when its window opens), `lindroids
export` and `lindroids doctor` list the apps the server has seen that are older.

### On the tablet (2026-11-10)

Galaxy Tab SM-X306B, Android 16, Termux from Google Play, a dev build of this round
(`0.1.178~dev`), VSCodium (Flatpak) in its own app and GNOME Calculator as a single run in the
shared windows app; both apps rebuilt from template 1 and installed over adb.

| What | Result |
|---|---|
| VSCodium's old app (29 Oct, template 0) with the new Lindroids | opens and works; Termux prints "was built by an older Lindroids (template 0, now 1) ... run `lindroids export com.vscodium.codium`" |
| VSCodium's card swiped away in Recents | "the window app says the window of run ... is closed"; every VSCodium process gone 9 s later |
| Calculator's card (shared windows app) swiped away | its process gone 7 s later |
| VSCodium closed with its own X | its window and card gone in under 3 s |
| Calculator closed with its own X | back in Termux 2 s later |
| Home for 5 s, back to VSCodium | the same window, typing goes on |
| Rotation: landscape, then portrait and landscape twice within 3 s | the window ends at 1920x1200, sharp, nothing stuck |
| Typing in VSCodium's terminal (`input text`, a long press of X) | every character once; the long press gives two, and nothing repeats after it |
| Drawing | WebGL 2 in a worker on the Mali-G68; page draw 0.8 ms a frame; server per frame: convert 4-5 ms, compose 2-3, diff 5-8, pack 0.1-1.9 |
| Server wakeups (`LINDROIDS_STATS=1`), VSCodium in its window and in use | 8-14 a second |
| Server wakeups, VSCodium hidden behind Home | 2.4 a second |

Not tested there: a hardware keyboard's Caps Lock and held keys (adb's key events carry no lock
state and no real hold), a mouse, the folder picker, and a card swiped away more than a minute
after its app left the screen (Android has stopped `CloseWatch` by then: the 10 minute limit
applies). A program that ignores Ctrl+C in Termux (VSCodium under proot does) used to keep
`lindroids run` waiting; when the signal reaches `lindroids` itself the program is now ended 5 s
later, but a Ctrl+C typed in the terminal goes to the program's own group, so close its window.
A single run's card in the shared windows app still shows the app's own icon, not the program's.

## What Android allows, and what we assume

Checked on the tablet unless marked *assumed*:

- An app may **start an activity of an app it cannot see** (explicit intent); it may not *look it
  up*. Starting needs the caller in front (or another background-start exemption).
- Android's installer acts only for an app that **declares `REQUEST_INSTALL_PACKAGES`**; the
  user's "install unknown apps" switch is per source app. My Files, Chrome, Samsung Internet and
  Firefox declare it on the tablet.
- An app targeting Android 11+ may create files in `Download/` by path without any storage
  permission. *Assumed* to hold on other vendors' Android 11+ builds.
- Play Protect's prompt for unknown apps is shown by `com.android.vending` on devices with Google
  Play; *assumed* absent on devices without it (LineageOS, Huawei, Fire).
- **Google's developer verification** (2026–2027, see BROWSER_AGNOSTIC_APK.md): apps signed by an
  unregistered key will need the user's one-time "advanced flow" or ADB once it covers all
  installs. *Untested*: it was not enforced on the tablet on 2026-10-29.
- The per-phone key: if Termux's data is cleared, the key is gone and the installed apps can only
  be uninstalled and installed again (Android refuses an update signed by another key).

## Still to test

- The browser route (a terminal app that may neither install nor write to Download).
- Other launchers' and vendors' Recents (Pixel, Launcher3), Android 7–10 (minSdk 24).
- An app installed but never started, then started from Termux (`lindroids launch`).
