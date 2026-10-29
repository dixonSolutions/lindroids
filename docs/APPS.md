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
| Termux from F-Droid or GitHub (declares `REQUEST_INSTALL_PACKAGES`) | Lindroids opens the installer itself, through Termux's file provider | *Install* (the first time: allow Termux to install apps) |
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
  to the program's window. A run's window now closes the app's own tasks that were opened from its
  icon.
- **MediaStore cannot be used from the helper**: `app_process` running as Termux is not an app
  Activity Manager knows ("Unable to find app for caller ... when getting content provider
  media"), and a Context made there calls itself `android` ("Given calling package android does
  not match caller's uid"). Writing the file path in Download directly works instead.
- The service worker the browser apps use for their offline page is not registered inside a
  Lindroids app (the app shows its own offline page).

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

- The direct installer route with Termux from F-Droid (lab VM b16) and the browser route.
- Updating an installed app (a new export) end to end; Play Protect on an update.
- Other launchers' and vendors' Recents (Pixel, Launcher3), Android 7–10 (minSdk 24).
- An app installed but never started, then started from Termux (`lindroids launch`).
