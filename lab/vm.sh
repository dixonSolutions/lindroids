#!/usr/bin/env bash
# Headless QEMU/KVM VMs for the Lindroids Android test lab.
#
#   lab/vm.sh <name> start [--fresh]   boot (in the background); --fresh recreates the data disk
#   lab/vm.sh <name> stop              clean power-off (adb reboot -p), then QMP quit
#   lab/vm.sh <name> status            running? booted? getenforce, Android version
#   lab/vm.sh <name> adb <args...>     adb against this VM (connects first)
#   lab/vm.sh <name> ssh <cmd...>      run a command in Termux over ssh (after provision.sh)
#   lab/vm.sh <name> scp <src> <dst>   copy into Termux (dst relative to Termux $HOME)
#   lab/vm.sh <name> screenshot <png>  adb screencap (falls back to QMP screendump)
#   lab/vm.sh <name> wait-boot [secs]  wait for sys.boot_completed=1
#   lab/vm.sh <name> console '<cmd>'   run a command on the serial console (root shell on x86 images)
#   lab/vm.sh list                     every lab VM and whether it runs
#
# Environment knobs (start):
#   SELINUX=enforcing|permissive|default  enforcing adds androidboot.selinux=enforcing and drops
#                                         any androidboot.selinux=permissive (default: default)
#   GPU=gl|virtio|std                     gl = virtio-vga-gl + egl-headless (virgl), default
#   EXTRA="k=v ..."                       extra kernel cmdline
#   RAM_MB=3072 CPUS=4
#
# The lead's VM (libvirt "xtbliss", adb 127.0.0.1:5555 / emulator-5554) is never touched: every
# adb call here names its own -s 127.0.0.1:<port>.
set -euo pipefail

LAB_HOME=${LAB_HOME:-$HOME/VMs/lindroids-lab}
LAB_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ADB=${ADB:-$HOME/Android/Sdk/platform-tools/adb}
[[ -x $ADB ]] || ADB=$HOME/.local/opt/platform-tools/adb
KEY=${LAB_KEY:-$HOME/Projects/lindroids/.scratch/lab_key}
MAX_VMS=${MAX_VMS:-2}
RESERVE_MB=${RESERVE_MB:-6144}   # host MemAvailable that must remain after a VM's RAM

# Boxes' QEMU has virgl (virtio-vga-gl); linuxbrew's does not.
QEMU_FLATPAK=(flatpak run --command=qemu-system-x86_64 org.gnome.Boxes)
QEMU_BREW=/home/linuxbrew/.linuxbrew/bin/qemu-system-x86_64

# name | adb port | ssh port (-> guest 8022) | kernel cmdline (SRC= finds the ISO on the cdrom)
vm_conf() {
  case $1 in
    a7)  echo "5601|8601|root=/dev/ram0 androidboot.selinux=permissive buildvariant=userdebug quiet SRC= DATA=vda" ;;
    a9)  echo "5603|8603|root=/dev/ram0 quiet SRC= DATA=vda" ;;
    b15) echo "5605|8605|root=/dev/ram0 quiet SRC= DATA=vda HWC=drm_minigbm GRALLOC=minigbm_arcvm SETUPWIZARD=0" ;;
    b16) echo "5607|8607|root=/dev/ram0 quiet SRC= DATA=vda HWC=drm_minigbm GRALLOC=minigbm_arcvm SETUPWIZARD=0" ;;
    *) return 1 ;;
  esac
}
ALL_VMS=(a7 a9 b15 b16)

die() { echo "vm.sh: $*" >&2; exit 1; }

load() {
  NAME=$1
  local c
  c=$(vm_conf "$NAME") || die "unknown VM $NAME (one of: ${ALL_VMS[*]})"
  IFS='|' read -r ADB_PORT SSH_PORT CMDLINE <<<"$c"
  IMG=$LAB_HOME/images/$NAME
  RUN=$LAB_HOME/run/$NAME
  DISK=$LAB_HOME/disks/$NAME-data.img
  SERIAL=127.0.0.1:$ADB_PORT
}

# Host pid of the VM's QEMU. Not the -pidfile: Boxes' QEMU runs in a flatpak pid namespace.
vm_pid() { pgrep -f "^(/app/bin/|/home/linuxbrew/[^ ]*/)?qemu-system-x86_64 -name lindroids-lab-$1 " | head -1; }
pid_of() { vm_pid "$NAME"; }
running() { pid_of >/dev/null; }

qmp() {
  # One QMP command over the unix socket, e.g. qmp '{"execute":"quit"}'.
  python3 - "$RUN/qmp.sock" "$1" <<'EOF'
import json, socket, sys
s = socket.socket(socket.AF_UNIX); s.settimeout(10); s.connect(sys.argv[1])
f = s.makefile("rw")
f.readline()
f.write('{"execute":"qmp_capabilities"}\n'); f.flush(); f.readline()
f.write(sys.argv[2] + "\n"); f.flush()
while True:
    l = f.readline()
    if not l: break
    if '"return"' in l or '"error"' in l:
        print(l.strip()); break
EOF
}

console() {
  # Run a command on the serial console (Android-x86 gives a root shell there) and print its output.
  python3 - "$RUN/serial.sock" "$1" "${2:-4}" <<'EOF'
import socket, sys, time
s = socket.socket(socket.AF_UNIX); s.connect(sys.argv[1]); s.settimeout(0.3)
s.sendall(b"\n" + sys.argv[2].encode() + b"\n")
out, end = b"", time.time() + float(sys.argv[3])
while time.time() < end:
    try:
        d = s.recv(65536)
        if not d: break
        out += d
    except socket.timeout:
        pass
sys.stdout.write(out.decode(errors="replace"))
EOF
}

adb_tcp_via_console() {
  # Android-x86 images do not always listen for adb on TCP: switch adbd to 5555 from the console.
  console 'setprop persist.adb.tcp.port 5555; setprop service.adb.tcp.port 5555; stop adbd; start adbd' 3 >/dev/null 2>&1 || true
}

adbc() {
  # adb against this VM only; (re)connect first — adbd restarts on `adb root`.
  if ! "$ADB" -s "$SERIAL" get-state >/dev/null 2>&1; then
    "$ADB" connect "$SERIAL" >/dev/null 2>&1 || true
    local i
    for i in 1 2 3 4 5; do "$ADB" -s "$SERIAL" get-state >/dev/null 2>&1 && break; sleep 1; done
  fi
  "$ADB" -s "$SERIAL" "$@"
}

count_running() {
  local n=0 v
  for v in "${ALL_VMS[@]}"; do [[ -n $(vm_pid "$v") ]] && n=$((n + 1)); done
  echo $n
}

make_disk() {
  mkdir -p "$(dirname "$DISK")"
  rm -f "$DISK"
  truncate -s 8G "$DISK"
  # Pre-formatted so the initrd mounts it as /data (DATA=vda). Conservative features: the 7.1 kernel
  # is 4.9 and cannot mount orphan_file / csum_seed filesystems read-write.
  mkfs.ext4 -q -F -L lab-data -O ^orphan_file,^metadata_csum_seed,^metadata_csum,^64bit "$DISK" 2>/dev/null ||
    mkfs.ext4 -q -F -L lab-data -O ^metadata_csum,^64bit "$DISK"
}

cmd_start() {
  local fresh=0
  [[ ${1:-} == --fresh ]] && fresh=1
  running && { echo "$NAME already running (pid $(pid_of))"; return 0; }
  [[ -s $IMG/kernel && -e $IMG/android.iso ]] || die "$NAME: run lab/fetch-images.sh $NAME first"
  local n avail ram=${RAM_MB:-3072}
  n=$(count_running)
  ((n < MAX_VMS)) || die "already $n lab VMs running (max $MAX_VMS); stop one first"
  avail=$(awk '/MemAvailable/ {print int($2/1024)}' /proc/meminfo)
  ((avail - ram >= RESERVE_MB)) || die "host MemAvailable ${avail} MiB - ${ram} MiB < reserve ${RESERVE_MB} MiB"
  if ((fresh)) || [[ ! -f $DISK ]]; then make_disk; fi
  mkdir -p "$RUN"

  local cmdline=$CMDLINE sel=${SELINUX:-default}
  if [[ $sel == enforcing ]]; then
    cmdline=$(sed -E 's/(^| )androidboot\.selinux=[a-z]+//g' <<<"$cmdline")
    cmdline+=" androidboot.selinux=enforcing"
  elif [[ $sel == permissive ]]; then
    cmdline=$(sed -E 's/(^| )androidboot\.selinux=[a-z]+//g' <<<"$cmdline")
    cmdline+=" androidboot.selinux=permissive"
  fi
  cmdline+=" console=ttyS0,115200 ${EXTRA:-}"
  echo "$sel" >"$RUN/selinux-requested"
  echo "$cmdline" >"$RUN/cmdline"

  local gpu=${GPU:-gl} qemu display
  case $gpu in
    gl) qemu=("${QEMU_FLATPAK[@]}"); display=(-device virtio-vga-gl -display egl-headless,rendernode=/dev/dri/renderD128) ;;
    virtio) qemu=("${QEMU_FLATPAK[@]}"); display=(-device virtio-vga -display none) ;;
    std) qemu=("${QEMU_FLATPAK[@]}"); display=(-vga std -display none) ;;
    *) die "GPU=$gpu?" ;;
  esac
  echo "$gpu" >"$RUN/gpu"

  # -daemonize keeps QEMU alive after this script (and the flatpak launcher) exit.
  "${qemu[@]}" -name "lindroids-lab-$NAME" \
    -machine q35,accel=kvm -cpu host -smp "${CPUS:-4}" -m "$ram" \
    -kernel "$IMG/kernel" -initrd "$IMG/initrd.img" -append "$cmdline" \
    -drive "file=$DISK,format=raw,if=virtio,cache=writeback,discard=unmap" \
    -drive "file=$IMG/android.iso,media=cdrom,readonly=on" \
    -netdev "user,id=n0,hostfwd=tcp:127.0.0.1:$ADB_PORT-:5555,hostfwd=tcp:127.0.0.1:$SSH_PORT-:8022" \
    -device virtio-net-pci,netdev=n0 \
    -device qemu-xhci -device usb-tablet -device usb-kbd \
    "${display[@]}" \
    -chardev "socket,id=ser0,path=$RUN/serial.sock,server=on,wait=off,logfile=$RUN/console.log" -serial chardev:ser0 \
    -qmp "unix:$RUN/qmp.sock,server=on,wait=off" \
    -pidfile "$RUN/qemu.pid" -daemonize
  echo "$NAME started (pid $(pid_of)), adb $SERIAL, ssh 127.0.0.1:$SSH_PORT, gpu $gpu, selinux=$sel"
}

cmd_wait_boot() {
  local t=${1:-300} t0=$SECONDS kicked=0
  while ((SECONDS - t0 < t)); do
    running || die "$NAME is not running (see $RUN/console.log)"
    if ((!kicked && SECONDS - t0 > 45)) && ! timeout 5 "$ADB" -s "$SERIAL" get-state >/dev/null 2>&1; then
      adb_tcp_via_console
      kicked=1
    fi
    if [[ $(adbc shell getprop sys.boot_completed 2>/dev/null | tr -d '\r') == 1 ]]; then
      echo "$NAME booted in $((SECONDS - t0)) s"
      return 0
    fi
    sleep 3
  done
  die "$NAME did not finish booting within $t s"
}

cmd_stop() {
  running || { echo "$NAME not running"; rm -f "$RUN/qemu.pid"; return 0; }
  # Flush /data first: the data disk is the only state that persists.
  timeout 15 "$ADB" -s "$SERIAL" shell 'sync; reboot -p' >/dev/null 2>&1 || timeout 10 "$ADB" -s "$SERIAL" shell sync >/dev/null 2>&1 || true
  local i
  for i in $(seq 1 30); do running || break; sleep 1; done
  if running; then qmp '{"execute":"quit"}' >/dev/null 2>&1 || kill "$(pid_of)"; fi
  for i in $(seq 1 10); do running || break; sleep 1; done
  running && kill -9 "$(pid_of)"
  "$ADB" disconnect "$SERIAL" >/dev/null 2>&1 || true
  rm -f "$RUN/qemu.pid"
  echo "$NAME stopped"
}

cmd_status() {
  if ! running; then echo "$NAME: stopped"; return 0; fi
  local boot sel rel sdk
  timeout 10 "$ADB" connect "$SERIAL" >/dev/null 2>&1 || true
  boot=$(timeout 10 "$ADB" -s "$SERIAL" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r' || true)
  sel=$(timeout 10 "$ADB" -s "$SERIAL" shell getenforce 2>/dev/null | tr -d '\r' || true)
  rel=$(timeout 10 "$ADB" -s "$SERIAL" shell getprop ro.build.version.release 2>/dev/null | tr -d '\r' || true)
  sdk=$(timeout 10 "$ADB" -s "$SERIAL" shell getprop ro.build.version.sdk 2>/dev/null | tr -d '\r' || true)
  echo "$NAME: running pid $(pid_of) adb $SERIAL ssh :$SSH_PORT boot_completed=${boot:-?} android=${rel:-?} sdk=${sdk:-?} selinux=${sel:-?} (requested $(cat "$RUN/selinux-requested" 2>/dev/null)) gpu=$(cat "$RUN/gpu" 2>/dev/null)"
}

ssh_user() {
  # Termux sshd accepts any user name; use the Termux uid's name when known, for readable logs.
  cat "$RUN/termux-user" 2>/dev/null || echo termux
}

cmd_ssh() {
  exec ssh -q -p "$SSH_PORT" -i "$KEY" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
    -o ConnectTimeout=10 -o ServerAliveInterval=15 -o LogLevel=ERROR "$(ssh_user)@127.0.0.1" "$@"
}

cmd_scp() {
  exec scp -q -P "$SSH_PORT" -i "$KEY" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
    -o ConnectTimeout=10 -o LogLevel=ERROR "$1" "$(ssh_user)@127.0.0.1:$2"
}

cmd_screenshot() {
  local out=$1
  if adbc exec-out screencap -p >"$out.tmp" 2>/dev/null && [[ $(head -c 4 "$out.tmp" | od -An -c | tr -d ' ') == *PNG* ]]; then
    mv "$out.tmp" "$out"
  else
    rm -f "$out.tmp"
    qmp "{\"execute\":\"screendump\",\"arguments\":{\"filename\":\"$out\",\"format\":\"png\"}}" >/dev/null
  fi
  echo "$out"
}

cmd_list() {
  local v
  for v in "${ALL_VMS[@]}"; do (load "$v"; if running; then echo "$v running pid $(pid_of) adb $SERIAL ssh :$SSH_PORT"; else echo "$v stopped"; fi); done
}

[[ ${1:-} == list ]] && { cmd_list; exit 0; }
[[ $# -ge 2 ]] || { sed -n '2,24p' "$0"; exit 2; }
load "$1"
action=$2
shift 2
case $action in
  start) cmd_start "$@" ;;
  stop) cmd_stop ;;
  status) cmd_status ;;
  wait-boot) cmd_wait_boot "$@" ;;
  adb) adbc "$@" ;;
  ssh) cmd_ssh "$@" ;;
  scp) cmd_scp "$@" ;;
  screenshot) cmd_screenshot "$@" ;;
  qmp) qmp "$1" ;;
  ports) echo "$ADB_PORT $SSH_PORT" ;;
  console) console "$1" "${2:-4}" ;;
  *) die "unknown action $action" ;;
esac
