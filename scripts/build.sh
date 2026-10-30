#!/usr/bin/env bash
# Build this checkout into an installable .deb: a dev build to try on a phone or a desktop.
# scripts/use-release.sh goes back to an official release.
#
#   scripts/build.sh [--target <arch>] [--device [serial]] [--install] [--push] [--out <dir>]
#
# Where it runs and what it builds:
#   in Termux             this device's Termux package (cargo, natively): `pkg install rust clang git`
#   on a PC, --device     that adb device's Termux package (cargo-ndk and the Android NDK)
#   on a PC, --target     Termux: aarch64 arm x86_64 i686 (cargo-ndk);
#                         Debian, Ubuntu, proot distros: arm64 armhf amd64 i386 (static musl)
#   on a PC, neither      this PC's Debian package
#
#   --install   install it here (Termux, or a Debian/Ubuntu PC with sudo)
#   --push      (--device) copy it to the device's Download folder and print the command that
#               installs it in Termux (Termux needs `termux-setup-storage` once to read Download)
#   --out       where the .deb goes (default target/dev-build)
#
# The package is the `lindroids` package, version <release version>~dev.<commit>[.dirty]: it
# installs over a release, `lindroids version` says `+<commit>-dev`, and the release with the
# same number (which sorts after it) replaces it on a later `pkg upgrade`.
# xwayland-satellite (X11 programs) is taken from target/xwls/out when
# scripts/build-xwayland-satellite.sh made it, else from the latest release's package.
#
# Environment: ANDROID_NDK_HOME for cargo-ndk (the NDK zip alone, unpacked; default the newest in
# ~/Android/Sdk/ndk); ADB (default adb, else ~/Android/Sdk/platform-tools/adb);
# LINDROIDS_GITHUB_REPO (default dixonSolutions/lindroids).
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD

TARGET='' DEVICE='' SERIAL='' INSTALL=0 PUSH=0 OUT=$ROOT/target/dev-build
while [ $# -gt 0 ]; do
  case "$1" in
    --target) TARGET=$2; shift 2 ;;
    --device)
      DEVICE=1; shift
      if [ $# -gt 0 ] && [ "${1#-}" = "$1" ]; then SERIAL=$1; shift; fi ;;
    --install) INSTALL=1; shift ;;
    --push) PUSH=1; shift ;;
    --out) OUT=$(realpath -m "$2"); shift 2 ;;
    -h|--help) awk 'NR > 1 && !/^#/ {exit} NR > 1 {sub(/^# ?/, ""); print}' "$0"; exit 0 ;;
    *) echo "build: unknown argument $1 (--help)" >&2; exit 2 ;;
  esac
done
die() { echo "build: $*" >&2; exit 1; }

IN_TERMUX=0
if [ -n "${TERMUX_VERSION:-}" ] || [ "${PREFIX:-}" = /data/data/com.termux/files/usr ]; then IN_TERMUX=1; fi
GITHUB_REPO=${LINDROIDS_GITHUB_REPO:-dixonSolutions/lindroids}

adb_() {
  local adb=${ADB:-}
  if [ -z "$adb" ]; then
    adb=$(command -v adb || true)
    [ -n "$adb" ] || adb=${ANDROID_HOME:-$HOME/Android/Sdk}/platform-tools/adb
  fi
  [ -x "$adb" ] || command -v "$adb" >/dev/null || die "no adb: set ADB or install platform-tools"
  "$adb" ${SERIAL:+-s "$SERIAL"} "$@"
}

if [ -n "$DEVICE" ]; then
  [ $IN_TERMUX = 0 ] || die "--device is for a PC; in Termux, leave it out"
  [ -z "$TARGET" ] || die "--device and --target: one or the other"
  abi=$(adb_ shell getprop ro.product.cpu.abi | tr -d '\r')
  case "$abi" in
    arm64-v8a) TARGET=aarch64 ;;
    armeabi-v7a|armeabi) TARGET=arm ;;
    x86_64) TARGET=x86_64 ;;
    x86) TARGET=i686 ;;
    *) die "the device's ABI is '$abi' (is one device attached and authorized? adb devices)" ;;
  esac
  echo "==> device $(adb_ get-serialno | tr -d '\r'): $abi"
elif [ $IN_TERMUX = 1 ]; then
  [ -z "$TARGET" ] || die "--target is for a PC; in Termux this device's package is built"
  case "$(uname -m)" in
    aarch64) TARGET=aarch64 ;;
    armv7l|armv8l) TARGET=arm ;;
    x86_64) TARGET=x86_64 ;;
    i686|i386) TARGET=i686 ;;
    *) die "unknown CPU $(uname -m)" ;;
  esac
elif [ -z "$TARGET" ]; then
  case "$(uname -m)" in
    x86_64) TARGET=amd64 ;;
    aarch64) TARGET=arm64 ;;
    armv7l) TARGET=armhf ;;
    i686) TARGET=i386 ;;
    *) die "unknown CPU $(uname -m): pass --target" ;;
  esac
fi
[ $PUSH = 0 ] || [ -n "$DEVICE" ] || die "--push needs --device"

# <deb arch> -> flavour, rust triple, Android ABI (cargo-ndk)
case "$TARGET" in
  aarch64) FLAVOUR=termux TRIPLE=aarch64-linux-android ABI=arm64-v8a ;;
  arm) FLAVOUR=termux TRIPLE=armv7-linux-androideabi ABI=armeabi-v7a ;;
  x86_64) FLAVOUR=termux TRIPLE=x86_64-linux-android ABI=x86_64 ;;
  i686) FLAVOUR=termux TRIPLE=i686-linux-android ABI=x86 ;;
  arm64) FLAVOUR=debian TRIPLE=aarch64-unknown-linux-musl ;;
  armhf) FLAVOUR=debian TRIPLE=armv7-unknown-linux-musleabihf ;;
  amd64) FLAVOUR=debian TRIPLE=x86_64-unknown-linux-musl ;;
  i386) FLAVOUR=debian TRIPLE=i686-unknown-linux-musl ;;
  *) die "unknown target '$TARGET': aarch64 arm x86_64 i686 (Termux), arm64 armhf amd64 i386 (Debian)" ;;
esac
for tool in cargo dpkg-deb git; do
  command -v $tool >/dev/null || die "$tool is required$([ $IN_TERMUX = 1 ] && echo ": pkg install rust git dpkg")"
done

# The version: the release this commit would be, marked as a dev build of it.
eval "$(scripts/version.sh)"
SHA=$(git rev-parse --short HEAD)
DIRTY=
git diff --quiet HEAD 2>/dev/null || DIRTY=.dirty
DEB_VERSION=$LINDROIDS_VERSION~dev.$SHA$DIRTY
export LINDROIDS_VERSION LINDROIDS_BUILD_ID=$SHA-dev${DIRTY:+-dirty}
echo "==> lindroids $DEB_VERSION ($FLAVOUR $TARGET)"

TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT/target}
if [ $IN_TERMUX = 1 ]; then
  cargo build -p lindroids --release
  BIN=$TARGET_DIR/release/lindroids
elif [ $FLAVOUR = termux ]; then
  command -v cargo-ndk >/dev/null || die "cargo-ndk is required: cargo install cargo-ndk; rustup target add $TRIPLE"
  : "${ANDROID_NDK_HOME:=$(ls -d "${ANDROID_HOME:-$HOME/Android/Sdk}"/ndk/* 2>/dev/null | sort -V | tail -1)}"
  [ -f "${ANDROID_NDK_HOME:-/nonexistent}/source.properties" ] || die "no Android NDK: set ANDROID_NDK_HOME (android-ndk-r27c-linux.zip, unpacked)"
  export ANDROID_NDK_HOME
  cargo ndk -t "$ABI" -P 24 build -p lindroids --release
  BIN=$TARGET_DIR/$TRIPLE/release/lindroids
else
  cargo build -p lindroids --release --target "$TRIPLE"
  BIN=$TARGET_DIR/$TRIPLE/release/lindroids
fi
[ -x "$BIN" ] || die "no binary at $BIN"

# packaging/build-debs.sh takes <dist>/bin/<triple>/lindroids and the xwayland-satellite layout.
WORK=$TARGET_DIR/dev-build-work
rm -rf "$WORK"
mkdir -p "$WORK/bin/$TRIPLE" "$OUT"
cp "$BIN" "$WORK/bin/$TRIPLE/lindroids"

XWLS=$ROOT/target/xwls/out
if [ ! -f "$XWLS/$FLAVOUR-$TARGET/xwayland-satellite" ] || [ ! -f "$XWLS/LICENSE.xwayland-satellite" ]; then
  XWLS=$WORK/xwls
  command -v curl >/dev/null || die "curl is required (for xwayland-satellite from the latest release)"
  latest=$(curl -fsSL "https://dixonsolutions.github.io/lindroids/VERSION") || die "cannot fetch the latest version"
  deb=lindroids_${latest}_$TARGET.deb
  echo "==> xwayland-satellite from the $latest release"
  curl -fsSL -o "$WORK/$deb" "https://github.com/$GITHUB_REPO/releases/download/v$latest/$deb" || die "cannot download $deb"
  curl -fsSL -o "$WORK/SHA256SUMS" "https://github.com/$GITHUB_REPO/releases/download/v$latest/SHA256SUMS" || die "cannot download SHA256SUMS"
  # SHA256SUMS names it debs/<deb>
  sum=$(awk -v f="$deb" '$2 == f || $2 == "debs/" f {print $1}' "$WORK/SHA256SUMS")
  [ -n "$sum" ] && [ "$(sha256sum "$WORK/$deb" | cut -d' ' -f1)" = "$sum" ] || die "$deb does not match SHA256SUMS"
  dpkg-deb -x "$WORK/$deb" "$WORK/release"
  prefix=usr
  [ $FLAVOUR = termux ] && prefix=data/data/com.termux/files/usr
  mkdir -p "$XWLS/$FLAVOUR-$TARGET"
  cp "$WORK/release/$prefix/libexec/lindroids/xwayland-satellite" "$XWLS/$FLAVOUR-$TARGET/"
  cp "$WORK/release/$prefix/share/doc/lindroids/"{LICENSE.xwayland-satellite,xwayland-satellite-crates.txt} "$XWLS/"
  pin=$(sed -n 's/^SATELLITE_VERSION=//p' scripts/build-xwayland-satellite.sh)
  grep -q "xwayland-satellite $pin " "$WORK/release/$prefix/share/doc/lindroids/copyright" ||
    echo "build: note: this checkout pins xwayland-satellite $pin, the release ships another; the release's is used" >&2
fi

packaging/build-debs.sh --version "$DEB_VERSION" --dist "$WORK" --xwls "$XWLS" --out "$OUT" --arches "$TARGET" >/dev/null
DEB=$OUT/lindroids_${DEB_VERSION}_$TARGET.deb
[ -f "$DEB" ] || die "packaging/build-debs.sh made no $DEB"
rm -rf "$WORK"
echo "==> $DEB"

# An idle server is stopped so the next program starts the new build; one with windows is left.
restart_idle() {
  local st
  st=$(lindroids status 2>/dev/null) || return 0
  if printf '%s\n' "$st" | grep -q '^window '; then
    echo "The server has windows open: close them and run \`lindroids stop\` to use the new build."
  elif printf '%s\n' "$st" | grep -q '^server '; then
    lindroids stop >/dev/null
  fi
}

if [ $PUSH = 1 ]; then
  adb_ push "$DEB" /sdcard/Download/ >/dev/null
  echo
  echo "Copied to the device's Download folder. In Termux:"
  echo "  apt-get install --reinstall --allow-downgrades -y /sdcard/Download/$(basename "$DEB") && lindroids stop"
  echo "Back to a release: scripts/use-release.sh (or: apt-get install --reinstall --allow-downgrades lindroids=<version>)"
elif [ $INSTALL = 1 ]; then
  if [ $IN_TERMUX = 1 ]; then
    apt-get install --reinstall --allow-downgrades -y "$DEB"
  elif [ $FLAVOUR = debian ] && command -v apt-get >/dev/null; then
    sudo=; [ "$(id -u)" = 0 ] || sudo=sudo
    $sudo apt-get install --reinstall --allow-downgrades -y "$DEB"
  else
    die "--install installs in Termux or on a Debian/Ubuntu PC; for a phone use --device --push"
  fi
  lindroids version
  restart_idle
  echo "Back to a release: scripts/use-release.sh"
fi
