#!/usr/bin/env bash
# Deploy Lindroids to a provisioned lab VM and test it end to end, the way a user has it: the CLI
# in Termux and nothing else — no app, no Termux setting (allow-external-apps is removed), the
# browser that is on the device as the display:
#   push the CLI, `lindroids run foot` (which opens the page in the browser), check the page linked
#   and shows the window, type into it, stop+start the server and check the page relinks, then
#   export foot, add its shortcut, stop Termux and tap the shortcut.
#
#   lab/test-lindroids.sh <name>
#
# Results: lab/results/<name>/<selinux mode>/ (screenshots, logcat, avc denials, result.txt), and a
# row in lab/results/SUMMARY.md. Uses the x86_64 CLI that is there now.
set -uo pipefail

LAB_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$LAB_DIR/.." && pwd)
VM=$LAB_DIR/vm.sh
CLI=${CLI:-$REPO/target/x86_64-linux-android/release/lindroids}
OLD_APP=io.github.dixonsolutions.lindroids
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
  # logcat-1 = deploy + pairing (saved before the closing test clears the buffer), then the rest.
  { cat "$OUT/logcat-1.txt" 2>/dev/null; adb_ logcat -d -v threadtime 2>/dev/null; } >"$OUT/logcat.txt"
  rm -f "$OUT/logcat-1.txt"
  # Denials whose subject is an app domain (Termux and the browser are both untrusted_app*;
  # Android 9 logs no app= tag), or that name Termux or the browser.
  grep -E 'avc: +denied' "$OUT/logcat.txt" | grep -E "scontext=u:r:untrusted_app|app=(${BROWSER:-none}|com\.termux)" >"$OUT/avc-denied.txt" || true
  # One line per distinct denial, for the report.
  grep -oE 'avc: +denied \{[^}]*\} for .*tclass=[a-z_]+ permissive=[01]' "$OUT/avc-denied.txt" |
    sed -E 's/ (ino|pid)=[0-9]+//g; s/pipe:\[[0-9]+\]/pipe:[N]/' | sort | uniq -c >"$OUT/avc-denied-unique.txt" || true
  grep -E 'avc: +granted' "$OUT/logcat.txt" | grep -E "app=(${BROWSER:-none}|com\.termux)" >"$OUT/avc-granted.txt" || true
  tsh 'cat $TMPDIR/lindroids-$(id -u)/server.log' >"$OUT/server.log" 2>/dev/null
  tsh 'cat ~/run.log' >"$OUT/run.log" 2>/dev/null
  R[avc_denied]=$(wc -l <"$OUT/avc-denied.txt")
  {
    for k in vm android image kernel selinux cmdline_selinux gpu cli browser cli_push opened_by linked \
      window window_size typing close_ends_program second_run shortcut avc_denied result notes; do
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

| VM | SELinux | Android | Image | Browser | Linked | Window | Typing | Closing ends it | AVC denials (browser/Termux) | Result | Date |
|----|---------|---------|-------|---------|--------|--------|--------|-----------|------------------------------|--------|------|
EOF
  fi
  grep -vF "$key" "$f" >"$f.tmp" || true
  printf '| %s | %s | %s | %s | %s | %s | %s | %s | %s | %s | %s | %s |\n' "$NAME" "${R[selinux]}" "${R[android]}" \
    "${R[image]}" "${R[browser]:-}" "${R[linked]:-}" "${R[window]:-}" "${R[typing]:-}" "${R[close_ends_program]:-}" \
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
# A clean slate: no Lindroids app (there is none any more), no Termux setting that lets apps run
# commands in Termux, no display paired (the browser pairs again with the ticket in its URL).
tsh 'lindroids stop >/dev/null 2>&1; pkill -x foot; rm -rf ~/.local/share/lindroids/displays; sed -i "/allow-external-apps/d" ~/.termux/termux.properties 2>/dev/null; termux-reload-settings 2>/dev/null; true'
sh_ "pm uninstall $OLD_APP" >/dev/null 2>&1 || true
# The lab images' own crashes (Media Storage on a nearly full disk) put dialogs over the screen
# that swallow the test's taps: no crash dialogs in the lab.
sh_ 'settings put global hide_error_dialogs 1' >/dev/null 2>&1 || true
# The browser whose Custom Tabs are the windows: the first of the CLI's list that is installed
# (crates/lindroids-cli/src/shortcut.rs, WINDOW_BROWSERS), not the default browser.
BROWSER=""
pkgs=$(sh_ 'pm list packages' | tr -d '\r')
for b in org.mozilla.firefox com.android.chrome com.microsoft.emmx com.sec.android.app.sbrowser com.brave.browser com.vivaldi.browser org.mozilla.firefox_beta org.mozilla.fenix com.chrome.beta org.chromium.chrome; do
  grep -qx "package:$b" <<<"$pkgs" && { BROWSER=$b; break; }
done
[[ -n $BROWSER ]] || fail "no browser with Custom Tabs on the device"
R[browser]=$BROWSER
sh_ "am force-stop $BROWSER" >/dev/null 2>&1 || true
log "browser $BROWSER"
# Termux's `am` (app_process) is killed by seccomp where an ARM translation layer is loaded, as on
# these x86 images; phones have none, and Termux's am socket server is not up in every Termux. Turn
# the bridge off for the test, as a phone would have it (needs root and Magisk's resetprop).
if [[ $(sh_ getprop ro.dalvik.vm.native.bridge) != 0 && -f ${LAB_HOME:-$HOME/VMs/lindroids-lab}/tools/magisk ]]; then
  adb_ push "${LAB_HOME:-$HOME/VMs/lindroids-lab}/tools/magisk" /data/local/tmp/magisk >/dev/null 2>&1
  sh_ 'cd /data/local/tmp && chmod 755 magisk && ln -sf magisk resetprop && ./resetprop ro.dalvik.vm.native.bridge 0'
  R[notes]="${R[notes]:-} native-bridge-off"
fi
"$VM" "$NAME" scp "$CLI" "$TERMUX_PREFIX/bin/lindroids.new" &&
  tsh 'mv $PREFIX/bin/lindroids.new $PREFIX/bin/lindroids && chmod 755 $PREFIX/bin/lindroids && lindroids version 2>&1 | grep -v "^WARNING: linker"' >"$OUT/cli-version.txt" 2>&1 &&
  R[cli_push]="ok ($(cat "$OUT/cli-version.txt"))" || fail "CLI push: $(cat "$OUT/cli-version.txt")"
log "CLI: $(cat "$OUT/cli-version.txt")"

# --------------------------------------------------------------- server + program + page
# Termux in front, as when the user types the command.
sh_ "am start -n com.termux/.app.TermuxActivity" >/dev/null 2>&1
sleep 2
tsh 'cd ~; rm -f .lab-typed run.log; (nohup sh -c "lindroids run foot; echo RUN-EXIT=\$?" >run.log 2>&1 </dev/null &) ; sleep 12; cat run.log' >"$OUT/start.txt" 2>&1
tsh 'lindroids doctor' >"$OUT/doctor.txt" 2>&1
linked() { tsh 'lindroids status' 2>/dev/null | grep -qE '^display +linked'; }
for i in $(seq 1 20); do linked && break; sleep 0.5; done
linked && R[linked]=yes || { R[linked]=no; shot not-linked; fail "the page did not link ($(tr '\n' ' ' <"$OUT/start.txt"))"; }
R[opened_by]=$(tsh 'grep -oE "browser at [^ ]+ [^ ]+" $TMPDIR/lindroids-$(id -u)/server.log | tail -1' | tr -d '\r')
log "linked"

# --------------------------------------------------------------- window
win=""
for i in $(seq 1 30); do
  win=$(tsh 'lindroids status' | grep -E '^window ' | head -1)
  [[ -n $win && $win != *" 0x0"* ]] && break
  sleep 0.5
done
tsh 'lindroids status' >"$OUT/status.txt" 2>&1
R[window_size]=$(grep -oE '[0-9]+x[0-9]+$' <<<"$win")
top=$(sh_ 'dumpsys activity activities' | grep -E 'mResumedActivity|topResumedActivity|ResumedActivity:' | head -1)
sleep 1
shot window
if [[ $top == *"$BROWSER"* && -n ${R[window_size]} && ${R[window_size]} != 0x0 ]]; then R[window]=yes; else
  R[window]=no
  R[notes]="${R[notes]:-} top=[$(xargs <<<"$top")] status-window=[$win]"
fi
log "window: ${R[window]} (${R[window_size]:-?})"

# --------------------------------------------------------------- typing
if [[ ${R[window]} == yes ]]; then
  # Focus the page (the middle of the screen is the window), then type.
  read -r W H < <(sh_ wm size | grep -oE '[0-9]+x[0-9]+' | tail -1 | tr x ' ')
  sh_ "input tap $((W / 2)) $((H / 2))"
  sleep 0.5
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

# --------------------------------------------------------------- closing the window ends the program
adb_ logcat -d -v threadtime >"$OUT/logcat-1.txt" 2>/dev/null
adb_ logcat -c
# Back leaves full screen, then closes the Custom Tab; the page says it is closing, and the relay
# ends the run 5 s later.
sh_ 'input keyevent 4'; sleep 1; sh_ 'input keyevent 4'
R[close_ends_program]="no (foot still running after 15 s)"
for i in $(seq 1 15); do
  sleep 1
  tsh 'pgrep -x foot >/dev/null' || { R[close_ends_program]="yes ${i}s"; break; }
done
tsh 'grep RUN-EXIT ~/run.log; grep -E "page for run|ending" $TMPDIR/lindroids-$(id -u)/server.log | tail -2' >"$OUT/close.txt" 2>&1
log "closing the window: ${R[close_ends_program]}"

# --------------------------------------------------------------- a second run: no pairing again
sh_ "am start -n com.termux/.app.TermuxActivity" >/dev/null 2>&1
sleep 2
tsh 'cd ~; (nohup lindroids run foot >run2.log 2>&1 </dev/null &); sleep 12; lindroids status; lindroids displays' >"$OUT/status-second-run.txt" 2>&1
shot window-second-run
if grep -qE '^window .* [1-9][0-9]*x[1-9]' "$OUT/status-second-run.txt" && [[ $(tsh 'ls ~/.local/share/lindroids/displays | wc -l' | tr -d '\r ') == 1 ]]; then
  R[second_run]="yes (same display)"
else
  R[second_run]=no
fi
log "second run: ${R[second_run]}"
tsh 'pkill -x foot; true'

# --------------------------------------------------------------- home screen shortcut
# `lindroids export` pins a shortcut owned by Termux; tapping it with Termux stopped must bring
# Termux, the server, the program and the browser back.
tsh 'lindroids remove lab.foot >/dev/null 2>&1; lindroids export --id lab.foot --name LabFoot foot 2>&1' >"$OUT/export.txt"
xy=""
for i in $(seq 1 10); do xy=$(ui_find '(?i)^add( to home screen)?$') && break; sleep 1; done
shot pin-dialog
if [[ -z $xy ]]; then
  R[shortcut]="no pin dialog ($(tr '\n' ' ' <"$OUT/export.txt"))"
else
  sh_ "input tap $xy"
  sleep 2
  sh_ 'am force-stop com.termux'
  sh_ 'input keyevent KEYCODE_HOME'
  sleep 2
  shot home-shortcut
  if xy=$(ui_find '^LabFoot$'); then
    t0=$(date +%s)
    sh_ "input tap $xy"
    R[shortcut]="no (no window within 60 s)"
    for i in $(seq 1 60); do
      top=$(sh_ 'dumpsys activity activities' | grep -E 'topResumedActivity|mResumedActivity' | head -1)
      foot=$(sh_ 'ps -A -o NAME' | grep -cx foot)
      echo "$i top=[$(xargs <<<"$top")] foot=$foot" >>"$OUT/shortcut-poll.txt"
      if [[ $top == *"$BROWSER"* && $foot -gt 0 ]]; then R[shortcut]="yes $(( $(date +%s) - t0 ))s"; break; fi
      sleep 1
    done
    sleep 2
    shot shortcut-window
  else
    R[shortcut]="no (not on the home screen)"
  fi
  # ssh again: a new Termux session (Ctrl+Alt+C) running sshd.
  sh_ 'am start -n com.termux/.app.TermuxActivity' >/dev/null 2>&1
  sleep 2
  sh_ 'input keycombination KEYCODE_CTRL_LEFT KEYCODE_ALT_LEFT KEYCODE_C'
  sleep 2
  sh_ 'input text sshd'
  sh_ 'input keyevent 66'
  sleep 3
fi
log "shortcut: ${R[shortcut]}"
tsh 'pkill -x foot; lindroids remove lab.foot >/dev/null 2>&1; true'

if [[ ${R[linked]} == yes && ${R[window]} == yes && ${R[typing]} == yes && ${R[close_ends_program]} == yes* && ${R[second_run]} == yes* && ${R[shortcut]} == yes* ]]; then
  R[result]=PASS
else
  R[result]=FAIL
fi
finish
[[ ${R[result]} == PASS ]]
