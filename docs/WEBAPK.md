# A program as an app of its own: `export --webapk`

`lindroids export --webapk <program>` has the browser install a Linux program as an Android app
of its own, with the program's name and icon: on the home screen, in the app list, and in Recents
on every launcher, Samsung's One UI included. Its windows open full screen from the start.

It is **opt-in**. Plain `lindroids export` (a Termux icon, a Custom Tab window in any browser) is
unchanged and stays the default ([DISPLAY.md](DISPLAY.md)). Where no browser on the phone can do
it, `--webapk` says why and exports nothing.

## What "WebAPK" means

An installed web app on Android is either a home screen shortcut that the browser opens (every
browser), or a **WebAPK**: a small real APK that a server builds from the page's web app manifest
(name, icons, start URL, scope, display), and that Android installs like any app. It holds no code
of Lindroids': it starts the browser to show the page, in a task of its own whose root activity
is the WebAPK's, so Android shows the app's icon and name for it everywhere.

Only two browsers can have one built:

| Browser | WebAPK | Built by |
|---|---|---|
| Chrome (also Beta, Dev, Canary) | Yes, with Google Play services and the Play Store | Google's WebAPK server, through a Play services API only Chrome may use |
| Samsung Internet | Yes, on Samsung phones with the Galaxy Store | Samsung's |
| Edge, Brave, Opera, Vivaldi, Cromite | No: a home screen shortcut, which only its icon opens standalone (tested with Cromite) | |
| Firefox | No: it installs only https pages, and then as a shortcut | |
| Phones with microG instead of Play services, or without either | No | |

So `--webapk` needs Chrome or Samsung Internet *installed*, not as the default browser: the app
belongs to the browser that installed it and always opens in it.

## What happens

1. **Checking** (`Shortcut.java app-browsers`): Chrome counts if it is on, and Google Play services
   and the Play Store are installed and not microG's (microG's Play services have `org.microg.*`
   activities). Samsung Internet counts on a Samsung phone with the Galaxy Store. None: the error
   says what was found, and nothing is written. `LINDROIDS_WEBAPK_BROWSER=<package>` names a
   browser to use regardless (for testing).
2. **The program's pages.** The record (`<state>/apps/<id>.json`) gets `"webapk"`, and the server
   serves the program's own pages under `/app/<id>/` in the usual origin
   (`http://<label>.localhost:<port>`, see [DISPLAY.md](DISPLAY.md), *The origin*):
   - `manifest.webmanifest`: name, `id`/`start_url`/`scope` `/app/<id>/`, `display: fullscreen`,
     two icons: `icon.png` (512 px: Chrome installs no app with an icon under 144 px) and
     `maskable.png` (the program's drawing on a backdrop of its colour, as on its Termux icon).
     Both are made by the helper (`app-icons`). Their URLs carry a hash of the image, since Chrome
     takes an icon's URL as never changing.
   - `/app/<id>/?install`: the install page.
   - `/app/<id>/`: what the app opens with.
   - `/app/<id>/?window&run=<pid>`: a run's window, inside the app's scope.
   - `sw.js`, `offline`, `offline.js`: below.
3. **Installing.** `export` opens `/app/<id>/?install#t=<ticket>` in that browser, as an ordinary
   tab. The ticket pairs the browser, as for a window. The page shows an *Install* button
   (`beforeinstallprompt`), or tells how to install from the browser's menu. The user confirms
   the browser's own dialog: no API lets another app install one.
4. **Waiting.** `export` waits up to 3 minutes for an installed app, other than a browser, that
   opens the program's scope (`apps-for`: activities for its URL, less those for any site), and
   keeps its package in the record. A browser that made a shortcut instead (no Play services) is
   reported after the wait. Exporting again finds an app installed later.

Exporting again updates the record; the browser updates the app's name and icon from the manifest
when it next checks (Chrome 144 and later offers it in its menu rather than forcing it).

## Starting it

| From | What happens |
|---|---|
| The app's icon, server running | The page at `/app/<id>/` links (as an *app page*: not a display, nothing is shown in it), sends `Launch { id }`. The server runs `lindroids launch <id>` with `LINDROIDS_PAGE_LAUNCH=<id>`. When that run's window is ready, the CLI hands its URL to the server (`AppWindow`) instead of opening it, and the server sends the page there (`Navigate`): the app goes to `/app/<id>/?window&run=<pid>&via=<app>#t=<ticket>`. The page goes there itself because Termux is in the background then, and Android lets no app in the background start an activity. A program already running is brought back the same way. |
| The app's icon, Termux closed | The page cannot load. The app's service worker shows its own page instead: "<name> runs in Termux, which is not running now", and *Start <name>*, which shares the program's launch URL with Termux (`navigator.share`). Choosing Termux starts it and runs `lindroids launch <id>`, as a Termux icon does, and the window opens in the app. The page goes back to the program's page by itself once the server answers. |
| `lindroids launch <id>`, typed | The window opens in the app (`view`: a plain `VIEW` intent for the app's package). |
| The running program's notification | Opens the app at the run's window (the window noted for the run in `<runtime>/windows`). |

**One window per app.** The app is one task. When it shows another run of the program that still
runs (`run --new`, the window's *New window*), the next window opens as a Custom Tab, as without
`--webapk`. A run whose window was a Custom Tab before moves to the app when brought back from its
icon (the Custom Tab stands down: the newest page wins).

## Removing

`lindroids remove <id>` deletes the record and opens Android's settings page for the app, where
*Uninstall* takes it away: asking for the uninstall itself (`ACTION_DELETE`) takes a permission
Termux does not have. Long-pressing the app's icon also offers it.

## What is tested, and what is not

*(tested, 2026-10-28)*

- On a desktop, headless Chromium 153 with a server on its own port: the install page paired the
  browser with the ticket; `Page.getInstallabilityErrors` reported nothing for the program's pages
  at `http://<label>.localhost:<port>/app/<id>/`, and complained only about a 128 px icon (hence
  the 512 px one); the service worker registered for `/app/<id>/`; the app page asked for the
  launch and the server started it; with the server stopped, the app's page was the offline page.
- On b16 (Android 13, no Chrome, microG): `export --webapk` stopped with the error and wrote
  nothing; `app-icons` made the 432 px maskable and 512 px plain icons from a 48 px one;
  `apps-for` found the apps for `market:` and `mailto:` links and none for the program's scope.

**Not tested:** a WebAPK actually being built and opened. That needs Chrome with Google Play
services (or Samsung Internet on a Samsung phone), which the lab does not have. Open questions,
for a real phone:

- whether Google's (and Samsung's) server builds a WebAPK for an `http://<label>.localhost:<port>`
  page. Chromium's own check refuses only URLs with a user name or password
  (`webapps_utils.cc`, `IsUrlWebApkCompatible`), and the page is a secure context;
- whether Recents shows the program's icon on One UI (the reason for all this);
- whether Chrome keeps the `#t=` ticket on a `VIEW` intent to the app, and how a WebAPK raised with
  its current URL behaves (reload or not);
- Samsung Internet's apps (their packages, intent filters) at all.

Privacy: building the app sends the program's name, icon and page URL (with the origin label) to
Google's or Samsung's server, once. The run tickets never leave the phone: they are in URL
fragments, which browsers do not send.
