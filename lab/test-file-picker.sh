#!/usr/bin/env bash
# A program's file dialogs must be Android's picker: Open File, Save As and (where the program has
# one) Open Folder each ask the desktop portal (org.freedesktop.portal.FileChooser OpenFile /
# SaveFile on the server's session bus) and bring up Android's DocumentsUI. A window of the
# program's own (GTK's "Open File" dialog) fails the check, with its title.
#
#   lab/test-file-picker.sh <vm> [app]          (default com.vscodium.codium)
#   LAB_BROWSER=org.cromite.cromite lab/test-file-picker.sh b16    (the window's browser for this
#                                               run only: LINDROIDS_BROWSER; the default is left)
#   LAB_PROC='[m]ousepad' lab/test-file-picker.sh b16 mousepad
#
# The bus is watched from before the program starts (dbus-monitor), so its startup probe of the
# portal (Properties.Get of FileChooser's `version`) is in the log too. Open File picks
# Documents/meeting.txt when DocumentsUI shows it and checks the path the program got is readable
# on the Linux side. Results: lab/results/<vm>/file-picker-<app>/ (result.txt, dbus.log,
# server.log, screenshots). Uses the lindroids that is on the VM now; stops its server first
# (programs other agents left running on the VM end with it).
set -uo pipefail

LAB_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$LAB_DIR/.." && pwd)
VM=$LAB_DIR/vm.sh
NAME=${1:?usage: test-file-picker.sh <vm> [app]}
APP=${2:-com.vscodium.codium}
case $APP in
  com.vscodium.codium) PROC_DEFAULT='[/]app/share/codium/' ;;
  *.*.*) PROC_DEFAULT='[/]app/bin/' ;;
  *) PROC_DEFAULT="[${APP:0:1}]${APP:1}" ;;
esac
PROC=${LAB_PROC:-$PROC_DEFAULT}
OUT=$REPO/lab/results/$NAME/file-picker-${APP//\//_}
rm -rf "$OUT"
mkdir -p "$OUT"
sh_() { "$VM" "$NAME" adb shell "$@" | tr -d '\r'; }
tsh() { timeout "${TSH_TIMEOUT:-60}" "$VM" "$NAME" ssh "$@"; }
log() { echo "[$(date +%T)] $NAME: $*"; }
shot() { "$VM" "$NAME" screenshot "$OUT/$1.png" >/dev/null 2>&1; }
declare -A R
RT='$TMPDIR/lindroids-$(id -u)'

running() { tsh "pgrep -f '$PROC' >/dev/null"; }
# Not `| grep -q`: with pipefail, grep leaving early fails the pipe on a match.
top_activity() { local a; a=$(sh_ 'dumpsys activity activities'); grep -m1 topResumedActivity <<<"$a"; }
picker_up() { [[ $(top_activity) == *[Dd]ocumentsui* ]]; }
# Lines of the bus log / server log, to look only at what came after.
dbus_lines() { tsh "wc -l < ~/file-picker-dbus.log" | tr -d '\r '; }
server_lines() { tsh "wc -l < $RT/server.log" | tr -d '\r '; }
dbus_since() { tsh "tail -n +\$(( $1 + 1 )) ~/file-picker-dbus.log"; }
server_since() { tsh "tail -n +\$(( $1 + 1 )) $RT/server.log"; }

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

# Back out of DocumentsUI (Back goes up a folder first) until the program's window is in front.
cancel_picker() {
  for i in 1 2 3 4 5 6; do
    picker_up || return 0
    sh_ 'input keyevent KEYCODE_BACK'
    sleep 1.5
  done
  picker_up && return 1 || return 0
}

# The program's window in front: two taps where the program has nothing to type into (a tap in
# text brings up the soft keyboard over the menus), the first of which the page may take for full
# screen. FOCUS: "x y" per program.
focus_window() {
  find_top; tap $FOCUS 2
  find_top; tap $FOCUS 2
  hide_keyboard
  find_top
}

# The soft keyboard covers the program's lower half: Back hides it (and only that).
hide_keyboard() {
  local im
  for i in 1 2; do
    im=$(sh_ 'dumpsys input_method')
    grep -q 'mInputShown=true' <<<"$im" || return 0
    sh_ 'input keyevent KEYCODE_BACK'
    sleep 1.5
  done
}

# Where the page shows the program's top left: below the browser's toolbar, unless full screen.
TOP=0
find_top() {
  if ui_find '(?i)^(close tab|return to previous app)$|:id/close_button$' >/dev/null; then TOP=81; else TOP=0; fi
}

# A tap on the program at x y (its own coordinates).
tap() { sh_ "input tap $1 $(( $2 + TOP ))"; sleep "${3:-3}"; }

# One dialog: <tag> <FileChooser method> <how>: a shell function that brings the dialog up (the
# program's menu: keys from the page do not reach it as shortcuts).
check_dialog() {
  local tag=$1 method=$2 how=$3 d0 s0 calls dialogs
  d0=$(dbus_lines); s0=$(server_lines)
  # Up to three tries: a menu tap is lost now and then while the page changes size.
  for try in 1 2 3; do
    focus_window
    $how
    for i in $(seq 1 10); do picker_up && break; sleep 1; done
    picker_up && break
    [[ -n $(server_since "$s0" | grep 'mapped:') ]] && break
  done
  sleep 1
  shot "$tag"
  calls=$(dbus_since "$d0" | grep -c "interface=org.freedesktop.portal.FileChooser; member=$method")
  # A window of the program's own that is not its main one: GTK's dialog.
  dialogs=$(server_since "$s0" | sed -n 's/.*window [0-9]* mapped: \(".*"\).*/\1/p' | paste -sd ',')
  if picker_up && [[ $calls -ge 1 ]] && [[ -z $dialogs ]]; then
    R[$tag]="yes: FileChooser.$method, Android's picker"
  elif [[ -n $dialogs ]]; then
    R[$tag]="no: the program's own dialog $dialogs (FileChooser.$method calls: $calls)"
  else
    R[$tag]="no: FileChooser.$method calls $calls, top: $(top_activity | sed 's/^ *//')"
  fi
  dbus_since "$d0" >"$OUT/$tag-dbus.txt"
  log "$tag: ${R[$tag]}"
}

# The picker's answer reached the program: Response 1 (cancelled), or 0 with a readable path.
check_response() { # <tag> <dbus line before> <expect: cancel|file>
  local tag=$1 d0=$2 want=$3 resp uri path
  for i in $(seq 1 10); do
    resp=$(dbus_since "$d0" | grep -A12 'interface=org.freedesktop.portal.Request; member=Response' | sed -n 's/^ *uint32 \([0-9]\)$/\1/p' | head -1)
    [[ -n $resp ]] && break
    sleep 1
  done
  if [[ $want == cancel ]]; then
    [[ $resp == 1 ]] && R[$tag-answer]="yes: cancelled (Response 1)" || R[$tag-answer]="no: Response ${resp:-none}"
  else
    uri=$(dbus_since "$d0" | grep -A12 'member=Response' | grep -m1 -o '"file://[^"]*"' | tr -d '"')
    path=$(python3 -c 'import sys, urllib.parse; print(urllib.parse.unquote(sys.argv[1][7:]))' "$uri" 2>/dev/null)
    if [[ $resp == 0 && -n $path ]] && tsh "test -r '$path'"; then
      R[$tag-answer]="yes: $path ($(tsh "head -c 60 '$path'" | tr -d '\r\n'))"
    else
      R[$tag-answer]="no: Response ${resp:-none} ${uri:-no uri}"
    fi
  fi
  log "$tag answer: ${R[$tag-answer]}"
}

"$VM" "$NAME" wait-boot 60 >/dev/null || exit 1
tsh 'lindroids stop >/dev/null 2>&1; pkill -f "[l]indroids server"; pkill -f "[d]bus-monitor --address"; sleep 2; true'
tsh "pkill -f '$PROC'; true"
sh_ 'input keyevent KEYCODE_HOME'
# The server (and its bus) first, so the bus is watched from the program's first message.
tsh 'lindroids start >/dev/null 2>&1; true'
tsh "rm -f ~/file-picker-dbus.log; (nohup dbus-monitor --address unix:path=$RT/bus > ~/file-picker-dbus.log 2>&1 </dev/null &); sleep 1; true"
log "lindroids run $APP"
tsh "cd ~; (${LAB_BROWSER:+LINDROIDS_BROWSER=$LAB_BROWSER} nohup lindroids run $APP > ~/file-picker-run.log 2>&1 </dev/null &); true"
for i in $(seq 1 60); do
  running && tsh "grep -q 'mapped:' $RT/server.log" && tsh "grep -q 'app linked' $RT/server.log" && break
  sleep 1
done
sleep "${LAB_SETTLE:-12}"
shot window
# Its startup probe: the version it read (or the error it got).
tsh "grep -A3 'member=Get' ~/file-picker-dbus.log | grep -A3 FileChooser" >"$OUT/startup-probe.txt" 2>&1
R[probe]=$(grep -q FileChooser "$OUT/startup-probe.txt" && echo "asked FileChooser's version at startup" || echo "no FileChooser probe at startup (asked at the first dialog)")

# Each program's menu, in its own coordinates (1280x800 screen, the window filling it).
case $APP in
  com.vscodium.codium)
    FOCUS="26 520"   # the activity bar, below its icons
    # File menu: Open File..., Open Folder..., Save Workspace As... (a save dialog like Save As,
    # whatever editor is in front).
    open_() { tap 54 53; tap 103 190; }
    folder_() { tap 54 53; tap 103 215; }
    new_() { :; }
    save_() { tap 54 53; tap 103 321; }
    ;;
  org.gnome.TextEditor)
    FOCUS="640 24"   # the header bar's title
    # Open's popover, its browse button; Save As... in the main menu.
    open_() { tap 36 23; tap 338 76; }
    new_() { :; }
    save_() { tap 1215 23; tap 1100 279; }
    ;;
  mousepad)
    FOCUS="900 18"   # its title bar
    # File menu: Open..., Save As...
    open_() { tap 20 53; tap 52 177; }
    new_() { :; }
    save_() { tap 20 53; tap 60 271; }
    ;;
  *) log "no menu positions for $APP"; exit 2 ;;
esac

# Save first, while the program is as it started; then Open Folder; then Open File, which picks a
# file.
focus_window; new_; sleep 3
d0=$(dbus_lines)
check_dialog save SaveFile save_
cancel_picker; check_response save "$d0" cancel
if declare -F folder_ >/dev/null; then
  sleep 3
  d0=$(dbus_lines)
  check_dialog folder OpenFile folder_
  cancel_picker; check_response folder "$d0" cancel
fi
sleep 3
d0=$(dbus_lines)
check_dialog open OpenFile open_
xy=""
# DocumentsUI opens where it was last; Documents/meeting.txt is the lab's file.
picker_up && xy=$(ui_find '^meeting\.txt$')
if [[ -n $xy ]]; then
  sh_ "input tap $xy"
  check_response open "$d0" file
else
  cancel_picker
  check_response open "$d0" cancel
fi
sleep 3
shot after

tsh "cat $RT/server.log" >"$OUT/server.log" 2>&1
tsh "cat ~/file-picker-dbus.log" >"$OUT/dbus.log" 2>&1
# Leave nothing running: the program, its server and the bus monitor.
cancel_picker
tsh "lindroids stop >/dev/null 2>&1; pkill -f '$PROC'; pkill -f '[d]bus-monitor --address'; true"
sh_ 'input keyevent KEYCODE_HOME'
{
  for k in probe open open-answer save save-answer folder folder-answer; do
    [[ -n ${R[$k]:-} ]] && printf '%-14s %s\n' "$k" "${R[$k]}"
  done
} | tee "$OUT/result.txt"
! grep -q '^[a-z-]* *no' "$OUT/result.txt"
