#!/usr/bin/env bash
# Deploy Lindroids to a provisioned lab VM and test it end to end:
#   install the APK + push the CLI, grant permissions, pair (fresh: the app's data is cleared),
#   open foot, type into it, then stop+start the server and check the app reconnects unaided.
#
#   lab/test-lindroids.sh <name>
#
# Results: lab/results/<name>/<selinux mode>/ (screenshots, logcat, avc denials, result.txt), and a
# row in lab/results/SUMMARY.md. Uses whatever dist/lindroids.apk and the x86_64 CLI are there now.
set -uo pipefail

LAB_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$LAB_DIR/.." && pwd)
VM=$LAB_DIR/vm.sh
APK=${APK:-$REPO/dist/lindroids.apk}
CLI=${CLI:-$REPO/target/x86_64-linux-android/release/lindroids}
PKG=io.github.dixonsolutions.lindroids
TERMUX_PREFIX=/data/data/com.termux/files/usr

NAME=${1:?usage: test-lindroids.sh <name>}
log() { echo "[$(date +%T)] $NAME: $*"; }
adb_() { "$VM" "$NAME" adb "$@"; }
sh_() { "$VM" "$NAME" adb shell "$@" | tr -d '\r'; }
tsh() { timeout "${TSH_TIMEOUT:-60}" "$VM" "$NAME" ssh "$@"; }

"$VM" "$NAME" wait-boot 400 >/dev/null || exit 1
adb_ root >/dev/null 2>&1 || true
sleep 2
"$VM" "$NAME" wait-boot 60 >/dev/null || exit 1

REL=$(sh_ getprop ro.build.version.release)
SDK=$(sh_ getprop ro.build.version.sdk)
SEL=$(sh_ getenforce)
MODE=$(tr 'A-Z' 'a-z' <<<"${SEL:-unknown}")
OUT=$REPO/lab/results/$NAME/$MODE
rm -rf "$OUT"
mkdir -p "$OUT"
declare -A R
R[vm]=$NAME
R[android]="$REL (sdk $SDK)"
R[selinux]=$SEL
R[cmdline_selinux]=$(cat "${LAB_HOME:-$HOME/VMs/lindroids-lab}/run/$NAME/selinux-requested" 2>/dev/null)
R[image]=$(sh_ getprop ro.build.display.id)
R[kernel]=$(sh_ uname -r)
R[gpu]=$(cat "${LAB_HOME:-$HOME/VMs/lindroids-lab}/run/$NAME/gpu" 2>/dev/null)
R[apk]="$(basename "$APK") $(stat -c %y "$APK" | cut -d. -f1)"
R[cli]="$(basename "$CLI") $(stat -c %y "$CLI" | cut -d. -f1)"
log "Android $REL (sdk $SDK), SELinux $SEL -> $OUT"

shot() { "$VM" "$NAME" screenshot "$OUT/$1.png" >/dev/null 2>&1; }

# uiautomator: print "x y" (centre) of the first node whose text/resource-id/class matches a regex.
ui_find() {
  sh_ 'uiautomator dump /data/local/tmp/ui.xml >/dev/null 2>&1; cat /data/local/tmp/ui.xml' | python3 -c '
import re, sys, xml.etree.ElementTree as ET
pat = re.compile(sys.argv[1])  # (?i) for buttons: Android <= 9 shows them upper-case
try:
    root = ET.fromstring(sys.stdin.read())
except Exception:
    sys.exit(1)
for n in root.iter("node"):
    for k in ("text", "resource-id", "class", "content-desc"):
        if pat.search(n.get(k) or ""):
            x1, y1, x2, y2 = map(int, re.findall(r"\d+", n.get("bounds")))
            print((x1 + x2) // 2, (y1 + y2) // 2)
            sys.exit(0)
sys.exit(1)' "$1"
}

finish() {
  # logcat-1 = deploy + pairing (saved before the reconnect test clears the buffer), then the rest.
  { cat "$OUT/logcat-1.txt" 2>/dev/null; adb_ logcat -d -v threadtime 2>/dev/null; } >"$OUT/logcat.txt"
  rm -f "$OUT/logcat-1.txt"
  # Denials that involve our app or Termux (scontext of either, or tagged app=...).
  # Denials whose subject is an app domain (Termux and Lindroids are both untrusted_app*; Android 9
  # logs no app= tag and the socket path is hex), or that name our packages.
  grep -E 'avc: +denied' "$OUT/logcat.txt" | grep -E "scontext=u:r:untrusted_app|app=($PKG|com\.termux)" >"$OUT/avc-denied.txt" || true
  # One line per distinct denial, for the report.
  grep -oE 'avc: +denied \{[^}]*\} for .*tclass=[a-z_]+ permissive=[01]' "$OUT/avc-denied.txt" |
    sed -E 's/ (ino|pid)=[0-9]+//g; s/pipe:\[[0-9]+\]/pipe:[N]/' | sort | uniq -c >"$OUT/avc-denied-unique.txt" || true
  grep -E 'avc: +granted' "$OUT/logcat.txt" | grep -E "app=($PKG|com\.termux)" >"$OUT/avc-granted.txt" || true
  grep -E "linked over|link [0-9]+ to|failed to authenticate|$PKG|lindroids" "$OUT/logcat.txt" | grep -v 'avc:' >"$OUT/lindroids-logcat.txt" || true
  tsh 'cat $TMPDIR/lindroids-$(id -u)/server.log' >"$OUT/server.log" 2>/dev/null
  tsh 'cat ~/run.log' >"$OUT/run.log" 2>/dev/null
  R[avc_denied]=$(wc -l <"$OUT/avc-denied.txt")
  {
    for k in vm android image kernel selinux cmdline_selinux gpu apk cli install cli_push pairing_code paired transport \
      window window_size typing reconnect reconnect_transport window_after_reconnect avc_denied result notes; do
      printf '%-24s %s\n' "$k" "${R[$k]:-}"
    done
  } | tee "$OUT/result.txt"
  summary_row
}

summary_row() {
  local f=$REPO/lab/results/SUMMARY.md key="| $NAME | ${R[selinux]} |"
  if [[ ! -f $f ]]; then
    cat >"$f" <<'EOF'
# Lindroids lab results

Written by lab/test-lindroids.sh (one row per VM and SELinux mode; re-running replaces the row).

| VM | SELinux | Android | Image | Paired | Transport | Window | Typing | Reconnect | AVC denials (app/Termux) | Result | Date |
|----|---------|---------|-------|--------|-----------|--------|--------|-----------|--------------------------|--------|------|
EOF
  fi
  grep -vF "$key" "$f" >"$f.tmp" || true
  printf '| %s | %s | %s | %s | %s | %s | %s | %s | %s | %s | %s | %s |\n' "$NAME" "${R[selinux]}" "${R[android]}" \
    "${R[image]}" "${R[paired]:-}" "${R[transport]:-}" "${R[window]:-}" "${R[typing]:-}" "${R[reconnect]:-}" \
    "${R[avc_denied]:-}" "${R[result]:-}" "$(date +%F)" >>"$f.tmp"
  mv "$f.tmp" "$f"
}

fail() {
  R[result]=FAIL
  R[notes]="${R[notes]:-} $*"
  log "FAIL: $*"
  shot fail
  finish
  exit 1
}

# --------------------------------------------------------------- deploy
tsh true || fail "no ssh into Termux (run lab/provision.sh $NAME)"
TUID=$(tsh 'id -u' | tr -d '\r')
log "Termux uid $TUID"

adb_ logcat -c
# A clean slate: forget earlier pairings so every run pairs from scratch.
tsh 'lindroids stop >/dev/null 2>&1; pkill -x foot; true'
sh_ "am force-stop $PKG; pm clear $PKG" >/dev/null 2>&1 || true
if out=$(adb_ install -r -g "$APK" 2>&1 | tail -1) && [[ $out == *Success* ]]; then R[install]=ok; else
  # Older pm has no -g with -r on some images
  out=$(adb_ install -r "$APK" 2>&1 | tail -1)
  [[ $out == *Success* ]] && R[install]=ok || fail "adb install: $out"
fi
log "APK installed"
"$VM" "$NAME" scp "$CLI" "$TERMUX_PREFIX/bin/lindroids.new" &&
  tsh 'mv $PREFIX/bin/lindroids.new $PREFIX/bin/lindroids && chmod 755 $PREFIX/bin/lindroids && lindroids version 2>&1 | grep -v "^WARNING: linker"' >"$OUT/cli-version.txt" 2>&1 &&
  R[cli_push]="ok ($(cat "$OUT/cli-version.txt"))" || fail "CLI push: $(cat "$OUT/cli-version.txt")"
log "CLI: $(cat "$OUT/cli-version.txt")"

sh_ "appops set $PKG SYSTEM_ALERT_WINDOW allow"
((SDK >= 33)) && sh_ "pm grant $PKG android.permission.POST_NOTIFICATIONS"
sh_ "dumpsys deviceidle whitelist +$PKG" >/dev/null 2>&1 || true
sh_ "am start -n $PKG/.HomeActivity" >/dev/null
sleep 3
shot home

# --------------------------------------------------------------- server + program
tsh 'cd ~; rm -f .lab-typed run.log; lindroids start && (nohup lindroids run foot >run.log 2>&1 </dev/null &) ; sleep 4; cat run.log' >"$OUT/start.txt" 2>&1
tsh 'lindroids doctor' >"$OUT/doctor.txt" 2>&1
CODE=$(tsh 'cat $TMPDIR/lindroids-$(id -u)/pairing-code' 2>/dev/null | tr -dc 0-9)
R[pairing_code]=${CODE:-none}
[[ -n $CODE ]] || fail "no pairing code file (server not running?) $(tr '\n' ' ' <"$OUT/start.txt")"
log "pairing code $CODE"

# --------------------------------------------------------------- pairing
connected() { tsh 'lindroids status' 2>/dev/null | grep -qE '^app +connected'; }
pair_dialog() {
  local i
  for i in $(seq 1 10); do ui_find '(?i)^pair$' >/dev/null && return 0; sleep 1; done
  return 1
}
sleep 3
if ! connected; then
  # Start the (non-exported) pairing activity as root, as the notification would.
  sh_ "am start -n $PKG/.ApproveActivity --ei uid $TUID --es sandbox Termux --ez needCode true" >"$OUT/approve-start.txt" 2>&1
  if ! pair_dialog; then
    # No root: open it from the notification instead.
    sh_ 'cmd statusbar expand-notifications 2>/dev/null || service call statusbar 1' >/dev/null 2>&1
    sleep 2
    xy=$(ui_find 'wants to show Linux apps') && sh_ "input tap $xy"
    pair_dialog || fail "pairing dialog did not open ($(tr '\n' ' ' <"$OUT/approve-start.txt"))"
    R[notes]="${R[notes]:-} dialog-from-notification"
  fi
  shot pair-dialog
  xy=$(ui_find 'EditText') && sh_ "input tap $xy"
  sh_ "input text $CODE"
  shot pair-code
  xy=$(ui_find '(?i)^pair$') || fail "no Pair button"
  sh_ "input tap $xy"
  for i in $(seq 1 30); do connected && break; sleep 0.5; done
fi
connected && R[paired]=yes || { R[paired]=no; shot pair-failed; fail "app not connected after pairing"; }
log "paired"
R[transport]=$(adb_ logcat -d | grep -oE 'linked over [a-z]+' | head -1 | awk '{print $3}')

# --------------------------------------------------------------- window
win=""
for i in $(seq 1 30); do
  win=$(tsh 'lindroids status' | grep -E '^window ' | head -1)
  [[ -n $win && $win != *" 0x0"* ]] && break
  sleep 0.5
done
tsh 'lindroids status' >"$OUT/status.txt" 2>&1
R[window_size]=$(grep -oE '[0-9]+x[0-9]+$' <<<"$win")
top=""
for i in $(seq 1 20); do
  top=$(sh_ 'dumpsys activity activities' | grep -E 'mResumedActivity|topResumedActivity|ResumedActivity:' | head -1)
  [[ $top == *WindowActivity* ]] && break
  sleep 0.5
done
sleep 1
shot window
if [[ $top == *WindowActivity* && -n ${R[window_size]} && ${R[window_size]} != 0x0 ]]; then R[window]=yes; else
  R[window]=no
  R[notes]="${R[notes]:-} top=[$(xargs <<<"$top")] status-window=[$win]"
fi
log "window: ${R[window]} (${R[window_size]:-?})"

# --------------------------------------------------------------- typing
if [[ ${R[window]} == yes ]]; then
  # Double quotes for the host, single quotes for the device shell: | reaches `input text` intact.
  "$VM" "$NAME" adb shell "input text 'echo%slab-ok%s|%stee%s.lab-typed'"
  "$VM" "$NAME" adb shell input keyevent 66
  for i in $(seq 1 10); do [[ $(tsh 'cat ~/.lab-typed 2>/dev/null' | tr -d '\r') == lab-ok ]] && break; sleep 1; done
  shot typed
  [[ $(tsh 'cat ~/.lab-typed 2>/dev/null' | tr -d '\r') == lab-ok ]] && R[typing]=yes || R[typing]=no
else
  R[typing]=skipped
fi
log "typing: ${R[typing]}"

# --------------------------------------------------------------- reconnect without pairing
adb_ logcat -d -v threadtime >"$OUT/logcat-1.txt" 2>/dev/null
adb_ logcat -c
t=$(tsh 'lindroids stop >/dev/null; sleep 1; lindroids start >/dev/null; s=$(date +%s%N); for i in $(seq 1 100); do lindroids status | grep -qE "^app +connected" && { echo $(( ($(date +%s%N) - s) / 1000000 )); exit 0; }; sleep 0.1; done; echo none' | tr -d '\r')
if [[ $t =~ ^[0-9]+$ ]]; then
  R[reconnect]="yes ${t}ms"
else
  R[reconnect]="no (not within ~10 s)"
fi
R[reconnect_transport]=$(adb_ logcat -d | grep -oE 'linked over [a-z]+' | head -1 | awk '{print $3}')
log "reconnect: ${R[reconnect]} over ${R[reconnect_transport]:-?}"
tsh 'cd ~; (nohup lindroids run foot >run2.log 2>&1 </dev/null &); sleep 4; lindroids status' >"$OUT/status-after-reconnect.txt" 2>&1
sleep 1
shot window-after-reconnect
grep -qE '^window .* [1-9][0-9]*x[1-9]' "$OUT/status-after-reconnect.txt" && R[window_after_reconnect]=yes || R[window_after_reconnect]=no
tsh 'pkill -x foot; true'

if [[ ${R[paired]} == yes && ${R[window]} == yes && ${R[typing]} == yes && ${R[reconnect]} == yes* ]]; then
  R[result]=PASS
else
  R[result]=FAIL
fi
finish
[[ ${R[result]} == PASS ]]
