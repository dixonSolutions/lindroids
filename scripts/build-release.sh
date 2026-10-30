#!/usr/bin/env bash
# Build a complete Lindroids release into dist/: the one entry point used by CI and by hand.
#
#   scripts/build-release.sh [--clean]
#
# Produces (VERSION from scripts/version.sh, e.g. 0.1.42):
#   dist/bin/<triple>/lindroids                         the CLI for all 8 targets, and the
#                                                       glibc arm64/armhf builds for Cortex
#   dist/xwayland-satellite/{termux,debian}-<arch>/     xwayland-satellite, 4 bionic + 4 glibc
#   dist/debs/lindroids_VERSION_<arch>.deb              Termux and Debian packages
#   dist/pacman/{termux,arch}/<arch>/*.pkg.tar.xz       the same as pacman packages
#   dist/rpm/lindroids-VERSION-1.<arch>.rpm             RPM (dnf, yum, zypper): static CLI + glibc satellite
#   dist/apk/<arch>/lindroids-VERSION-r0.apk            Alpine & co.: static CLI only (musl)
#   dist/xbps/lindroids-VERSION_1.<arch>.xbps           Void (glibc and musl)
#   dist/lindroids-VERSION-<arch>-linux-musl.tar.gz     static CLI (bin/lindroids) + the glibc
#                                                       libexec/lindroids/xwayland-satellite
#   dist/lindroids-VERSION.apk                          the optional window app, with a release key
#   dist/SHA256SUMS, dist/VERSION
#
# xwayland-satellite (X11 support) is built from its pinned source by
# scripts/build-xwayland-satellite.sh, which skips builds that are up to date; the Debian half
# needs podman or docker, as do the rpm, apk and xbps packages (made with those distributions'
# tools in their containers).
#
# The CLI is built twice per ABI on purpose:
#   * against bionic (cargo-ndk, API 24) for Termux: Android's app seccomp filter kills static
#     musl binaries that use legacy syscalls on x86_64, x86 and arm32;
#   * as a static musl binary (linked with rust-lld, .cargo/config.toml) for glibc distros under
#     proot and for plain Linux.
# and once more against glibc for arm64 and armhf (scripts/build-glibc-cli.sh, in a container):
# Cortex Terminal's Ubuntu maps its paths with an LD_PRELOAD library that a static binary never
# loads, so the static CLI hands over to this one there.
#
# Environment: ANDROID_NDK_HOME, the Android NDK for cargo-ndk (r27c = 27.2.12479018; the NDK
# alone, unpacked from android-ndk-r27c-linux.zip: no Android SDK and no Java). Default: the
# newest NDK in $ANDROID_HOME/ndk or ~/Android/Sdk/ndk, if there is one.
# LINDROIDS_VERSION / LINDROIDS_BUILD_ID override scripts/version.sh.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD

CLEAN=0
while [ $# -gt 0 ]; do
  case "$1" in
    --clean) CLEAN=1; shift ;;
    -h|--help) awk 'NR > 1 && !/^#/ {exit} NR > 1 {sub(/^# ?/, ""); print}' "$0"; exit 0 ;;
    *) echo "build-release: unknown argument $1" >&2; exit 2 ;;
  esac
done

eval "$(scripts/version.sh)"
export LINDROIDS_VERSION LINDROIDS_BUILD_ID
VER=$LINDROIDS_VERSION
echo "==> Lindroids $VER (build $LINDROIDS_BUILD_ID)"

: "${ANDROID_NDK_HOME:=$(ls -d "${ANDROID_HOME:-$HOME/Android/Sdk}"/ndk/* 2>/dev/null | sort -V | tail -1)}"
if [ ! -f "${ANDROID_NDK_HOME:-/nonexistent}/source.properties" ]; then
  echo "build-release: no Android NDK: set ANDROID_NDK_HOME (android-ndk-r27c-linux.zip, unpacked)" >&2
  exit 1
fi
export ANDROID_NDK_HOME

ANDROID_TRIPLES="aarch64-linux-android armv7-linux-androideabi x86_64-linux-android i686-linux-android"
MUSL_TRIPLES="aarch64-unknown-linux-musl armv7-unknown-linux-musleabihf x86_64-unknown-linux-musl i686-unknown-linux-musl"
GLIBC_TRIPLES="aarch64-unknown-linux-gnu armv7-unknown-linux-gnueabihf"

DIST=$ROOT/dist
if [ $CLEAN = 1 ]; then rm -rf "$DIST"; fi
rm -rf "${DIST:?}/bin" "${DIST:?}/debs" "${DIST:?}/pacman" "${DIST:?}/rpm" "${DIST:?}/apk" "${DIST:?}/xbps" \
  "${DIST:?}/xwayland-satellite" "$DIST"/lindroids-*.tar.gz "$DIST"/lindroids-*.apk "$DIST/SHA256SUMS"
mkdir -p "$DIST/bin"

echo "==> CLI for Termux (bionic, API 24)"
NDK_ARGS=()
for t in $ANDROID_TRIPLES; do NDK_ARGS+=(-t "$t"); done
cargo ndk "${NDK_ARGS[@]}" -P 24 build -p lindroids --release

echo "==> CLI, static musl"
MUSL_ARGS=()
for t in $MUSL_TRIPLES; do MUSL_ARGS+=(--target "$t"); done
cargo build -p lindroids --release "${MUSL_ARGS[@]}"

TARGET_DIR=${CARGO_TARGET_DIR:-target}
for t in $ANDROID_TRIPLES $MUSL_TRIPLES; do
  mkdir -p "$DIST/bin/$t"
  install -m 755 "$TARGET_DIR/$t/release/lindroids" "$DIST/bin/$t/lindroids"
  # Another build in the same target dir (without LINDROIDS_BUILD_ID) can replace the binary
  # between cargo and this copy; never ship that. A separate CARGO_TARGET_DIR avoids it.
  if ! grep -aq "+$LINDROIDS_BUILD_ID" "$DIST/bin/$t/lindroids"; then
    echo "build-release: $t binary is not build $LINDROIDS_BUILD_ID (concurrent build in $TARGET_DIR?)" >&2
    exit 1
  fi
done
# Android 7's linker warns about DF_1_PIE (from NDK r27's lld) on every run of the Termux builds.
for t in $ANDROID_TRIPLES; do
  python3 scripts/elf-clear-pie-flag.py "$DIST/bin/$t/lindroids"
done
# Refuse to ship a musl binary that is not static: it would not start under proot-distro.
for t in $MUSL_TRIPLES; do
  if readelf -l "$DIST/bin/$t/lindroids" 2>/dev/null | grep -q 'Requesting program interpreter'; then
    echo "build-release: $t binary is dynamically linked" >&2
    exit 1
  fi
done

# Cortex Terminal: the glibc builds, which the arm64 and armhf .debs and tarballs carry.
scripts/build-glibc-cli.sh "$DIST/bin"
for t in $GLIBC_TRIPLES; do
  if ! grep -aq "+$LINDROIDS_BUILD_ID" "$DIST/bin/$t/lindroids"; then
    echo "build-release: $t binary is not build $LINDROIDS_BUILD_ID" >&2
    exit 1
  fi
done

# Both flavours before the .debs: a Termux .deb carries the glibc build too (for proot distros).
echo "==> window app (APK, no browser)"
if [ -n "${LINDROIDS_APP_KEYSTORE:-}${LINDROIDS_APP_KEYSTORE_FILE:-}" ]; then
  packaging/build-webview-apk.sh --version "$VER" --out "$DIST"
elif [ "${LINDROIDS_APP_TEST_SIGN:-}" = 1 ]; then
  packaging/build-webview-apk.sh --version "$VER" --out "$DIST" --debug-sign
else
  echo "    skipped: no LINDROIDS_APP_KEYSTORE (set LINDROIDS_APP_TEST_SIGN=1 for a test-signed APK)"
fi

scripts/build-xwayland-satellite.sh all
XWLS=$DIST/xwayland-satellite
mkdir -p "$XWLS"
cp -r "$ROOT"/target/xwls/out/termux-* "$ROOT"/target/xwls/out/debian-* "$XWLS/"
rm -f "$XWLS"/*/.stamp
python3 scripts/elf-clear-pie-flag.py "$XWLS"/termux-*/xwayland-satellite
install -m 644 "$ROOT"/target/xwls/out/LICENSE.xwayland-satellite \
  "$ROOT"/target/xwls/out/xwayland-satellite-crates.txt "$XWLS/"

packaging/build-debs.sh --version "$VER" --dist "$DIST" --out "$DIST/debs" --xwls "$XWLS"
echo "==> pacman packages"
rm -rf "${DIST:?}/pacman"
packaging/build-pacman.sh --debs "$DIST/debs" --out "$DIST/pacman"
# The same static CLI (and, for glibc systems, the same satellite) for the other package managers.
packaging/build-rpm.sh --version "$VER" --dist "$DIST" --out "$DIST/rpm" --xwls "$XWLS"
packaging/build-apk.sh --version "$VER" --dist "$DIST" --out "$DIST/apk"
packaging/build-xbps.sh --version "$VER" --dist "$DIST" --out "$DIST/xbps" --xwls "$XWLS"

echo "==> tarballs"
: "${SOURCE_DATE_EPOCH:=$(git log -1 --format=%ct 2>/dev/null || date +%s)}"
# Laid out as a prefix (bin/, libexec/), so <bindir>/../libexec/lindroids/ finds the satellite
# (and, for arm, lindroids-glibc for Cortex); install.sh installs it that way. The satellite is the glibc build: it needs glibc
# 2.34+, libxcb, libxcb-cursor and Xwayland from the distro (only for `run --x11`).
for t in $MUSL_TRIPLES; do
  arch=${t%%-*}
  case "$arch" in
    aarch64) deb_arch=arm64 ;;
    armv7) deb_arch=armhf ;;
    x86_64) deb_arch=amd64 ;;
    i686) deb_arch=i386 ;;
  esac
  name=lindroids-$VER-$arch-linux-musl
  stage=$ROOT/target/tarball/$name
  rm -rf "$stage" && mkdir -p "$stage/bin" "$stage/libexec/lindroids"
  install -m 755 "$DIST/bin/$t/lindroids" "$stage/bin/lindroids"
  install -m 755 "$XWLS/debian-$deb_arch/xwayland-satellite" "$stage/libexec/lindroids/xwayland-satellite"
  glibc_cli=$DIST/bin/${t/-musl/-gnu}/lindroids
  if [ -f "$glibc_cli" ]; then
    install -m 755 "$glibc_cli" "$stage/libexec/lindroids/lindroids-glibc"
  fi
  for f in README.md LICENSE LICENSE-MIT LICENSE-APACHE docs/INSTALL.md; do
    [ -f "$f" ] && install -m 644 "$f" "$stage/$(basename "$f")"
  done
  install -m 644 third_party/README.md "$stage/THIRD-PARTY.md"
  install -m 644 "$XWLS/LICENSE.xwayland-satellite" "$XWLS/xwayland-satellite-crates.txt" "$stage/"
  tar --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$SOURCE_DATE_EPOCH" \
    -C "$ROOT/target/tarball" -czf "$DIST/$name.tar.gz" "$name"
  rm -rf "$stage"
  echo "    $name.tar.gz"
done

echo "$VER" > "$DIST/VERSION"
( cd "$DIST" && { sha256sum lindroids-*.tar.gz debs/*.deb; if [ -f "lindroids-$VER.apk" ]; then sha256sum "lindroids-$VER.apk"; fi; } > SHA256SUMS )
echo "==> dist/ ready"
( cd "$DIST" && ls -1sh lindroids-*.tar.gz debs/*.deb )
