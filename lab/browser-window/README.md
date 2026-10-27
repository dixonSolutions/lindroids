# Browser window experiments (2026-10-27, b16)

The evidence behind [docs/DISPLAY.md](../../docs/DISPLAY.md): which Android windows Lindroids can
get without an APK, and what a relay sees when they open and close.

- `probe.py` — a stand-in relay (Python, no dependencies) run inside Termux:
  `python probe.py 48100`. `/app/<name>/` is an installable page with its own manifest and icon.
  It reports over a WebSocket where and how it runs: secure context, display mode, lifecycle
  events (`pagehide`, `visibilitychange`, `freeze`…), and heartbeats.
  `curl 127.0.0.1:48100/cmd/close/<name>` makes that page call `window.close()`. Everything goes
  to `~/exp.log`.
- `WinProbe.java` — tries each window type from `app_process` with Termux's uid. Build it like
  `scripts/build-shortcut-dex.sh`, run it with
  `app_process -Djava.class.path=winprobe.dex /system/bin probe.WinProbe <0-3>`.
- `Open.java` — starts an activity as Termux with extras `am` cannot send (the Custom Tabs
  session Binder): `app_process -Djava.class.path=open.dex /system/bin probe.Open com.termux <url>
  -p <browser> --cct -f 18080000`.
- `TaskProbe.java` — what Recents draws for each task: the activity that owns it and the
  TaskDescription (label, icon) that activity set; run as adb's shell:
  `adb shell CLASSPATH=/data/local/tmp/taskprobe.dex app_process /system/bin probe.TaskProbe`.
- `b16-2026-10-27.log`, `b16-2026-10-27-customtabs.log` — the probe's logs from the runs below,
  without heartbeats.

| Test | How | Result |
|---|---|---|
| `<label>.localhost` | `VIEW` `http://k7f3q9.localhost:48100/app/…` to Cromite, Firefox, Jellyfish | all load it from loopback; `isSecureContext` and `crypto.subtle` everywhere |
| Install, Chromium | Install button → *Install app* → launcher *Add to home screen* | pinned `ACTION_START_WEBAPP` shortcut with a MAC; icon opens a standalone task labelled with the app's name and icon |
| Open the installed app from outside | `VIEW` in-scope URL; `ACTION_START_WEBAPP` without and with the MAC, sent from Termux | a tab every time; only the launcher icon opens the app |
| Install, Firefox | menu → *Add to Home screen* | a plain bookmark shortcut (`VIEW` + `mozilla.components.pwa.category.SHORTCUT`) → tab; `VIEW_PWA` filter is https only |
| `window.close()` in the app | `/cmd/close/crA` | task removed; `pagehide(persisted=false)`, `unload`, close 1001 |
| Swipe from recents, other browser tasks exist | recents + swipe | `pagehide`, close 1001 |
| Swipe from recents, last browser task | recents + swipe | EOF ~1.2 s later, no close frame (browser process killed) |
| Force-stop the browser | `am force-stop` | EOF at once |
| Share to Termux, Termux running | `SEND` to `TermuxFileReceiverActivity` | Termux comes to the front, the opener runs in a session that is removed when it exits |
| Share to Termux, cold | same, after `am force-stop com.termux` | Termux front with a foreground service; when the opener exits Termux closes its window, the web app window returns to the front, the service stops |
| …with `termux-wake-lock` | opener takes the lock, starts a detached child, exits | service stays foreground (procState 4, adj 50) with no window; the child runs |
| Web Share from the app | `navigator.share({text})` | Android's share sheet, Termux listed |
| Second Termux window | `am start -f 0x18080000 TermuxActivity` | delivered to the existing task |
| Sixel in Termux 0.118.3 | `cat` a valid sixel | nothing drawn |
| Windows from `app_process` | `WinProbe` 0–3, overlay permission off | `BadTokenException` (token null) ×3, overlay `Adding window failed` |
| Custom Tab from Termux, Firefox | `Open … -p org.mozilla.firefox --cct -f 18080000` | own task (`ExternalAppBrowserActivity`), thin bar (close, title, menu); recents shows Firefox's icon |
| Custom Tab, Cromite | same, `-p org.cromite.cromite` | an ordinary tab |
| Full screen in the Custom Tab | `requestFullscreen` on `click` (on `pointerdown`: denied) | full screen, no browser UI, no status bar |
| `window.close()` in the Custom Tab | `/cmd/close/<name>`, fresh window | ignored (`beforeunload`, then still open) |
| `window.close()` on a tap | `?closetap`, tap the page | ignored as well |
| Close button of the Custom Tab | tap ✕ | `pagehide(persisted=false)`, close 1001, task gone |
| Back in the Custom Tab, one history entry | `input keyevent 4` | the task goes to the back, alive and still in recents |
| Android's accent colour | CSS `AccentColor` in Firefox | the system accent (`#824c76` on b16); `prefers-color-scheme` follows the system's dark mode |
| Raise a Custom Tab | same intent, `NEW_DOCUMENT` without `MULTIPLE_TASK` | the existing task comes to the front; the page reloads |
| Chromium's Custom Tabs (2026-10-28) | Cromite 153 as the default browser with Settings → Privacy → *Allow custom tab intents*; `lindroids run org.gnome.Calculator`, its home screen icon | `CustomTabActivity` in a task of its own; bar with the title (`TITLE_VISIBILITY`), the toolbar colour, no share; full screen on the first tap; Recents: the program's name and icon (TaskDescription from the page's title and favicon; a `data:` favicon is ignored, the task got Lindroids' own icon until the page linked `/run-icon/<run>.png`); the program's ✕ → `window.close()` → task gone; the tab's ✕ → the program ends |
| Whose icon Recents shows (2026-10-28) | `TaskProbe` on a running Calculator window; Firefox's manifest (`aapt2 dump xmltree`) | the task's activity is `org.mozilla.firefox/…customtabs.ExternalAppBrowserActivity`; its TaskDescription has no label and no icon (Firefox does not pass the page's title or favicon on); that activity declares no icon of its own, so Android uses Firefox's application icon (`@0x7f100000`, label "Firefox") |
