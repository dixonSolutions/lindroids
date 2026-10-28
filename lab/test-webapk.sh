#!/usr/bin/env bash
# `lindroids export --webapk` as far as a lab without Chrome or Google Play services goes, with
# Cromite on the VM:
#   - no browser that can make apps: the error, and nothing exported;
#   - LINDROIDS_WEBAPK_BROWSER=org.cromite.cromite: the program's install page opens in Cromite,
#     pairs with its ticket, and Cromite finds it installable (Page.getInstallabilityErrors);
#   - the app's start page (/app/<id>/) asks for the program and goes to its window, which links;
#   - with the server stopped, the app's own page (its service worker) says Termux is not running,
#     and its Start button offers Termux in Android's share sheet; choosing Termux starts the
#     program, and the page goes to its window.
# Cromite makes shortcuts, not WebAPKs: opening the installed app itself is for a phone with Chrome
# (docs/WEBAPK.md).
# Everything after the error needs LINDROIDS_WEBAPK_BROWSER (a browser to use, able or not), which
# 152ca85 took out of the CLI; without it the test stops there with PARTIAL (exit 2).
#
#   lab/test-webapk.sh <vm> [command]      (default foot)
#
# Pushes the Android build (CLI=..., default target/x86_64-linux-android/release/lindroids).
# Results: lab/results/<vm>/webapk/ (result.txt, screenshots).
set -uo pipefail

LAB_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$LAB_DIR/.." && pwd)
VM=$LAB_DIR/vm.sh
NAME=${1:?usage: test-webapk.sh <vm> [command]}
CMD=${2:-foot}
ID=lab.webapk
CLI=${CLI:-$REPO/target/x86_64-linux-android/release/lindroids}
OUT=$LAB_DIR/results/$NAME/webapk
CDP_PORT=${CDP_PORT:-9344}
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

# uiautomator: "x y" of the first node whose text/resource-id/content-desc matches a regex.
ui_find() {
  sh_ 'uiautomator dump /data/local/tmp/ui.xml >/dev/null 2>&1; cat /data/local/tmp/ui.xml' | python3 -c '
import re, sys, xml.etree.ElementTree as ET
pat = re.compile(sys.argv[1])
try:
    root = ET.fromstring(sys.stdin.read())
except Exception:
    sys.exit(1)
for n in root.iter("node"):
    for k in ("text", "resource-id", "content-desc"):
        if pat.search(n.get(k) or ""):
            x1, y1, x2, y2 = map(int, re.findall(r"\d+", n.get("bounds")))
            print((x1 + x2) // 2, (y1 + y2) // 2)
            sys.exit(0)
sys.exit(1)' "$1"
}

# Cromite's pages over DevTools: `cdp list` (id and URL), `cdp eval <page> <expression>` (with a
# user gesture), `cdp send <page> <method> [params]`, `cdp close <url part>` (every such page).
# <page>: its id, or a part of its URL. Prints the value, or nothing.
cat >"$OUT/cdp.js" <<'EOF'
const [mode, want, a, b] = process.argv.slice(2);
const port = process.env.CDP_PORT;
(async () => {
  const pages = (await (await fetch(`http://127.0.0.1:${port}/json`)).json()).filter((p) => p.type === 'page');
  if (mode === 'list') return pages.forEach((p) => console.log(p.id + ' ' + p.url));
  if (mode === 'close') {
    for (const p of pages.filter((x) => x.url.includes(want))) await fetch(`http://127.0.0.1:${port}/json/close/${p.id}`);
    return;
  }
  const p = pages.find((x) => x.id === want) || pages.find((x) => x.url.includes(want));
  if (!p) return;
  const ws = new WebSocket(p.webSocketDebuggerUrl);
  const msg = mode === 'eval'
    ? { method: 'Runtime.evaluate', params: { expression: a, userGesture: true, awaitPromise: true, returnByValue: true } }
    : { method: a, params: b ? JSON.parse(b) : {} };
  ws.onopen = () => ws.send(JSON.stringify({ id: 1, ...msg }));
  ws.onmessage = (m) => {
    const r = JSON.parse(m.data).result || {};
    console.log(JSON.stringify(mode === 'eval' ? (r.result || {}).value : r));
    ws.close();
  };
  setTimeout(() => process.exit(0), 10000);
})().catch((e) => console.error('cdp:', e.message));
EOF
cdp() { CDP_PORT=$CDP_PORT timeout 15 node "$OUT/cdp.js" "$@" 2>>"$OUT/cdp-errors.txt"; }
# Waits up to $1 s for a Cromite page whose URL has $2: its id.
page_with() { for _ in $(seq 1 "$1"); do cdp list | grep -F -- "$2" | head -1 | cut -d' ' -f1 | grep . && return 0; sleep 1; done; return 1; }
# The URL of page $1 (its id).
url_of() { cdp list | awk -v id="$1" '$1 == id { print $2 }'; }

# --------------------------------------------------------------- setup
[[ -x $CLI ]] || fail "no Android build at $CLI (cargo ndk -t x86_64 -P 24 build -p lindroids --release)"
sh_ 'pm list packages' | grep -x package:org.cromite.cromite >/dev/null || fail "Cromite is not installed"
# The helper (app_process) is killed where an ARM translation layer is loaded, as on these x86
# images; phones have none (see test-lindroids.sh).
if [[ $(sh_ getprop ro.dalvik.vm.native.bridge) != 0 && -f ${LAB_HOME:-$HOME/VMs/lindroids-lab}/tools/magisk ]]; then
  adb_ push "${LAB_HOME:-$HOME/VMs/lindroids-lab}/tools/magisk" /data/local/tmp/magisk >/dev/null 2>&1
  sh_ 'cd /data/local/tmp && chmod 755 magisk && ln -sf magisk resetprop && ./resetprop ro.dalvik.vm.native.bridge 0'
fi
"$VM" "$NAME" scp "$CLI" "/data/data/com.termux/files/usr/bin/lindroids.new" || fail "CLI push"
tsh 'mv $PREFIX/bin/lindroids.new $PREFIX/bin/lindroids && chmod 755 $PREFIX/bin/lindroids'
R[cli]=$(tsh 'lindroids version' | head -1)
tsh "lindroids stop >/dev/null 2>&1; rm -f ~/.local/share/lindroids/apps/$ID.*; true"
adb_ forward tcp:"$CDP_PORT" localabstract:chrome_devtools_remote >/dev/null
# Pages of earlier runs would answer for this one's.
cdp close "/app/$ID/"
log "CLI ${R[cli]}"

# --------------------------------------------------------------- no browser that can make apps
tsh "lindroids export --webapk --id $ID $CMD; echo exit=\$?; test -e ~/.local/share/lindroids/apps/$ID.json && echo written" >"$OUT/no-browser.txt"
if grep -q 'no browser here can install' "$OUT/no-browser.txt" && grep -q 'exit=1' "$OUT/no-browser.txt" && ! grep -q written "$OUT/no-browser.txt"; then
  R[no_browser]="error, nothing exported"
else
  fail "without a browser that can make apps: $(tr '\n' ' ' <"$OUT/no-browser.txt")"
fi
log "no browser: ${R[no_browser]}"

# --------------------------------------------------------------- the install page, in Cromite
# Termux in front, as when the user types the command (Android lets no app in the background
# start an activity).
sh_ 'am start -n com.termux/.app.TermuxActivity' >/dev/null 2>&1
sleep 2
tsh "LINDROIDS_WEBAPK_BROWSER=org.cromite.cromite LINDROIDS_WEBAPK_WAIT=8 lindroids export --webapk --id $ID $CMD; echo exit=\$?" >"$OUT/export.txt"
if grep -q 'no browser here can install' "$OUT/export.txt"; then
  # This lindroids does not take LINDROIDS_WEBAPK_BROWSER: Cromite cannot stand in for Chrome.
  R[result]="PARTIAL: only the error was checked (this lindroids ignores LINDROIDS_WEBAPK_BROWSER)"
  finish
  exit 2
fi
grep -q 'install page' "$OUT/export.txt" || fail "export: $(tr '\n' ' ' <"$OUT/export.txt")"
R[export]=$(grep -E 'No app was installed|is an app of its own' "$OUT/export.txt" | head -1)
TAB=$(page_with 20 "/app/$ID/?install") || fail "no install page in Cromite ($(cdp list | tr '\n' ' '))"
for _ in $(seq 1 15); do
  panel=$(cdp eval "$TAB" "document.getElementById('panel').innerText")
  grep -q 'Install' <<<"$panel" && break
  sleep 1
done
R[install_page]=$(tr -d '"' <<<"$panel" | tr '\n' ' ' | cut -c1-140)
grep -q 'Install' <<<"$panel" || fail "install page: $panel"
R[installability]=$(cdp send "$TAB" Page.getInstallabilityErrors)
R[manifest_errors]=$(cdp send "$TAB" Page.getAppManifest | python3 -c 'import json,sys; print(json.load(sys.stdin).get("errors"))' 2>/dev/null)
R[service_worker]=$(cdp eval "$TAB" "navigator.serviceWorker.getRegistration().then(r => r ? r.scope + (r.active ? ' active' : ' installing') : 'none')")
shot install-page
log "install page: ${R[install_page]}"

# --------------------------------------------------------------- the app's start page
# What the installed app opens with: it asks for the program and goes to its window.
cdp eval "$TAB" "location.href = '/app/$ID/'; 1" >/dev/null
win=""
for _ in $(seq 1 30); do
  win=$(url_of "$TAB")
  grep -q '?window&run=' <<<"$win" && break
  sleep 1
done
grep -q '?window&run=' <<<"$win" || fail "the start page did not go to a window ($(cdp list | tr '\n' ' '); $(tsh 'grep -E "launch of|page /app" $TMPDIR/lindroids-$(id -u)/server.log | tail -5' | tr '\n' ' '))"
R[start_page]="went to ${win#*/app/}"
run=$(sed -n 's/.*run=\([0-9]*\).*/\1/p' <<<"$win")
for _ in $(seq 1 20); do
  tsh 'lindroids status' >"$OUT/status.txt"
  grep -q "$run" "$OUT/status.txt" && grep -qE '^display +linked' "$OUT/status.txt" && break
  sleep 1
done
R[server_log]=$(tsh 'grep -E "launch of|page /app" $TMPDIR/lindroids-$(id -u)/server.log' | sed 's/^[0-9.]* //' | tr '\n' '|')
grep -q 'its page goes to its window' <<<"${R[server_log]}" || fail "the server did not send the page to the window: ${R[server_log]}"
sleep 3
R[window_title]=$(cdp eval "$TAB" "document.title")
shot window
log "start page: ${R[start_page]}"

# --------------------------------------------------------------- Termux closed: the offline page
tsh 'lindroids stop' >/dev/null
sleep 2
cdp eval "$TAB" "location.href = '/app/$ID/'; 1" >/dev/null
sleep 4
R[offline_page]=$(cdp eval "$TAB" "document.body.innerText" | tr -d '"' | sed 's/\\n/ /g' | cut -c1-160)
grep -q 'not running now' <<<"${R[offline_page]}" || fail "offline page: ${R[offline_page]}"
shot offline
cdp eval "$TAB" "document.getElementById('start').click(); 1" >/dev/null
R[share_sheet]="no Termux in it"
for _ in $(seq 1 10); do
  if xy=$(ui_find '^Termux$'); then R[share_sheet]="lists Termux"; break; fi
  sleep 1
done
shot share-sheet
# Termux chosen: its share receiver runs `lindroids launch`, which starts the server and the
# program, and opens its window: in the app on a phone (`view`); here, where Cromite made no app,
# a Custom Tab, which DevTools may not list yet (the page here stays behind it: a page out of
# sight links only once shown).
if [[ ${R[share_sheet]} == "lists Termux" ]]; then
  sh_ "input tap $xy"
  R[from_termux]="no window"
  for _ in $(seq 1 45); do
    pid=$(tsh "pgrep -x $CMD" | head -1)
    if [[ -n $pid ]] && tsh 'lindroids status' | grep -qE '^display +linked'; then
      R[from_termux]="$CMD runs (pid $pid), its window linked"
      break
    fi
    sleep 1
  done
  shot from-termux
else
  sh_ 'input keyevent KEYCODE_BACK'
fi
log "offline: ${R[offline_page]}; share sheet ${R[share_sheet]}; from Termux: ${R[from_termux]:-}"

# --------------------------------------------------------------- clean up
tsh "lindroids remove $ID; lindroids stop" >"$OUT/remove.txt"
adb_ forward --remove tcp:"$CDP_PORT" >/dev/null 2>&1
if [[ ${R[share_sheet]} != "lists Termux" ]]; then R[result]="FAIL: the share sheet has no Termux"
elif [[ ${R[from_termux]} == "no window" ]]; then R[result]="FAIL: Termux did not bring the program's window"
else R[result]=PASS; fi
finish
