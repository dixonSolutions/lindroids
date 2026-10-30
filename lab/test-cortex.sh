#!/usr/bin/env bash
# Lindroids in Cortex Terminal (Ubuntu without proot, docs/INSTALL.md): the static CLI hands over
# to lindroids-glibc, doctor sees Ubuntu, and foot gets a window in the browser.
#
#   lab/test-cortex.sh <vm> [--fresh <static> <glibc>]
#
# Needs Cortex (org.cortex.terminal, arm64 APK from github.com/ssfuisu/cortex) on the VM with its
# sshd started once, by typing in Cortex (the lab key from lab/provision.sh):
#   mkdir -p ~/.ssh && echo '<lab_key.pub>' >> ~/.ssh/authorized_keys && cortex-sshd start -p 8122
# and lindroids installed there (apt install -y curl, then the install script). --fresh copies an
# aarch64 static CLI and its glibc build (scripts/build-glibc-cli.sh) over the installed ones.
#
# Results: lab/results/<vm>/cortex/ (screenshot, result.txt).
set -uo pipefail

LAB_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
VM=$LAB_DIR/vm.sh
NAME=${1:?usage: test-cortex.sh <vm> [--fresh <static> <glibc>]}
KEY=${LAB_KEY:-$HOME/Projects/lindroids/.scratch/lab_key}
PORT=${CORTEX_SSH_PORT:-8722}
OUT=$LAB_DIR/results/$NAME/cortex
mkdir -p "$OUT"
log() { echo "[$(date +%T)] $NAME: $*" | tee -a "$OUT/result.txt"; }
cx() { timeout 120 ssh -q -p "$PORT" -i "$KEY" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null root@127.0.0.1 "$@"; }
: > "$OUT/result.txt"
FAIL=0
check() { if eval "$2"; then log "PASS $1"; else log "FAIL $1"; FAIL=1; fi; }

"$VM" "$NAME" adb forward "tcp:$PORT" tcp:8122 >/dev/null
cx true || { log "FAIL no ssh into Cortex on port $PORT (see the header)"; exit 1; }
if [ "${2:-}" = --fresh ]; then
  cx 'lindroids stop >/dev/null 2>&1; cat > /tmp/l.new && chmod 755 /tmp/l.new && mv /tmp/l.new /usr/bin/lindroids' < "$3"
  cx 'cat > /tmp/g.new && chmod 755 /tmp/g.new && mv /tmp/g.new /usr/libexec/lindroids/lindroids-glibc' < "$4"
fi
cx 'command -v foot >/dev/null || apt-get install -y -q foot >/dev/null 2>&1'

doctor=$(cx 'lindroids doctor 2>&1')
echo "$doctor" > "$OUT/doctor.txt"
check "hand-over: doctor sees Ubuntu in Cortex" 'grep -q "in Cortex" <<<"$doctor"'
check "Xwayland and satellite found" 'grep -q "^satellite *found" <<<"$doctor" && grep -q "^Xwayland *found" <<<"$doctor"'

# cortex-api open needs Cortex on screen.
"$VM" "$NAME" adb shell am start -n org.cortex.terminal/.MainActivity >/dev/null 2>&1
sleep 2
cx 'lindroids stop >/dev/null 2>&1; cd ~; (timeout 120 lindroids run foot > /tmp/lab-foot.log 2>&1 &); sleep 30'
status=$(cx 'lindroids status 2>&1; cat /tmp/lab-foot.log')
echo "$status" > "$OUT/status.txt"
check "foot has a window" 'grep -q "window .*foot" <<<"$status"'
check "a browser page is linked" 'grep -q "display *linked" <<<"$status"'
check "foot found a UTF-8 locale (LOCPATH)" '! grep -q "not a UTF-8 locale" <<<"$status"'
check "portal on D-Bus" 'cx "grep -q \"desktop portal and notifications\" \$CORTEX_ROOT/tmp/lindroids-0/server.log"'
"$VM" "$NAME" adb exec-out screencap -p > "$OUT/foot.png"
cx 'lindroids stop >/dev/null 2>&1'
[ $FAIL = 0 ] && log "OK" || log "FAILED"
exit $FAIL
