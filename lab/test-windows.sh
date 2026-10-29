#!/usr/bin/env bash
# A program's window of its own, launched both ways — `lindroids run` and its exported home screen
# icon (Termux closed) — with the browser Lindroids picks on this VM:
#   - Recents: the window's task label and icon (the TaskDescription its browser set), and a
#     screenshot of Recents;
#   - Back, three times from full screen, leaves the window and its program as they are;
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

# These are the browser windows; the default export is now the window app.
tsh 'lindroids config browser default' >/dev/null 2>&1 || true

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

# Back from full screen, three times (the first leaves full screen; the browser closed the window
# on the next, and its program ended): the window and its program stay.
check_back() { # <tag> <run>
  local tag=$1 run=$2
  sh_ 'input tap 640 400'
  sleep 2
  for i in 1 2 3; do sh_ 'input keyevent KEYCODE_BACK'; sleep 1.5; done
  sleep 6
  if running && grep -qE "window&run=$run([^0-9]|$)" <<<"$(sh_ 'dumpsys activity recents')"; then
    R[$tag-back]="window and program kept"
  else
    R[$tag-back]="no: $(running && echo "program running, window gone" || echo "program ended")"
  fi
  "$VM" "$NAME" screenshot "$OUT/$tag-after-back.png" >/dev/null 2>&1
  log "$tag Back x3: ${R[$tag-back]}"
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

# ------------------------------------------------------------------ several instances
# LAB_MODE=instances: two instances of one app at once, one closed, the icon tapped again, the end
# cleaned up, the limits. LAB_MAIN: a pgrep -f pattern for the app's main process (one per
# process of its own); LAB_EXPECT: "claim" (the second hands over: one process, two windows),
# "two" (two processes, two windows), "focus" (one window, brought forward).
#   LAB_MODE=instances LAB_EXPECT=claim LAB_MAIN='[/]app/share/codium/com.vscodium.codium --ext' \
#     LAB_PROC='[/]app/share/codium/' lab/test-windows.sh b16 com.vscodium.codium
MAIN=${LAB_MAIN:-$PROC}
mains() { tsh "pgrep -f '$MAIN' | wc -l" | tr -d '\r ' ; }
# The instances `lindroids instances` lists for this app: "pid ordinal name..." lines.
instances() { tsh "lindroids instances $KEY 2>/dev/null" | tr -d '\r' | grep -E '^[0-9]+ +[0-9]+ ' ; }
# Recents tasks showing one of this app's instances (or the given runs).
tasks_of() { local recents n=0 p; recents=$(sh_ 'dumpsys activity recents'); for p in "$@"; do grep -qE "window&run=$p([^0-9]|$)" <<<"$recents" && n=$((n+1)); done; echo $n; }

# Start A, then B with --new; what B became.
check_two_instances() {
  local before
  tsh "lindroids stop >/dev/null 2>&1; pkill -f '$PROC'; true"
  sleep 2
  n=$(notes)
  tsh "cd ~; (nohup lindroids run $APP > ~/inst-a.log 2>&1 </dev/null &); true"
  wait_window "$n" || { R[two]="no: the first instance has no window"; return 1; }
  A=$RUN
  sleep 5
  KEYLINE=$(tsh "lindroids instances" | tr -d '\r' | grep -E "^$A ")
  [[ -n $KEYLINE ]] || { R[two]="no: run $A not in lindroids instances"; return 1; }
  before=$(mains)
  tsh "cd ~; (nohup lindroids run --new $APP > ~/inst-b.log 2>&1 </dev/null &); true"
  sleep "${LAB_WAIT_B:-30}"
  tsh "cat ~/inst-b.log; lindroids instances" | tr -d '\r' >"$OUT/two-b.txt"
  "$VM" "$NAME" screenshot "$OUT/two-b.png" >/dev/null 2>&1
  mapfile -t PIDS < <(instances | awk '{print $1}')
  B=$(instances | awk -v a="$A" '$1 != a {print $1; exit}')
  local nproc ntask
  nproc=$(mains); ntask=$(tasks_of "${PIDS[@]}")
  R[two]="instances: $(instances | awk '{$1=""; print}' | paste -sd ';'); main processes $before -> $nproc; window tasks $ntask"
  case ${LAB_EXPECT:-two} in
    claim) [[ ${#PIDS[@]} == 2 && $nproc == "$before" && $ntask == 2 ]] && grep -q 'a window of another' <<<"$(instances)" && R[two]="yes (handed over, claimed): ${R[two]}" || R[two]="no: ${R[two]}" ;;
    two) [[ ${#PIDS[@]} == 2 && $nproc -gt $before && $ntask == 2 ]] && R[two]="yes (two processes): ${R[two]}" || R[two]="no: ${R[two]}" ;;
    focus) [[ ${#PIDS[@]} == 1 && $ntask == 1 ]] && R[two]="yes (one window, brought forward): ${R[two]}" || R[two]="no: ${R[two]}" ;;
  esac
  log "two instances: ${R[two]}"
}

# The newest window (B's, on top) closed with its ✕: A and its process stay.
check_close_one() {
  local xy="" left
  [[ -n ${B:-} ]] || { R[close-one]="skipped (one instance)"; return; }
  sh_ 'input tap 640 400'
  sleep 2
  for i in 1 2 3; do
    sh_ 'input keyevent KEYCODE_BACK'
    sleep 1.5
    xy=$(ui_find '(?i)^(close tab|return to previous app)$|:id/close_button$') && break
  done
  [[ -n $xy ]] || { R[close-one]="no ✕ found"; return; }
  sh_ "input tap $xy"
  sleep 10
  left=$(instances | awk '{print $1}' | paste -sd ' ')
  if [[ $left == "$A" ]] && [[ $(mains) -ge 1 ]] && [[ $(tasks_of "$A") == 1 && $(tasks_of "$B") == 0 ]]; then
    R[close-one]="yes: instance 2 gone, instance 1 (run $A) and its process kept"
  else
    R[close-one]="no: instances left [$left], main processes $(mains), tasks A $(tasks_of "$A") B $(tasks_of "$B")"
  fi
  "$VM" "$NAME" screenshot "$OUT/close-one.png" >/dev/null 2>&1
  log "close one: ${R[close-one]}"
}

# The program's icon again (`lindroids launch <id>`, what the shortcut runs) while it runs: its
# window comes back, nothing new starts. Then --new adds one, which goes again.
check_tap_again() {
  local p0 t0 p1 t1
  tsh "lindroids export $APP >/dev/null 2>&1; true"
  sh_ 'input keyevent KEYCODE_HOME'
  sleep 2
  p0=$(mains); mapfile -t PIDS < <(instances | awk '{print $1}'); t0=$(tasks_of "${PIDS[@]}")
  TSH_TIMEOUT=60 tsh "lindroids launch $KEY" >"$OUT/tap-again.txt" 2>&1
  sleep 4
  p1=$(mains); mapfile -t P2 < <(instances | awk '{print $1}'); t1=$(tasks_of "${P2[@]}")
  local top; top=$(sh_ 'dumpsys activity activities' | grep -m1 topResumedActivity)
  if [[ $p0 == "$p1" && ${#PIDS[@]} == "${#P2[@]}" && $t0 == "$t1" ]] && grep -q -i 'customtab\|firefox' <<<"$top"; then
    R[tap-again]="yes: window back in front, processes $p0, instances ${#P2[@]}, tasks $t1"
  else
    R[tap-again]="no: processes $p0 -> $p1, instances ${#PIDS[@]} -> ${#P2[@]}, tasks $t0 -> $t1, top: $top"
  fi
  log "tap again: ${R[tap-again]}"
}

# Limits: a third with LINDROIDS_MAX_INSTANCES=1 is refused, with a reason; --force passes.
check_limits() {
  local out rc
  out=$(tsh "LINDROIDS_MAX_INSTANCES=1 lindroids run --new $APP 2>&1; echo rc=\$?" | tr -d '\r')
  rc=$(sed -n 's/^rc=//p' <<<"$out")
  if [[ $rc == 1 ]] && grep -q 'LINDROIDS_MAX_INSTANCES=1' <<<"$out"; then
    R[limits]="refused: $(grep -m1 lindroids: <<<"$out")"
  else
    R[limits]="no: rc $rc, $(head -c 300 <<<"$out")"
  fi
  log "limits: ${R[limits]}"
}

# The program ending (killed): every instance goes from `lindroids instances`, and their
# notifications and windows go.
check_cleanup() {
  local left notes_left
  tsh "pkill -f '$PROC'; true"
  sleep 12
  left=$(instances | wc -l)
  notes_left=$(sh_ 'dumpsys notification --noredact' | grep -c 'android.text=String (Running. Tap to show its window')
  if [[ $left == 0 && $(tasks_of "$A" ${B:-}) == 0 && $notes_left == 0 ]]; then
    R[cleanup]="yes: no instances, no window tasks ($notes_left running notifications left)"
  else
    R[cleanup]="no: $left instances, tasks $(tasks_of "$A" ${B:-}), notifications $notes_left"
  fi
  log "cleanup: ${R[cleanup]}"
}

# The home screen icon tapped twice: the first tap starts the program, the second brings its
# window back (nothing new starts); the "(new window)" icon (export --new-window) adds one.
check_shortcut_taps() {
  local label="$1" t1 i1 p1 t2 i2 p2 top
  tsh "lindroids stop >/dev/null 2>&1; pkill -f '$PROC'; true"
  sleep 2
  sh_ 'input keyevent KEYCODE_HOME'
  sleep 2
  icon() { local xy re; re=$(sed 's/[][().*+?^$|\\]/\\&/g' <<<"$1"); xy=$(ui_find "^$re\$") || { sh_ 'input swipe 1000 400 200 400 300'; sleep 1; xy=$(ui_find "^$re\$"); } || return 1; sh_ "input tap $xy"; }
  icon "$label" || { R[taps]="icon $label not on the home screen"; return; }
  n=$(notes); wait_window "$n" || { R[taps]="no window after the first tap"; return; }
  sleep 5
  mapfile -t P < <(instances | awk '{print $1}'); i1=${#P[@]}; t1=$(tasks_of "${P[@]}"); p1=$(mains)
  sh_ 'input keyevent KEYCODE_HOME'; sleep 2
  icon "$label"
  sleep 12
  mapfile -t P < <(instances | awk '{print $1}'); i2=${#P[@]}; t2=$(tasks_of "${P[@]}"); p2=$(mains)
  top=$(sh_ 'dumpsys activity activities' | grep -m1 topResumedActivity)
  "$VM" "$NAME" screenshot "$OUT/second-tap.png" >/dev/null 2>&1
  if [[ $i1 == 1 && $i2 == 1 && $t1 == "$t2" && $p1 == "$p2" ]] && grep -qi 'customtab' <<<"$top"; then
    R[taps]="yes: tap 1 started it, tap 2 brought it back (instances $i2, tasks $t2, processes $p2)"
  else
    R[taps]="no: instances $i1 -> $i2, tasks $t1 -> $t2, processes $p1 -> $p2, top $top"
  fi
  log "two taps: ${R[taps]}"
  # The "(new window)" icon.
  tsh "lindroids export --new-window $KEY 2>&1" >"$OUT/export-new-window.txt"
  local xy; xy=$(ui_find '(?i)^add( to home screen)?$') && sh_ "input tap $xy"
  sleep 2
  sh_ 'input keyevent KEYCODE_HOME'; sleep 2
  icon "$label (new window)" || { R[new-icon]="icon \"$label (new window)\" not on the home screen ($(cat "$OUT/export-new-window.txt" | head -2))"; return; }
  sleep "${LAB_WAIT_B:-30}"
  mapfile -t P < <(instances | awk '{print $1}')
  "$VM" "$NAME" screenshot "$OUT/new-window-icon.png" >/dev/null 2>&1
  [[ ${#P[@]} == 2 && $(tasks_of "${P[@]}") == 2 ]] && R[new-icon]="yes: a second instance, its own window ($(instances | awk '{$1="";print}' | paste -sd ';'))" ||
    R[new-icon]="no: instances ${#P[@]}, tasks $(tasks_of "${P[@]}")"
  log "new-window icon: ${R[new-icon]}"
}

if [[ ${LAB_MODE:-} == taps ]]; then
  "$VM" "$NAME" wait-boot 60 >/dev/null || exit 1
  OUT=$REPO/lab/results/$NAME/taps-${APP//\//_}
  rm -rf "$OUT"; mkdir -p "$OUT"
  if [[ $APP == *.*.* ]]; then KEY=$APP; else KEY=termux.$APP; fi
  tsh "lindroids export $APP >/dev/null 2>&1; true"
  xy=$(ui_find '(?i)^add( to home screen)?$') && sh_ "input tap $xy"
  label=$(tsh "lindroids list 2>/dev/null" | tr -d '\r' | awk -v a="$KEY" '$1 == a' | sed -E 's/^[^ ]+ +//; s/  +.*//' | head -1)
  check_shortcut_taps "${label:-$APP}"
  { for k in taps new-icon; do [[ -n ${R[$k]:-} ]] && printf '%-12s %s\n' "$k" "${R[$k]}"; done; } | tee "$OUT/result.txt"
  exit 0
fi

if [[ ${LAB_MODE:-} == instances ]]; then
  "$VM" "$NAME" wait-boot 60 >/dev/null || exit 1
  OUT=$REPO/lab/results/$NAME/instances-${APP//\//_}
  rm -rf "$OUT"; mkdir -p "$OUT"
  # Its key: the Flatpak id, or termux.<program>.
  if [[ $APP == *.*.* ]]; then KEY=$APP; else KEY=termux.$APP; fi
  sh_ 'input keyevent KEYCODE_HOME'
  check_two_instances && {
    if [[ ${LAB_EXPECT:-two} != focus ]]; then check_tap_again; check_close_one; fi
    check_limits
    check_cleanup
  }
  {
    for k in two tap-again close-one limits cleanup; do
      [[ -n ${R[$k]:-} ]] && printf '%-12s %s\n' "$k" "${R[$k]}"
    done
  } | tee "$OUT/result.txt"
  exit 0
fi

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
check_back run "$RUN"
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
  for k in browser run-recents run-background run-back run-close-window run-program-ends export-browser export-recents export-close-window export-program-ends export; do
    [[ -n ${R[$k]:-} ]] && printf '%-24s %s\n' "$k" "${R[$k]}"
  done
} | tee "$OUT/result.txt"
