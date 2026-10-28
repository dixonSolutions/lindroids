#!/usr/bin/env bash
# A program's window of its own, launched both ways — `lindroids run` and its exported home screen
# icon (Termux closed) — with the browser Lindroids picks on this VM:
#   - Recents: the window's task label and icon (the TaskDescription its browser set), and a
#     screenshot of Recents;
#   - the window's ✕ ends the program;
#   - the program ending closes the window (Chromium; Firefox keeps it, with its end screen);
#   - (run) another app in front and Android killing the browser's page process, as a phone does to
#     a browser in the background: the program keeps running, and its Recents card brings the
#     window back, linked to it again.
#
#   lab/test-windows.sh <vm> [flatpak-app-id]     (default org.gnome.Calculator, installed in Termux)
#   LAB_PROC='[/]app/share/codium/' lab/test-windows.sh b16 com.vscodium.codium
#
# Results: lab/results/<vm>/windows-<browser>/ (screenshots, Recents icons, result.txt). Uses the
# lindroids that is on the VM now.
set -uo pipefail

LAB_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$LAB_DIR/.." && pwd)
VM=$LAB_DIR/vm.sh
NAME=${1:?usage: test-windows.sh <vm> [app-id]}
APP=${2:-org.gnome.Calculator}
# A Flatpak app's own process: /app/bin/<name>; LAB_PROC for apps elsewhere (VSCodium:
# '[/]app/share/codium/').
PROC=${LAB_PROC:-"[/]app/bin/"}
sh_() { "$VM" "$NAME" adb shell "$@" | tr -d '\r'; }
tsh() { timeout "${TSH_TIMEOUT:-60}" "$VM" "$NAME" ssh "$@"; }
log() { echo "[$(date +%T)] $NAME: $*"; }
declare -A R

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

running() { tsh "pgrep -f '$PROC' >/dev/null"; }
window_tasks() { sh_ 'dumpsys activity recents' | grep -c 'window&run='; }

# The newest window's run (its pid) and the browser it went to, from `lindroids run`'s notes.
last_window() { tsh 'tail -1 $TMPDIR/lindroids-$(id -u)/windows' | tr -d '\r'; }

# Recents: the window's TaskDescription (label, icon file), the icon pulled, and Recents on screen.
check_recents() { # <tag>
  local tag=$1 desc label iconfile
  # The browser sets the icon once it has the page's favicon: give it up to 10 s.
  for i in $(seq 1 20); do
    sh_ 'dumpsys activity activities' >"$OUT/$tag-activities.txt"
    desc=$(awk '/window&run=/{f=1} f&&/taskDescription:/{print; exit}' "$OUT/$tag-activities.txt")
    grep -q 'iconFilename=/' <<<"$desc" && break
    sleep 0.5
  done
  label=$(sed -n 's/.*taskDescription: label="\([^"]*\)".*/\1/p' <<<"$desc")
  iconfile=$(sed -n 's/.*iconFilename=\([^ ]*\).*/\1/p' <<<"$desc")
  [[ $label == null ]] && label=""
  [[ $iconfile == null ]] && iconfile=""
  # Android writes the icon file a few seconds after the browser set it; Recents opened before
  # shows the browser's icon until then.
  if [[ -n $iconfile ]]; then
    for i in $(seq 1 20); do [[ $(sh_ "test -f $iconfile && echo yes") == yes ]] && break; sleep 0.5; done
  fi
  if [[ -n $iconfile ]] && "$VM" "$NAME" adb exec-out cat "$iconfile" >"$OUT/$tag-recents-icon.png" 2>/dev/null &&
    [[ $(head -c 4 "$OUT/$tag-recents-icon.png" | tail -c 3) == PNG ]]; then
    R[$tag-recents]="program's icon and name \"$label\" ($tag-recents-icon.png)"
  else
    rm -f "$OUT/$tag-recents-icon.png"
    R[$tag-recents]="browser's icon (label \"${label:-none}\")"
  fi
  sh_ 'input keyevent KEYCODE_APP_SWITCH'
  sleep 2
  "$VM" "$NAME" screenshot "$OUT/$tag-recents.png" >/dev/null 2>&1
  # Back to the window: its card, in the middle (Back could go home).
  sh_ 'input tap 640 350'
  sleep 2
  log "$tag recents: ${R[$tag-recents]}"
}

# The window's ✕ ends the program (5 s after the page says it is closing).
check_close_window() { # <tag>
  local tag=$1 xy=""
  # Out of full screen (Back once or twice: the first may only take the keyboard), then the ✕.
  sh_ 'input tap 640 400'
  sleep 2
  for i in 1 2 3; do
    sh_ 'input keyevent KEYCODE_BACK'
    sleep 1.5
    xy=$(ui_find '(?i)^(close tab|return to previous app)$|:id/close_button$') && break
  done
  if [[ -z $xy ]]; then
    R[$tag-close-window]="no ✕ found ($tag-no-close.png)"
    "$VM" "$NAME" screenshot "$OUT/$tag-no-close.png" >/dev/null 2>&1
    return
  fi
  sh_ "input tap $xy"
  R[$tag-close-window]="no (program still running after 15 s)"
  for i in $(seq 1 15); do
    sleep 1
    running || { R[$tag-close-window]="yes ${i}s"; break; }
  done
  log "$tag ✕ ends the program: ${R[$tag-close-window]}"
}

# Another app in front, then the window's page process killed (what Android does to a browser in
# the background, for memory): the program keeps running, and the window's Recents card brings it
# back, linked to the program again.
check_background() { # <tag>
  local tag=$1 links
  sh_ 'am start -a android.settings.SETTINGS' >/dev/null
  sleep 5
  sh_ "for p in \$(ps -A -o PID,NAME | grep -E '$BROWSER:(sandboxed|tab)' | awk '{print \$1}'); do kill -9 \$p; done"
  sleep 15
  if ! running; then
    R[$tag-background]="no: the program ended when the page was killed"
    log "$tag in the background: ${R[$tag-background]}"
    return
  fi
  links=$(tsh 'grep -c "linked again" $TMPDIR/lindroids-$(id -u)/server.log' | tr -d '\r')
  # Recents: Settings in the middle, the window to its left.
  sh_ 'input keyevent KEYCODE_APP_SWITCH'
  sleep 2
  "$VM" "$NAME" screenshot "$OUT/$tag-background-recents.png" >/dev/null 2>&1
  sh_ 'input tap 90 190'
  R[$tag-background]="kept running; the window came back without linking ($tag-background-back.png)"
  for i in $(seq 1 15); do
    sleep 1
    [[ $(tsh 'grep -c "linked again" $TMPDIR/lindroids-$(id -u)/server.log' | tr -d '\r') -gt $links ]] && running &&
      { R[$tag-background]="kept running; back from Recents, linked again in ${i}s"; break; }
  done
  sleep 2
  "$VM" "$NAME" screenshot "$OUT/$tag-background-back.png" >/dev/null 2>&1
  log "$tag in the background: ${R[$tag-background]}"
}

# The program ending closes its window (its task leaves Recents).
check_program_ends() { # <tag> <run>
  local tag=$1 run=$2
  tsh "pkill -f '$PROC'; true"
  R[$tag-program-ends]="no (window still in Recents after 10 s)"
  local recents
  for i in $(seq 1 10); do
    sleep 1
    recents=$(sh_ 'dumpsys activity recents')
    grep -q 'Recent #0' <<<"$recents" || continue   # adb had nothing to say this time
    grep -qE "window&run=$run([^0-9]|$)" <<<"$recents" || { R[$tag-program-ends]="yes ${i}s"; break; }
  done
  [[ ${R[$tag-program-ends]} == no* && $BROWSER == org.mozilla.* ]] && R[$tag-program-ends]="no: Firefox keeps it (its end screen asks for ✕)"
  "$VM" "$NAME" screenshot "$OUT/$tag-program-ended.png" >/dev/null 2>&1
  log "$tag program ending closes the window: ${R[$tag-program-ends]}"
}

# The newest note (`lindroids run` keeps the last 64, so their count stops growing): a new window
# makes a new one, with its run's ticket.
notes() { tsh 'tail -1 $TMPDIR/lindroids-$(id -u)/windows 2>/dev/null' | tr -d '\r' | cut -d' ' -f1; }
# Until the program runs and `lindroids` noted a window opened after <the newest note before>, and that
# window (its run) is in Recents. Sets RUN.
wait_window() { # <notes before>
  local before=${1:-0} now
  RUN=""
  for i in $(seq 1 40); do
    now=$(notes)
    if running && [[ -n $now && $now != "$before" ]]; then
      RUN=$(last_window | grep -oE 'run=[0-9]+' | cut -d= -f2)
      # Not `| grep -q`: with pipefail, grep leaving early fails the pipe on a match.
      grep -qE "window&run=$RUN([^0-9]|$)" <<<"$(sh_ 'dumpsys activity recents')" && { sleep 3; return 0; }
    fi
    sleep 1
  done
  log "wait_window: notes before $before, now ${now:-?}, run ${RUN:-?}"
  return 1
}

"$VM" "$NAME" wait-boot 60 >/dev/null || exit 1
# No server left over (Termux force-stopped does not take it): the new one needs its port.
tsh 'lindroids stop >/dev/null 2>&1; pkill -f "[l]indroids server"; sleep 2; true'
tsh "pkill -f '$PROC'; true"
sh_ 'input keyevent KEYCODE_HOME'

# ------------------------------------------------------------------ lindroids run
log "lindroids run $APP"
n=$(notes)
tsh "cd ~; (nohup lindroids run $APP > ~/windows-run.log 2>&1 </dev/null &); true"
wait_window "$n" || { log "no window"; exit 1; }
read -r url BROWSER < <(last_window)
OUT=$REPO/lab/results/$NAME/windows-$BROWSER
rm -rf "$OUT"
mkdir -p "$OUT"
R[browser]=$BROWSER
log "window in $BROWSER"
"$VM" "$NAME" screenshot "$OUT/run-window.png" >/dev/null 2>&1
check_recents run
check_background run
check_close_window run
n=$(notes)
tsh "cd ~; (nohup lindroids run $APP > ~/windows-run.log 2>&1 </dev/null &); true"
wait_window "$n" || log "no window the second time"
check_program_ends run "$RUN"

# ------------------------------------------------------------------ the exported icon
log "lindroids export $APP, tapped with Termux stopped"
tsh "lindroids export $APP 2>&1" >"$OUT/export.txt"
xy=$(ui_find '(?i)^add( to home screen)?$') && sh_ "input tap $xy"
sleep 2
# Its id is the Flatpak's, or termux.<program> for a Termux program.
label=$(tsh "lindroids list 2>/dev/null" | tr -d '\r' | awk -v a="$APP" '$1 == a || $1 == "termux." a' | sed -E 's/^[^ ]+ +//; s/  +.*//' | head -1)
[[ -n $label ]] || { log "export: $APP not in lindroids list"; label=$APP; }
sh_ 'am force-stop com.termux'
sh_ 'input keyevent KEYCODE_HOME'
sleep 2
tap_icon() {
  local xy
  xy=$(ui_find "^$label\$") || { sh_ 'input swipe 1000 400 200 400 300'; sleep 1; xy=$(ui_find "^$label\$"); } || return 1
  sh_ "input tap $xy"
}
restart_sshd() { # a shortcut's run owns Termux's only session: a new one for sshd
  sh_ 'am start -n com.termux/.app.TermuxActivity' >/dev/null 2>&1
  sleep 2
  sh_ 'input keycombination KEYCODE_CTRL_LEFT KEYCODE_ALT_LEFT KEYCODE_C'
  sleep 2
  sh_ 'input text sshd'
  sh_ 'input keyevent 66'
  sleep 3
}
if tap_icon; then
  sleep 12
  restart_sshd
  # Back to the window (the run's notification, or Recents): bring it with its intent again.
  sh_ 'input keyevent KEYCODE_HOME'
  sleep 1
  n=$(notes)
  tap_icon
  wait_window "$n" || log "export: no window"
  read -r url B2 < <(last_window)
  R[export-browser]=$B2
  check_recents export
  check_close_window export
  n=$(notes)
  tap_icon
  wait_window "$n" || log "export: no window the second time"
  check_program_ends export "$RUN"
else
  R[export]="icon $label not on the home screen"
  log "${R[export]}"
fi

{
  for k in browser run-recents run-background run-close-window run-program-ends export-browser export-recents export-close-window export-program-ends export; do
    [[ -n ${R[$k]:-} ]] && printf '%-24s %s\n' "$k" "${R[$k]}"
  done
} | tee "$OUT/result.txt"
