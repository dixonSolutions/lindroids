#!/usr/bin/env bash
# Test install.sh in a lab VM's Termux the way a user runs it (`curl .../install.sh | sh`), against
# a repository built here from the Termux packages given, signed with a throwaway key and served
# to the VM (10.0.2.2 is the host): how long it takes, that Lindroids and Flatpak (with Flathub)
# are there after it, that a second run (an update) works, and that a Flathub app installs and
# runs with `lindroids run`.
#
#   lab/test-install.sh <vm> [--debs DIR] [--flatpak yes|build|no] [--app APP-ID] [--no-update]
#
#   --debs     Termux .debs to publish (lindroids_*_x86_64.deb, lindroids-flatpak_*_x86_64.deb);
#              without a lindroids one in it, the published lindroids for x86_64 is used
#   --flatpak  LINDROIDS_FLATPAK for the installer (default yes: the package when the repository
#              has it, else built from source; build: always from source)
#   --app      a Flathub app to install and run afterwards (default none), e.g. org.gnome.Calculator
#
# Results: lab/results/<vm>/install/ (install-1.log, install-2.log with seconds since the start of
# each line, app.png, result.txt). Needs dpkg-deb, gpg, gpgv (packaging/repo/build-repo.sh) and a
# provisioned VM (lab/provision.sh); run it where those tools are (e.g. a distrobox).
set -uo pipefail

LAB_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$LAB_DIR/.." && pwd)
VM=$LAB_DIR/vm.sh
SITE_URL=${LINDROIDS_SITE_URL:-https://dixonsolutions.github.io/lindroids}

NAME=${1:?usage: test-install.sh <vm> [--debs DIR] [--flatpak yes|build|no] [--app APP-ID] [--no-update]}
shift
DEBS=
FLATPAK=yes
APP=
UPDATE=1
while [ $# -gt 0 ]; do
  case "$1" in
    --debs) DEBS=$(readlink -f "$2"); shift 2 ;;
    --flatpak) FLATPAK=$2; shift 2 ;;
    --app) APP=$2; shift 2 ;;
    --no-update) UPDATE=; shift ;;
    *) echo "test-install: unknown option $1" >&2; exit 2 ;;
  esac
done

log() { echo "[$(date +%T)] $NAME: $*"; }
sh_() { "$VM" "$NAME" adb shell "$@" | tr -d '\r'; }
tsh() { timeout "${TSH_TIMEOUT:-60}" "$VM" "$NAME" ssh "$@"; }
OUT=$REPO/lab/results/$NAME/install
rm -rf "$OUT"
mkdir -p "$OUT"
declare -A R
fail() {
  R[result]=FAIL
  R[notes]="${R[notes]:-} $*"
  log "FAIL: $*"
  report
  exit 1
}
report() {
  for k in vm android flatpak_mode install_secs lindroids flatpak flathub flatpak_from update_secs flatpak_only_secs app app_install_secs app_window result notes; do
    printf '%-18s %s\n' "$k" "${R[$k]:-}"
  done | tee "$OUT/result.txt"
}
R[vm]=$NAME
R[flatpak_mode]=$FLATPAK

tsh true || fail "no ssh into Termux (lab/provision.sh $NAME)"
R[android]="$(sh_ getprop ro.build.version.release) (sdk $(sh_ getprop ro.build.version.sdk)), $(tsh 'uname -m' | tr -d '\r')"

# --- the repository ------------------------------------------------------------------------
T=$(mktemp -d)
server=
cleanup() {
  [ -z "$server" ] || kill "$server" 2>/dev/null
  gpgconf --homedir "$T/gnupg" --kill gpg-agent >/dev/null 2>&1
  rm -rf "$T"
}
trap cleanup EXIT
mkdir -p "$T/debs" "$T/gnupg"
chmod 700 "$T/gnupg"
[ -z "$DEBS" ] || cp "$DEBS"/*.deb "$T/debs/" 2>/dev/null
if ! ls "$T/debs"/lindroids_*_x86_64.deb >/dev/null 2>&1; then
  ver=$(curl -fsS "$SITE_URL/VERSION") || fail "cannot read $SITE_URL/VERSION"
  curl -fsSL -o "$T/debs/lindroids_${ver}_x86_64.deb" "$SITE_URL/pool/termux/main/l/lindroids/lindroids_${ver}_x86_64.deb" \
    || fail "cannot download lindroids $ver for x86_64"
fi
log "packages: $(cd "$T/debs" && echo *.deb)"
PORT=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
GNUPGHOME=$T/gnupg gpg --batch --quiet --pinentry-mode loopback --passphrase '' \
  --quick-generate-key "lindroids lab <lab@example.invalid>" ed25519 sign never
GNUPGHOME=$T/gnupg gpg --batch --export > "$T/test.gpg"
env -u LINDROIDS_GPG_PRIVATE_KEY -u LINDROIDS_APK_RSA_KEY GNUPGHOME="$T/gnupg" LINDROIDS_APK_RSA_KEY_FILE=/nonexistent \
  "$REPO/packaging/repo/build-repo.sh" "$T/site" --debs "$T/debs" --pacman "$T/none" --rpm "$T/none" \
  --apk "$T/none" --xbps "$T/none" --public-key "$T/test.gpg" --base-url "http://10.0.2.2:$PORT" \
  > "$OUT/build-repo.log" 2>&1 || fail "build-repo.sh (see $OUT/build-repo.log)"
python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$T/site" > /dev/null 2>&1 &
server=$!
sleep 1

# --- install, and again (an update) ---------------------------------------------------------
# Every line with the seconds since the start; the last line is the total.
install_run() { # <log>
  TSH_TIMEOUT=3600 tsh "s=\$(date +%s); curl -fsSL http://10.0.2.2:$PORT/install.sh | LINDROIDS_FLATPAK=$FLATPAK sh 2>&1 \
    | while IFS= read -r l; do printf '[%4ds] %s\n' \$((\$(date +%s) - s)) \"\$l\"; done; echo \"TOTAL \$((\$(date +%s) - s))\"" > "$1" 2>&1
  sed -n 's/^TOTAL //p' "$1"
}
log "install.sh (LINDROIDS_FLATPAK=$FLATPAK)"
R[install_secs]=$(install_run "$OUT/install-1.log")
log "  ${R[install_secs]} s"
grep -q 'Lindroids is installed' "$OUT/install-1.log" || fail "install.sh did not finish (install-1.log)"
R[lindroids]=$(tsh 'lindroids version 2>/dev/null' | tr -d '\r')
[ -n "${R[lindroids]}" ] || fail "no lindroids after installing"
if [ "$FLATPAK" != no ]; then
  R[flatpak]=$(tsh 'flatpak --version' | tr -d '\r')
  [ -n "${R[flatpak]}" ] || fail "no flatpak after installing"
  tsh 'flatpak --user remotes' | grep -q '^flathub' && R[flathub]=yes || fail "no Flathub remote"
  if tsh 'dpkg -s lindroids-flatpak' 2>/dev/null | grep -q '^Status: install ok installed'; then
    R[flatpak_from]="package $(tsh "dpkg-query -W -f '\${Version}' lindroids-flatpak" | tr -d '\r')"
  else
    R[flatpak_from]="built here"
  fi
fi
if [ -n "$UPDATE" ]; then
  log "install.sh again (update)"
  R[update_secs]=$(install_run "$OUT/install-2.log")
  log "  ${R[update_secs]} s"
  grep -q 'Lindroids is installed' "$OUT/install-2.log" || fail "the second install.sh did not finish (install-2.log)"
fi
# `lindroids setup flatpak` (install.sh --flatpak-only), with the package removed first: it must
# come back from the repository (no apt update has run in that invocation).
if [ "$FLATPAK" = yes ] && [ "${R[flatpak_from]%% *}" = package ]; then
  log "install.sh --flatpak-only, lindroids-flatpak removed first"
  s=$(date +%s)
  TSH_TIMEOUT=1800 tsh "apt remove -y lindroids-flatpak > /dev/null 2>&1; curl -fsSL http://10.0.2.2:$PORT/install.sh | LINDROIDS_FLATPAK=yes sh -s -- --flatpak-only 2>&1" \
    > "$OUT/flatpak-only.log" 2>&1
  R[flatpak_only_secs]=$(($(date +%s) - s))
  tsh 'dpkg -s lindroids-flatpak' 2>/dev/null | grep -q '^Status: install ok installed' && tsh 'flatpak --version' > /dev/null \
    || fail "install.sh --flatpak-only did not bring lindroids-flatpak back (flatpak-only.log)"
fi

# --- a Flathub app, in a window ------------------------------------------------------------
if [ -n "$APP" ]; then
  R[app]=$APP
  log "flatpak --user install flathub $APP"
  s=$(date +%s)
  TSH_TIMEOUT=3600 tsh "flatpak --user install -y --noninteractive flathub $APP" > "$OUT/app-install.log" 2>&1 \
    || fail "flatpak install $APP (app-install.log)"
  R[app_install_secs]=$(($(date +%s) - s))
  # Termux's `am` needs the ARM translation layer off on these x86 images (lab/test-lindroids.sh).
  if [[ $(sh_ getprop ro.dalvik.vm.native.bridge) != 0 && -f ${LAB_HOME:-$HOME/VMs/lindroids-lab}/tools/magisk ]]; then
    "$VM" "$NAME" adb root >/dev/null 2>&1; sleep 2
    "$VM" "$NAME" adb push "${LAB_HOME:-$HOME/VMs/lindroids-lab}/tools/magisk" /data/local/tmp/magisk >/dev/null 2>&1
    sh_ 'cd /data/local/tmp && chmod 755 magisk && ln -sf magisk resetprop && ./resetprop ro.dalvik.vm.native.bridge 0'
  fi
  log "lindroids run $APP"
  tsh "cd ~; (nohup sh -c 'lindroids run $APP; echo RUN-EXIT=\$?' > app-run.log 2>&1 < /dev/null &); sleep 45; lindroids status" \
    > "$OUT/app-status.txt" 2>&1
  "$VM" "$NAME" screenshot "$OUT/app.png" >/dev/null 2>&1
  tsh 'cat ~/app-run.log' > "$OUT/app-run.log" 2>&1
  if grep -qE '^display +linked' "$OUT/app-status.txt" && ! grep -q RUN-EXIT "$OUT/app-run.log"; then
    R[app_window]="running, display linked (app.png)"
  else
    fail "$APP is not running with a linked display (app-status.txt, app-run.log, app.png)"
  fi
fi
R[result]=PASS
report
