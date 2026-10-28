#!/usr/bin/env bash
# Is a program's window full screen the moment it opens, and what does it take to get there?
# For each browser: `lindroids run <app>` with that browser, then, before anything touches the
# screen, the page's own report ("opened in/out of full screen", from its trace in server.log) and
# a screenshot; then one tap in the middle and which input the browser granted full screen on.
#
#   lab/test-fullscreen.sh <vm> [flatpak-app-id]   (default org.gnome.Calculator)
#   LAB_BROWSERS="org.cromite.cromite org.mozilla.firefox"   (default: both)
#   LAB_BIN=target/x86_64-linux-android/release/lindroids     test this build (copied to the VM as
#                                                            ~/.lab-fullscreen/lindroids; the VM's
#                                                            own lindroids is left as it is). Needs
#                                                            LAB_RESTART=1: its page is in its server
#   LAB_RESTART=1   stop the lindroids server first (one per user: on a shared VM, someone else's)
#
# By default the test leaves the server alone (the VM is shared): it uses the one running, or the
# VM's lindroids starts one, and ends only its own program, with the window's ✕. Pages from before
# 2026-10-28 do not say "opened … full screen"; their "full screen" trace (fullscreenchange) is
# used instead. The default browser is not changed (LINDROIDS_BROWSER picks the browser). No
# uiautomator before the measurement: accessibility services change what a browser does.
#
# Results: lab/results/<vm>/fullscreen/<browser>/ (opened.png, tapped.png, trace.txt) and
# lab/results/<vm>/fullscreen/result.txt.
set -uo pipefail

LAB_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$LAB_DIR/.." && pwd)
VM=$LAB_DIR/vm.sh
NAME=${1:?usage: test-fullscreen.sh <vm> [app-id]}
APP=${2:-org.gnome.Calculator}
PROC=${LAB_PROC:-"[/]app/bin/"}
BROWSERS=${LAB_BROWSERS:-org.cromite.cromite org.mozilla.firefox}
sh_() { "$VM" "$NAME" adb shell "$@" | tr -d '\r'; }
tsh() { timeout "${TSH_TIMEOUT:-60}" "$VM" "$NAME" ssh "$@"; }
log() { echo "[$(date +%T)] $NAME: $*"; }
LOGF='$TMPDIR/lindroids-$(id -u)/server.log'
OUT=$REPO/lab/results/$NAME/fullscreen
mkdir -p "$OUT"

"$VM" "$NAME" wait-boot 60 >/dev/null || exit 1
LIND=lindroids
[[ -n ${LAB_BIN:-} && -z ${LAB_RESTART:-} ]] && { log "LAB_BIN needs LAB_RESTART=1 (the page comes from the server)"; exit 2; }
if [[ -n ${LAB_BIN:-} ]]; then
  tsh 'mkdir -p ~/.lab-fullscreen'
  "$VM" "$NAME" scp "$LAB_BIN" .lab-fullscreen/lindroids >/dev/null || { log "cannot copy $LAB_BIN"; exit 1; }
  tsh 'chmod 755 ~/.lab-fullscreen/lindroids'
  LIND='~/.lab-fullscreen/lindroids'
fi
stop_all() {
  [[ -n ${LAB_RESTART:-} ]] || return 0
  tsh "$LIND stop >/dev/null 2>&1; pkill -f '[l]indroids server'; sleep 2; true"
}
# Lines of the server's log from <n> on.
since() { tsh "tail -n +$(( $1 + 1 )) $LOGF 2>/dev/null" | tr -d '\r'; }
lines() { tsh "wc -l < $LOGF 2>/dev/null || echo 0" | tr -d '\r '; }

# The ✕ of the window in front, found with uiautomator (after the measurements).
close_window() {
  local xy
  for i in 1 2 3; do
    sh_ 'input keyevent KEYCODE_BACK'
    sleep 1.5
    xy=$(sh_ 'uiautomator dump /data/local/tmp/ui.xml >/dev/null 2>&1; cat /data/local/tmp/ui.xml' | python3 -c '
import re, sys
for n in re.findall(r"<node [^>]*>", sys.stdin.read()):
    if re.search(r"(content-desc|text)=\"(Close tab|Return to previous app)\"|:id/close_button\"", n):
        b = list(map(int, re.findall(r"\d+", re.search(r"bounds=\"([^\"]+)\"", n).group(1))))
        print((b[0] + b[2]) // 2, (b[1] + b[3]) // 2)
        break') && [[ -n $xy ]] && break
  done
  [[ -n $xy ]] && sh_ "input tap $xy"
}

declare -A R
for B in $BROWSERS; do
  D=$OUT/$B
  rm -rf "$D"
  mkdir -p "$D"
  stop_all
  sh_ 'input keyevent KEYCODE_HOME'
  log "$B: lindroids run $APP"
  n0=$(lines)
  tsh "cd ~; (LINDROIDS_BROWSER=$B nohup $LIND run $APP > ~/fullscreen-run.log 2>&1 </dev/null &); true"
  # The page's report, a second after it loaded (nothing has touched the screen); for older pages,
  # their link and no full screen trace.
  opened=""
  for i in $(seq 1 40); do
    [[ $(lines) -lt $n0 ]] && n0=0   # a new server, a new log
    log_=$(since "$n0")
    opened=$(grep -o 'trace: opened [^,]*full screen.*' <<<"$log_" | tail -1)
    [[ -z $opened ]] && grep -q 'mapped: ' <<<"$log_" && [[ $i -gt 12 ]] &&
      opened=$(grep -q 'trace: full screen$' <<<"$log_" && echo "full screen (old page)" || echo "out of full screen (old page: no full screen trace)")
    [[ -n $opened ]] && break
    sleep 1
  done
  sleep 2
  "$VM" "$NAME" screenshot "$D/opened.png" >/dev/null 2>&1
  front=$(sh_ 'dumpsys activity activities' | grep -m1 topResumedActivity | grep -oE '[a-z0-9_.]+/[A-Za-z0-9_.]+' | head -1)
  if [[ -z $opened ]]; then
    R[$B]="no window report in 40 s (front: $front)"
    log "$B: ${R[$B]}"
    since "$n0" >"$D/server.log"
    stop_all
    continue
  fi
  case $front in org.cromite*|org.mozilla*|com.android.chrome*|org.chromium*) ;; *)
    R[$B]="${opened#trace: }, but $front is in front: no tap"; log "$B: ${R[$B]}"; stop_all; continue ;; esac
  # One tap in the middle of the window.
  n1=$(lines)
  sh_ 'input tap 640 420'
  asked=""
  for i in $(seq 1 10); do
    sleep 1
    log_=$(since "$n1")
    asked=$(grep -o 'trace: full screen asked on [a-z]*' <<<"$log_" | tail -1)
    [[ -z $asked ]] && grep -q 'trace: full screen$' <<<"$log_" && asked="full screen (old page)"
    [[ -n $asked ]] && break
  done
  sleep 1
  "$VM" "$NAME" screenshot "$D/tapped.png" >/dev/null 2>&1
  since "$n0" | grep -E 'page .*(trace|layout): ' >"$D/trace.txt"
  R[$B]="at open: ${opened#trace: } (front: $front); after one tap: ${asked:-not full screen}"
  log "$B: ${R[$B]}"
  close_window
  sleep 2
  stop_all
done
sh_ 'input keyevent KEYCODE_HOME'
[[ -n ${LAB_BIN:-} ]] && tsh 'rm -rf ~/.lab-fullscreen'
for B in $BROWSERS; do printf '%-22s %s\n' "$B" "${R[$B]:-}"; done | tee "$OUT/result.txt"
