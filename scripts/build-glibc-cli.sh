#!/usr/bin/env bash
# Build the CLI against glibc for Cortex Terminal (Ubuntu on Android without proot):
#
#   scripts/build-glibc-cli.sh <out dir>     -> <out>/<triple>/lindroids
#
# Cortex maps /usr, /etc, /tmp ... into its own files with an LD_PRELOAD library, which a static
# binary never loads: the static CLI there sees Android's file system, not Ubuntu's. The .debs
# for arm64 and armhf carry this build too (libexec/lindroids/lindroids-glibc), and the static
# CLI hands over to it when it finds itself in Cortex (crates/lindroids-cli/src/cortex.rs).
#
# Built in debian:bookworm (glibc 2.36, older than Ubuntu 24.04's 2.39) with podman or docker,
# as the Debian half of scripts/build-xwayland-satellite.sh; needs the aarch64-unknown-linux-gnu
# and armv7-unknown-linux-gnueabihf Rust targets. LINDROIDS_VERSION / LINDROIDS_BUILD_ID pass
# through to the build.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
IMAGE=docker.io/library/debian:bookworm
TRIPLES="aarch64-unknown-linux-gnu armv7-unknown-linux-gnueabihf"
MAX_GLIBC=2.36

die() { echo "build-glibc-cli: $*" >&2; exit 1; }

# Inside the container: cross compilers, then cargo.
if [ "${1:-}" = --in-container ]; then
  out=$2
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -q >/dev/null
  apt-get install -y -q --no-install-recommends ca-certificates gcc libc6-dev clang lld \
    gcc-aarch64-linux-gnu libc6-dev-arm64-cross gcc-arm-linux-gnueabihf libc6-dev-armhf-cross >/dev/null
  export PATH=/opt/rust/bin:$PATH CARGO_HOME=$ROOT/target/glibc-cli/cargo-home CARGO_TARGET_DIR=$ROOT/target/glibc-cli/build
  for t in $TRIPLES; do
    case "$t" in
      aarch64-*) cc=aarch64-linux-gnu-gcc ;;
      armv7-*) cc=arm-linux-gnueabihf-gcc ;;
    esac
    T=${t^^}
    T=${T//-/_}
    echo "==> CLI, glibc ($t)"
    env "CARGO_TARGET_${T}_LINKER=$cc" "CC_${t//-/_}=$cc" cargo build --locked -p lindroids --release --target "$t"
    mkdir -p "$out/$t"
    install -m 755 "$CARGO_TARGET_DIR/$t/release/lindroids" "$out/$t/lindroids"
  done
  if [ -n "${CHOWN_TO:-}" ]; then chown -R "$CHOWN_TO" "$out" "$ROOT/target/glibc-cli"; fi
  exit 0
fi

[ $# -eq 1 ] || die "usage: $0 <out dir>"
mkdir -p "$1"
OUT=$(cd "$1" && pwd)
case "$OUT" in "$ROOT"/*) ;; *) die "the out dir must be inside $ROOT (it is mounted into the container)" ;; esac
# shellcheck source=../packaging/container.sh
. packaging/container.sh
engine=$(container_engine)
sysroot=$(rustc --print sysroot)
for t in $TRIPLES; do
  [ -d "$sysroot/lib/rustlib/$t/lib" ] || die "the Rust target $t is not installed: rustup target add $t"
done
chown_to=
case "$engine" in *docker*) chown_to="$(id -u):$(id -g)" ;; esac
# shellcheck disable=SC2086
$engine run --rm --security-opt label=disable \
  -v "$ROOT:$ROOT" -v "$sysroot:/opt/rust:ro" -w "$ROOT" \
  -e CHOWN_TO="$chown_to" -e LINDROIDS_VERSION -e LINDROIDS_BUILD_ID -e CARGO_TERM_COLOR \
  -e CARGO_PROFILE_RELEASE_STRIP=symbols \
  "$IMAGE" bash "$ROOT/scripts/build-glibc-cli.sh" --in-container "$OUT"

for t in $TRIPLES; do
  f=$OUT/$t/lindroids
  readelf -l "$f" | grep -q 'Requesting program interpreter' || die "$f is not dynamically linked"
  v=$(readelf -V --wide "$f" | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/GLIBC_//' | sort -uV | tail -1)
  [ "$(printf '%s\n%s\n' "$v" "$MAX_GLIBC" | sort -V | tail -1)" = "$MAX_GLIBC" ] || die "$f needs glibc $v"
  echo "    $t: GLIBC_$v, $(du -h "$f" | cut -f1)"
done
