# A window of its own, without a browser: the Lindroids window app

*Design, 2026-10-29. **Now the default export is B, built in Rust on the phone**: every exported
program gets an app of its own (`org.lindroids.app.<id>`), made from a template in milliseconds
with no SDK or Java on the phone, signed with a key made on the phone, and offered to Android's
installer — see [docs/APPS.md](docs/APPS.md), which also records the tests on a Samsung tablet
(Android 16, Termux from Google Play). The shared app (A) stays for windows of programs that are
not exported (`config browser app`, `lindroids app install`). The rest of this page is the design
as it was decided; where it says "A is the default", read B.*

A Linux program's window used to be a browser's: a Custom Tab per run
([docs/DISPLAY.md](docs/DISPLAY.md)), or a WebAPK that Chrome or Samsung Internet makes
([docs/WEBAPK.md](docs/WEBAPK.md)). Now a small Android app of ours (`org.lindroids`, one APK) is
the default: it shows the program's page in Android's own WebView, with no browser at all, and the
browser paths stay for `config browser default` or `webapk`.

## Why

What the browsers cannot do, after the lab tests of 2026-10-27 and 28:

| Wanted | Custom Tab (any browser) | WebAPK (Chrome, Samsung Internet) | Our window app |
|---|---|---|---|
| Works with any default browser, or none | Needs one that does Custom Tabs | Chrome with Google Play services, or Samsung Internet with the Galaxy Store, installed | Yes: WebView is part of Android |
| Full screen from the start | No: only after the first tap (no route found, DISPLAY.md *Full screen*) | Yes | Yes (immersive mode, set by the app) |
| The program's icon in Recents on One UI | No: Chrome's | Yes | Yes with one app per program; the Lindroids icon with the shared app (below) |
| Back goes to the program, not out of the window | Only through `CloseWatcher` in Chromium | Same | Yes: the app decides |
| The window closes when the program ends | Chromium only; Firefox ignores `window.close()` | Same | Yes (`finishAndRemoveTask`) |
| Nothing sent to Google or Samsung | Yes | No: the name, icon and URL go to their build server | Yes |
| Huawei, Amazon Fire, LineageOS/GrapheneOS and microG phones | Shortcut only | No | Yes |

Tauri was looked at and left out: it builds the same WebView app, but adds the Android SDK, NDK,
Gradle and a Rust runtime of megabytes for a part that only shows a page. The app cannot see
Termux's files or processes (it runs as its own Android user), so all the work stays in the
Lindroids server in Termux; what the app needs is Android code: tasks, immersive mode, Back, file
pickers. A few hundred lines of Java do it, built with the same `javac` and `d8` as
`crates/lindroids-cli/shortcut/Shortcut.java`.

## What the app is

One APK, about 160 KB, minSdk 24 (Android 7, as Termux), targetSdk 35 (Android 14 and later refuse
to install an app whose target is under 23). It asks for one permission, `INTERNET`, and holds no
code of the program's. The APK installs the app as `org.lindroids`.

- **One activity per run**, opened as a document task of its own (`FLAG_ACTIVITY_NEW_DOCUMENT`), with
  the program's name and icon set on the task (`setTaskDescription`).
- **A WebView** that loads the program's page from the server in Termux, exactly as a browser does
  now: the run's window URL, `http://<label>.localhost:<port>/?window&run=<pid>`, for a run and
  for an exported program's launch. Chromium, and so WebView, takes `*.localhost` as the phone
  itself. Cleartext `http` is allowed for `localhost` only (network security config); any other
  address opens in the user's browser instead, so the app can never be pointed at a website.
- **Full screen before the page shows** (`WindowInsetsController`, immersive), so no first tap.
- **Back** is handed to the page (the same message the page's `CloseWatcher` handles now); the
  window closes only when the program ends, or the user closes the task from Recents.
- **The program ends: the task goes** (`finishAndRemoveTask`), Recents included.
- **Termux not running:** the app shows its own page, "<name> runs in Termux, which is not running
  now", and *Start <name>*, which hands the launch URL to Termux as the offline page does now
  (`app-offline.js`, with an explicit `SEND` to `com.termux` instead of the share sheet).
- **A small JavaScript bridge**, offered only to `*.localhost` pages, for what WebView lacks:

  | Page feature | In Chromium today | In the app |
  |---|---|---|
  | File dialogs (`showOpenFilePicker`, `showSaveFilePicker`, `showDirectoryPicker`) | File System Access | Android's own pickers (`OPEN_DOCUMENT`, `CREATE_DOCUMENT`, `OPEN_DOCUMENT_TREE`), written back through the bridge: the same abilities, not Firefox's copy-and-download fallback |
  | Downloads | The browser's | `CREATE_DOCUMENT`, then the bridge writes |
  | Clipboard | `navigator.clipboard` | `ClipboardManager` through the bridge |
  | `navigator.share` | The browser's | `ACTION_SEND` |

The server, the pages, the protocol, the run tickets and `Launch`/`Navigate` stay as they are: the
app is one more place a page is shown. A page tells it is in the app by the bridge being there.

## Who signs it

Android installs only signed APKs, and updates an app only with an APK signed by the same key. A
key can be made in two places:

- **On the phone**, by Lindroids, once. Every phone then has its own key ("Lindroids on this
  phone"); it lives in Termux's storage, and if that is cleared the apps can no longer be updated,
  only installed again. Google knows nothing of it.
- **The project's own key**, kept as a CI secret. It can never ship in the Termux package: anyone
  could take it out and sign anything as Lindroids, and Android would take that as an update of
  our apps. So whatever the project signs is built in our CI or on a server of ours, and the phone
  only downloads it.

That gives three ways of delivering the same app:

| | A. One shared app, signed by us | B. One app per program, built and signed on the phone | C. One app per program, built and signed by our server |
|---|---|---|---|
| Built | In CI, per release | In Termux, per export | On our server, per export |
| The program in the app list and One UI Recents | No: one *Lindroids* entry; the program has a home screen shortcut, and its name and icon in Recents on Launcher3-style launchers | Yes | Yes |
| Offline, nothing leaves the phone | Yes | Yes | No: the name and icon go to our server |
| Extra on the phone | Nothing | `pkg install aapt aapt2 apksigner` (Java, several hundred MB), chosen by the user | Nothing |
| Installs after Google's verification (2027, below) | Yes, registered once | Only after the user's one-time *advanced flow*, or over ADB | Only if Google lets us register package names in bulk (**unknown**) |
| Runs on our infrastructure | CI | Nothing | A server, for good, that must not sign phishing apps for anyone who asks |

## Google's developer verification

From 30 September 2026 certified Android devices in Brazil, Indonesia, Singapore and Thailand
install apps from participating stores only if their package name and signing key are registered
with Google; in 2027 this covers all installs, everywhere. An unregistered app then installs only
over ADB, or once the user has gone through the *advanced flow*: developer mode, a check that
nobody is coaching them, a restart, a one-day wait, then fingerprint or PIN, after which it can
stay on. A free *limited distribution* account covers 20 devices: not for a public package.

So A is the only way that installs with no extra step everywhere after 2027. B works today and
afterwards behind the advanced flow. C depends on a question to Google first.

## Using it (the default export)

```sh
lindroids export <program>               # installs the app on first use, then exports
lindroids export --browser app <program> # the same, explicitly
lindroids app install                    # optional: install before the first export
lindroids app status                     # org.lindroids, installed yes/no
```

`lindroids export` downloads the APK and opens Android's installer when the app is missing; there
is no separate bundling step. `lindroids run` then opens `org.lindroids/WindowActivity` as one document task; the page is the
same one the server serves (`http://<label>.localhost:<port>/?window&run=<pid>`), so runs, tickets
and launches are unchanged. With the app not installed, an explicit `config browser app` fails with the install command;
with the default setting, a run falls back to the browser until an export installs it. The source is
[`packaging/webview-app`](packaging/webview-app), built by
[`packaging/build-webview-apk.sh`](packaging/build-webview-apk.sh); the CLI side is
[`crates/lindroids-cli/src/app.rs`](crates/lindroids-cli/src/app.rs).

The bridge (`window.lindroids`, only on `*.localhost` pages) provides Android's file pickers
(open, save, folder), documents written back in chunks, downloads (`CREATE_DOCUMENT`), the
clipboard both ways, sharing, and starting Termux from the offline page. The page uses it where it
would otherwise use File System Access, `navigator.clipboard` or the browser's download. Back is
dispatched to the page as `lindroids:back`; the program's end calls the bridge's `close()`
(`finishAndRemoveTask`).

### What was tested

On b16 (Android 13, no Chrome, microG), `lab/test-window-app.sh b16` passed on 2026-10-29:
`config browser app` made foot open in `org.lindroids/.WindowActivity` as one full-screen task
(status bar `isVisible=false`); Back kept the task resumed; killing foot finished and removed it;
with the server stopped, the app showed its own "<name> runs in Termux, which is not running now"
page rather than the WebView error; the DevTools check found the bridge and round-tripped the
clipboard. By hand: the open picker returned a picked file, the save picker wrote a document
(`/sdcard/lindroids-save.txt`), and the APK is about 160 KB. The VM has no Chrome or Play services, which
is the point: none is needed.

## The plan

The app's code is the same in A, B and C; only how it is built and signed differs. So:

1. **The app** *(done)*: the activity, the WebView, immersive mode, Back, closing, the offline
   page, the bridge (file pickers, downloads, clipboard, share). Built with the SDK's
   `javac`/`d8`/`aapt2`/`apksigner` (`packaging/build-webview-apk.sh`); the page uses the bridge
   where it uses File System Access, the clipboard and downloads.
2. **Lab proof on b16** *(done for foot; VSCodium, Mousepad, Calculator and One UI still to run
   on the user's phone)*: full screen on open, Back, the program ending, Termux closed, file
   open/save, clipboard both ways. Recents on Launcher3 was not checked.
3. **A: the shared app**, the recommended default way (done in this tree):
   - Released with Lindroids (GitHub Releases; F-Droid or Google Play are **open**), signed by the
     project key, registered with Google's verification.
   - `lindroids export` checks whether it is installed; if not, it downloads the release APK for
     this version and opens Android's installer directly (Termux's FileProvider, temporarily
     allowing external apps to read the APK; `termux.properties` is restored afterwards). The
     first time, Android asks to allow Termux to install apps; then the user taps *Install*. From
     Google Play it would be the store page instead, with no "unknown sources" step.
     `lindroids app install` is only the pre-install.
   - `export` keeps the existing home screen shortcut (owned by Termux, with the program's name
     and icon), whose launch URL runs `lindroids launch <id>`; that window opens with an explicit
     intent to the app, as `Shortcut.java open` does for a Custom Tab. An app-owned shortcut
     (`ShortcutManager.requestPinShortcut` from the app) is not built: the shared app still has
     Termux's badge on pinned icons, as the browser shortcuts do.
   - A new value of `lindroids config browser` (and `export --browser`) picks it, beside `webapk`,
     `default`, a browser's package and `home-screen`; `convert` moves an export to and from it.
     `lindroids run` uses it too when the setting says `app`.
4. **B: one app per program, on the phone** *(done, and the default export, without the build
   tools below: `crates/lindroids-apk` patches a prebuilt template's binary manifest and icons and
   signs with APK Signature Scheme v2 in Rust, so nothing is installed on the phone; docs/APPS.md)*.
   The original plan, kept for the record, needed the user to install the build tools: The same app,
   per program `org.lindroids.p<id>`, built from the release's `classes.dex`:
   ```sh
   keytool -genkeypair -keystore <state>/apk/key.p12 ...        # once per phone
   aapt2 compile -o res.zip res/mipmap/ic_launcher.png          # the program's 512 px icon
   aapt2 link -o unsigned.apk --manifest AndroidManifest.xml \
        -I /system/framework/framework-res.apk res.zip          # untested: no SDK android.jar on the phone
   zip unsigned.apk classes.dex
   zipalign -f -p 4 unsigned.apk aligned.apk
   apksigner sign --ks <state>/apk/key.p12 --out <id>.apk aligned.apk
   termux-open --content-type application/vnd.android.package-archive <id>.apk
   ```
   Termux's `aapt` package has `aapt` and `zipalign`, `aapt2` is its own package, `apksigner`
   brings `openjdk-21` (and `keytool`). A few seconds per export, mostly Java starting. Lindroids
   offers the `pkg install` line; it installs nothing by itself.
5. **C: our build service** *(only if Google confirms package names can be registered in bulk)*.
   Otherwise its apps are no better off than B's after 2027, and cost a server and the program's
   name and icon leaving the phone.

## What stays open

- **Which ways to build**: A alone, A then B, or C (the user's call). A is built and is the
  default export; B and C are not.
- **Where A is published**: GitHub Releases only, F-Droid, Google Play. Every release carries the
  APK, signed with the project key in CI (`LINDROIDS_APP_KEYSTORE`); the workflow fails without
  it, so a test-signed APK is never published.
- **The project key**: the CI secret names are in
  [docs/PACKAGING.md](docs/PACKAGING.md#what-a-release-contains).
- **The "no APK" rule** ([docs/DISPLAY.md](docs/DISPLAY.md), the project so far): this brings back an
  Android app, now the default export; `config browser default` keeps the browser path. Unlike the
  old pairing app it pairs with nothing: it only shows pages from the server, which already checks
  every page by its run ticket.
- Untested: `aapt2 link` against the phone's `framework-res.apk` (B); Play Protect's reaction to
  the apps; how long B's build takes on a phone; pinned shortcut badges on One UI; Recents on
  Launcher3 and One UI with the shared app (b16's launcher is not one of them).

Sources: Android's [developer verification FAQ](https://developer.android.com/developer-verification/guides/faq)
and [limited distribution](https://developer.android.com/developer-verification/guides/limited-distribution);
9to5Google on the [advanced flow](https://9to5google.com/2026/03/19/android-advanced-flow-sideloading/)
and [its rollout](https://9to5google.com/2026/08/18/google-gradually-rolling-out-androids-advanced-sideloading-ahead-of-developer-verification/);
[termux-packages](https://github.com/termux/termux-packages/tree/master/packages) (`aapt`, `aapt2`,
`apksigner`, `d8`) and [termux/android-build-tools](https://github.com/termux/android-build-tools)
(`zipalign`); the [Termux app's manifest](https://github.com/termux/termux-app/blob/master/app/src/main/AndroidManifest.xml)
(`REQUEST_INSTALL_PACKAGES`, the `com.termux.files` provider).
