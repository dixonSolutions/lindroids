#!/usr/bin/env bash
# Build a complete Lindroids release into dist/: the one entry point used by CI and by hand.
#
#   scripts/build-release.sh [--skip-proot] [--skip-apk] [--clean]
#
# Produces (VERSION from scripts/version.sh, e.g. 0.1.42):
#   dist/bin/<triple>/lindroids                         the CLI for all 8 targets
#   dist/lindroids-VERSION.apk                          the app, all 4 ABIs, with proot
#   dist/debs/lindroids_VERSION_<arch>.deb              Termux and Debian packages
#   dist/lindroids-VERSION-<arch>-linux-musl.tar.gz     static CLI + README, for other distros
#   dist/SHA256SUMS, dist/VERSION
#
# The CLI is built twice per ABI on purpose:
#   * against bionic (cargo-ndk, API 24) for Termux: Android's app seccomp filter kills static
#     musl binaries that use legacy syscalls on x86_64, x86 and arm32;
#   * as a static musl binary (linked with rust-lld, .cargo/config.toml) for glibc distros under
#     proot and for plain Linux.
#
# Environment: ANDROID_HOME, ANDROID_NDK_HOME (defaults as in build-apk.sh);
# LINDROIDS_KEYSTORE, LINDROIDS_KEYSTORE_PASS, LINDROIDS_KEY_ALIAS for a release-signed APK
# (without them the APK is debug-signed and cannot update a release install);
# LINDROIDS_VERSION / LINDROIDS_VERSION_CODE / LINDROIDS_BUILD_ID to override scripts/version.sh.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD

SKIP_PROOT=0
SKIP_APK=0
CLEAN=0
while [ $# -gt 0 ]; do
  case "$1" in
    --skip-proot) SKIP_PROOT=1; shift ;;
    --skip-apk) SKIP_APK=1; shift ;;
    --clean) CLEAN=1; shift ;;
    -h|--help) awk 'NR > 1 && !/^#/ {exit} NR > 1 {sub(/^# ?/, ""); print}' "$0"; exit 0 ;;
    *) echo "build-release: unknown argument $1" >&2; exit 2 ;;
  esac
done

eval "$(scripts/version.sh)"
export LINDROIDS_VERSION LINDROIDS_VERSION_CODE LINDROIDS_BUILD_ID
VER=$LINDROIDS_VERSION
echo "==> Lindroids $VER (versionCode $LINDROIDS_VERSION_CODE, build $LINDROIDS_BUILD_ID)"

: "${ANDROID_HOME:=$HOME/Android/Sdk}"
: "${ANDROID_NDK_HOME:=$(ls -d "$ANDROID_HOME"/ndk/* 2>/dev/null | sort -V | tail -1)}"
export ANDROID_HOME ANDROID_NDK_HOME

ANDROID_TRIPLES="aarch64-linux-android armv7-linux-androideabi x86_64-linux-android i686-linux-android"
MUSL_TRIPLES="aarch64-unknown-linux-musl armv7-unknown-linux-musleabihf x86_64-unknown-linux-musl i686-unknown-linux-musl"

DIST=$ROOT/dist
if [ $CLEAN = 1 ]; then rm -rf "$DIST"; fi
rm -rf "${DIST:?}/bin" "${DIST:?}/debs" "$DIST"/lindroids-*.tar.gz "$DIST"/lindroids-*.apk "$DIST/SHA256SUMS"
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
# Refuse to ship a musl binary that is not static: it would not start under proot-distro.
for t in $MUSL_TRIPLES; do
  if readelf -l "$DIST/bin/$t/lindroids" 2>/dev/null | grep -q 'Requesting program interpreter'; then
    echo "build-release: $t binary is dynamically linked" >&2
    exit 1
  fi
done

if [ $SKIP_PROOT = 0 ]; then
  scripts/fetch-proot.sh
fi

APK=$DIST/lindroids-$VER.apk
if [ $SKIP_APK = 0 ]; then
  scripts/build-apk.sh --abis "arm64-v8a armeabi-v7a x86_64 x86" --out "$APK"
  if [ -z "${LINDROIDS_KEYSTORE:-}" ]; then
    echo "    WARNING: debug-signed APK; set LINDROIDS_KEYSTORE for a release" >&2
  fi
elif [ -f "$DIST/lindroids.apk" ]; then
  cp "$DIST/lindroids.apk" "$APK"
else
  echo "build-release: --skip-apk needs an existing dist/lindroids.apk" >&2
  exit 1
fi

packaging/build-debs.sh --version "$VER" --dist "$DIST" --apk "$APK" --out "$DIST/debs"

echo "==> tarballs"
: "${SOURCE_DATE_EPOCH:=$(git log -1 --format=%ct 2>/dev/null || date +%s)}"
for t in $MUSL_TRIPLES; do
  arch=${t%%-*}
  name=lindroids-$VER-$arch-linux-musl
  stage=$ROOT/target/tarball/$name
  rm -rf "$stage" && mkdir -p "$stage"
  install -m 755 "$DIST/bin/$t/lindroids" "$stage/lindroids"
  for f in README.md LICENSE LICENSE-MIT LICENSE-APACHE docs/INSTALL.md; do
    [ -f "$f" ] && install -m 644 "$f" "$stage/$(basename "$f")"
  done
  tar --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$SOURCE_DATE_EPOCH" \
    -C "$ROOT/target/tarball" -czf "$DIST/$name.tar.gz" "$name"
  rm -rf "$stage"
  echo "    $name.tar.gz"
done

echo "$VER" > "$DIST/VERSION"
( cd "$DIST" && sha256sum lindroids-*.apk lindroids-*.tar.gz debs/*.deb > SHA256SUMS )
echo "==> dist/ ready"
( cd "$DIST" && ls -1sh lindroids-*.apk lindroids-*.tar.gz debs/*.deb )
