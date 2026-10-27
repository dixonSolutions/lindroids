#!/usr/bin/env bash
# Build xwayland-satellite from its pinned release source, for Termux (bionic) and for
# Debian/Ubuntu (glibc), for each of the four Lindroids CPU architectures:
#
#   target/xwls/out/termux-<arch>/xwayland-satellite   arch: aarch64 arm x86_64 i686
#                                                      (cargo-ndk, API 24, runs inside Termux)
#   target/xwls/out/debian-<arch>/xwayland-satellite   arch: arm64 armhf amd64 i386
#                                                      (built in debian:bookworm: glibc 2.36)
#   target/xwls/out/LICENSE.xwayland-satellite         the MPL-2.0 text from the source
#   target/xwls/out/xwayland-satellite-crates.txt      the Rust crates linked in, with licences
#
#   scripts/build-xwayland-satellite.sh [termux|debian|all] [--arches "aarch64 amd64"] [--force]
#   scripts/build-xwayland-satellite.sh --check          only verify the existing outputs
#   scripts/build-xwayland-satellite.sh --update-pins    print TERMUX_PINS for today's Termux
#
# Why: `lindroids run --x11` runs X11 programs through xwayland-satellite, which runs Xwayland
# rootless and turns each X11 window into its own xdg_toplevel (so each becomes an Android
# window). Nobody packages it for Termux, Debian or Ubuntu, so Lindroids builds and ships it; the
# CLI looks for <dir of lindroids>/../libexec/lindroids/xwayland-satellite (packaging/).
#
# The source is the GitHub tag tarball of SATELLITE_VERSION, checked against SATELLITE_SHA256,
# built unmodified with default features (no systemd, no fontconfig: the decoration font is
# embedded). It links libxcb and libxcb-cursor; xcb-util-cursor-sys runs bindgen, so the build
# needs libclang and the xcb headers of the target:
#
#   termux  a per-arch sysroot from Termux's own .debs (pinned + sha256)
#           that pkg-config, bindgen and the linker are pointed at; RUNPATH is Termux's lib dir,
#           as for Termux's own binaries, and segments are 16 KB aligned (Android 15+).
#           Needs: cargo, cargo-ndk, the 4 *-linux-android Rust targets, ANDROID_NDK_HOME (or
#           ANDROID_HOME/ndk/*), libclang, pkg-config, curl, ar, tar with xz, sha256sum.
#   debian  a debian:bookworm container (podman, or docker; inside a distrobox it uses the
#           host's podman) with multiarch libxcb1-dev/libxcb-cursor-dev, libclang and cross gcc,
#           so the binaries need at most glibc 2.36. The host's Rust toolchain is mounted
#           read-only: it needs the x86_64/aarch64/i686-unknown-linux-gnu and
#           armv7-unknown-linux-gnueabihf targets. Crates are downloaded inside the container.
#
# Patches (third_party/xwayland-satellite/): xwayland-satellite-*.patch would apply to the
# satellite source (there are none: it is built unmodified). ANDROID_CRATE_PATCHES fix
# dependency crates that do not compile for Android; the Termux build uses a patched copy of the
# locked crate through `--config patch.crates-io.<crate>.path` (Cargo.lock then records it as a
# path dependency, and --locked checks nothing else moved). The Debian build uses none.
#
# Every output is checked with readelf (NEEDED, RUNPATH, interpreter, page alignment, glibc
# version). Outputs carry a stamp of this script, so an unchanged pin is not rebuilt (CI caches
# target/xwls/out keyed by this file and the patches); intermediate build directories are
# deleted afterwards.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD

SATELLITE_VERSION=0.8.2
SATELLITE_COMMIT=8d135d3b2854b30fd01ea6cd6c27e523dd50a839
SATELLITE_URL=https://github.com/Supreeeme/xwayland-satellite/archive/refs/tags/v$SATELLITE_VERSION.tar.gz
SATELLITE_SHA256=cb50bb6948582d5ec3aa511d2d66ad622989bb14bef94e3bb81bae8b64c120b1

DEBIAN_IMAGE=docker.io/library/debian:bookworm

# <crate> <version, as locked> <patch>. smithay-client-toolkit (shm pools) and wayland-cursor (its
# cursor themes) fall back to shm_open through rustix::shm, which rustix does not build for
# Android (bionic has no shm_open), so neither compiles there; the patches make memfd_create the
# only path on Android (sctk only tried it on target_os = "linux"). Upstream, as of
# smithay-client-toolkit 0.21.1 and wayland-cursor 0.31.14, still has both problems.
ANDROID_CRATE_PATCHES="
smithay-client-toolkit 0.20.0 smithay-client-toolkit-0.20.0-android-memfd.patch
wayland-cursor 0.31.11 wayland-cursor-0.31.11-android-no-shm.patch
"

# Termux removes superseded .debs from its pool; a pin that 404s on every mirror needs
# `--update-pins` and a commit. The sha256 check makes any mirror as good as the origin.
MIRRORS=(
  ${LINDROIDS_TERMUX_MIRROR:+"$LINDROIDS_TERMUX_MIRROR"}
  https://packages.termux.dev/apt/termux-
  https://packages-cf.termux.dev/apt/termux-
  https://grimler.se/termux/termux-
)
# <mirror><repo>/<pool path>; repo `main` is suite stable, `x11` is x11-repo (suite x11).
# Headers ship in Termux's main packages; xorgproto provides xproto.pc for xau/xdmcp.
#
# termux-arch  repo  package  pool path  sha256   (from dists/<suite>/main/binary-<arch>/Packages)
TERMUX_PINS="
aarch64 main libxcb              pool/main/libx/libxcb/libxcb_1.17.0-1_aarch64.deb                          191c3642b6a8c754bc75b69b94a81858ecb5f18766dc085aff55559b8087ad34
aarch64 main libxau              pool/main/libx/libxau/libxau_1.0.12-2_aarch64.deb                          2d86bed61ce89dd8fdb55e565aede10155f2ce3adaccb61d5f1375aec3314083
aarch64 main libxdmcp            pool/main/libx/libxdmcp/libxdmcp_1.1.5-2_aarch64.deb                       caea0cf5c0028af0840774094b8900ef7afffd8f9bbd7b1abfbe66851f0d6077
aarch64 main libandroid-shmem    pool/main/liba/libandroid-shmem/libandroid-shmem_0.7_aarch64.deb           0da3a24d558b93c92bcf8d611e0826a99ff96e396b148e6cdf33b47c47c57ff6
aarch64 main xorgproto           pool/main/x/xorgproto/xorgproto_2026.1_all.deb                             2463651143a386eba0d7baebac87ca5d2db5239d36cf38f9b2086f3c1bfdcbce
aarch64 x11  xcb-util-cursor     pool/main/x/xcb-util-cursor/xcb-util-cursor_0.1.6_aarch64.deb              4d0793ae0cae824fd63dc8916a42b82106196674cb09a6bc64f5bea8d2224926
aarch64 x11  xcb-util-image      pool/main/x/xcb-util-image/xcb-util-image_0.4.1-2_aarch64.deb              c81d28922a8f338a2831cabc582fadd242145fc98158fd3d32add29942708574
aarch64 x11  xcb-util-renderutil pool/main/x/xcb-util-renderutil/xcb-util-renderutil_0.3.10-1_aarch64.deb   6ecce3b800cb93b26775cbd2d46c77bc25604d4e3263cbda4ba5df4de8f2e12d
aarch64 x11  xcb-util            pool/main/x/xcb-util/xcb-util_0.4.1-1_aarch64.deb                          95f985a71136c6036a61db655315b291b7ef83e281bedc4493838c91d9da4f2e
arm     main libxcb              pool/main/libx/libxcb/libxcb_1.17.0-1_arm.deb                              fff800917f83666b522fb0f9923f724aaf8edd802d7761d742ea55645a79c5c9
arm     main libxau              pool/main/libx/libxau/libxau_1.0.12-2_arm.deb                              46a7c913fdb1ccf3e803fed55ceac41deecbfd36a6dca86c851341a99e460fb3
arm     main libxdmcp            pool/main/libx/libxdmcp/libxdmcp_1.1.5-2_arm.deb                           0db6b14a17c769598cba3cfac38814a4493c445a44a625f89bd99223cf142e74
arm     main libandroid-shmem    pool/main/liba/libandroid-shmem/libandroid-shmem_0.7_arm.deb               5832fd11dca9be2a288dd8fbc2b2799b289c812c7a8764f1f8234c425aa64ce5
arm     main xorgproto           pool/main/x/xorgproto/xorgproto_2026.1_all.deb                             2463651143a386eba0d7baebac87ca5d2db5239d36cf38f9b2086f3c1bfdcbce
arm     x11  xcb-util-cursor     pool/main/x/xcb-util-cursor/xcb-util-cursor_0.1.6_arm.deb                  7464c50be581f26851101396923ac06fdd0827c27a96f51544eecd6153eee62e
arm     x11  xcb-util-image      pool/main/x/xcb-util-image/xcb-util-image_0.4.1-2_arm.deb                  69dd822a0754afbaec3944df2c48e4cad2a9071306e23e42c33b01da3d2f7390
arm     x11  xcb-util-renderutil pool/main/x/xcb-util-renderutil/xcb-util-renderutil_0.3.10-1_arm.deb       058f78b6bcf0801444e4708a1b6d88849821bf10e736d96c42a0a13e1de8414b
arm     x11  xcb-util            pool/main/x/xcb-util/xcb-util_0.4.1-1_arm.deb                              a08d8afd8b0eca1b089a4f21028cccb91f94dfd678cfabbbd20d6a5ccb4bc493
x86_64  main libxcb              pool/main/libx/libxcb/libxcb_1.17.0-1_x86_64.deb                           38da04c89e95026282c1277ca475fe02aca78735e0af48e3aada6b5aed78b894
x86_64  main libxau              pool/main/libx/libxau/libxau_1.0.12-2_x86_64.deb                           b964143f84f11270ba14baf9a747b9732f5a9b84d2ca0e70ca26ba0364e0ce0b
x86_64  main libxdmcp            pool/main/libx/libxdmcp/libxdmcp_1.1.5-2_x86_64.deb                        814a80d52fdfd193286f3a2e50215a10f03f6e64579cbdef3886cf499adf896d
x86_64  main libandroid-shmem    pool/main/liba/libandroid-shmem/libandroid-shmem_0.7_x86_64.deb            ffa9e4c87467b158b148d0ff92dda796aa038276c2075af3269cdcdb06f25797
x86_64  main xorgproto           pool/main/x/xorgproto/xorgproto_2026.1_all.deb                             2463651143a386eba0d7baebac87ca5d2db5239d36cf38f9b2086f3c1bfdcbce
x86_64  x11  xcb-util-cursor     pool/main/x/xcb-util-cursor/xcb-util-cursor_0.1.6_x86_64.deb               4d9a23656763fe845caac91b91174223049eb6a707eaac841a30774ae91794b1
x86_64  x11  xcb-util-image      pool/main/x/xcb-util-image/xcb-util-image_0.4.1-2_x86_64.deb               8d5411dbf1401dcaff03a1db40e5d4f47f36df1fb8538ee8ed2889c16cfb8e63
x86_64  x11  xcb-util-renderutil pool/main/x/xcb-util-renderutil/xcb-util-renderutil_0.3.10-1_x86_64.deb    3685d42c5483267d6d89c9bfb59e8dd601a40d5849f82322839802bce938aec1
x86_64  x11  xcb-util            pool/main/x/xcb-util/xcb-util_0.4.1-1_x86_64.deb                           ad2f3098378db6acd008f69dbbd9deef15ef6074f21a179cb535275aa93977b5
i686    main libxcb              pool/main/libx/libxcb/libxcb_1.17.0-1_i686.deb                             535163d311af6c1037a448b33e8cbc99446caa8e66ae11861c454c3fdd3d5d6c
i686    main libxau              pool/main/libx/libxau/libxau_1.0.12-2_i686.deb                             3610d1bcc14f04b96d17b91741094a1600856a61e4ad97019c95b9efa712b4f3
i686    main libxdmcp            pool/main/libx/libxdmcp/libxdmcp_1.1.5-2_i686.deb                          0f3a98b048730fd1ebe964da3d637540edcef8a2c9485fd84182cc7671715ba8
i686    main libandroid-shmem    pool/main/liba/libandroid-shmem/libandroid-shmem_0.7_i686.deb              e9ccecee1aeed7dd70ac93bf44a6ba1bf6d4cb9559aabeb086acdc89accb4ba4
i686    main xorgproto           pool/main/x/xorgproto/xorgproto_2026.1_all.deb                             2463651143a386eba0d7baebac87ca5d2db5239d36cf38f9b2086f3c1bfdcbce
i686    x11  xcb-util-cursor     pool/main/x/xcb-util-cursor/xcb-util-cursor_0.1.6_i686.deb                 98b191d3b26c3408308c3746d700290351c9b9a0e928bc38304d580168f58965
i686    x11  xcb-util-image      pool/main/x/xcb-util-image/xcb-util-image_0.4.1-2_i686.deb                 df8acf5d2ce1c81b8aa69d267579f35ee2c5136ed07091bee6418937950bbf05
i686    x11  xcb-util-renderutil pool/main/x/xcb-util-renderutil/xcb-util-renderutil_0.3.10-1_i686.deb      dd1dadead5df21b9e529e3d598bf724084483bb7a815576a7183752a0d49e2b7
i686    x11  xcb-util            pool/main/x/xcb-util/xcb-util_0.4.1-1_i686.deb                             e4d7de56cb6d90d40ae317f1194759b06b97a2ec748c9f28632b396a2d67beb6
"
TERMUX_MAIN_PACKAGES="libxcb libxau libxdmcp libandroid-shmem xorgproto"
TERMUX_X11_PACKAGES="xcb-util-cursor xcb-util-image xcb-util-renderutil xcb-util"
TERMUX_PREFIX=/data/data/com.termux/files/usr

# Libraries the outputs may link: the Android or glibc system libraries plus libxcb and
# libxcb-cursor (Termux packages libxcb + xcb-util-cursor; Debian libxcb1 + libxcb-cursor0).
TERMUX_ALLOWED_NEEDED="libc.so libdl.so libm.so liblog.so libxcb.so libxcb-cursor.so"
DEBIAN_ALLOWED_NEEDED="libc.so.6 libm.so.6 libdl.so.2 libpthread.so.0 librt.so.1 libgcc_s.so.1
  ld-linux-aarch64.so.1 ld-linux-armhf.so.3 ld-linux-x86-64.so.2 ld-linux.so.2
  libxcb.so.1 libxcb-cursor.so.0"
DEBIAN_MAX_GLIBC=2.36

ALL_TERMUX_ARCHES="aarch64 arm x86_64 i686"
ALL_DEBIAN_ARCHES="arm64 armhf amd64 i386"

WORK=$ROOT/target/xwls
OUT=$WORK/out
SRC=$WORK/src/xwayland-satellite-$SATELLITE_VERSION
STAMP=$(cat "$ROOT/scripts/build-xwayland-satellite.sh" "$ROOT"/third_party/xwayland-satellite/*.patch 2>/dev/null \
  | sha256sum | cut -d' ' -f1)
# Upstream builds from git get `git describe` (v0.8.2) as their version string; the tarball has
# no .git, and target/ sits inside the Lindroids checkout, whose describe must not leak in.
export VERGEN_GIT_DESCRIBE=v$SATELLITE_VERSION
export GIT_CEILING_DIRECTORIES=$WORK/src
export CARGO_PROFILE_RELEASE_STRIP=symbols

die() { echo "build-xwayland-satellite: $*" >&2; exit 1; }
need() { local t; for t in "$@"; do command -v "$t" >/dev/null || die "$t is required"; done; }

termux_triple() {
  case "$1" in
    aarch64) echo aarch64-linux-android ;;
    arm) echo armv7-linux-androideabi ;;
    x86_64) echo x86_64-linux-android ;;
    i686) echo i686-linux-android ;;
    *) return 1 ;;
  esac
}
debian_triple() {
  case "$1" in
    arm64) echo aarch64-unknown-linux-gnu ;;
    armhf) echo armv7-unknown-linux-gnueabihf ;;
    amd64) echo x86_64-unknown-linux-gnu ;;
    i386) echo i686-unknown-linux-gnu ;;
    *) return 1 ;;
  esac
}

# ---------------------------------------------------------------------------------------------
# Inside the debian:bookworm container (the script runs itself there; see build_debian).
# ---------------------------------------------------------------------------------------------
if [ "${1:-}" = --in-container ]; then
  shift
  arches=$*
  export DEBIAN_FRONTEND=noninteractive
  pkgs="ca-certificates gcc libc6-dev libclang-dev pkgconf"
  for a in $arches; do
    [ "$a" = amd64 ] || dpkg --add-architecture "$a"
    case "$a" in
      arm64) pkgs+=" gcc-aarch64-linux-gnu" ;;
      armhf) pkgs+=" gcc-arm-linux-gnueabihf" ;;
      i386) pkgs+=" gcc-i686-linux-gnu" ;;
    esac
    pkgs+=" libc6-dev:$a libxcb1-dev:$a libxcb-cursor-dev:$a"
  done
  apt-get update -q >/dev/null
  # shellcheck disable=SC2086
  apt-get install -y -q --no-install-recommends $pkgs >/dev/null
  export PATH=/opt/rust/bin:$PATH CARGO_HOME=$WORK/cargo-home CARGO_TARGET_DIR=$WORK/build-debian
  cd "$SRC"
  cp Cargo.lock.upstream Cargo.lock
  for a in $arches; do
    t=$(debian_triple "$a")
    case "$a" in
      arm64) cc=aarch64-linux-gnu-gcc; ma=aarch64-linux-gnu ;;
      armhf) cc=arm-linux-gnueabihf-gcc; ma=arm-linux-gnueabihf ;;
      amd64) cc=gcc; ma=x86_64-linux-gnu ;;
      i386) cc=i686-linux-gnu-gcc; ma=i386-linux-gnu ;;
    esac
    T=${t^^}
    T=${T//-/_}
    echo "==> xwayland-satellite for Debian $a ($t)"
    env "CARGO_TARGET_${T}_LINKER=$cc" "CC_${t//-/_}=$cc" \
      PKG_CONFIG_ALLOW_CROSS=1 PKG_CONFIG_LIBDIR="/usr/lib/$ma/pkgconfig:/usr/share/pkgconfig" \
      cargo build --locked --release --target "$t" --bin xwayland-satellite
    mkdir -p "$OUT/debian-$a"
    install -m 755 "$CARGO_TARGET_DIR/$t/release/xwayland-satellite" "$OUT/debian-$a/xwayland-satellite"
  done
  if [ -n "${CHOWN_TO:-}" ]; then chown -R "$CHOWN_TO" "$WORK"; fi
  exit 0
fi

# ---------------------------------------------------------------------------------------------
if [ "${1:-}" = --update-pins ]; then
  need curl
  for arch in $ALL_TERMUX_ARCHES; do
    for repo in main x11; do
      suite=stable
      pkgs=$TERMUX_MAIN_PACKAGES
      if [ $repo = x11 ]; then suite=x11; pkgs=$TERMUX_X11_PACKAGES; fi
      idx=$(curl -fsSL "${MIRRORS[0]}$repo/dists/$suite/main/binary-$arch/Packages") \
        || die "cannot read the $repo index for $arch"
      for pkg in $pkgs; do
        printf '%s\n' "$idx" | awk -v RS= -v p="$pkg" -v a="$arch" -v r="$repo" '
          $0 ~ "(^|\n)Package: " p "\n" {
            n = split($0, l, "\n")
            for (i = 1; i <= n; i++) {
              if (l[i] ~ /^Filename: /) f = substr(l[i], 11)
              if (l[i] ~ /^SHA256: /) s = substr(l[i], 9)
            }
            printf "%-7s %-4s %-19s %-74s %s\n", a, r, p, f, s
          }'
      done
    done
  done
  exit 0
fi

FLAVOUR=all
ARCHES=
FORCE=0
CHECK_ONLY=0
while [ $# -gt 0 ]; do
  case "$1" in
    termux|debian|all) FLAVOUR=$1; shift ;;
    --arches) ARCHES=$2; shift 2 ;;
    --force) FORCE=1; shift ;;
    --check) CHECK_ONLY=1; shift ;;
    -h|--help) awk 'NR > 1 && !/^#/ {exit} NR > 1 {sub(/^# ?/, ""); print}' "$0"; exit 0 ;;
    *) die "unknown argument $1" ;;
  esac
done
want() { # <arch>: is it selected?
  [ -z "$ARCHES" ] && return 0
  case " $ARCHES " in *" $1 "*) return 0 ;; esac
  return 1
}
TERMUX_ARCHES=
DEBIAN_ARCHES=
if [ "$FLAVOUR" != debian ]; then
  for a in $ALL_TERMUX_ARCHES; do want "$a" && TERMUX_ARCHES+=" $a"; done
fi
if [ "$FLAVOUR" != termux ]; then
  for a in $ALL_DEBIAN_ARCHES; do want "$a" && DEBIAN_ARCHES+=" $a"; done
fi
[ -n "$TERMUX_ARCHES$DEBIAN_ARCHES" ] || die "no architecture selected (termux: $ALL_TERMUX_ARCHES; debian: $ALL_DEBIAN_ARCHES)"

up_to_date() { # <out subdir>
  [ $FORCE = 0 ] && [ -x "$OUT/$1/xwayland-satellite" ] && [ -f "$OUT/$1/.stamp" ] \
    && [ "$(cat "$OUT/$1/.stamp")" = "$STAMP" ]
}

fetch_source() {
  [ -f "$SRC/Cargo.lock" ] && [ "$(cat "$SRC/.lindroids-stamp" 2>/dev/null)" = "$STAMP" ] && return 0
  need curl tar sha256sum
  local tarball=$WORK/cache/xwayland-satellite-$SATELLITE_VERSION.tar.gz
  mkdir -p "$WORK/cache"
  if ! { [ -f "$tarball" ] && echo "$SATELLITE_SHA256  $tarball" | sha256sum -c --status; }; then
    echo "==> downloading $SATELLITE_URL"
    curl -fsSL --retry 3 -o "$tarball.part" "$SATELLITE_URL" || die "cannot download $SATELLITE_URL"
    echo "$SATELLITE_SHA256  $tarball.part" | sha256sum -c --status \
      || die "checksum mismatch for $SATELLITE_URL (expected $SATELLITE_SHA256)"
    mv "$tarball.part" "$tarball"
  fi
  # GitHub writes the commit into the archive's pax header; check it names the pinned commit.
  if command -v git >/dev/null; then
    local commit
    commit=$(gzip -dc "$tarball" | git get-tar-commit-id || true)
    [ "$commit" = "$SATELLITE_COMMIT" ] || die "the tarball is commit '$commit', not $SATELLITE_COMMIT"
  fi
  rm -rf "$WORK/src"
  mkdir -p "$WORK/src"
  tar -xzf "$tarball" -C "$WORK/src"
  [ -f "$SRC/Cargo.lock" ] || die "unexpected tarball layout (no $SRC/Cargo.lock)"
  local p
  for p in "$ROOT"/third_party/xwayland-satellite/xwayland-satellite-*.patch; do
    [ -f "$p" ] || continue
    echo "    applying $(basename "$p")"
    patch -d "$SRC" -p1 --forward --quiet < "$p"
  done
  cp "$SRC/Cargo.lock" "$SRC/Cargo.lock.upstream"
  echo "$STAMP" > "$SRC/.lindroids-stamp"
}

# Patched copies of the ANDROID_CRATE_PATCHES crates; sets CRATE_PATCH_ARGS and writes the
# matching Cargo.lock (those packages become path dependencies: no source, no checksum).
CRATE_PATCH_ARGS=()
android_crate_patches() {
  need curl tar sha256sum patch
  cp "$SRC/Cargo.lock.upstream" "$SRC/Cargo.lock"
  CRATE_PATCH_ARGS=()
  local crate ver patchfile sum file dir
  while read -r crate ver patchfile; do
    [ -n "$crate" ] || continue
    sum=$(awk -v n="$crate" -v v="$ver" '
      $0 == "name = \"" n "\"" { name = 1; next }
      name && $0 == "version = \"" v "\"" { hit = 1 }
      { name = 0 }
      hit && /^checksum = / { gsub(/^checksum = "|"$/, ""); print; exit }
      /^$/ { hit = 0 }' "$SRC/Cargo.lock.upstream")
    [ -n "$sum" ] || die "$crate $ver is not in xwayland-satellite's Cargo.lock"
    file=$WORK/cache/$crate-$ver.crate
    if ! { [ -f "$file" ] && echo "$sum  $file" | sha256sum -c --status; }; then
      curl -fsSL --retry 3 -o "$file.part" "https://static.crates.io/crates/$crate/$crate-$ver.crate" \
        || die "cannot download $crate $ver"
      echo "$sum  $file.part" | sha256sum -c --status || die "checksum mismatch for $crate $ver"
      mv "$file.part" "$file"
    fi
    dir=$WORK/crates/$crate-$ver
    rm -rf "$dir"
    mkdir -p "$WORK/crates"
    tar -xzf "$file" -C "$WORK/crates"
    patch -d "$dir" -p1 --forward --quiet < "$ROOT/third_party/xwayland-satellite/$patchfile"
    echo "    $crate $ver patched with $patchfile"
    CRATE_PATCH_ARGS+=(--config "patch.crates-io.$crate.path=\"$dir\"")
    awk -v n="$crate" -v v="$ver" '
      $0 == "name = \"" n "\"" { name = 1; print; next }
      name && $0 == "version = \"" v "\"" { hit = 1 }
      { name = 0 }
      hit && /^(source|checksum) = / { next }
      /^$/ { hit = 0 }
      { print }' "$SRC/Cargo.lock" > "$SRC/Cargo.lock.new"
    mv "$SRC/Cargo.lock.new" "$SRC/Cargo.lock"
  done <<<"$ANDROID_CRATE_PATCHES"
}

# ---- Termux sysroot from pinned debs ---------------------------------------------------------
fetch_deb() { # <repo> <pool path> <sha256> -> path in the cache
  local repo=$1 path=$2 sum=$3 file m
  file=$WORK/debs/$(basename "$path")
  if [ -f "$file" ] && echo "$sum  $file" | sha256sum -c --status; then
    echo "$file"
    return
  fi
  for m in "${MIRRORS[@]}"; do
    if curl -fsSL --retry 3 -o "$file.part" "$m$repo/$path" 2>/dev/null; then
      if echo "$sum  $file.part" | sha256sum -c --status; then
        mv "$file.part" "$file"
        echo "$file"
        return
      fi
      echo "build-xwayland-satellite: checksum mismatch for $m$repo/$path" >&2
    fi
  done
  rm -f "$file.part"
  die "cannot fetch $repo/$path with the pinned sha256 from any mirror.
  Termux drops superseded versions from its pool; run scripts/build-xwayland-satellite.sh
  --update-pins, paste the output into TERMUX_PINS in this script and commit it."
}

extract_deb() { # <deb> <dir>
  local deb=$1 dir=$2 tmp data
  tmp=$(mktemp -d "$WORK/ar.XXXXXX")
  ( cd "$tmp" && ar x "$deb" )
  data=$(ls "$tmp"/data.tar.* | head -1)
  case "$data" in
    *.xz) tar -xJf "$data" -C "$dir" ;;
    *.gz) tar -xzf "$data" -C "$dir" ;;
    *.zst) tar --zstd -xf "$data" -C "$dir" ;;
    *) tar -xf "$data" -C "$dir" ;;
  esac
  rm -rf "$tmp"
}

termux_sysroot() { # <arch> -> sysroot dir
  local arch=$1 sr=$WORK/sysroot/termux-$1 a repo pkg path sum deb
  rm -rf "$sr"
  mkdir -p "$sr" "$WORK/debs"
  while read -r a repo pkg path sum; do
    [ "$a" = "$arch" ] || continue
    deb=$(fetch_deb "$repo" "$path" "$sum")
    extract_deb "$deb" "$sr"
  done <<<"$(printf '%s\n' "$TERMUX_PINS" | sed '/^$/d')"
  [ -f "$sr$TERMUX_PREFIX/include/xcb/xcb_cursor.h" ] || die "no xcb_cursor.h in the $arch sysroot"
  echo "$sr"
}

build_termux() {
  local arch todo=
  for arch in $TERMUX_ARCHES; do up_to_date "termux-$arch" || todo+=" $arch"; done
  [ -n "$todo" ] || { echo "==> xwayland-satellite for Termux: up to date"; return 0; }
  need cargo cargo-ndk pkg-config curl ar tar sha256sum
  : "${ANDROID_HOME:=$HOME/Android/Sdk}"
  : "${ANDROID_NDK_HOME:=$(ls -d "$ANDROID_HOME"/ndk/* 2>/dev/null | sort -V | tail -1)}"
  [ -d "${ANDROID_NDK_HOME:-}" ] || die "no NDK: set ANDROID_NDK_HOME"
  export ANDROID_HOME ANDROID_NDK_HOME
  fetch_source
  android_crate_patches
  local build=$WORK/build-termux
  for arch in $todo; do
    local t T sr lib
    t=$(termux_triple "$arch")
    T=${t^^}
    T=${T//-/_}
    echo "==> xwayland-satellite for Termux $arch ($t)"
    sr=$(termux_sysroot "$arch")
    lib=$sr$TERMUX_PREFIX/lib
    # pkg-config prefixes the .pc paths with the sysroot, which gives bindgen its -I and the
    # linker its -L; cargo-ndk sets bindgen's --sysroot to the NDK's. Same 16 KB alignment as
    # the CLI (.cargo/config.toml); RUNPATH as in Termux's own binaries.
    ( cd "$SRC" && env \
        PKG_CONFIG_ALLOW_CROSS=1 PKG_CONFIG_SYSROOT_DIR="$sr" \
        PKG_CONFIG_LIBDIR="$lib/pkgconfig:$sr$TERMUX_PREFIX/share/pkgconfig" \
        PKG_CONFIG_PATH= \
        "CARGO_TARGET_${T}_RUSTFLAGS=-C link-arg=-Wl,-z,max-page-size=16384 -C link-arg=-Wl,--enable-new-dtags -C link-arg=-Wl,-rpath,$TERMUX_PREFIX/lib -C link-arg=-Wl,-rpath-link,$lib -L native=$lib" \
        CARGO_TARGET_DIR="$build" \
        cargo ndk -t "$t" -P 24 build --locked --release --bin xwayland-satellite "${CRATE_PATCH_ARGS[@]}" )
    mkdir -p "$OUT/termux-$arch"
    install -m 755 "$build/$t/release/xwayland-satellite" "$OUT/termux-$arch/xwayland-satellite"
    check_termux "$arch"
    echo "$STAMP" > "$OUT/termux-$arch/.stamp"
    rm -rf "$sr"
  done
  rm -rf "$build" "$WORK/sysroot" "$WORK/crates"
  cp "$SRC/Cargo.lock.upstream" "$SRC/Cargo.lock"
}

# ---- Debian, in a container ------------------------------------------------------------------
container_engine() {
  if [ -n "${LINDROIDS_CONTAINER_ENGINE:-}" ]; then
    echo "$LINDROIDS_CONTAINER_ENGINE"
  elif command -v podman >/dev/null; then
    echo podman
  elif command -v docker >/dev/null; then
    echo docker
  elif command -v distrobox-host-exec >/dev/null && distrobox-host-exec sh -c 'command -v podman' >/dev/null 2>&1; then
    echo "distrobox-host-exec podman"   # inside a distrobox: $HOME is the same path on the host
  else
    die "the Debian build needs podman or docker"
  fi
}

build_debian() {
  local arch todo=
  for arch in $DEBIAN_ARCHES; do up_to_date "debian-$arch" || todo+=" $arch"; done
  [ -n "$todo" ] || { echo "==> xwayland-satellite for Debian: up to date"; return 0; }
  need rustc
  local sysroot t
  sysroot=$(rustc --print sysroot)
  for arch in $todo; do
    t=$(debian_triple "$arch")
    [ -d "$sysroot/lib/rustlib/$t/lib" ] || die "the Rust target $t is not installed: rustup target add $t"
  done
  fetch_source
  local engine chown_to=
  engine=$(container_engine)
  # Rootless podman maps container root to the calling user; docker's root is the real root.
  case "$engine" in *docker*) chown_to="$(id -u):$(id -g)" ;; esac
  echo "==> Debian builds in $DEBIAN_IMAGE ($engine):$todo"
  # shellcheck disable=SC2086
  $engine run --rm --security-opt label=disable \
    -v "$ROOT:$ROOT" -v "$sysroot:/opt/rust:ro" -w "$ROOT" \
    -e CHOWN_TO="$chown_to" -e VERGEN_GIT_DESCRIBE -e GIT_CEILING_DIRECTORIES \
    -e CARGO_PROFILE_RELEASE_STRIP -e CARGO_TERM_COLOR \
    "$DEBIAN_IMAGE" bash "$ROOT/scripts/build-xwayland-satellite.sh" --in-container $todo
  for arch in $todo; do
    check_debian "$arch"
    echo "$STAMP" > "$OUT/debian-$arch/.stamp"
  done
  rm -rf "$WORK/build-debian" "$WORK/cargo-home"
}

# ---- Checks ----------------------------------------------------------------------------------
needed() { readelf -d "$1" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p' | tr '\n' ' ' | sed 's/ $//'; }
runpath() { readelf -d "$1" | sed -n 's/.*(\(RPATH\|RUNPATH\)).*\[\(.*\)\]/\1 \2/p'; }
interp() { readelf -l "$1" | sed -n 's/.*Requesting program interpreter: \(.*\)\]/\1/p'; }
max_glibc() { readelf -V --wide "$1" | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/GLIBC_//' | sort -uV | tail -1; }
check_needed() { # <file> <allowed list>
  local lib allowed
  allowed=" $(printf '%s' "$2" | tr -s ' \n' '  ') "
  for lib in $(needed "$1"); do
    case "$allowed" in *" $lib "*) ;; *) die "$1 needs $lib, which is not allowed:$allowed" ;; esac
  done
}

check_termux() { # <arch>
  local f=$OUT/termux-$1/xwayland-satellite want_interp=/system/bin/linker64 align
  [ "$1" = arm ] || [ "$1" = i686 ] && want_interp=/system/bin/linker
  check_needed "$f" "$TERMUX_ALLOWED_NEEDED"
  [ "$(runpath "$f")" = "RUNPATH $TERMUX_PREFIX/lib" ] || die "$f: RUNPATH is '$(runpath "$f")'"
  [ "$(interp "$f")" = "$want_interp" ] || die "$f: interpreter is '$(interp "$f")'"
  # Every LOAD segment aligned to at least 16 KB.
  for align in $(readelf -lW "$f" | awk '$1 == "LOAD" {print $NF}'); do
    [ $((align)) -ge 16384 ] || die "$f: LOAD segment aligned to $align, not 16 KB"
  done
  echo "    termux-$1: NEEDED $(needed "$f"); $(runpath "$f"); interp $(interp "$f"); $(du -h "$f" | cut -f1)"
}

check_debian() { # <arch>
  local f=$OUT/debian-$1/xwayland-satellite v
  check_needed "$f" "$DEBIAN_ALLOWED_NEEDED"
  [ -z "$(runpath "$f")" ] || die "$f has $(runpath "$f")"
  [ -n "$(interp "$f")" ] || die "$f has no program interpreter"
  v=$(max_glibc "$f")
  [ "$(printf '%s\n%s\n' "$v" "$DEBIAN_MAX_GLIBC" | sort -V | tail -1)" = "$DEBIAN_MAX_GLIBC" ] \
    || die "$f needs glibc $v, newer than bookworm's $DEBIAN_MAX_GLIBC"
  echo "    debian-$1: NEEDED $(needed "$f"); interp $(interp "$f"); GLIBC_$v; $(du -h "$f" | cut -f1)"
}

notices() {
  [ -f "$OUT/LICENSE.xwayland-satellite" ] && [ -f "$OUT/xwayland-satellite-crates.txt" ] \
    && [ "$(cat "$OUT/.notices-stamp" 2>/dev/null)" = "$STAMP" ] && return 0
  fetch_source
  need cargo
  install -m 644 "$SRC/LICENSE" "$OUT/LICENSE.xwayland-satellite"
  # The crates the binary is built from (normal dependencies on Linux and Android, including
  # proc-macros); macros and wl_drm are workspace members of xwayland-satellite itself.
  {
    echo "Rust crates xwayland-satellite $SATELLITE_VERSION is built from (its Cargo.lock):"
    echo "<crate> <version> <licence>"
    ( cd "$SRC" && cargo tree --locked -e normal --prefix none --format '{p} {l}' \
        --target x86_64-unknown-linux-gnu --target x86_64-linux-android ) \
      | sed 's/ (proc-macro)//; s/ (\*)//; s/ (\/[^)]*)//' \
      | awk 'NF == 2 { $0 = $0 " MPL-2.0 (part of xwayland-satellite)" } { print }' | sort -u
  } > "$OUT/xwayland-satellite-crates.txt"
  echo "$STAMP" > "$OUT/.notices-stamp"
}

need readelf
mkdir -p "$OUT"
if [ $CHECK_ONLY = 1 ]; then
  for a in $TERMUX_ARCHES; do check_termux "$a"; done
  for a in $DEBIAN_ARCHES; do check_debian "$a"; done
  exit 0
fi
[ -z "$TERMUX_ARCHES" ] || build_termux
[ -z "$DEBIAN_ARCHES" ] || build_debian
notices
echo "==> xwayland-satellite $SATELLITE_VERSION in target/xwls/out"
for a in $TERMUX_ARCHES; do check_termux "$a"; done
for a in $DEBIAN_ARCHES; do check_debian "$a"; done
