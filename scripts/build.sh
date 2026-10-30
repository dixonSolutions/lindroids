#!/usr/bin/env bash
# For development only: build this checkout into a package, to test changes before they are
# released. To install Lindroids, use install.sh (README); scripts/use-release.sh goes back from
# a dev build to an official release.
#
#   scripts/build.sh [--format deb|pacman|rpm|apk] [--target <arch>] [--device [serial]]
#                    [--install] [--push] [--out <dir>] [--yes]
#
# What it builds:
#   in Termux             this device's Termux package, built with Termux's Rust
#   in a Linux system     its own package (Debian, Ubuntu, Arch, Fedora, openSUSE, Alpine and
#                         their kin, as containers, proot distros or desktops): the static CLI
#   on a PC, --device     the adb device's Termux package (cargo-ndk and the Android NDK)
#   --target <arch>       Termux: aarch64 arm x86_64 i686; Linux: arm64 armhf amd64 i386
#
#   --format    the package: deb (apt), pacman, rpm (dnf, yum, zypper) or apk (Alpine). Default:
#               the package manager of this system (Termux: apt, or pacman when that is what it
#               has; with --device: deb). rpm and apk are made with podman or docker, or, without
#               one, natively on an rpm system or Alpine (packaging/container.sh)
#   --install   install it here, with this system's package manager
#   --push      (--device) copy it to the device's Download folder and print the command that
#               installs it (Termux needs `termux-setup-storage` once to read Download)
#   --out       where the package goes (default target/dev-build)
#   --yes       install missing build tools without asking
#
# It checks for everything the build needs and offers to install what is missing: in Termux
# with pkg; elsewhere Rust with rustup and its targets, cargo-ndk, the Android NDK r27c
# (downloaded and checked, into ~/.local/share/lindroids-dev), and the rest (a C compiler for
# build scripts, dpkg-deb, bsdtar, adb, ...) with the system's apt, dnf, yum, zypper, pacman or apk.
#
# Versions: the release this commit would be, marked as a dev build so that it installs over
# that release's predecessors and the release itself replaces it on a later upgrade:
#   deb, rpm  0.1.134~dev.<commit>[.dirty]     pacman  0.1.134dev.<commit>[.dirty]
#   apk       0.1.134_pre<commit time>         `lindroids version`: 0.1.134+<commit>-dev
# xwayland-satellite (X11 programs) is taken from target/xwls/out when
# scripts/build-xwayland-satellite.sh made it, else from the latest release's package (apk
# packages have none: Alpine's own is used).
#
# Environment: ANDROID_NDK_HOME for cargo-ndk; ADB (default adb, else
# ~/Android/Sdk/platform-tools/adb); LINDROIDS_GITHUB_REPO (default dixonSolutions/lindroids).
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD

FORMAT='' TARGET='' DEVICE='' SERIAL='' INSTALL=0 PUSH=0 YES=0 OUT=$ROOT/target/dev-build
while [ $# -gt 0 ]; do
  case "$1" in
    --yes|-y) YES=1; shift ;;
    --format) FORMAT=$2; shift 2 ;;
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
echo "Lindroids dev build: for testing changes. To install Lindroids, use install.sh (see the README)."
SUDO=''
if [ $IN_TERMUX = 0 ] && [ "$(id -u)" != 0 ]; then SUDO=sudo; fi

# --- This system's package manager: apt pacman dnf yum zypper apk xbps, or none -------------

detect_pm() {
  if [ $IN_TERMUX = 1 ]; then
    if command -v apt-get >/dev/null; then echo apt; else echo pacman; fi
    return
  fi
  local ids='' cand='' pm c
  [ -r /etc/os-release ] && ids=$(. /etc/os-release && echo " ${ID:-} ${ID_LIKE:-} ")
  case "$ids" in
    *" debian "*|*" ubuntu "*) cand=apt ;;
    *" arch "*|*" archlinux "*) cand=pacman ;;
    *suse*) cand=zypper ;;
    *" fedora "*|*" rhel "*|*" centos "*|*" amzn "*) cand="dnf yum" ;;
    *" alpine "*|*" postmarketos "*|*" chimera "*) cand=apk ;;
    *" void "*) cand=xbps ;;
  esac
  for pm in $cand apt pacman dnf yum zypper apk xbps; do
    case $pm in apt) c=apt-get ;; xbps) c=xbps-install ;; *) c=$pm ;; esac
    if command -v "$c" >/dev/null; then echo "$pm"; return; fi
  done
  echo none
}
PM=$(detect_pm)

# --- Build tools: check, and offer to install what is missing -------------------------------

# confirm <question>: yes with --yes; asks on a terminal; no otherwise.
confirm() {
  [ $YES = 1 ] && return 0
  [ -t 0 ] || return 1
  local a
  read -r -p "$1 [Y/n] " a
  case "$a" in ""|y|Y|yes|Yes) return 0 ;; *) return 1 ;; esac
}
# offer <what> <command>: run <command> (a shell line) if the user agrees, else stop.
offer() {
  echo "build: missing: $1."
  if confirm "Install it with: $2 ?"; then
    bash -c "$2" || die "installing $1 failed"
  else
    die "needed: $1. Install with: $2 (or run with --yes)"
  fi
}
# The package that has <tool>, for this system's package manager.
pkg_for() {
  if [ $IN_TERMUX = 1 ]; then
    case "$1" in cargo) echo rust ;; dpkg-deb) echo dpkg ;; xz) echo xz-utils ;; *) echo "$1" ;; esac
    return
  fi
  case "$1:$PM" in
    dpkg-deb:*) echo dpkg ;;
    readelf:*) echo binutils ;;
    bsdtar:apt|bsdtar:apk) echo libarchive-tools ;;
    bsdtar:pacman) echo libarchive ;;
    xz:apt) echo xz-utils ;;
    adb:apt) echo adb ;;
    adb:*) echo android-tools ;;
    cc:apt) echo "gcc libc6-dev" ;;
    cc:apk) echo "gcc musl-dev" ;;
    cc:*) echo gcc ;;
    *) echo "$1" ;;
  esac
}
# The command that installs <packages> with this system's package manager.
install_cmd() {
  if [ $IN_TERMUX = 1 ]; then echo "pkg install -y $1"; return; fi
  local s=${SUDO:+$SUDO }
  case $PM in
    apt) echo "${s}apt-get update -q && ${s}apt-get install -y $1" ;;
    dnf) echo "${s}dnf install -y $1" ;;
    yum) echo "${s}yum install -y $1" ;;
    zypper) echo "${s}zypper --non-interactive install --force-resolution $1" ;;
    # -S alone (a partial upgrade with -Sy is unsupported on Arch), unless there are no sync
    # databases yet, as in a fresh container
    pacman)
      if compgen -G '/var/lib/pacman/sync/*.db' >/dev/null; then echo "${s}pacman -S --needed --noconfirm $1"
      else echo "${s}pacman -Sy --needed --noconfirm $1"
      fi ;;
    apk) echo "${s}apk add -U $1" ;;
    xbps) echo "${s}xbps-install -Sy $1" ;;
  esac
}
# need <tool>...: offer the packages of those that are missing, in one go.
need() {
  local t pkgs='' missing=''
  for t in "$@"; do
    command -v "$t" >/dev/null && continue
    missing+=" $t"
    pkgs+=" $(pkg_for "$t")"
  done
  [ -n "$missing" ] || return 0
  local cmd
  cmd=$(install_cmd "${pkgs# }")
  [ -n "$cmd" ] || die "needed:$missing (packages:$pkgs); there is no package manager here to install them with"
  [ -n "$SUDO" ] && ! command -v sudo >/dev/null && die "needed:$missing; install them as root: ${cmd//sudo /}"
  missing=${missing# }
  offer "${missing// /, }" "$cmd"
  for t in "$@"; do command -v "$t" >/dev/null || die "$t is still not on the PATH"; done
}

[ -d .git ] || [ -f .git ] || die "not a git checkout: git clone https://github.com/$GITHUB_REPO"
need git curl
if [ $IN_TERMUX = 0 ]; then
  # a C compiler and linker first: Rust's build scripts are linked with cc
  need cc
  [ -x "$HOME/.cargo/bin/cargo" ] && PATH=$HOME/.cargo/bin:$PATH
  if ! command -v cargo >/dev/null; then
    offer "Rust" "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal"
    PATH=$HOME/.cargo/bin:$PATH
  fi
  command -v clang >/dev/null ||
    echo "build: note: no clang, so two small preload libraries are left out (GTK 3's file dialogs as Android's picker; xterm in x86 distros): $(install_cmd 'clang lld')"
else
  need cargo clang
fi

adb_() {
  local adb=${ADB:-}
  if [ -z "$adb" ]; then
    adb=$(command -v adb || true)
    [ -n "$adb" ] || adb=${ANDROID_HOME:-$HOME/Android/Sdk}/platform-tools/adb
    if [ ! -x "$adb" ]; then need adb; adb=adb; fi
  fi
  "$adb" ${SERIAL:+-s "$SERIAL"} "$@"
}

# The whole history: the version is the number of commits.
if [ "$(git rev-parse --is-shallow-repository)" = true ]; then
  offer "the git history (this is a shallow clone, and the version counts commits)" "git fetch --unshallow"
fi

# --- What to build -------------------------------------------------------------------------

if [ -n "$DEVICE" ]; then
  [ $IN_TERMUX = 0 ] || die "--device is for a PC; in Termux, leave it out"
  if [ -z "$TARGET" ]; then
    abi=$(adb_ shell getprop ro.product.cpu.abi | tr -d '\r')
    case "$abi" in
      arm64-v8a) TARGET=aarch64 ;;
      armeabi-v7a|armeabi) TARGET=arm ;;
      x86_64) TARGET=x86_64 ;;
      x86) TARGET=i686 ;;
      *) die "the device's ABI is '$abi' (is one device attached and authorized? adb devices)" ;;
    esac
    echo "==> device $(adb_ get-serialno | tr -d '\r'): $abi"
  fi
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
    armv7l|armv8l) TARGET=armhf ;;
    i686) TARGET=i386 ;;
    *) die "unknown CPU $(uname -m): pass --target" ;;
  esac
fi
[ $PUSH = 0 ] || [ -n "$DEVICE" ] || die "--push needs --device"

# <deb arch> -> flavour, rust triple, Android ABI (cargo-ndk), rpm and apk arch
case "$TARGET" in
  aarch64) FLAVOUR=termux TRIPLE=aarch64-linux-android ABI=arm64-v8a ;;
  arm) FLAVOUR=termux TRIPLE=armv7-linux-androideabi ABI=armeabi-v7a ;;
  x86_64) FLAVOUR=termux TRIPLE=x86_64-linux-android ABI=x86_64 ;;
  i686) FLAVOUR=termux TRIPLE=i686-linux-android ABI=x86 ;;
  arm64) FLAVOUR=debian TRIPLE=aarch64-unknown-linux-musl RPM_ARCH=aarch64 APK_ARCH=aarch64 ;;
  armhf) FLAVOUR=debian TRIPLE=armv7-unknown-linux-musleabihf RPM_ARCH=armv7hl APK_ARCH=armv7 ;;
  amd64) FLAVOUR=debian TRIPLE=x86_64-unknown-linux-musl RPM_ARCH=x86_64 APK_ARCH=x86_64 ;;
  i386) FLAVOUR=debian TRIPLE=i686-unknown-linux-musl RPM_ARCH=i686 APK_ARCH=x86 ;;
  *) die "unknown target '$TARGET': aarch64 arm x86_64 i686 (Termux), arm64 armhf amd64 i386 (Linux)" ;;
esac

if [ -z "$FORMAT" ]; then
  if [ -n "$DEVICE" ] && [ $FLAVOUR = termux ]; then
    FORMAT=deb
  else
    case $PM in
      apt|none) FORMAT=deb ;;
      pacman) FORMAT=pacman ;;
      dnf|yum|zypper) FORMAT=rpm ;;
      apk) FORMAT=apk ;;
      xbps) die "Void's xbps has no dev packages: pass --format deb|pacman|rpm|apk to build one for another system" ;;
    esac
  fi
fi
case "$FORMAT:$FLAVOUR" in
  deb:*|pacman:*|rpm:debian|apk:debian) ;;
  rpm:termux|apk:termux) die "Termux packages are deb or pacman, not $FORMAT" ;;
  *) die "unknown format '$FORMAT': deb, pacman, rpm or apk" ;;
esac
# --install: checked now, before a build it could not install.
if [ $INSTALL = 1 ] && [ $PUSH = 0 ]; then
  if [ $FLAVOUR = termux ] && [ $IN_TERMUX = 0 ]; then die "--install puts a Termux package in Termux; from a PC, use --device --push"; fi
  if [ $FLAVOUR = debian ] && [ $IN_TERMUX = 1 ]; then die "--install in Termux installs Termux's package: leave out --target"; fi
  case "$FORMAT:$PM" in
    deb:apt|pacman:pacman|rpm:dnf|rpm:yum|rpm:zypper|apk:apk) ;;
    *) die "this system installs with $PM, not $FORMAT packages: build with --format of its own" ;;
  esac
  if [ -n "$SUDO" ] && ! command -v sudo >/dev/null; then die "installing needs root (or sudo)"; fi
fi

# --- Tools for this target and format ------------------------------------------------------

NDK_RELEASE=r27c NDK_VERSION=27.2.12479018
NDK_SHA256=59c2f6dc96743b5daf5d1626684640b20a6bd2b1d85b13156b90333741bad5cc
if [ $IN_TERMUX = 0 ]; then
  if command -v rustup >/dev/null; then
    rustup target list --installed | grep -qx "$TRIPLE" || offer "the Rust target $TRIPLE" "rustup target add $TRIPLE"
  else
    echo "build: note: no rustup, so the Rust target $TRIPLE cannot be checked; if the build fails, add it to your Rust"
  fi
  if [ $FLAVOUR = termux ]; then
    command -v cargo-ndk >/dev/null || offer cargo-ndk "cargo install cargo-ndk"
    DEV_NDK=${XDG_DATA_HOME:-$HOME/.local/share}/lindroids-dev/android-ndk-$NDK_RELEASE
    if [ -z "${ANDROID_NDK_HOME:-}" ]; then
      # shellcheck disable=SC2046
      for d in "$DEV_NDK" "${ANDROID_HOME:-$HOME/Android/Sdk}/ndk/$NDK_VERSION" $(ls -d "${ANDROID_HOME:-$HOME/Android/Sdk}"/ndk/* 2>/dev/null | sort -rV); do
        if [ -f "$d/source.properties" ]; then ANDROID_NDK_HOME=$d; break; fi
      done
    fi
    if [ ! -f "${ANDROID_NDK_HOME:-/nonexistent}/source.properties" ]; then
      need unzip
      offer "the Android NDK $NDK_RELEASE (a 650 MB download, 2 GB unpacked; no SDK, no Java)" \
        "set -e; mkdir -p '$(dirname "$DEV_NDK")'; cd '$(dirname "$DEV_NDK")'
         curl -fL --retry 3 -o ndk.zip https://dl.google.com/android/repository/android-ndk-$NDK_RELEASE-linux.zip
         echo '$NDK_SHA256  ndk.zip' | sha256sum -c -
         unzip -q ndk.zip; rm ndk.zip"
      ANDROID_NDK_HOME=$DEV_NDK
    fi
    export ANDROID_NDK_HOME
  fi
fi
case $FORMAT in
  deb) need dpkg-deb; [ $FLAVOUR = termux ] || need readelf ;;
  pacman) need dpkg-deb bsdtar xz; [ $FLAVOUR = termux ] || need readelf ;;
  rpm) need dpkg-deb ;;
esac
if [ "$FORMAT" = rpm ] || [ "$FORMAT" = apk ]; then
  # shellcheck source=packaging/container.sh
  . packaging/container.sh
  image=fedora; [ "$FORMAT" = apk ] && image=alpine
  container_engine >/dev/null 2>&1 || native_for "$image" ||
    die "$FORMAT packages are made with podman or docker, or natively on $([ "$FORMAT" = rpm ] && echo "an rpm system (Fedora, openSUSE, ...)" || echo Alpine); this system has neither"
fi

# --- Build --------------------------------------------------------------------------------

eval "$(scripts/version.sh)"
SHA=$(git rev-parse --short HEAD)
DIRTY=''
git diff --quiet HEAD 2>/dev/null || DIRTY=.dirty
case $FORMAT in
  deb|rpm) PKG_VERSION=$LINDROIDS_VERSION~dev.$SHA$DIRTY ;;
  pacman) PKG_VERSION=${LINDROIDS_VERSION}dev.$SHA$DIRTY ;;
  apk)
    # apk versions take no commit: the commit's time (or now, for uncommitted changes)
    if [ -n "$DIRTY" ]; then stamp=$(date -u +%Y%m%d%H%M%S); else stamp=$(TZ=UTC git log -1 --format=%cd --date=format-local:%Y%m%d%H%M%S); fi
    PKG_VERSION=${LINDROIDS_VERSION}_pre$stamp ;;
esac
export LINDROIDS_VERSION LINDROIDS_BUILD_ID=$SHA-dev${DIRTY:+-dirty}
echo "==> lindroids $PKG_VERSION ($FORMAT, $FLAVOUR $TARGET)"

TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT/target}
if [ $IN_TERMUX = 1 ]; then
  cargo build -p lindroids --release
  BIN=$TARGET_DIR/release/lindroids
elif [ $FLAVOUR = termux ]; then
  cargo ndk -t "$ABI" -P 24 build -p lindroids --release
  BIN=$TARGET_DIR/$TRIPLE/release/lindroids
else
  # .cargo/config.toml links the musl targets with rust-lld, for cross builds. On a musl system
  # (Alpine) the target is the host, and that linker would also link the build's own tools and
  # proc macros, which need the system's libraries: the system's cc links instead.
  if [ "$(rustc -vV | sed -n 's/^host: //p')" = "$TRIPLE" ]; then
    triple_env=$(echo "$TRIPLE" | tr 'a-z-' 'A-Z_')
    export "CARGO_TARGET_${triple_env}_LINKER=cc"
  fi
  cargo build -p lindroids --release --target "$TRIPLE"
  BIN=$TARGET_DIR/$TRIPLE/release/lindroids
fi
[ -x "$BIN" ] || die "no binary at $BIN"

# The packaging scripts take <dist>/bin/<triple>/lindroids and the xwayland-satellite layout.
WORK=$TARGET_DIR/dev-build-work
rm -rf "$WORK"
mkdir -p "$WORK/bin/$TRIPLE" "$OUT"
cp "$BIN" "$WORK/bin/$TRIPLE/lindroids"

XWLS=$ROOT/target/xwls/out
if [ "$FORMAT" != apk ] && { [ ! -f "$XWLS/$FLAVOUR-$TARGET/xwayland-satellite" ] || [ ! -f "$XWLS/LICENSE.xwayland-satellite" ]; }; then
  XWLS=$WORK/xwls
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

case $FORMAT in
  deb)
    packaging/build-debs.sh --version "$PKG_VERSION" --dist "$WORK" --xwls "$XWLS" --out "$OUT" --arches "$TARGET" >/dev/null
    PKG=$OUT/lindroids_${PKG_VERSION}_$TARGET.deb ;;
  pacman)
    packaging/build-debs.sh --version "$PKG_VERSION" --dist "$WORK" --xwls "$XWLS" --out "$WORK/debs" --arches "$TARGET" >/dev/null
    packaging/build-pacman.sh --debs "$WORK/debs" --out "$WORK/pacman" >/dev/null
    f=$(find "$WORK/pacman" -name '*.pkg.tar.xz' | head -1)
    [ -n "$f" ] || die "packaging/build-pacman.sh made no package"
    PKG=$OUT/$(basename "$f")
    mv "$f" "$PKG" ;;
  rpm)
    packaging/build-rpm.sh --version "$PKG_VERSION" --dist "$WORK" --xwls "$XWLS" --out "$OUT" --arches "$RPM_ARCH" >/dev/null
    PKG=$OUT/lindroids-$PKG_VERSION-1.$RPM_ARCH.rpm ;;
  apk)
    packaging/build-apk.sh --version "$PKG_VERSION" --dist "$WORK" --out "$WORK/apk" --arches "$APK_ARCH" >/dev/null
    PKG=$OUT/lindroids-$PKG_VERSION-r0.apk
    mv "$WORK/apk/$APK_ARCH/lindroids-$PKG_VERSION-r0.apk" "$PKG" ;;
esac
[ -f "$PKG" ] || die "no package at $PKG"
rm -rf "$WORK"
echo "==> $PKG"

# --- Install or push -----------------------------------------------------------------------

# How <file> installs over whatever lindroids package is there (an older or a newer one).
install_line() {
  local s=${SUDO:+$SUDO }
  [ "${2:-}" = device ] && s=''
  case $FORMAT in
    deb) echo "${s}apt-get install --reinstall --allow-downgrades -y $1" ;;
    pacman) echo "${s}pacman -U --noconfirm $1" ;;
    rpm) echo "${s}rpm -U --oldpackage --replacepkgs $1" ;;
    apk) echo "${s}apk add --allow-untrusted $1" ;;
  esac
}

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
  adb_ push "$PKG" /sdcard/Download/ >/dev/null 2>&1 || die "adb push failed"
  where="Termux"; [ $FLAVOUR = debian ] && where="the Linux distro (proot-distro shares /sdcard)"
  echo
  echo "Copied to the device's Download folder. In $where:"
  echo "  $(install_line "/sdcard/Download/$(basename "$PKG")" device)"
  echo "  lindroids stop    # if a server is running, so the next program starts the new build"
  echo "Back to a release: scripts/use-release.sh"
elif [ $INSTALL = 1 ]; then
  bash -c "$(install_line "'$PKG'")"
  lindroids version
  restart_idle
  echo "Back to a release: scripts/use-release.sh"
fi
