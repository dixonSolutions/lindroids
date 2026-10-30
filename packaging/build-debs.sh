#!/usr/bin/env bash
# Build the lindroids .deb packages from the outputs of scripts/build-release.sh.
#
#   packaging/build-debs.sh --version 0.1.42 [--dist dist] [--out dist/debs]
#                           [--arches "amd64 aarch64"]   (default: all eight)
#                           [--xwls dist/xwayland-satellite]
#
# Two flavours, one package name (`lindroids`):
#   Termux  (suite `termux`): Architecture aarch64/arm/x86_64/i686, files under
#           /data/data/com.termux/files/usr, the CLI built against bionic (cargo-ndk, API 24).
#   Debian  (suite `stable`): Architecture arm64/armhf/amd64/i386, files under /usr, the static
#           musl CLI. For Debian/Ubuntu under proot-distro, UserLAnd, Andronix, and plain Linux.
# Input binaries: <dist>/bin/<rust triple>/lindroids.
#
# Both also carry xwayland-satellite (MPL-2.0, built by scripts/build-xwayland-satellite.sh) at
# <prefix>/libexec/lindroids/xwayland-satellite, where the CLI looks for it (<bindir>/../libexec):
#   Termux  the bionic build (X11 programs in proot distros reach it too: they share Termux's
#           display);
#   Debian  the glibc build (bookworm baseline). The CLI stays static; the satellite's libraries
#           (and Xwayland) are Recommends, so the package installs anywhere.
# Input: <xwls>/{termux,debian}-<deb arch>/xwayland-satellite, LICENSE.xwayland-satellite and
# xwayland-satellite-crates.txt (the layout of target/xwls/out).
#
# Needs dpkg-deb (Fedora: dnf install dpkg; Debian/Ubuntu: preinstalled). Compression is xz,
# which every Termux and Debian dpkg reads (zstd is not accepted by Termux's dpkg everywhere).
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD

VERSION=
DIST=dist
OUT=
XWLS=
ARCHES="aarch64 arm x86_64 i686 arm64 armhf amd64 i386"
while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION=$2; shift 2 ;;
    --dist) DIST=$2; shift 2 ;;
    --out) OUT=$2; shift 2 ;;
    --arches) ARCHES=$2; shift 2 ;;
    --xwls) XWLS=$2; shift 2 ;;
    *) echo "build-debs: unknown argument $1" >&2; exit 2 ;;
  esac
done
[ -n "$VERSION" ] || { echo "build-debs: --version is required" >&2; exit 2; }
OUT=${OUT:-$DIST/debs}
XWLS=${XWLS:-$DIST/xwayland-satellite}
for f in LICENSE.xwayland-satellite xwayland-satellite-crates.txt; do
  [ -f "$XWLS/$f" ] || { echo "build-debs: no $XWLS/$f (scripts/build-xwayland-satellite.sh)" >&2; exit 1; }
done
command -v dpkg-deb >/dev/null || { echo "build-debs: dpkg-deb is required" >&2; exit 1; }
# readelf finds the Debian satellite's glibc version; Termux packages (scripts/build.sh in Termux,
# which may not have binutils) need none.
case " $ARCHES " in
  *" arm64 "*|*" armhf "*|*" amd64 "*|*" i386 "*)
    command -v readelf >/dev/null || { echo "build-debs: readelf is required" >&2; exit 1; } ;;
esac
# The pin comes from the build script, so the copyright file names exactly what is shipped.
xwls_pin() { sed -n "s/^$1=//p" scripts/build-xwayland-satellite.sh; }
XWLS_VERSION=$(xwls_pin SATELLITE_VERSION)
XWLS_SHA256=$(xwls_pin SATELLITE_SHA256)
XWLS_URL="https://github.com/Supreeeme/xwayland-satellite/archive/refs/tags/v$XWLS_VERSION.tar.gz"

MAINTAINER="dixonSolutions <lindroids@users.noreply.github.com>"
HOMEPAGE="https://github.com/${LINDROIDS_GITHUB_REPO:-dixonSolutions/lindroids}"
: "${SOURCE_DATE_EPOCH:=$(git log -1 --format=%ct 2>/dev/null || date +%s)}"
export SOURCE_DATE_EPOCH

WORK=$ROOT/target/debs
rm -rf "$WORK"
mkdir -p "$WORK" "$OUT"

copyright() { # <flavour>
  local mpl="LICENSE.xwayland-satellite in this directory"
  [ "$1" = debian ] && mpl="$mpl and, on Debian systems,
 in /usr/share/common-licenses/MPL-2.0"
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

Files: libexec/lindroids/xwayland-satellite
Copyright: Shawn Wallace and the xwayland-satellite contributors
License: MPL-2.0
 The full text of the Mozilla Public License 2.0 is in
 $mpl.
Comment: xwayland-satellite $XWLS_VERSION (https://github.com/Supreeeme/xwayland-satellite),
 built from its unmodified release source:
 $XWLS_URL
 sha256 $XWLS_SHA256
 The Termux build also uses two dependency crates (smithay-client-toolkit,
 wayland-cursor; MIT) with small patches so that they compile for Android; the
 patches are in third_party/xwayland-satellite/ of the Lindroids source (see
 THIRD-PARTY.md). The Rust crates it is built from, with their licences, are
 listed in xwayland-satellite-crates.txt. It embeds the Open Sans font
 (SIL Open Font License 1.1).
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
  mkdir -p "$d/DEBIAN" "$d/$prefix/bin" "$doc"
  install -m 755 "$bin" "$d/$prefix/bin/lindroids"
  local f
  for f in README.md LICENSE LICENSE-MIT LICENSE-APACHE LICENSE.md; do
    [ -f "$f" ] && install -m 644 "$f" "$doc/$f"
  done
  [ -f third_party/README.md ] && install -m 644 third_party/README.md "$doc/THIRD-PARTY.md"
  install -m 644 "$XWLS/LICENSE.xwayland-satellite" "$XWLS/xwayland-satellite-crates.txt" "$doc/"
  copyright "$flavour" > "$doc/copyright"
  chmod 644 "$doc/copyright"

  # xwayland-satellite: the build for this flavour.
  local xwls=$XWLS/$flavour-$arch/xwayland-satellite
  [ -f "$xwls" ] || { echo "build-debs: missing $xwls" >&2; exit 1; }
  mkdir -p "$d/$prefix/libexec/lindroids"
  install -m 755 "$xwls" "$d/$prefix/libexec/lindroids/xwayland-satellite"
  # Cortex Terminal (Ubuntu without proot, arm only): the glibc CLI, which the static one hands
  # over to there (scripts/build-glibc-cli.sh).
  local glibc_cli=$DIST/bin/${triple/-musl/-gnu}/lindroids
  if [ "$flavour" = debian ] && [ -f "$glibc_cli" ]; then
    install -m 755 "$glibc_cli" "$d/$prefix/libexec/lindroids/lindroids-glibc"
  fi
  # Flatpak's bubblewrap in Termux (packaging/termux-flatpak): `lindroids bwrap`, which leaves out
  # the namespaces Android kernels lack.
  if [ "$flavour" = termux ]; then
    printf '#!/%s/bin/sh\nexec /%s/bin/lindroids bwrap "$@"\n' "$prefix" "$prefix" > "$d/$prefix/libexec/lindroids/bwrap"
    chmod 755 "$d/$prefix/libexec/lindroids/bwrap"
  fi

  # The CLI itself needs none of these (it is static on Debian); xwayland-satellite does, for
  # `lindroids run --x11`. Recommends, so the package still installs without them, e.g. in a
  # Termux without x11-repo (xcb-util-cursor and xwayland are there; libxcb is in main).
  local recommends suggests glibc
  if [ "$flavour" = termux ]; then
    recommends="libxcb, xcb-util-cursor, xwayland"
    suggests="proot, proot-distro"
  else
    glibc=$(readelf -V --wide "$xwls" | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/GLIBC_//' | sort -uV | tail -1)
    recommends="xwayland, libc6 (>= $glibc), libxcb1, libxcb-cursor0"
    suggests="flatpak"
  fi
  local size
  size=$(du -sk --exclude=DEBIAN "$d" | cut -f1)
  cat > "$d/DEBIAN/control" <<EOF
Package: lindroids
Version: $VERSION
Architecture: $arch
Maintainer: $MAINTAINER
Installed-Size: $size
Recommends: $recommends
Suggests: $suggests
Section: x11
Priority: optional
Homepage: $HOMEPAGE
Description: Linux GUI programs on Android, in windows and apps of their own
 Lindroids is a package for the Linux environments on Android (Termux,
 proot distros, Flatpak); it is not an Android app or a container itself.
 It opens their GUI programs as Android windows, with touch, keyboard,
 clipboard and sound, and "lindroids export" builds a program its own
 Android app on the phone. This package contains the lindroids CLI (which
 is also the Wayland compositor and sound server) and xwayland-satellite,
 which runs X11 programs under Xwayland (lindroids run --x11).
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
