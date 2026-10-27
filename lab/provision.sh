#!/usr/bin/env bash
# Provision a booted lab VM: skip the setup wizard, install Termux, bootstrap it, and make it run a
# setup script (openssh + foot, the lab key, sshd) by typing into it.
#
#   lab/provision.sh <name>
#
# Afterwards `lab/vm.sh <name> ssh <cmd>` runs commands in Termux (QEMU forwards
# 127.0.0.1:<ssh port> to the guest's 8022, so no `adb forward` is needed and it survives adbd
# restarts). The setup script is served from the host (10.0.2.2 in the guest) by python http.server
# on the VM's lab HTTP port (ssh port + 100), bound to 127.0.0.1 only.
set -euo pipefail

LAB_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
LAB_HOME=${LAB_HOME:-$HOME/VMs/lindroids-lab}
VM=$LAB_DIR/vm.sh
TERMUX_APK=${TERMUX_APK:-$HOME/Projects/lindroids/.scratch/termux.apk}
PUBKEY=${LAB_KEY_PUB:-$HOME/Projects/lindroids/.scratch/lab_key.pub}
# One reliable mirror instead of Termux's random pick (some mirrors lag or are down).
TERMUX_MIRROR=${TERMUX_MIRROR:-https://packages-cf.termux.dev/apt}

NAME=${1:?usage: provision.sh <name>}
RUN=$LAB_HOME/run/$NAME
mkdir -p "$RUN"
SSH_PORT=$("$VM" "$NAME" ports | cut -d' ' -f2)
HTTP_PORT=$((SSH_PORT + 100))
log() { echo "[$(date +%T)] $NAME: $*"; }
adb_() { "$VM" "$NAME" adb "$@"; }
sh_() { "$VM" "$NAME" adb shell "$@" | tr -d '\r'; }

"$VM" "$NAME" wait-boot 400
SDK=$(sh_ getprop ro.build.version.sdk)
log "Android $(sh_ getprop ro.build.version.release) (sdk $SDK), selinux $(sh_ getenforce)"

# Root adbd (userdebug/eng images, and Bliss allows it on user builds): needed for the setup-wizard
# switches and to start Lindroids' non-exported pairing activity later.
adb_ root >/dev/null 2>&1 || true
sleep 3
"$VM" "$NAME" wait-boot 60 >/dev/null
log "adb runs as $(sh_ id -un)"

# --- the lab device: no setup wizard, no lock screen, screen always on, one home app
sh_ 'settings put global device_provisioned 1; settings put secure user_setup_complete 1' || true
for p in $(sh_ pm list packages | sed -n 's/^package://p' | grep -iE 'setupwizard|provision' || true); do
  sh_ "pm disable $p" >/dev/null 2>&1 && log "disabled $p" || true
done
sh_ 'svc power stayon true; settings put system screen_off_timeout 2147483647; settings put secure lockscreen.disabled 1' || true
sh_ 'locksettings set-disabled true' >/dev/null 2>&1 || true
sh_ 'settings put global package_verifier_enable 0; settings put global verifier_verify_adb_installs 0' || true
if ((SDK >= 31)); then
  # Android 12+ kills "phantom" child processes of apps (Termux's sshd, the lindroids server).
  sh_ 'device_config set_sync_disabled_for_tests persistent; device_config put activity_manager max_phantom_processes 2147483647; settings put global settings_enable_monitor_phantom_procs false' || true
fi
home=$(sh_ 'cmd package query-activities --brief -a android.intent.action.MAIN -c android.intent.category.HOME 2>/dev/null' | grep -E '^ *[a-z].*/' | grep -iE 'launcher3|quickstep' | head -1 | tr -d ' ' || true)
if [[ -n $home ]]; then
  sh_ "cmd package set-home-activity $home" >/dev/null 2>&1 || sh_ "pm set-home-activity $home" >/dev/null 2>&1 || true
  log "home app $home"
fi
sh_ 'input keyevent 224; input keyevent 82; input keyevent 3' || true

# --- Termux
if ! sh_ pm list packages com.termux | grep -q 'package:com.termux$'; then
  log "installing Termux"
  adb_ install -r -g "$TERMUX_APK" | tail -1
fi
TUID=$(sh_ 'dumpsys package com.termux' | sed -n 's/.*userId=\([0-9]*\).*/\1/p' | head -1)
log "Termux uid $TUID"
echo "$TUID" >"$RUN/termux-uid"
# Termux sshd accepts any user name; the u0_aNNN form keeps logs readable.
echo "u0_a$((TUID - 10000))" >"$RUN/termux-user"
sh_ "appops set com.termux SYSTEM_ALERT_WINDOW allow" || true
((SDK >= 33)) && sh_ "pm grant com.termux android.permission.POST_NOTIFICATIONS" >/dev/null 2>&1 || true
sh_ "dumpsys deviceidle whitelist +com.termux" >/dev/null 2>&1 || true

# Already provisioned (sshd answers)? Then only make sure sshd runs.
if timeout 15 "$VM" "$NAME" ssh true 2>/dev/null; then
  log "ssh already works"
  exit 0
fi
# Provisioned before, but rebooted since: sshd is not running. Type `sshd` into Termux.
if [[ $(sh_ 'tail -n 1 /data/data/com.termux/files/home/lab-setup.log' 2>/dev/null) == LAB-SETUP-DONE ]]; then
  log "already provisioned; starting sshd"
  for try in 1 2 3; do
    sh_ 'am start -n com.termux/.app.TermuxActivity' >/dev/null
    sleep $((3 * try))
    "$VM" "$NAME" adb shell "input text 'sshd'"
    "$VM" "$NAME" adb shell input keyevent 66
    for i in $(seq 1 10); do
      timeout 15 "$VM" "$NAME" ssh true 2>/dev/null && { log "ssh works"; exit 0; }
      sleep 2
    done
  done
  echo "sshd did not come up" >&2
  exit 1
fi

log "starting Termux (bootstrap)"
sh_ 'am start -n com.termux/.app.TermuxActivity' >/dev/null
t0=$SECONDS
while ! sh_ 'ls /data/data/com.termux/files/usr/bin/bash' 2>/dev/null | grep -q bash; do
  ((SECONDS - t0 < 300)) || { "$VM" "$NAME" screenshot "$RUN/bootstrap-fail.png"; echo "bootstrap did not finish" >&2; exit 1; }
  sleep 3
done
log "bootstrap extracted after $((SECONDS - t0)) s"
sleep 8   # the first prompt appears after the bootstrap's second stage

# --- the setup script, served from the host
WWW=$RUN/www
mkdir -p "$WWW"
cp "$PUBKEY" "$WWW/k"
cat >"$WWW/s" <<EOF
#!/data/data/com.termux/files/usr/bin/sh
# Lindroids lab setup (served by lab/provision.sh). Output goes to ~/lab-setup.log, whose last line
# (LAB-SETUP-DONE / LAB-SETUP-FAILED) the host reads over root adb.
step() {
set -ex
echo "deb $TERMUX_MIRROR/termux-main stable main" > \$PREFIX/etc/apt/sources.list
export DEBIAN_FRONTEND=noninteractive
A="-y -o Dpkg::Options::=--force-confnew"
apt-get update
# full-upgrade, not upgrade: the old bootstrap's apt needs new libraries (liblz4, openssl 3) and a
# plain upgrade leaves apt and curl unable to start.
apt-get \$A full-upgrade
apt-get \$A install x11-repo
[ -d \$PREFIX/etc/apt/sources.list.d ] && echo "deb $TERMUX_MIRROR/termux-x11 x11 main" > \$PREFIX/etc/apt/sources.list.d/x11.list
apt-get update
apt-get \$A install openssh foot
mkdir -p ~/.ssh ~/.termux
curl -fsS http://10.0.2.2:$HTTP_PORT/k > ~/.ssh/authorized_keys
chmod 700 ~/.ssh; chmod 600 ~/.ssh/authorized_keys
pgrep sshd >/dev/null || sshd
id > ~/lab-id
}
if ( step ) > ~/lab-setup.log 2>&1; then echo LAB-SETUP-DONE >> ~/lab-setup.log; else echo LAB-SETUP-FAILED >> ~/lab-setup.log; fi
tail -n 25 ~/lab-setup.log
EOF

pkill -f "http.server $HTTP_PORT --bind 127.0.0.1" 2>/dev/null || true
(cd "$WWW" && exec python3 -m http.server "$HTTP_PORT" --bind 127.0.0.1 >"$RUN/http.log" 2>&1) &
HTTP_PID=$!
trap 'kill $HTTP_PID 2>/dev/null || true' EXIT
sleep 1

type_setup() {
  sh_ 'am start -n com.termux/.app.TermuxActivity' >/dev/null
  sleep 2
  # Quoting: double quotes for the host, single quotes for the device shell, so | stays literal
  # for `input text` instead of making the (root) device shell build a pipeline. %s is a space.
  "$VM" "$NAME" adb shell "input text 'curl%s-fsS%s10.0.2.2:$HTTP_PORT/s|sh'"
  "$VM" "$NAME" adb shell input keyevent 66
}

log "typing the setup command into Termux"
type_setup
t0=$SECONDS
tries=1
SETUP_LOG=/data/data/com.termux/files/home/lab-setup.log
until timeout 15 "$VM" "$NAME" ssh true 2>/dev/null; do
  if [[ $(sh_ "tail -n 1 $SETUP_LOG" 2>/dev/null) == LAB-SETUP-FAILED ]]; then
    sh_ "cat $SETUP_LOG" >"$RUN/lab-setup.log" || true
    "$VM" "$NAME" screenshot "$RUN/setup-fail.png" >/dev/null
    tail -n 20 "$RUN/lab-setup.log" >&2
    echo "the setup script failed; see $RUN/lab-setup.log" >&2
    exit 1
  fi
  if ((SECONDS - t0 > 900)); then
    "$VM" "$NAME" screenshot "$RUN/setup-fail.png"
    echo "setup did not finish (see $RUN/setup-fail.png)" >&2
    exit 1
  fi
  # Nothing fetched the script after 60 s: the prompt was not ready; type it again.
  if ((SECONDS - t0 > 60 * tries)) && ! grep -q 'GET /s' "$RUN/http.log" && ((tries < 4)); then
    log "script not fetched yet, typing again"
    tries=$((tries + 1))
    type_setup
  fi
  sleep 10
done
sh_ "cat $SETUP_LOG" >"$RUN/lab-setup.log" 2>/dev/null || true
log "ssh works after $((SECONDS - t0)) s: $("$VM" "$NAME" ssh 'id -un; uname -m' | tr '\n' ' ')"
"$VM" "$NAME" screenshot "$RUN/provisioned.png" >/dev/null
