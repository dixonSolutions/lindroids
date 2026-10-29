# A program's own app

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

1. **The template** (`crates/lindroids-apk/template/window-app.apk`, unsigned, 148 KB) is built by
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
