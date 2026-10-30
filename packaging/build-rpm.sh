#!/usr/bin/env bash
# Build the lindroids RPM packages from the outputs of scripts/build-release.sh: the static musl
# CLI and the glibc xwayland-satellite (these are all glibc distributions).
#
#   packaging/build-rpm.sh --version 0.1.42 [--dist dist] [--out dist/rpm]
#                          [--arches "aarch64 armv7hl x86_64 i686"] [--xwls dist/xwayland-satellite]
#
# One package for dnf and yum (Fedora, RHEL, Rocky, Alma, CentOS, Oracle, Amazon Linux) and
# zypper (openSUSE, SLE): /usr/bin/lindroids and /usr/libexec/lindroids/xwayland-satellite, no
# hard dependencies (AutoReqProv off: the CLI is static and must install anywhere). What the
# satellite needs for `lindroids run --x11` is Recommends, by file and soname, so the same
# package fits Fedora's and openSUSE's package names: /usr/bin/Xwayland, libxcb.so.1,
# libxcb-cursor.so.0.
# Output: <out>/lindroids-<version>-1.<arch>.rpm, unsigned (packaging/repo/build-repo.sh signs
# the repository metadata, which holds every package's checksum).
#
# Built with rpmbuild in a fedora container (podman or docker; packaging/container.sh).
# Input: <dist>/bin/<musl triple>/lindroids, <xwls>/debian-<deb arch>/xwayland-satellite.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
# shellcheck source=packaging/container.sh
. "$ROOT/packaging/container.sh"

FEDORA_IMAGE=${LINDROIDS_FEDORA_IMAGE:-registry.fedoraproject.org/fedora:latest}

VERSION=
DIST=dist
OUT=
XWLS=
ARCHES="aarch64 armv7hl x86_64 i686"
while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION=$2; shift 2 ;;
    --dist) DIST=$2; shift 2 ;;
    --out) OUT=$2; shift 2 ;;
    --arches) ARCHES=$2; shift 2 ;;
    --xwls) XWLS=$2; shift 2 ;;
    *) echo "build-rpm: unknown argument $1" >&2; exit 2 ;;
  esac
done
[ -n "$VERSION" ] || { echo "build-rpm: --version is required" >&2; exit 2; }
OUT=${OUT:-$DIST/rpm}
XWLS=${XWLS:-$DIST/xwayland-satellite}
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
: "${SOURCE_DATE_EPOCH:=$(git log -1 --format=%ct 2>/dev/null || date +%s)}"
export SOURCE_DATE_EPOCH
HOMEPAGE="https://github.com/${LINDROIDS_GITHUB_REPO:-dixonSolutions/lindroids}"
MAINTAINER="dixonSolutions <lindroids@users.noreply.github.com>"

WORK=$ROOT/target/rpm
rm -rf "$WORK"
mkdir -p "$WORK"

n=0
for arch in $ARCHES; do
  case "$arch" in
    aarch64) triple=aarch64-unknown-linux-musl deb=arm64 lib64='()(64bit)' ;;
    armv7hl) triple=armv7-unknown-linux-musleabihf deb=armhf lib64= ;;
    x86_64) triple=x86_64-unknown-linux-musl deb=amd64 lib64='()(64bit)' ;;
    i686) triple=i686-unknown-linux-musl deb=i386 lib64= ;;
    *) echo "build-rpm: unknown architecture $arch" >&2; exit 2 ;;
  esac
  bin=$DIST/bin/$triple/lindroids
  xwls=$XWLS/debian-$deb/xwayland-satellite
  [ -f "$bin" ] || { echo "build-rpm: missing $bin" >&2; exit 1; }
  [ -f "$xwls" ] || { echo "build-rpm: missing $xwls (scripts/build-xwayland-satellite.sh)" >&2; exit 1; }
  root=$WORK/$arch/root
  mkdir -p "$root/usr/bin" "$root/usr/libexec/lindroids" "$root/usr/share/doc/lindroids" \
    "$root/usr/share/licenses/lindroids"
  install -m 755 "$bin" "$root/usr/bin/lindroids"
  install -m 755 "$xwls" "$root/usr/libexec/lindroids/xwayland-satellite"
  install -m 644 README.md "$root/usr/share/doc/lindroids/README.md"
  [ -f third_party/README.md ] && install -m 644 third_party/README.md "$root/usr/share/doc/lindroids/THIRD-PARTY.md"
  for f in LICENSE-MIT LICENSE-APACHE "$XWLS/LICENSE.xwayland-satellite" "$XWLS/xwayland-satellite-crates.txt"; do
    [ -f "$f" ] && install -m 644 "$f" "$root/usr/share/licenses/lindroids/$(basename "$f")"
  done
  cat > "$WORK/$arch/lindroids.spec" <<EOF
Name:           lindroids
Version:        $VERSION
Release:        1
Summary:        Linux GUI programs on Android, in windows and apps of their own
License:        (MIT OR Apache-2.0) AND MPL-2.0
URL:            $HOMEPAGE
Packager:       $MAINTAINER
AutoReqProv:    no
Recommends:     /usr/bin/Xwayland
Recommends:     libxcb.so.1$lib64
Recommends:     libxcb-cursor.so.0$lib64

# v4 packages with an xz payload: what every rpm still in use reads (rpm 6 would make v6 + zstd).
%global _rpmformat 4
%global _binary_payload w6.xzdio
%global debug_package %{nil}
%global __os_install_post %{nil}
%global _build_id_links none
%global _binaries_in_noarch_packages_terminate_build 0
%global source_date_epoch_from_changelog 0
%global use_source_date_epoch_as_buildtime 1
%global clamp_mtime_to_source_date_epoch 1

%description
Lindroids is a package for the Linux environments on Android (Termux,
proot distros, Flatpak); it is not an Android app or a container itself.
It opens their GUI programs as Android windows, with touch, keyboard,
clipboard and sound, and "lindroids export" builds a program its own
Android app on the phone. This package contains the lindroids CLI (which
is also the Wayland compositor and sound server) and xwayland-satellite,
which runs X11 programs under Xwayland (lindroids run --x11).

%install
cp -a $root/. %{buildroot}/

%files
/usr/bin/lindroids
%dir /usr/libexec/lindroids
/usr/libexec/lindroids/xwayland-satellite
%doc /usr/share/doc/lindroids
%license /usr/share/licenses/lindroids
EOF
  n=$((n + 1))
done
[ $n -gt 0 ] || exit 0

echo "==> rpm packages $VERSION (in $FEDORA_IMAGE)"
# shellcheck disable=SC2016,SC2086
in_container "$FEDORA_IMAGE" "$WORK" "$OUT" -- sh -ec '
  work=$1 out=$2; shift 2
  # The fedora image, or (packaging/container.sh, natively) any rpm system: dnf, yum or zypper.
  if ! command -v rpmbuild >/dev/null; then
    if command -v dnf >/dev/null; then dnf -q -y install rpm-build >/dev/null 2>&1 || dnf -y install rpm-build
    elif command -v yum >/dev/null; then yum -q -y install rpm-build
    # --force-resolution: minimal openSUSE images have busybox-gawk, which the gawk of rpm-build replaces
    else zypper --non-interactive install --force-resolution rpm-build
    fi
  fi
  for arch in "$@"; do
    rpmbuild -bb --quiet --target "$arch-linux" \
      --define "_topdir $work/$arch/build" --define "_rpmdir $work/$arch/rpms" \
      --define "_build_name_fmt %%{NAME}-%%{VERSION}-%%{RELEASE}.%%{ARCH}.rpm" \
      "$work/$arch/lindroids.spec"
    cp "$work/$arch/rpms/"*.rpm "$out/"
  done' sh "$WORK" "$OUT" $ARCHES
for arch in $ARCHES; do
  f=$OUT/lindroids-$VERSION-1.$arch.rpm
  [ -s "$f" ] || { echo "build-rpm: $f was not made" >&2; exit 1; }
  echo "    $(basename "$f") ($(du -h "$f" | cut -f1))"
done
rm -rf "$WORK"
