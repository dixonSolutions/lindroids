#!/usr/bin/env bash
# The Lindroids window app on a lab VM (BROWSER_AGNOSTIC_APK.md): no browser, a WebView showing
# the page the Lindroids server in Termux serves.
#
#   lab/test-window-app.sh <vm> [command]       (default foot)
#
# Checks: the APK installs; `config browser app` makes a run open in org.lindroids; the window is
# one full-screen document task before the page is touched; Back keeps it; the program ending
# finishes and removes the task; the app's own offline page appears when the server is not
# running; and the bridge is there (window.lindroids) for the page. With LINDROIDS_APP_DEBUG the
# window advertises WebView DevTools, so the bridge is checked over CDP.
#
# APK=<file> (default target/apk-test/lindroids-*.apk), CLI=<file> (default the x86_64 Android
# build), or build the APK first with packaging/build-webview-apk.sh.
# Results: lab/results/<vm>/window-app/.
set -uo pipefail

LAB_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$LAB_DIR/.." && pwd)
VM=$LAB_DIR/vm.sh
NAME=${1:?usage: test-window-app.sh <vm> [command]}
CMD=${2:-foot}
ID=lab.window-app
CLI=${CLI:-$REPO/target/x86_64-linux-android/release/lindroids}
APK=${APK:-}
OUT=$LAB_DIR/results/$NAME/window-app
mkdir -p "$OUT"
rm -f "$OUT"/*.png "$OUT"/*.txt
sh_() { "$VM" "$NAME" adb shell "$@" | tr -d '\r'; }
adb_() { "$VM" "$NAME" adb "$@"; }
tsh() { timeout "${TSH_TIMEOUT:-90}" "$VM" "$NAME" ssh "$@" 2>&1 | grep -v '^WARNING: linker' | tr -d '\r'; }
log() { echo "[$(date +%T)] $NAME: $*"; }
shot() { adb_ exec-out screencap -p >"$OUT/$1.png" 2>/dev/null; }
declare -A R
finish() {
  { for k in "${!R[@]}"; do printf '%s: %s\n' "$k" "${R[$k]}"; done | sort; } >"$OUT/result.txt"
  cat "$OUT/result.txt"
}
fail() { R[result]="FAIL: $*"; shot fail; finish; exit 1; }

[[ -x $CLI ]] || fail "no Android build at $CLI (cargo ndk -t x86_64 -P 24 build -p lindroids --release)"
if [ -z "$APK" ]; then
  APK=$(ls -1 "$REPO"/target/apk-test/lindroids-*.apk "$REPO"/dist/lindroids-*.apk 2>/dev/null | head -1)
fi
[ -f "$APK" ] || fail "no APK (packaging/build-webview-apk.sh --version 0.1.0 --out target/apk-test --debug-sign, or APK=...)"
R[apk]=$(basename "$APK")

log "installing $(basename "$APK")"
adb_ install -r "$APK" >"$OUT/install.txt" 2>&1 || fail "adb install ($(tr '\n' ' ' <"$OUT/install.txt"))"
R[installed]=no
for _ in $(seq 1 10); do
  if sh_ 'pm list packages' | grep -x package:org.lindroids >/dev/null; then R[installed]=yes; break; fi
  sleep 1
done
[ "${R[installed]}" = yes ] || fail "org.lindroids is not installed ($(tail -3 "$OUT/install.txt" | tr '\n' ' '))"

# The helper (app_process) is killed where an ARM translation layer is loaded, as on these x86
# images; phones have none (see lab/test-lindroids.sh).
if [[ $(sh_ getprop ro.dalvik.vm.native.bridge) != 0 && -f ${LAB_HOME:-$HOME/VMs/lindroids-lab}/tools/magisk ]]; then
  adb_ push "${LAB_HOME:-$HOME/VMs/lindroids-lab}/tools/magisk" /data/local/tmp/magisk >/dev/null 2>&1
  sh_ 'cd /data/local/tmp && chmod 755 magisk && ln -sf magisk resetprop && ./resetprop ro.dalvik.vm.native.bridge 0'
fi

"$VM" "$NAME" scp "$CLI" "/data/data/com.termux/files/usr/bin/lindroids.new" || fail "CLI push"
tsh 'mv $PREFIX/bin/lindroids.new $PREFIX/bin/lindroids && chmod 755 $PREFIX/bin/lindroids'
R[cli]=$(tsh 'lindroids version' | head -1)
tsh "lindroids stop >/dev/null 2>&1; lindroids config browser app; rm -f ~/.local/share/lindroids/apps/$ID.json" >/dev/null
R[setting]=$(tsh 'lindroids config browser')
[ "${R[setting]}" = app ] || fail "config browser is ${R[setting]}"

# The app's task and window, opened by a run with no browser involved.
tsh "cd ~; pkill -x $CMD 2>/dev/null || true; (nohup sh -c 'LINDROIDS_APP_DEBUG=1 lindroids run $CMD' >window-app-run.log 2>&1 < /dev/null &)"
R[linked]="no"
for _ in $(seq 1 40); do
  if tsh 'lindroids status' | grep -E '^display +linked' >/dev/null; then R[linked]=yes; break; fi
  sleep 1
done
[ "${R[linked]}" = yes ] || fail "the run did not link ($(tsh 'lindroids status' | tr '\n' ' '))"
R[activity]="no"
for _ in $(seq 1 20); do
  if sh_ 'dumpsys activity activities' | grep 'org.lindroids/.WindowActivity' >/dev/null; then R[activity]=yes; break; fi
  sleep 1
done
[ "${R[activity]}" = yes ] || fail "the window is not org.lindroids/.WindowActivity"
R[task]=$(sh_ 'dumpsys activity activities' | grep -m1 -o 'Task{[^}]*org.lindroids/.WindowActivity[^}]*}' | sed 's/.*#//; s/ .*//')
R[fullscreen]=$(sh_ 'dumpsys window windows' | sed -n '/StatusBar}/,/Window #/p' | grep -m1 'isVisible=' | tr -d ' ')
shot window
log "window: task ${R[task]}, ${R[fullscreen]}"

# Back stays in the window while the program runs.
sh_ 'input keyevent KEYCODE_BACK' >/dev/null
sleep 2
sh_ 'dumpsys activity activities' | grep 'topResumedActivity=.*org.lindroids/.WindowActivity' >/dev/null || fail "Back left the window"
R[back]='kept the window'
shot back

# The bridge (window.lindroids), over CDP when the app was started with LINDROIDS_APP_DEBUG.
ROOT=$(sh_ 'cat /proc/net/unix | grep -o "webview_devtools_remote[^ ]*" | head -1')
if [ -n "$ROOT" ]; then
  adb_ forward tcp:9224 "localabstract:$ROOT" >/dev/null
  cat >"$OUT/cdp.js" <<'EOF'
const http = require('http');
http.get('http://127.0.0.1:9224/json', (r) => {
  let b = ''; r.on('data', (d) => (b += d)); r.on('end', () => {
    const p = (JSON.parse(b).find((x) => x.type === 'page') || JSON.parse(b)[0]);
    const ws = new WebSocket(p.webSocketDebuggerUrl);
    const expr = process.argv[2];
    ws.onopen = () => ws.send(JSON.stringify({ id: 1, method: 'Runtime.evaluate', params: { expression: expr, returnByValue: true, awaitPromise: true } }));
    ws.onmessage = (m) => { const j = JSON.parse(m.data); console.log(JSON.stringify(j.result && j.result.result && j.result.result.value)); ws.close(); };
    setTimeout(() => process.exit(0), 8000);
  });
}).on('error', () => process.exit(1));
EOF
  R[bridge]=$(PORT=9224 node "$OUT/cdp.js" 'window.lindroids && Object.keys(window.lindroids).join(",")' 2>>"$OUT/cdp.txt")
  R[clipboard]=$(PORT=9224 node "$OUT/cdp.js" 'window.lindroids.writeClipboard("lab"); window.lindroids.readClipboard()' 2>>"$OUT/cdp.txt")
  R[bridge]=$(tr -d '"' <<<"${R[bridge]}")
  R[clipboard]=$(tr -d '"' <<<"${R[clipboard]}")
  adb_ forward --remove tcp:9224 >/dev/null 2>&1
  grep 'pickFiles' >/dev/null <<<"${R[bridge]}" || fail "the bridge is missing (${R[bridge]})"
  [ "${R[clipboard]}" = lab ] || fail "the clipboard bridge did not answer (${R[clipboard]})"
else
  R[bridge]='not checked (no DevTools socket; start with LINDROIDS_APP_DEBUG=1)'
fi

# The program ending finishes and removes the task.
tsh "pkill -x $CMD" >/dev/null
R[ended]='no'
for _ in $(seq 1 20); do
  if ! sh_ 'dumpsys activity activities' | grep 'org.lindroids/.WindowActivity' >/dev/null; then R[ended]=yes; break; fi
  sleep 1
done
[ "${R[ended]}" = yes ] || fail "the program ended but its window is still there"
log "end: ${R[ended]}"

# Termux not running: the app's own page, not the WebView's error page.
tsh 'lindroids stop' >/dev/null 2>&1
sh_ 'am force-stop org.lindroids; am start -a android.intent.action.VIEW -d http://offline.localhost:47999/?window -n org.lindroids/.WindowActivity --es org.lindroids.NAME Mousepad --es org.lindroids.LAUNCH http://127.0.0.1:47999/launch/termux/lab.offline --ez org.lindroids.DEBUG true' >/dev/null
sleep 3
shot offline
R[offline]=no
ROOT=$(sh_ 'cat /proc/net/unix | grep -o "webview_devtools_remote[^ ]*" | head -1')
if [ -n "$ROOT" ] && [ -f "$OUT/cdp.js" ]; then
  adb_ forward tcp:9224 "localabstract:$ROOT" >/dev/null
  body=$(PORT=9224 node "$OUT/cdp.js" 'document.body.innerText' 2>>"$OUT/cdp.txt")
  adb_ forward --remove tcp:9224 >/dev/null 2>&1
  grep 'not running now' >/dev/null <<<"$body" && R[offline]=yes
  R[offline_text]=$(tr -d '"' <<<"$body" | tr '\n' ' ' | cut -c1-100)
fi
[ "${R[offline]}" = yes ] || fail "the offline page did not appear (${R[offline_text]:-no DevTools})"
log "offline: ${R[offline]}"

sh_ 'am force-stop org.lindroids' >/dev/null
R[result]=PASS
finish
