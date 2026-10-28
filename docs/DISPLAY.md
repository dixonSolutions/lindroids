# Windows, the relay and runs

How a Linux program started in Termux (or a proot distro, or a Flatpak) gets a window on Android,
how that window and the program find and trust each other, and how they live and die together.
Claims marked *(tested)* were checked on the lab VM b16 on 2026-10-27:

- BlissOS 16 (Android 13), with Termux 0.118.3 from F-Droid.
- Browsers: Cromite, Firefox 158 and Jellyfish.
- The probes and raw logs are in [`lab/browser-window/`](../lab/browser-window/).
- `lab/test-lindroids.sh b16` passes with SELinux permissive and enforcing (`lab/README.md`).

## What is wanted

1. Everything lives inside the container: data, programs, the relay and the window's page.
   The container is Termux's prefix, a proot distro or a Flatpak. One package, installed there.
2. Nothing else on Android:
   - no root;
   - no APK of our own;
   - no Termux setting (`allow-external-apps`);
   - no special permission;
   - nothing that depends on an Android version or vendor.
3. Every program gets a **window of its own**: not a tab, no browser interface, not something
   installed through the browser. Lindroids opens and manages it.
4. **The program's icon is used.**
5. **Single run:** `lindroids run X` opens X's window. The window and the program end together.
6. **Export:** `lindroids export X` puts X on the home screen. Tapping it works even when Termux is
   not running.

## What Android allows

On Android, every window on screen is an Activity, and every Activity belongs to an installed app.
A shortcut is only an intent and draws nothing. So *some installed app* draws the window. Without
an APK of ours, that leaves Termux, which is installed, and the browser, which every phone has.
Everything else was tried:

| Window drawn by | Result *(tested)* |
|---|---|
| Termux's terminal | One window only: `TermuxActivity` is `singleTask`, so `NEW_DOCUMENT\|MULTIPLE_TASK` still lands in its one task. It cannot draw pixels: a valid sixel image shows nothing in 0.118.3. |
| A process started from Termux with `app_process` | No window at all. App, toast and panel windows fail with `BadTokenException: token null`. An overlay without "Display over other apps" fails with `Adding window failed`. With it granted (a special permission), Android 13's system_server crashes (branch `experiment/standalone-overlay`). |
| The browser's installed web app ("PWA") | Standalone, with the program's icon in recents. But the user must install it from inside the browser (Install, then Add to home screen), and **only the browser's own home screen icon opens it**. A `VIEW` intent for its URL, or Chromium's `ACTION_START_WEBAPP` (even with the MAC from its shortcut), opens a tab. Firefox installs web apps only for https. Rejected: it is created from the browser, and Lindroids cannot open or close it. |
| The browser as a tab | Every browser can do it. It sits in the browser's task, with tabs and an address bar. Rejected. |
| **The browser as a Custom Tab, started by Termux** | **The window Lindroids uses** (below). |

### The window: a Custom Tab, opened by Termux

A Custom Tab is the browser's own mechanism for showing a web page as part of *another* app. Any
app opens one with an ordinary intent that carries a Custom Tabs session extra; no permission and
no setting are involved. `am` cannot put that extra (a Binder) into an intent, so Lindroids sends
it from its `app_process` helper, as Termux, the way export pins shortcuts
(`crates/lindroids-cli/shortcut/Shortcut.java`, `open`; first tried with
`lab/browser-window/Open.java`).

*(tested with Firefox)*

- **Its own window.** With `NEW_TASK | NEW_DOCUMENT`, every Custom Tab for another URL is a task
  of its own (`ExternalAppBrowserActivity`). Each run has its own URL, so each program gets its
  own window, side by side in recents.
- **No browser interface after the first tap.** The window shows only a thin bar: close, title,
  menu. No tabs, no address bar. On the first tap, the page asks for full screen and gets it:
  nothing of the browser is left, not even the status bar. Firefox briefly shows "Drag from top
  & use back gesture to exit".
- **Raising a window.** Sending the same intent again (the URL it opened with, ticket and all; no
  `MULTIPLE_TASK`) brings that window's task to the front. Chromium does not: its Custom Tabs
  always take a new task, so a second window opens, takes over the run, and the old one closes
  itself *(tested with Cromite)*. Firefox also loads a second copy of the page nobody sees, which the
  relay tells to stand down (see *The handshake*).
- **Closing by the user.** The close button, Back, or swiping the window from recents gives the
  relay a clean `pagehide` and WebSocket close 1001, and the task goes away.
- **Closing by the program: Chromium yes, Firefox no.** In Chromium's Custom Tabs the page's
  `window.close()` ends the window when the program exits *(tested with Cromite)*. It is ignored
  in a Firefox Custom Tab, even a fresh one, and nothing outside Firefox may finish its activity:
  there the window shows the program's end screen, and Back closes it.

**By browser:**

| Browser | Custom Tab |
|---|---|
| Firefox | Works as above *(tested)*. |
| Chrome, Edge, Brave, Vivaldi, Samsung Internet | Chromium's Custom Tabs, as Cromite's below. Chrome on a Samsung phone (One UI, reported by the user): a window of its own, but Recents shows Chrome's icon (below). |
| Cromite | With Settings → Privacy and security → *Allow custom tab intents* on, and as the default browser (it needs both): a window of its own whose task in Recents has the program's name and icon (Chromium's `CustomTabTaskDescriptionHelper` takes the page's title and favicon), which closes itself when the program ends (`window.close()` with one history entry), and File System Access *(tested, Cromite 153)*. Without the setting: an ordinary tab. |
| WebView-based browsers (Jellyfish…) | No Custom Tabs; an ordinary tab. |

Lindroids tries, in order:

1. the browsers named in `LINDROIDS_BROWSER` (comma-separated packages);
2. the default browser, if it is one of these and not Firefox;
3. Chrome, Edge, Brave, Vivaldi, Samsung Internet, Chrome Beta, Chromium, Cromite, then Firefox
   and its beta and nightly builds. Chromium's windows show the program in Recents and close
   themselves; Firefox's cannot (below), so it comes last even as the default.

It uses the first one that opens a window of its own (`WINDOW_BROWSERS` in
`crates/lindroids-cli/src/shortcut.rs`). Custom Tabs are not taken for granted: the helper starts
the window with `startActivityAndWait` and reads which activity the browser opened. A browser
that opens an ordinary tab instead (Cromite without *Allow custom tab intents*, WebView-based
browsers) is passed over; the page in that tab closes itself when the window in the next browser
links, because each attempt's URL names its browser (`&via=<package>`) and the server keeps the
newer browser's page *(tested: Cromite with the setting off → a Firefox window, the Cromite tab
gone)*. If no browser opens a window, the first ordinary tab is kept.

That answer used to be lost: Android 11+ makes every process sample the app ops its calls note,
puts them before the reply, and reading them throws outside an app (`SyncNotedAppOp`). The
helper turns the sampling off (`AppOpsManager.sConfig`, a config that never expires) and gets
whole replies.

### The program's icon

| Where | Icon |
|---|---|
| Home screen | The exported program's own icon and name, as an adaptive icon (the program's drawing on a backdrop of its colour), on a shortcut pinned for Termux. The launcher adds a small Termux badge. Exporting again updates it in place *(tested)*. |
| Status bar and shade | While the program runs: its one-colour (`-symbolic`) icon in the status bar, its name and icon in a notification the helper posts as Termux; a tap brings back its window *(tested)*. Its own notifications show the same way. |
| The window | The page's favicon and title are the program's. While it starts, and after it ends, the page shows the program's icon and name full size *(tested: "Calculator closed." under the Calculator icon)*. |
| Recents | Chromium-based browsers: **the program's icon and name** *(tested, Cromite)*, where the launcher draws a task's own icon. Samsung's One UI launcher draws the browser's icon on every Chrome window's card instead *(reported, Chrome on a Samsung phone)*: the same Chromium code sets the program's icon on the task in Cromite, and the card's name is the program's, so it is the launcher that draws the app's icon over the task's. Only an installed app of the program's own would change that. Chromium sets the page's title, favicon (32 dp or more; the program's is 192 px, at `/run-icon/<run>.png`: a `data:` favicon is not taken) and theme colour on a Custom Tab in a task of its own (`CustomTabTaskDescriptionHelper`). Firefox: **the browser's icon** *(tested)*, with a live thumbnail of the program; it sets a task's icon only for web apps installed through it, which are rejected above. |

## The parts

```
 Android                              │  the container (Termux / proot distro / Flatpak via proot)
                                      │
 browser ── Custom Tab per run ───────┼─ HTTP + one WebSocket per window ─┐
   (own task; full screen)            │  <label>.localhost:<port>         │
                                      │                                   ▼
 Termux                               │            lindroids server  ("the relay", one per uid)
   share receiver ────────────────────┼──▶ termux-  compositor (Wayland; X11 via xwayland-satellite),
   (home screen shortcut)             │    url-opener  sound, portal, the page, launches, tickets
   foreground service ◀── session ────┼──                       ▲ control socket (unix, 0700 dir, uid)
   app_process helper ◀───────────────┼── opens / raises       │
   (Custom Tab intent, shortcuts)     │   windows    lindroids run / launch  (one per run: supervisor)
                                      │                         │ fork
                                      │                         ▼
                                      │                  the program and its children
```

- **The relay** (`lindroids server`):
  - One per uid, started on demand, detached (`setsid`).
  - Serves the page and the WebSocket on loopback.
  - Composites windows, plays sound and answers the desktop portal.
  - Keeps the launches (what `run` started, by pid), the tickets, and one link per page.
  - Stops by itself if its control socket disappears: Termux empties its tmp dir when it starts
    again, and an unreachable relay would hold the port for good.
- **A run** is one start of one program. It consists of:
  - the supervisor;
  - the program and everything under it;
  - its Wayland and X11 windows;
  - the one Custom Tab that shows them.

  A run is named by the supervisor's child's pid.
- **The supervisor** is `lindroids run` (single run) or `lindroids launch` (exported program).
  - It is the program's parent and waits for it.
  - It holds the program's stdio: the terminal for `run`, `<runtime>/launch-<id>.log` for
    `launch`.
  - It never carries pixels.
  - It exits with the program's status, or 128 + the signal number if the program was killed.
- **The window** is the page at `/?window&run=<pid>`, in a Custom Tab of its own. The program's
  toplevels and dialogs are drawn inside it, one tab each in the page's own tab bar. Programs
  started on a desktop, or where no browser has Custom Tabs, share the plain page (`/`) as before.

### Stdio or a process?

The program is an ordinary process, not a stream. `run` forks it with its environment pointing at
the relay's sockets in the runtime dir: `WAYLAND_DISPLAY`, `DISPLAY`, `PULSE_SERVER` and
`DBUS_SESSION_BUS_ADDRESS`. It is not moved to a process group of its own, so one that reads the
terminal does not stop on SIGTTIN. Its stdin, stdout and stderr stay where a desktop would put
them:

- For `run`, the terminal, so `lindroids run gimp` behaves like `gimp`.
- For `launch`, a log file.

The relay ties windows to runs (`icons.rs`, `x11.rs`):

- **A Wayland client** belongs to the launch it descends from. The relay starts from the client's
  pid (`SO_PEERCRED`) and walks up `/proc/<pid>/stat`. This works under proot, whose tracees are
  real processes *(tested: Xournal++ in a proot Debian)*.
- **An X11 window** comes through xwayland-satellite, a single client of the relay. For each of
  satellite's toplevels, the relay asks Xwayland which X client owns the window with that title
  and class (X-Resource, `XResQueryClientIds`), and walks up from that pid *(tested: xclock got
  its own window)*. This runs on a thread with a 500 ms limit, because Xwayland reaches the
  compositor through satellite. It is only asked about satellite's windows, so asking never
  starts X11.
- **A window that appears before its launch is registered** moves to its run's window once it is.

## The origin

The page is served at `http://<label>.localhost:<port>/`:

- `<port>` is fixed per uid (`server_tcp_port`: 47000 + the app id's offset).
- `<label>` is 26 base32 characters (130 random bits), made once and kept in
  `~/.local/share/lindroids/origin` (0600).

Why a `.localhost` name:

- **Every browser treats it as loopback and as a secure context** *(tested)*. Cromite, Firefox and
  Jellyfish all resolve `k7f3q9.localhost` to 127.0.0.1, report `isSecureContext`, and offer
  WebCrypto. No TLS is needed.
- **The origin becomes a capability.** Browser storage belongs to an origin, which includes the
  port, and any app can bind a free loopback port. With the plain `127.0.0.1:<port>` origin, an
  app that binds the port while the relay is down and sends the browser there runs its own page in
  our origin. It can then read or use what the page stored. With an unguessable label, it can
  only serve an origin of its own.
- **Anything without the label gets nothing**, not even the label: the relay answers
  `421 wrong host` *(tested: `curl 127.0.0.1:<port>`)*.
  - It answers only requests whose `Host` is exactly `<label>.localhost:<port>` (case aside).
  - It upgrades a WebSocket only if its `Origin` is exactly `http://<label>.localhost:<port>`.
  - Web sites and DNS rebinding are kept out by the same checks.

The label stays the same for good, so that windows find their origin's storage again. It is not
a secret from everything: it is in the browser's history. Worse, a page that tries to reconnect
while the relay is down sends it to whatever holds the port then. That is why the keys and run
tickets below exist.

## The bridge and its security

### Transport: no TLS, no message encryption

The link is HTTP/1.1, plus one WebSocket per window, over loopback TCP.

- **TLS is not available.** For a local name, the browser would accept only a certificate from a
  CA the user installs in Android's settings, which is a special setting.
- **TLS would protect against nobody.** Reading loopback traffic or injecting into an established
  loopback connection takes root, and root or adb can already read Termux's files and keys.
- **Encrypting each message in the page is rejected for the same reason.** AES-GCM over WebCrypto
  is possible, but it costs CPU on every frame and adds no protection.

What the link needs is for both ends to prove who they are, at the start of each connection. A
loopback TCP connection cannot be taken over once established.

### Threat model

| Who | Can | Stopped by |
|---|---|---|
| Another app | connect to the port | no display key: the handshake fails *(tested: `failed to authenticate`)* |
| Another app | probe `127.0.0.1:<port>` for the page or the label | `421 wrong host` *(tested)* |
| Another app | bind the port while the relay is down | the page checks the relay's signature against the key it pinned and refuses, even with a ticket in hand; only the pairing code makes it accept a new relay key *(tested: replaced `relay.key`)* |
| Another app that learned the label that way, and loads its own page into our origin | use the display key there (it cannot read it out, but it can use it) | that page has no run ticket, so it attaches to no run's window *(tested: a run URL opened by hand without its ticket)* |
| A web site | open a WebSocket to loopback, use DNS rebinding, navigate to our URLs | the `Origin` and `Host` checks; the label |
| Another Android user or work profile | the same as another app | the same |
| Root, adb, the browser itself, the user's own shell | anything | out of scope: they can read the keys |

What is left: such an app's page could link as the *shared* page (`/`), which shows what was
started without a window of its own. On Android every run has its own window, so that is nothing.
On a desktop, it is the programs shown in the browser tab.

### Keys and tokens

| What | Kind | Where it lives | Lifetime | Purpose |
|---|---|---|---|---|
| Origin label | 130-bit random | `~/.local/share/lindroids/origin`; in the browser, as its origin | for good | the origin only our pages run in |
| Relay key | ECDSA P-256 key pair | `~/.local/share/lindroids/relay.key` (0600) | for good | the relay proves it is the relay |
| Display key | ECDSA P-256 key pair, made in the page, `extractable: false` | private key: the origin's IndexedDB (a CryptoKey: usable, not readable); public key: `~/.local/share/lindroids/displays/<id>.json` | until `lindroids displays forget <id>` | the page proves it is a paired browser |
| Run ticket | 128-bit random | relay memory; URL fragment `#t=`; the window's history entry (`history.state`) | while the run's process lives (24 h at most) | pairs a browser the first time; lets a window attach to its run, every time |
| Shared-page ticket | 128-bit random | relay memory; URL fragment `#t=` | 10 minutes | pairs a browser for the shared page |
| Pairing code | 6 digits | relay memory; `<runtime>/pairing-code` (0600); shown by `lindroids pair` | single use; 5 wrong tries close pairing | pairs a page opened by hand; accepts a new relay key |

**Why P-256 and not Ed25519:** every browser with WebCrypto has ECDSA P-256. Ed25519 arrived only
recently. On the wire, public keys are uncompressed SEC1 points and signatures are r‖s (what
WebCrypto makes), both base64url. The Rust side is RustCrypto's `p256`.

**Why key pairs instead of the shared HMAC secret Lindroids used before:**

- The display's private key never exists in readable form, not even for script in our own origin.
- The relay keeps only public keys, so its files can't be used to pose as a display.
- No long-lived secret travels in URLs.
- Each browser is listed and forgotten on its own (`lindroids displays [forget <id>]`).

**How tickets reach the page:**

1. The CLI asks the relay for a ticket over the control socket. The socket lives in the 0700
   runtime dir, and the relay also checks the caller's uid with `SO_PEERCRED`.
2. The CLI puts the ticket in the URL fragment. The helper sends that URL in an intent addressed
   to the browser package, which other apps cannot see.
3. Browsers never send the fragment over HTTP. The page removes it from the address bar at once
   with `history.replaceState`, and keeps it in that history entry's state: a page the browser
   loads again (after dropping it in the background, or a reload) attaches to its run with it.
   No other page can read another window's history state.

Run tickets are not single use, on purpose. The same run must get the same URL every time, because
the window is found again by its URL (raising it, and Firefox restoring it). A run ticket is
useless once its process is gone, and the relay forgets them all when it stops.

### The handshake *(tested; protocol 5)*

These are the first messages on every WebSocket (`lindroids-proto`: `ToServer`, `ToApp`, `auth.rs`).
`origin` is the page's origin (empty for `debug-app`, which is not a page), and `‖` stands for a NUL
separator.

```
page  → Hello     { version, build, density, screen, nonce: n_p, page: "/?window&run=<pid>" }
relay → Challenge { nonce: n_r, relay_key: K_r, proof: Sign(k_r, "lindroids/relay" ‖ origin ‖ n_p ‖ n_r) }
page  : the proof must verify with K_r, and K_r must be the key pinned at pairing.
        Otherwise stop, and accept a new key only with the pairing code.
page  → Auth { display: id, proof: Sign(k_d, "lindroids/display" ‖ origin ‖ n_r ‖ n_p), ticket }   paired
     or Auth { public_key: K_d, proof: (the same, with the new key), ticket | code, name }           pairing
relay : a known display's signature; or, for a new key, its signature plus a good ticket or the code.
        A run's page also needs a good ticket for that run.
relay → Paired { display: id }                 (after pairing; the page keeps k_d, id and K_r)
      → Hello { build: <page build> }          (a page from another build reloads)
      → Run { name, icon }                     (a run's page: shown until the first window)
      or AuthFailed { reason, pairing_open }
```

**One window per run.** When a second page links for a run whose window is still linked, the
*new* page stands down with `Bye` *(tested)*. This is because bringing a window back sends its
intent again, and Firefox then raises the old window *and* loads a second copy nobody sees. The
shared page keeps "newest wins".

## Lifecycles

### Run states

```
 started ──its page links──▶ shown ──page closed (Closing, then close 1001)── 5 s ──▶ ended
    │                         │  ▲                                                   ▲
    │                         │  └──its page links again (within the grace)──┐       │
    │                         └──link lost without Closing──▶ detached ──────┴─ 60 s ┘
 any state ──program exited / Ctrl+C / SIGHUP──▶ ended
```

**When the window goes** (the grace is over):

1. The relay sends SIGTERM to the run's whole process tree, children first. A program under proot
   is proot's tracee and outlives proot otherwise *(tested: Xournal++ was left running before
   this)*.
2. The relay sends SIGKILL to whatever is left 5 seconds later.
3. The supervisor's `wait` returns, and it exits with 128 + the signal number.

**When the program goes:**

1. The supervisor exits with its status.
2. The page shows the program's icon and "<name> closed. Press Back to close this window."
3. It also calls `window.close()` after 1.5 s, in case a splash window gave way to the real one.
   Browsers that allow it close the window; Firefox's Custom Tabs ignore it.

### What the relay can see *(tested)*

| What happened | What the relay sees | Treated as |
|---|---|---|
| Program exited | the supervisor's `wait` returns | end |
| Back, the close button, or swiped away while the browser keeps running | `Closing`, then close frame **1001** | window closed: end in 5 s |
| A reload (Firefox reloads a raised window) | `Closing`, then a new link within a second | the same window: keeps running |
| Swiped away, and it was the browser's last task | **EOF**, no close frame, ~1 s later: Android killed the browser | lost: end in 60 s |
| Browser killed for memory, force-stopped or crashed | **EOF** at once | lost: end in 60 s |
| Window in the background | `visibilitychange: hidden`; timers keep running, throttled | still shown; frames held back |

**The grace period.** A lost connection is ambiguous: swiping the browser's last window away
looks exactly like a memory kill. After a memory kill, the browser reloads the page when the user
comes back to that window; the page links to its run again (the run is in its URL), and nothing
was lost. The default grace is 60 seconds (`LINDROIDS_LINGER`, in seconds):

- A swiped window's program outlives it by at most a minute.
- A memory-killed browser has a minute to come back.

**The page links again by itself** when its link is lost while its program runs, also unseen in
the background *(tested: a dropped link, a reload, both relinked to the running Calculator)*. It
says "<name> closed" only when the program has ended: the server said so (`RunEnded`), it refused
the window's ticket on the way back (a run's ticket goes with its process), or the server cannot
be reached (the program's display went with it). Before, a window took any lost link for the
end, and said so while its program still ran in Termux *(reported: Chrome on a Samsung phone,
after leaving the window and coming back)*.
The page logs how the link was lost (`linked again after the browser dropped the link (code …,
hidden|shown)`) and the server how each page went (`close 1001`, `no close frame`…), in
`<runtime>/server.log`.

**No heartbeat.** A dead browser is reported over loopback at once (EOF). And browsers freeze
pages in the background, so a heartbeat would take a window the user left in the background for
a dead one and end its program.

### Single run: `lindroids run X` *(tested: Mousepad, xclock, foot, Flatpak Calculator, Xournal++ in proot Debian)*

1. Make sure the relay is up.
2. Fork the program, keeping the terminal as its stdio.
3. Register it with the relay: name, icon, and "a window of its own".
4. Open the window:
   - Ask the relay for this run's ticket.
   - The helper (`shortcut.dex open`, run with `app_process` as Termux) sends the Custom Tab
     intent for `http://<label>.localhost:<port>/?window&run=<pid>#t=<ticket>`.
   - The intent goes to the first installed browser of the list in `shortcut.rs`
     (`LINDROIDS_BROWSER` goes first).
   - The intent goes out while Termux is in the foreground, because the user just typed the
     command. Android 10 and later block activity starts from apps in the background.
5. The page pairs (first time) or signs in, and attaches. It shows the program's icon and name
   until the first window comes, then the program's windows, and goes full screen on the first
   tap.
6. Either end ends the run, as above. Ctrl+C in the terminal, or closing the Termux session, ends
   the program; its window then shows the end screen, out of full screen so that the browser's ✕
   shows (Back only sends a Firefox Custom Tab behind; ✕ removes it *(tested)*). A run started
   from a home screen icon ends with Termux's session, server and all: the page cannot reach the
   server again, and takes that as the end of the program.
7. While it runs, a notification stands for it (its icon in the status bar), posted by the helper
   as Termux; requests from the program (sound, file pickers, links, notifications) go to its own
   window, by pid → run.

### Export: `lindroids export X` *(tested)*

1. Write `<state>/apps/<id>.json`: name, icon, and the `run` arguments with the distro pinned.
2. Pin a home screen shortcut with **the program's icon and name**, for Termux, through the
   `app_process` helper. Its intent shares `http://127.0.0.1:<port>/launch/termux/<id>` with
   Termux's share receiver, which needs no permission and no Termux setting.

   Exporting an id that was removed before works: the helper re-enables the shortcut first,
   because Android keeps a removed pinned shortcut disabled and refuses to pin it again.

No browser is involved in exporting. The browser install page and per-program web app manifests
of commit fdaf800 are reverted.

**Tapping the icon, Termux running or not:**

1. Termux starts if needed, and its share receiver runs `~/bin/termux-url-opener`. `export`
   installs that opener; it keeps the user's own opener for every other URL.
2. The opener runs `lindroids launch <id>` in a Termux session. That session keeps Termux's
   foreground service, and with it the relay and the program, alive.
3. `launch` is `run` with the export's arguments. The window opens over Termux (allowed: Termux is
   in the foreground at that moment).
4. When the program ends, `launch` exits. If that session was Termux's only one, Termux closes
   itself.

Timings in the lab: the window was there 2 s after the tap, from a stopped Termux.

**Tapping the icon of a program that is already running** brings its window back instead of
starting another *(tested: one Mousepad, the same window)*. `launch` notes the pid it started in
`<runtime>/launched/<id>.pid` and raises that run's window.

## Keeping the relay alive on Android

Termux's process survives in the background only while `TermuxService` is a foreground service,
that is while a terminal session exists or Termux's wake lock is held. Otherwise Android caches
the process. From Android 11 on, it may freeze the cached process, with the relay and every
program in it, or kill it.

Lindroids needs no wake lock:

- **A single run** is started from a terminal session, which exists while the program runs.
- **An exported program** runs in the session the share receiver opened, until it ends.

*(tested)* The wake lock is the way out for runs that leave their session:

- After a cold start through the share receiver, a session that exited took the service with it.
- With `termux-wake-lock` taken first, the service stayed in the foreground (`procState 4`, OOM
  adj 50) with Termux's window closed, and a detached child kept running.

One limit no app can fix: Android 12's phantom process killer can kill an app's children. It
does so beyond 32 of them across the system, or for children busy in the background. Android 14
has a developer option to turn it off. Lindroids keeps its own process count low and reports
`signal 9` deaths as such.

## The requirements, met

| Requirement | Met by |
|---|---|
| Everything in the container, one package | Yes. The relay, CLI, page, helper, keys and records are all in the package and in `$HOME`. |
| No root, no APK, no special permission or setting | Yes. Lindroids uses intents to the browser and to Termux's share receiver, and pinned shortcuts: nothing else. |
| Any Android version and vendor | Yes, with a browser that has Custom Tabs, which every major one does. Without one, the windows fall back to a plain browser tab. |
| A window of its own, not a tab, not installed through the browser | Yes. A Custom Tab in its own task per program, opened and raised by Lindroids. Full screen from the first tap; before that, a thin bar with close, title and menu. |
| The program's icon | Yes on the home screen, in the status bar and notifications, in the window, as the favicon, and on the start and end screens. In recents too with a Chromium-based browser, except on Samsung's launcher; Firefox shows its own icon there. |
| Single run tied to its process | Window gone: the program ends 5 s later, or 60 s later if the browser died. Program gone: `run` exits and the window closes itself (Chromium), or shows its end screen until ✕ (Firefox does not let a page close a Custom Tab). |
| Export works with Termux closed | Yes. A Termux-owned shortcut with the program's icon, through the share receiver; 2 s to the window in the lab. |

## Ideas to make it better

These have not been tried yet.

- **Stop from the running program's notification.** It has the program's icon and a tap that
  brings back the window (done). A *Stop* button needs a way back from Android to the relay
  without Termux coming to the front (a PendingIntent can only start Termux's activities).
- **Chrome.** Chromium's Custom Tabs are tested through Cromite; Chrome itself only on the user's
  Samsung phone so far.
  On phones with Google Play, Chrome's minted WebAPKs answer `VIEW` intents for their URLs.
- **Closing Firefox's window from outside.** Its source offers no way: Gecko does ask to close
  a one-entry window, but only normal tabs act on it; `ExternalAppBrowserActivity` ends itself
  only when its tab is gone, and only its ✕, "Open in Firefox" or its own end remove that tab
  (the activity is not exported). Every Custom Tab intent it receives gets a new tab, and it
  strips `FLAG_ACTIVITY_CLEAR_TASK` from them: re-sending the window's intent with it left the
  window as it was *(tested, Firefox 158)*. Its Recents icon is Firefox's for the same reason:
  only Firefox's installed web apps (https only) set a task's icon.
- **`app_process` exits 137** after the helper starts an activity (the activity does start; the
  helper prints its result first). Find out why.
