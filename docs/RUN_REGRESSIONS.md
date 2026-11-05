# Launcher, CA and FileChooser regressions

Validated on an unprivileged x86_64 Linux host, 2026-11-05. No Android device was connected.
The original edits to `app.rs`, `run.rs` and `lindroids.js` were retained. No branches were
changed, commits made or crates added.

## Process lifetime

The shell/Node launcher is not necessarily the GUI's lifetime. VS Code's
[CLI spawns with `detached: true`](https://github.com/microsoft/vscode/blob/main/src/vs/code/node/cli.ts),
so following parents or retaining only its process group is insufficient.

`lindroids run` now becomes a Linux child subreaper before spawning the program. Orphaned GUI
processes, including ones that call `setsid()`, are adopted by that CLI. The server retains the
launch's supervisor pid and start time, and maps its descendants to the original run. Dedicated
process groups also cover ordinary forked launchers. Stop/close traversals include adopted
children, and zombies and reused pids do not count as the original process.
If this CLI also started the server, that server and its infrastructure descendants are
explicitly excluded from ownership and signal forwarding. A separate run-level check starts
the server this way and verifies it survives termination of the adopted GUI.

On a successful launcher exit, the CLI asks the server for that run's windows and surviving
processes. It waits up to five seconds for a first mapped window, reports its app id, opens the
run's Android window when applicable, and follows the GUI until its last window closes or its
processes end. A helper without a window cannot keep the run marked running indefinitely.
Runs in the shared display are also marked ended, without sending a whole-page close message
to the shared display. Terminal input and termination signals continue to reach the launched
program; termination also reaches GUI processes in detached sessions.

The regression launcher forks, exits successfully, and its child calls `setsid()`, waits, then
maps a real shared-memory Wayland toplevel. The test verifies ownership, the reported app id,
window closure, termination, ordinary no-window exits and a helper without a window. A unit
test separately checks group identity after reparenting and that zombie children do not keep
a run alive.

## Certificates

For direct program runs inheriting a colon-separated `SSL_CERT_DIR`, Lindroids builds
`<runtime>/ca-<supervisor-pid>/certs` and `bundle.pem`. The directory contains symlinks to readable
PEM certificate files, preserving hash basenames, with the first source winning basename
collisions. Broken links, missing directories and non-certificate files are skipped. The bundle
deduplicates PEM certificate blocks and includes the existing `SSL_CERT_FILE`, if readable.
The child receives one directory in `SSL_CERT_DIR` and the bundle in `SSL_CERT_FILE`. Explicit
`run --env` settings still take precedence. An empty merge fails with an explanation instead
of exporting an empty trust store.

The reported `Cannot open directory ...` message is from Node's
[system CA loader](https://github.com/nodejs/node/blob/v24.17.0/src/crypto/crypto_context.cc),
which scans `SSL_CERT_DIR` as one path. This is distinct from OpenSSL's colon-separated search
path support. Electron has both Node TLS and Chromium networking; reading system certificates
does not by itself establish which trust configuration an application's request uses.

Actual host results from inside runs:

- curl/OpenSSL: `https://example.com` and `https://update.code.visualstudio.com` each returned
  HTTP 200 and `ssl_verify_result=0`, using the merged directory and bundle, without disabling
  certificate verification.
- Official Electron 43.0.0 (Node 24.17.0): `tls.getCACertificates('system')` read the merged
  exports and returned 590 certificates on this host. Electron's Chromium `net.fetch` also
  returned HTTP 200 from `https://update.code.visualstudio.com` inside a run with those exports.

This verifies that Electron's Node system CA loader honours the exports and that Chromium TLS
works on this host. It does not prove that Chromium uses these variables as its trust store.
The Cortex-specific `ERR_SSL_PROTOCOL_ERROR` was not reproduced here; its resolution still
needs an actual device request. A protocol error must not be treated as proof of a missing CA.

## FileChooser

The existing FileChooser already publishes lowercase `version`, of type `u`, equal to 4.
Portal startup completes before the server begins answering CLI status requests, so a newly
started run can read it immediately. A previously running server must be restarted after
installing a new build; an already running Electron instance must also be restarted to repeat
its portal probe. Electron 43 still contains the older
[startup probe](https://github.com/electron/electron/blob/v43.0.0/shell/browser/ui/file_dialog_linux_portal.cc),
but its Chromium on-demand dialog path can bypass that code and its version override (see
[Electron issue 50057](https://github.com/electron/electron/issues/50057)).

The reproduced environmental failure was earlier than the property read: under fake root,
GLib's initial Unix socket message sends apparent uid/gid 0 in `SCM_CREDENTIALS`. The kernel
rejects this with `EPERM`. Allowing ANONYMOUS D-Bus authentication cannot repair an earlier
failed socket send. On the host, the same GIO call succeeds with the real uid and fails with
`Error sending credentials: ... Operation not permitted` after preloading fake uid/gid calls.

The fix is a small preload enabled for fake-root runs using the portal. It corrects only a
`sendmsg` credential record naming this process's apparent identity to its kernel pid/uid/gid;
other messages and application uid queries retain their existing behavior. The shim sits
before the existing Cortex preload. It builds for arm64, armv7, x86 and x86_64.

Verified with the corrected run environment:

- `Properties.Get(FileChooser, version)` returns `<uint32 4>` at startup.
- `GTK_USE_PORTAL=1` is present.
- A GIO `FileChooser.OpenFile` call reaches the debug app's picker.
- Electron 43 under fake root, with `--xdg-portal-required-version=4`, receives a selected file
  through `dialog.showOpenDialog`, with `canceled: false`.
- As a counter-check, stock Electron 43.0.0 also selected a file through portal 4 when given
  `--xdg-portal-required-version=999`, with `GTK_USE_PORTAL=0` and the GTK preload disabled.
  Thus the version override is ineffective in this tested build. `GTK_USE_PORTAL=1` affects
  GTK's own behavior; it does not repair the credential failure or force Electron's own choice.

No Electron flags were added to production launches. The host debug app stands in for the
Android/browser UI.

## Reproduce

Requires an unprivileged Linux user, Python 3, `dbus-daemon`, `gdbus`, and a C compiler. clang
must be available at build time to embed the credential shim. TLS checks require curl and
network access. The default source directories in the host TLS test suit Debian and Fedora;
at least one must contain readable PEM certificates.

```sh
cargo build -p lindroids -p lindroids-server -p lindroids-proto
cargo test -p lindroids -p lindroids-server -p lindroids-proto
LINDROIDS_TEST_TLS=1 python3 tests/run-regressions.py
LINDROIDS_TEST_TLS=1 LINDROIDS_TEST_ELECTRON=/path/to/electron-43/electron \
  python3 tests/run-regressions.py
git diff --check
```

The script starts an isolated server and debug app and tears them down. No normal session
server or browser is used. The credential shim was also cross-linked successfully with clang
and Rust's bundled `ld.lld` for all four supported CPU targets; only x86_64 was executed here.
The build passed; tests reported 65 CLI tests passed (1 existing test ignored), 12 protocol
tests passed and 79 server tests passed. The complete run-level suite, including the optional
TLS and Electron checks, passed. `git diff --check` was clean. Host execution logs are in
`target/launcher-evidence/cargo-test.log` and `target/launcher-evidence/run-regressions.log`.

Remaining device checks: Cortex's actual preload ordering and filesystem mappings, its
VSCodium 1.140/Electron 43 update request, window attachment in the Android app/Custom Tab,
and the visible Android picker and selected-file round trip. `adb devices -l` was empty.
No implementation decisions are pending.
