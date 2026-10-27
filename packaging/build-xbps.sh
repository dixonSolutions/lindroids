#!/usr/bin/env bash
# Build the lindroids Void Linux packages (.xbps) from the outputs of scripts/build-release.sh.
#
#   packaging/build-xbps.sh --version 0.1.42 [--dist dist] [--out dist/xbps]
#                           [--arches "aarch64 aarch64-musl armv7l armv7l-musl x86_64 x86_64-musl i686"]
#                           [--xwls dist/xwayland-satellite]
#
# Every architecture gets the static musl CLI (/usr/bin/lindroids); the glibc ones also the glibc
# xwayland-satellite (/usr/libexec/lindroids/), the musl ones not (Void's own xwayland-satellite
# package is for them, through `lindroids setup x11`). No dependencies: xbps has no Recommends,
# and the CLI needs nothing.
# Output: <out>/lindroids-<version>_1.<arch>.xbps, unsigned: packaging/repo/build-repo.sh signs
# them (.sig2) and the repository index.
#
# Made with xbps-create in a Void container (podman or docker; packaging/container.sh).
# Input: <dist>/bin/<musl triple>/lindroids, <xwls>/debian-<deb arch>/xwayland-satellite.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
# shellcheck source=packaging/container.sh
. "$ROOT/packaging/container.sh"

VOID_IMAGE=${LINDROIDS_VOID_IMAGE:-ghcr.io/void-linux/void-glibc-full:latest}

VERSION=
DIST=dist
OUT=
XWLS=
ARCHES="aarch64 aarch64-musl armv7l armv7l-musl x86_64 x86_64-musl i686"
while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION=$2; shift 2 ;;
    --dist) DIST=$2; shift 2 ;;
    --out) OUT=$2; shift 2 ;;
    --arches) ARCHES=$2; shift 2 ;;
    --xwls) XWLS=$2; shift 2 ;;
    *) echo "build-xbps: unknown argument $1" >&2; exit 2 ;;
  esac
done
[ -n "$VERSION" ] || { echo "build-xbps: --version is required" >&2; exit 2; }
OUT=${OUT:-$DIST/xbps}
XWLS=${XWLS:-$DIST/xwayland-satellite}
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
: "${SOURCE_DATE_EPOCH:=$(git log -1 --format=%ct 2>/dev/null || date +%s)}"
export SOURCE_DATE_EPOCH
HOMEPAGE="https://github.com/${LINDROIDS_GITHUB_REPO:-dixonSolutions/lindroids}"
MAINTAINER="dixonSolutions <lindroids@users.noreply.github.com>"

WORK=$ROOT/target/xbps
rm -rf "$WORK"
mkdir -p "$WORK"

n=0
for arch in $ARCHES; do
  case "${arch%-musl}" in
    aarch64) triple=aarch64-unknown-linux-musl deb=arm64 ;;
    armv7l) triple=armv7-unknown-linux-musleabihf deb=armhf ;;
    x86_64) triple=x86_64-unknown-linux-musl deb=amd64 ;;
    i686) triple=i686-unknown-linux-musl deb=i386 ;;
    *) echo "build-xbps: unknown architecture $arch" >&2; exit 2 ;;
  esac
  [ "$arch" = i686-musl ] && { echo "build-xbps: Void has no i686-musl" >&2; exit 2; }
  bin=$DIST/bin/$triple/lindroids
  [ -f "$bin" ] || { echo "build-xbps: missing $bin" >&2; exit 1; }
  root=$WORK/$arch
  mkdir -p "$root/usr/bin" "$root/usr/share/doc/lindroids" "$root/usr/share/licenses/lindroids"
  install -m 755 "$bin" "$root/usr/bin/lindroids"
  install -m 644 README.md "$root/usr/share/doc/lindroids/README.md"
  [ -f third_party/README.md ] && install -m 644 third_party/README.md "$root/usr/share/doc/lindroids/THIRD-PARTY.md"
  for f in LICENSE-MIT LICENSE-APACHE; do
    [ -f "$f" ] && install -m 644 "$f" "$root/usr/share/licenses/lindroids/$f"
  done
  case "$arch" in
    *-musl) ;;
    *)
      xwls=$XWLS/debian-$deb/xwayland-satellite
      [ -f "$xwls" ] || { echo "build-xbps: missing $xwls (scripts/build-xwayland-satellite.sh)" >&2; exit 1; }
      mkdir -p "$root/usr/libexec/lindroids"
      install -m 755 "$xwls" "$root/usr/libexec/lindroids/xwayland-satellite"
      for f in "$XWLS/LICENSE.xwayland-satellite" "$XWLS/xwayland-satellite-crates.txt"; do
        install -m 644 "$f" "$root/usr/share/licenses/lindroids/"
      done
      ;;
  esac
  find "$root" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} +
  n=$((n + 1))
done
[ $n -gt 0 ] || exit 0

echo "==> xbps packages $VERSION (in $VOID_IMAGE)"
# shellcheck disable=SC2016,SC2086
in_container "$VOID_IMAGE" "$WORK" "$OUT" -- sh -ec '
  work=$1 out=$2 version=$3 homepage=$4 maintainer=$5; shift 5
  cd "$out"
  for arch in "$@"; do
    license="MIT, Apache-2.0"
    [ -e "$work/$arch/usr/libexec/lindroids/xwayland-satellite" ] && license="$license, MPL-2.0"
    rm -f "lindroids-${version}_1.$arch.xbps"
    xbps-create -q -A "$arch" -n "lindroids-${version}_1" \
      -s "Run Linux apps on Android, shown in the web browser" \
      -l "$license" -H "$homepage" -m "$maintainer" \
      -D "" "$work/$arch"
  done' sh "$WORK" "$OUT" "$VERSION" "$HOMEPAGE" "$MAINTAINER" $ARCHES
for arch in $ARCHES; do
  f=$OUT/lindroids-${VERSION}_1.$arch.xbps
  [ -s "$f" ] || { echo "build-xbps: $f was not made" >&2; exit 1; }
  echo "    $(basename "$f") ($(du -h "$f" | cut -f1))"
done
rm -rf "$WORK"
