#!/usr/bin/env bash
# Deploy Lindroids to a provisioned lab VM and test it end to end, the way a user has it: the CLI
# in Termux and nothing else — no app, no Termux setting (allow-external-apps is removed), the
# browser that is on the device as the display:
#   push the CLI, `lindroids run foot` (which opens the page in the browser), check the page linked
#   and shows the window, type into it, stop+start the server and check the page relinks, then
#   export foot, add its shortcut, stop Termux and tap the shortcut. Also: whose icon and name the
#   window has in Recents, and that the program ending closes its window (Chromium; Firefox cannot).
#
#   [LAB_BROWSER=<package>] lab/test-lindroids.sh <name>
#
# LAB_BROWSER makes that browser the default first (Cromite: its "Allow custom tab intents" must be
# on, which the lab's is).
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
OUT=$REPO/lab/results/$NAME/$MODE${LAB_BROWSER:+-$LAB_BROWSER}
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
      default_browser window window_size recents typing close_ends_program second_run end_closes_window shortcut avc_denied result notes; do
      printf '%-24s %s\n' "$k" "${R[$k]:-}"
    done
  } | tee "$OUT/result.txt"
  summary_row
}

summary_row() {
  # One row per VM, SELinux mode and browser, in the last section (re-running replaces the row).
  local f=$REPO/lab/results/SUMMARY.md browser="${R[browser]:-} (default ${R[default_browser]:-none})"
  local key="| $NAME | ${R[selinux]} | ${R[android]} | $browser |"
  grep -q '^## Recents and closing both ways' "$f" 2>/dev/null || cat >>"$f" <<'EOF'

## Recents and closing both ways (from 2026-10-28)

| VM | SELinux | Android | Browser | Linked | Window | Recents | Typing | Window closed → program ends | Program ends → window closes | Shortcut | AVC denials (browser/Termux) | Result | Date |
|----|---------|---------|---------|--------|--------|---------|--------|------------------------------|------------------------------|----------|------------------------------|--------|------|
EOF
  grep -vF "$key" "$f" >"$f.tmp" || true
  printf '| %s | %s | %s | %s | %s | %s | %s | %s | %s | %s | %s | %s | %s | %s |\n' "$NAME" "${R[selinux]}" "${R[android]}" \
    "$browser" "${R[linked]:-}" "${R[window]:-}" "${R[recents]:-}" "${R[typing]:-}" "${R[close_ends_program]:-}" \
    "${R[end_closes_window]:-}" "${R[shortcut]:-}" "${R[avc_denied]:-}" "${R[result]:-}" "$(date +%F)" >>"$f.tmp"
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
tsh 'lindroids stop >/dev/null 2>&1; pkill -x foot; rm -rf ~/.local/share/lindroids/displays; rm -f $TMPDIR/lindroids-$(id -u)/window-browser $TMPDIR/lindroids-$(id -u)/windows; sed -i "/allow-external-apps/d" ~/.termux/termux.properties 2>/dev/null; termux-reload-settings 2>/dev/null; true'
sh_ "pm uninstall $OLD_APP" >/dev/null 2>&1 || true
# The lab images' own crashes (Media Storage on a nearly full disk) put dialogs over the screen
# that swallow the test's taps: no crash dialogs in the lab.
sh_ 'settings put global hide_error_dialogs 1' >/dev/null 2>&1 || true
# The browser for the windows is the CLI's choice (crates/lindroids-cli/src/shortcut.rs,
# WINDOW_BROWSERS: the default browser unless it is Firefox, Chromium ones, Firefox last; one that
# opens an ordinary tab is passed over), read back once the window is open. LAB_BROWSER makes a
# browser the default first.
[[ -n ${LAB_BROWSER:-} ]] && sh_ "cmd role add-role-holder android.app.role.BROWSER $LAB_BROWSER 0" >/dev/null
R[default_browser]=$(sh_ 'dumpsys role' | grep -A1 'name=android.app.role.BROWSER' | sed -n 's/.*holders=//p' | head -1)
pkgs=$(sh_ 'pm list packages' | tr -d '\r')
for b in com.android.chrome com.microsoft.emmx com.brave.browser com.vivaldi.browser com.sec.android.app.sbrowser com.chrome.beta org.chromium.chrome org.cromite.cromite org.mozilla.firefox org.mozilla.firefox_beta org.mozilla.fenix; do
  grep -qx "package:$b" <<<"$pkgs" && sh_ "am force-stop $b" >/dev/null 2>&1
done
BROWSER=""
log "default browser ${R[default_browser]:-none}"
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
# This test is the browser Custom Tab path: the default is now the window app.
tsh 'lindroids config browser default' >/dev/null 2>&1 || true

# --------------------------------------------------------------- server + program + page
# Termux in front, as when the user types the command.
sh_ "am start -n com.termux/.app.TermuxActivity" >/dev/null 2>&1
sleep 2
tsh 'cd ~; rm -f .lab-typed run.log; (nohup sh -c "lindroids run foot; echo RUN-EXIT=\$?" >run.log 2>&1 </dev/null &) ; sleep 12; cat run.log' >"$OUT/start.txt" 2>&1
tsh 'lindroids doctor' >"$OUT/doctor.txt" 2>&1
linked() { grep -qE '^display +linked' <<<"$(tsh 'lindroids status' 2>/dev/null)"; }
for i in $(seq 1 20); do linked && break; sleep 0.5; done
linked && R[linked]=yes || { R[linked]=no; shot not-linked; fail "the page did not link ($(tr '\n' ' ' <"$OUT/start.txt"))"; }
# Settle: a browser passed over for an ordinary tab links first; the window's page (a cold
# browser can take 15 s) then replaces it. Wait until no page has linked for 5 s.
links() { tsh 'grep -c "app linked" $TMPDIR/lindroids-$(id -u)/server.log' | tr -d '\r '; }
n=$(links); same=0
for i in $(seq 1 40); do
  sleep 1
  m=$(links)
  if [[ $m == "$n" ]]; then same=$((same + 1)); else same=0; n=$m; fi
  [[ $same -ge 5 ]] && break
done
R[opened_by]=$(tsh 'grep -oE "browser at [^ ]+ [^ ]+" $TMPDIR/lindroids-$(id -u)/server.log | tail -1' | tr -d '\r')
BROWSER=$(tsh 'cat $TMPDIR/lindroids-$(id -u)/window-browser 2>/dev/null' | tr -d '\r ')
R[browser]=${BROWSER:-none}
# Chromium's Custom Tabs take the page's title and icon to Recents and close when the page asks.
[[ $BROWSER == org.mozilla.* ]] && CHROMIUM=no || CHROMIUM=yes
log "linked, window in ${R[browser]}"
# A browser passed over for opening an ordinary tab pairs too: the second run must add none.
DISPLAYS=$(tsh 'ls ~/.local/share/lindroids/displays | wc -l' | tr -d '\r ')

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

# --------------------------------------------------------------- Recents: the program's name and icon
# The window's task as Recents draws it: the TaskDescription its activity set (Chromium: the page's
# title and favicon; Firefox sets none, so Android shows Firefox's own).
recents() {
  sh_ 'dumpsys activity activities' >"$OUT/activities.txt"
  awk '/window&run=/{f=1} f&&/taskDescription:/{print; exit}' "$OUT/activities.txt"
}
desc=$(recents)
label=$(sed -n 's/.*taskDescription: label="\([^"]*\)".*/\1/p' <<<"$desc")
[[ $label == null ]] && label=""
iconfile=$(sed -n 's/.*iconFilename=\([^ ]*\).*/\1/p' <<<"$desc")
[[ $iconfile == null ]] && iconfile=""
if [[ -n $iconfile ]] && adb_ exec-out cat "$iconfile" >"$OUT/recents-icon.png" 2>/dev/null && [[ $(head -c 4 "$OUT/recents-icon.png" | tail -c 3) == PNG ]]; then
  R[recents]="program's: \"$label\", icon $(python3 -c 'import struct,sys; d=open(sys.argv[1],"rb").read(24); print("%dx%d" % struct.unpack(">II", d[16:24]))' "$OUT/recents-icon.png" 2>/dev/null) (recents-icon.png)"
else
  rm -f "$OUT/recents-icon.png"
  R[recents]="browser's icon (label \"${label:-none}\")"
fi
log "recents: ${R[recents]}"

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
# Back leaves full screen, then the window's ✕ (Firefox: "Return to previous app"; Chromium:
# close_button) closes the Custom Tab; the page says it is closing, and the relay ends the run 5 s
# later. (A second Back would only send a Chromium window to the background, alive.)
# Full screen settles first: a Back while it comes in is lost. A Back may only take the keyboard
# away; press it until the bar (and its ✕) shows, never once more: in Chromium that would send the
# window to the background.
sleep 2
xy=""
for i in 1 2 3; do
  sh_ 'input keyevent 4'
  sleep 1.5
  xy=$(ui_find '(?i)^(close tab|return to previous app)$|:id/close_button$') && break
done
if [[ -n $xy ]]; then sh_ "input tap $xy"; else R[notes]="${R[notes]:-} no-close-button"; fi
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
if grep -qE '^window .* [1-9][0-9]*x[1-9]' "$OUT/status-second-run.txt" && [[ $(tsh 'ls ~/.local/share/lindroids/displays | wc -l' | tr -d '\r ') == "$DISPLAYS" ]]; then
  R[second_run]="yes (same display)"
else
  R[second_run]=no
fi
log "second run: ${R[second_run]}"

# --------------------------------------------------------------- the program ending closes its window
# The window's URL names the run by the pid `lindroids run` started: foot's own.
run=$(tsh 'pgrep -xn foot' | tr -d '\r ')
sh_ 'dumpsys activity recents' >"$OUT/recents-before-end.txt"
if [[ -z $run ]] || ! grep -q "window&run=$run\b" "$OUT/recents-before-end.txt"; then
  R[end_closes_window]="no window for run ${run:-?} before it ended"
else
  tsh 'pkill -x foot; true'
  R[end_closes_window]="no (window still in Recents after 10 s)"
  for i in $(seq 1 10); do
    sleep 1
    sh_ 'dumpsys activity recents' >"$OUT/recents-after-end.txt"
    grep -q 'Recent #0' "$OUT/recents-after-end.txt" || continue
    grep -q "window&run=$run\b" "$OUT/recents-after-end.txt" || { R[end_closes_window]="yes ${i}s"; break; }
  done
fi
shot program-ended
[[ $CHROMIUM == no && ${R[end_closes_window]} == no* ]] && R[end_closes_window]="no: Firefox ignores window.close(), the ended screen asks for ✕"
log "program ending closes the window: ${R[end_closes_window]}"

# --------------------------------------------------------------- home screen shortcut
# `lindroids export` pins a shortcut owned by Termux; tapping it with Termux stopped must bring
# Termux, the server, the program and the browser back.
tsh 'lindroids remove lab.foot >/dev/null 2>&1; lindroids export --id lab.foot --name LabFoot foot 2>&1' >"$OUT/export.txt"
xy=""
for i in $(seq 1 10); do xy=$(ui_find '(?i)^add( to home screen)?$') && break; sleep 1; done
shot pin-dialog
# Pinned before (`lindroids remove` can only disable a pinned shortcut): exporting again updates
# and enables it in place, without the dialog.
if [[ -z $xy ]] && ! grep -q 'is updated' "$OUT/export.txt"; then
  R[shortcut]="no pin dialog ($(tr '\n' ' ' <"$OUT/export.txt"))"
else
  [[ -n $xy ]] && sh_ "input tap $xy"
  sleep 2
  sh_ 'am force-stop com.termux'
  sh_ 'input keyevent KEYCODE_HOME'
  sleep 2
  shot home-shortcut
  # Exported icons go on the launcher's next page when the first is full.
  xy=$(ui_find '^LabFoot$') || { read -r W H < <(sh_ wm size | grep -oE '[0-9]+x[0-9]+' | tail -1 | tr x ' '); sh_ "input swipe $((W * 4 / 5)) $((H / 2)) $((W / 5)) $((H / 2)) 300"; sleep 1; xy=$(ui_find '^LabFoot$'); } || xy=""
  shot home-shortcut-found
  if [[ -n $xy ]]; then
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

# Chromium must also show the program in Recents and close its window when it ends.
chromium_ok=yes
[[ $CHROMIUM == yes && ( ${R[recents]} != program* || ${R[end_closes_window]} != yes* ) ]] && chromium_ok=no
if [[ ${R[linked]} == yes && ${R[window]} == yes && ${R[typing]} == yes && ${R[close_ends_program]} == yes* && ${R[second_run]} == yes* && ${R[shortcut]} == yes* && $chromium_ok == yes ]]; then
  R[result]=PASS
else
  R[result]=FAIL
fi
finish
[[ ${R[result]} == PASS ]]
