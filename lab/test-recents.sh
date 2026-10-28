#!/usr/bin/env bash
# Whose icon a window's card can get in Recents, for every Custom Tab browser on the VM and every
# intent shape Termux could send. No Lindroids needed: a stand-in page (browser-window/probe.py,
# with a title, a 192 px favicon and a manifest) opened as Termux by browser-window/Open.java.
#
# A launcher draws a task's card from what Android keeps for the task (RecentTaskInfo): the
# TaskDescription (label, icon) its activities set, else the icon of an installed component (the
# root activity's app). This records both for each window:
#   - "TaskDescription launcher" (AOSP Launcher3, as on the lab VMs): the TaskDescription icon if
#     the browser set one, else the root activity's app icon;
#   - "app icon launcher" (Samsung One UI, reported: Chrome's logo on every Chrome window, with
#     the page's title as the label): always the root activity's app icon.
# And whether Firefox could open an installed web app (VIEW_PWA, the only Firefox window that
# sets a task icon) for an http://*.localhost page. Then what the terminal app itself could do
# about a task's icon (browser-window/Route.java, run with its uid; see "From the terminal app's
# side" below).
#
#   lab/test-recents.sh <vm>                       (default b16)
#   ROUTES_ONLY=1 lab/test-recents.sh <vm>         only the terminal app's side (keeps the default browser)
#
# Needs a JDK (javac, java) and the Android SDK (android.jar, d8): ANDROID_HOME, JAVA_BIN.
# Results: lab/results/<vm>/recents/ (result.txt, the icons Android kept). Leaves the default
# browser as it found it and removes the windows it opened.
set -uo pipefail

LAB_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
VM=$LAB_DIR/vm.sh
NAME=${1:-b16}
PORT=48100
OUT=$LAB_DIR/results/$NAME/recents
mkdir -p "$OUT"
: "${ANDROID_HOME:=$HOME/Android/Sdk}"
sh_() { "$VM" "$NAME" adb shell "$@" | tr -d '\r'; }
tsh() { timeout 60 "$VM" "$NAME" ssh "$@"; }
log() { echo "[$(date +%T)] $NAME: $*" | tee -a "$OUT/result.txt"; }
: >"$OUT/result.txt"

# The two probes, built fresh (names of their own on the VM: other tests use open.dex).
build_dex() { # <java file> <out dex>
  local jar d8 tmp
  jar=$(ls -d "$ANDROID_HOME"/platforms/android-*/android.jar | sort -V | tail -1)
  d8=$(ls -d "$ANDROID_HOME"/build-tools/*/d8 | sort -V | tail -1)
  tmp=$(mktemp -d)
  PATH=${JAVA_BIN:+$JAVA_BIN:}$PATH javac --release 11 -Xlint:-options -cp "$jar" -d "$tmp" "$1" &&
    PATH=${JAVA_BIN:+$JAVA_BIN:}$PATH "$d8" --min-api 26 --lib "$jar" --output "$tmp" $(find "$tmp" -name '*.class') &&
    cp "$tmp/classes.dex" "$2"
  local rc=$?
  rm -rf "$tmp"
  return $rc
}
build_dex "$LAB_DIR/browser-window/Open.java" /tmp/lab-open.dex || { echo "cannot build Open.java" >&2; exit 1; }
build_dex "$LAB_DIR/browser-window/TaskProbe.java" /tmp/lab-taskprobe.dex || { echo "cannot build TaskProbe.java" >&2; exit 1; }
build_dex "$LAB_DIR/browser-window/Route.java" /tmp/lab-route.dex || { echo "cannot build Route.java" >&2; exit 1; }
"$VM" "$NAME" scp /tmp/lab-open.dex open-recents.dex >/dev/null
"$VM" "$NAME" scp "$LAB_DIR/browser-window/probe.py" probe-recents.py >/dev/null
"$VM" "$NAME" adb push /tmp/lab-taskprobe.dex /data/local/tmp/taskprobe-recents.dex >/dev/null
probe() { sh_ "CLASSPATH=/data/local/tmp/taskprobe-recents.dex app_process /system/bin probe.TaskProbe $*"; }

# The page: started once, detached (it outlives this ssh session).
if ! tsh "curl -fs 127.0.0.1:$PORT/app/x/ >/dev/null"; then
  tsh "setsid python ~/probe-recents.py $PORT >~/probe-recents.out 2>&1 </dev/null &"
  sleep 2
fi
tsh "curl -fs 127.0.0.1:$PORT/app/x/ >/dev/null" || { log "probe page did not start"; exit 1; }

default_browser=$(sh_ 'dumpsys role' | awk '/name=android.app.role.BROWSER/{f=1} f&&/holders=/{sub(/.*holders=/,""); print; exit}')
log "default browser before: ${default_browser:-none}"
browsers=$(sh_ 'cmd package query-services --brief -a android.support.customtabs.action.CustomTabsService' |
  sed -n 's#^ *\([a-zA-Z0-9_.]*\)/.*#\1#p' | sort -u)
log "Custom Tab browsers: $(echo $browsers)"

opened=()
n=0
[[ ${ROUTES_ONLY:-0} == 1 ]] && log "ROUTES_ONLY: browsers' windows skipped"
for b in $([[ ${ROUTES_ONLY:-0} == 1 ]] || echo $browsers); do
  # Chromium takes Custom Tab intents from other apps only as the default browser (Cromite also
  # needs Settings → Privacy → Allow custom tab intents).
  sh_ "cmd role add-role-holder android.app.role.BROWSER $b 0" >/dev/null
  # NEW_TASK|NEW_DOCUMENT (what Lindroids sends), NEW_TASK, NEW_TASK|MULTIPLE_TASK
  for flags in 10080000 10000000 18000000; do
    n=$((n + 1))
    tag=w$n
    url="http://rt$tag.localhost:$PORT/app/$tag/"
    tsh "/system/bin/app_process -Djava.class.path=\$HOME/open-recents.dex /system/bin probe.Open com.termux $url -p $b --cct -f $flags" >/dev/null 2>&1
    # The browser sets the icon once it has the favicon; Android writes the file a moment later.
    info=""
    for i in $(seq 1 20); do
      sleep 0.5
      info=$(probe | awk -v u="$url" '/^task /{t=$0; blk=""} {blk=blk"\n"$0} /data:/{if (index($0,u)) hit=t} /top activity:/{if (hit==t && t!="") {print blk; exit}}')
      grep -q 'icon file: /' <<<"$info" && break
    done
    task=$(sed -n 's/^task \([0-9]*\).*/\1/p' <<<"$info" | head -1)
    root=$(sed -n 's/.*owner): *//p' <<<"$info")
    label=$(sed -n 's/.*TaskDescription label: *//p' <<<"$info")
    iconfile=$(sed -n 's/.*icon file: *//p' <<<"$info")
    [[ -n $task ]] && opened+=("$task")
    td="browser's icon"
    if [[ $iconfile == /* ]]; then
      for i in $(seq 1 20); do [[ $(sh_ "test -f $iconfile && echo yes") == yes ]] && break; sleep 0.5; done
      "$VM" "$NAME" adb exec-out cat "$iconfile" >"$OUT/$tag-$b-icon.png" 2>/dev/null
      if [[ $(head -c 4 "$OUT/$tag-$b-icon.png" | tail -c 3) == PNG ]]; then
        td="the page's icon ($tag-$b-icon.png)"
      else
        rm -f "$OUT/$tag-$b-icon.png"
        td="an icon file Android had not written yet ($iconfile)"
      fi
    fi
    log "$b flags 0x$flags: task ${task:-none}, root ${root:-none}, label ${label:-none}"
    log "    TaskDescription launcher: $td; app icon launcher: ${root%%/*}'s app icon"
  done
done

# Firefox's installed web apps (VIEW_PWA) set a task icon; Firefox installs them only for https.
for d in $([[ ${ROUTES_ONLY:-0} == 1 ]] || echo "http://rtpwa.localhost:$PORT/app/pwa/" https://example.com/); do
  r=$(sh_ "cmd package query-activities --brief -a mozilla.components.feature.pwa.VIEW_PWA -d $d" | grep -c '/')
  log "VIEW_PWA resolves for $d: $([[ $r -gt 0 ]] && echo yes || echo no)"
done

[[ ${#opened[@]} -gt 0 ]] && probe remove "${opened[@]}" >/dev/null

# From the terminal app's side, as its uid (Route.java names the package from the uid: nothing here
# assumes Termux). Could it put the window into a task of its own package, whose root activity
# would then decide the icon on an "app icon launcher", or set a task's TaskDescription itself?
"$VM" "$NAME" scp /tmp/lab-route.dex route-recents.dex >/dev/null
route() { tsh "/system/bin/app_process -Djava.class.path=\$HOME/route-recents.dex /system/bin probe.Route $*" 2>&1 | grep -vE '^\s+at |^\s*$|Remote stack trace'; }
host=$(route pkg | awk '{print $1}')
log "terminal app (from this process' uid): ${host:-unknown}"
tasks=$(route apptasks)
apptask=$(sed -n 's/^app task \([0-9]*\).*/\1/p' <<<"$tasks" | head -1)
log "its own tasks: $(sed 's/ baseActivity.* top/, top/' <<<"$tasks" | tr '\n' ';')"
cct=$(sh_ 'cmd package query-services --brief -a android.support.customtabs.action.CustomTabsService' | sed -n 's#^ *\([a-zA-Z0-9_.]*\)/.*#\1#p' | head -1)
if [[ -n $apptask && -n $cct ]]; then
  log "$cct Custom Tab into its task $apptask, IAppTask.startActivity: $(route intask "$apptask" "$cct" "http://rtin.localhost:$PORT/app/in/" | grep '^intask' | head -1)"
  log "    ActivityOptions.setLaunchTaskId / startActivityFromRecents: $(route launchtask "$apptask" "$cct" "http://rtin.localhost:$PORT/app/in/" | grep -E '^(launchtask|startActivityFromRecents)' | sed 's/Intent {.*}/Intent {…}/' | tr '\n' ' ')"
fi
log "TaskDescription without a token: $(route settd RouteProbe FF2E7D32 | tr '\n' ';')"
launcher=$(sh_ "cmd package resolve-activity --brief -c android.intent.category.LAUNCHER $host" | tail -1)
log "addAppTask (a Recents entry with an icon of its own): $(route addtask "$launcher" RouteProbe | grep -E '^addAppTask|Exception' | cut -c1-160 | tr '\n' ';')"
# Its activities: which could root a window. Any one that is not singleTask opens as a document
# of its own (NEW_DOCUMENT|MULTIPLE_TASK), but shows its own content, not the page.
aapt=$(ls -d "$ANDROID_HOME"/build-tools/*/aapt2 | sort -V | tail -1)
apk=$(sh_ "pm path $host" | sed -n 's/^package://p' | head -1)
help=""
if [[ -n $apk && -x $aapt ]] && "$VM" "$NAME" adb pull "$apk" /tmp/lab-host.apk >/dev/null 2>&1; then
  "$aapt" dump xmltree /tmp/lab-host.apk --file AndroidManifest.xml >"$OUT/host-manifest.txt"
  acts=$(awk '/E: activity/{a=1} a&&/android:name\(/{match($0,/"[^"]*"/); print substr($0,RSTART+1,RLENGTH-2); a=0}' "$OUT/host-manifest.txt")
  log "its activities: $(echo $acts)"
  help=$(grep -m1 -i help <<<"$acts")
fi
if [[ -n $help ]]; then
  for d in doc1 doc2; do route doc "$host/$help" 18080000 "http://rt$d.localhost:$PORT/app/$d/" >/dev/null; done
  sleep 3
  "$VM" "$NAME" screenshot "$OUT/host-document.png" >/dev/null
  docs=$(route apptasks | grep -F "$help" | sed -n 's/^app task \([0-9]*\).*/\1/p')
  log "$help as documents: tasks $(echo $docs); the page was asked for $(tsh 'grep -c "/app/doc" ~/probe-recents.out') times (host-document.png)"
  for t in $docs; do route remove "$t" >/dev/null; done
  sh_ "input keyevent HOME" >/dev/null
fi

[[ -n $default_browser ]] && sh_ "cmd role add-role-holder android.app.role.BROWSER $default_browser 0" >/dev/null
log "done; default browser back to ${default_browser:-none}"
