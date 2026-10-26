#!/usr/bin/env bash
# Build the lindroids .deb packages from the outputs of scripts/build-release.sh.
#
#   packaging/build-debs.sh --version 0.1.42 [--dist dist] [--apk dist/lindroids-0.1.42.apk]
#                           [--out dist/debs] [--arches "amd64 aarch64"]   (default: all eight)
#
# Two flavours, one package name (`lindroids`):
#   Termux  (suite `termux`): Architecture aarch64/arm/x86_64/i686, files under
#           /data/data/com.termux/files/usr, the CLI built against bionic (cargo-ndk, API 24).
#   Debian  (suite `stable`): Architecture arm64/armhf/amd64/i386, files under /usr, the static
#           musl CLI. For Debian/Ubuntu under proot-distro, UserLAnd, Andronix, and plain Linux.
# Both carry the APK at <prefix>/share/lindroids/lindroids.apk so the CLI can install or update
# the app. Input binaries: <dist>/bin/<rust triple>/lindroids.
#
# Needs dpkg-deb (Fedora: dnf install dpkg; Debian/Ubuntu: preinstalled). Compression is xz,
# which every Termux and Debian dpkg reads (zstd is not accepted by Termux's dpkg everywhere).
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD

VERSION=
DIST=dist
APK=
OUT=
ARCHES="aarch64 arm x86_64 i686 arm64 armhf amd64 i386"
while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION=$2; shift 2 ;;
    --dist) DIST=$2; shift 2 ;;
    --apk) APK=$2; shift 2 ;;
    --out) OUT=$2; shift 2 ;;
    --arches) ARCHES=$2; shift 2 ;;
    *) echo "build-debs: unknown argument $1" >&2; exit 2 ;;
  esac
done
[ -n "$VERSION" ] || { echo "build-debs: --version is required" >&2; exit 2; }
APK=${APK:-$DIST/lindroids-$VERSION.apk}
OUT=${OUT:-$DIST/debs}
[ -f "$APK" ] || { echo "build-debs: no APK at $APK" >&2; exit 1; }
command -v dpkg-deb >/dev/null || { echo "build-debs: dpkg-deb is required" >&2; exit 1; }

MAINTAINER="dixonSolutions <lindroids@users.noreply.github.com>"
HOMEPAGE="https://github.com/${LINDROIDS_GITHUB_REPO:-dixonSolutions/lindroids}"
: "${SOURCE_DATE_EPOCH:=$(git log -1 --format=%ct 2>/dev/null || date +%s)}"
export SOURCE_DATE_EPOCH

WORK=$ROOT/target/debs
rm -rf "$WORK"
mkdir -p "$WORK" "$OUT"

copyright() {
  cat <<EOF
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: lindroids
Source: $HOMEPAGE

Files: *
Copyright: dixonSolutions and the Lindroids contributors
License: MIT or Apache-2.0
 Lindroids is dual-licensed under the MIT licence and the Apache License 2.0;
 see LICENSE-MIT and LICENSE-APACHE in the source repository. On Debian
 systems the Apache License 2.0 is in /usr/share/common-licenses/Apache-2.0.

Files: share/lindroids/lindroids.apk
Comment: The APK also contains proot (GPL-2.0-or-later), talloc
 (LGPL-3.0-or-later) and libandroid-shmem (BSD-3-Clause), prebuilt by Termux.
 See THIRD-PARTY.md in this directory for versions, sources and changes.
EOF
}

# <flavour> <deb arch> <rust triple> <prefix without leading slash>
build() {
  local flavour=$1 arch=$2 triple=$3 prefix=$4
  case " $ARCHES " in *" $arch "*) ;; *) return 0 ;; esac
  local bin=$DIST/bin/$triple/lindroids
  [ -f "$bin" ] || { echo "build-debs: missing $bin" >&2; exit 1; }
  local d=$WORK/$flavour-$arch
  local doc=$d/$prefix/share/doc/lindroids
  mkdir -p "$d/DEBIAN" "$d/$prefix/bin" "$d/$prefix/share/lindroids" "$doc"
  install -m 755 "$bin" "$d/$prefix/bin/lindroids"
  install -m 644 "$APK" "$d/$prefix/share/lindroids/lindroids.apk"
  local f
  for f in README.md LICENSE LICENSE-MIT LICENSE-APACHE LICENSE.md; do
    [ -f "$f" ] && install -m 644 "$f" "$doc/$f"
  done
  [ -f third_party/README.md ] && install -m 644 third_party/README.md "$doc/THIRD-PARTY.md"
  copyright > "$doc/copyright"
  chmod 644 "$doc/copyright"

  local suggests
  if [ "$flavour" = termux ]; then
    suggests="proot, proot-distro, xwayland"
  else
    suggests="xwayland, flatpak"
  fi
  local size
  size=$(du -sk --exclude=DEBIAN "$d" | cut -f1)
  cat > "$d/DEBIAN/control" <<EOF
Package: lindroids
Version: $VERSION
Architecture: $arch
Maintainer: $MAINTAINER
Installed-Size: $size
Suggests: $suggests
Section: x11
Priority: optional
Homepage: $HOMEPAGE
Description: Run Linux apps in Android windows
 Lindroids is a Wayland compositor and sound server whose output is the
 Lindroids Android app: every Linux window becomes an Android window, with
 touch, keyboard, IME, clipboard and audio. \`lindroids export\` puts a Linux
 app on the Android home screen. This package contains the lindroids CLI
 (which is also the server) and the Lindroids APK.
EOF
  find "$d" -type d -exec chmod 755 {} +
  local deb=$OUT/lindroids_${VERSION}_${arch}.deb
  # Stock dpkg-deb warns that "x86_64" is not a Debian architecture name; Termux's dpkg uses it.
  if ! dpkg-deb --root-owner-group -Zxz -b "$d" "$deb" >/dev/null 2>"$WORK/dpkg-deb.err"; then
    cat "$WORK/dpkg-deb.err" >&2
    exit 1
  fi
  grep -vE "x86_64|ignoring 1 warning" "$WORK/dpkg-deb.err" >&2 || true
  echo "    $(basename "$deb") ($(du -h "$deb" | cut -f1))"
}

echo "==> debs $VERSION"
TERMUX=data/data/com.termux/files/usr
build termux aarch64 aarch64-linux-android "$TERMUX"
build termux arm armv7-linux-androideabi "$TERMUX"
build termux x86_64 x86_64-linux-android "$TERMUX"
build termux i686 i686-linux-android "$TERMUX"
build debian arm64 aarch64-unknown-linux-musl usr
build debian armhf armv7-unknown-linux-musleabihf usr
build debian amd64 x86_64-unknown-linux-musl usr
build debian i386 i686-unknown-linux-musl usr
rm -rf "$WORK"
