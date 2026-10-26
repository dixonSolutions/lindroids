#!/usr/bin/env bash
# Fetch Termux's prebuilt proot (+ the libraries it links) and lay it out for the APK:
#
#   third_party/proot/<abi>/libproot.so            the proot executable
#   third_party/proot/<abi>/libproot-loader.so     libexec/proot/loader
#   third_party/proot/<abi>/libproot-loader32.so   libexec/proot/loader32 (64-bit ABIs only)
#   third_party/proot/<abi>/libtalloc.so           talloc, SONAME rewritten to libtalloc.so
#   third_party/proot/<abi>/libandroid-shmem.so    SysV shm emulation that Termux's proot links
#
# The files are named lib*.so so the package installer extracts them into the app's
# nativeLibraryDir, the one place an app targeting API 29+ could exec from (and where W^X is not
# an issue at target 28 either). The app runs libproot.so with LD_LIBRARY_PATH=nativeLibraryDir,
# so the Termux RUNPATH is removed and libtalloc.so.2 becomes libtalloc.so.
#
#   scripts/fetch-proot.sh                 fetch the pinned versions (cached in target/proot-debs)
#   scripts/fetch-proot.sh --update-pins   print pin lines for the versions Termux serves today
#   scripts/fetch-proot.sh --force         rebuild third_party/proot even if it is up to date
#
# Needs: curl, ar (binutils), tar with xz, sha256sum, patchelf, readelf.
# Licences: proot is GPL-2.0, talloc LGPL-3.0-or-later, libandroid-shmem BSD-3-Clause; see
# third_party/README.md. The outputs are build artifacts (third_party/proot/ is gitignored).
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD

# Termux removes superseded .debs from its pool, so a pin that 404s everywhere needs
# `--update-pins` and a commit. The sha256 check makes any mirror as good as the origin.
MIRRORS=(
  ${LINDROIDS_TERMUX_MIRROR:+"$LINDROIDS_TERMUX_MIRROR"}
  https://packages.termux.dev/apt/termux-main
  https://packages-cf.termux.dev/apt/termux-main
  https://grimler.se/termux/termux-main
)

# termux-arch  package  pool path  sha256   (from dists/stable/main/binary-<arch>/Packages)
PINS="
aarch64 proot            pool/main/p/proot/proot_5.1.107.101_aarch64.deb                       a28355e9be257e1f87f4e9294b3118470d4fd7a5442fed8b278f4443c83407d7
aarch64 libtalloc        pool/main/libt/libtalloc/libtalloc_2.5.0_aarch64.deb                  556591f43bb773ad8777e1a29522640866a55f95dab71914418b94a8c58ad5a7
aarch64 libandroid-shmem pool/main/liba/libandroid-shmem/libandroid-shmem_0.7_aarch64.deb      0da3a24d558b93c92bcf8d611e0826a99ff96e396b148e6cdf33b47c47c57ff6
arm     proot            pool/main/p/proot/proot_5.1.107.101_arm.deb                           dd1c7010dbe4289030da86450164f10a332b69d1f79ea0751a81252a30977ae4
arm     libtalloc        pool/main/libt/libtalloc/libtalloc_2.5.0_arm.deb                      98ace4f1fcbb04b75e859a12b689a9ae4ac91ff8fd34ab221e5359c95ba47f52
arm     libandroid-shmem pool/main/liba/libandroid-shmem/libandroid-shmem_0.7_arm.deb          5832fd11dca9be2a288dd8fbc2b2799b289c812c7a8764f1f8234c425aa64ce5
x86_64  proot            pool/main/p/proot/proot_5.1.107.101_x86_64.deb                        85adb8d0d2e1edc6c56e60c17136adf05e54ce75f69407ec7116a4c5997eaed5
x86_64  libtalloc        pool/main/libt/libtalloc/libtalloc_2.5.0_x86_64.deb                   b8c6d95f20075dc1f9ec6573575b2444e8d526e48e0d8d6d5cf4e071e6e06530
x86_64  libandroid-shmem pool/main/liba/libandroid-shmem/libandroid-shmem_0.7_x86_64.deb       ffa9e4c87467b158b148d0ff92dda796aa038276c2075af3269cdcdb06f25797
i686    proot            pool/main/p/proot/proot_5.1.107.101_i686.deb                          c50a2029c459d295a7a9c90be55b334cc276e4ea2f5b67e11af89daa24d542fc
i686    libtalloc        pool/main/libt/libtalloc/libtalloc_2.5.0_i686.deb                     7994c6c6ec74a117630f5e8e88273bd701c4e475e638b69826879a32916f5e64
i686    libandroid-shmem pool/main/liba/libandroid-shmem/libandroid-shmem_0.7_i686.deb         e9ccecee1aeed7dd70ac93bf44a6ba1bf6d4cb9559aabeb086acdc89accb4ba4
"
PACKAGES="proot libtalloc libandroid-shmem"
# Libraries the patched files may link: Android system libraries plus the ones we ship.
ALLOWED_NEEDED="libc.so libdl.so libm.so liblog.so libtalloc.so libandroid-shmem.so"

abi_of() {
  case "$1" in
    aarch64) echo arm64-v8a ;;
    arm) echo armeabi-v7a ;;
    x86_64) echo x86_64 ;;
    i686) echo x86 ;;
  esac
}

die() { echo "fetch-proot: $*" >&2; exit 1; }

if [ "${1:-}" = "--update-pins" ]; then
  for arch in aarch64 arm x86_64 i686; do
    idx=$(curl -fsSL "${MIRRORS[0]}/dists/stable/main/binary-$arch/Packages") || die "cannot read the $arch index"
    for pkg in $PACKAGES; do
      printf '%s\n' "$idx" | awk -v RS= -v p="$pkg" -v a="$arch" '
        $0 ~ "(^|\n)Package: " p "\n" {
          n = split($0, l, "\n")
          for (i = 1; i <= n; i++) {
            if (l[i] ~ /^Filename: /) f = substr(l[i], 11)
            if (l[i] ~ /^SHA256: /) s = substr(l[i], 9)
          }
          printf "%-7s %-16s %-69s %s\n", a, p, f, s
        }'
    done
  done
  exit 0
fi
FORCE=0
[ "${1:-}" = "--force" ] && FORCE=1

for tool in curl ar tar sha256sum patchelf readelf; do
  command -v "$tool" >/dev/null || die "$tool is required (patchelf: dnf/apt install patchelf)"
done

CACHE=$ROOT/target/proot-debs
OUT=$ROOT/third_party/proot
STAMP=$OUT/.pins
mkdir -p "$CACHE"
want_stamp=$(printf '%s' "$PINS" | sha256sum | cut -d' ' -f1)
if [ $FORCE = 0 ] && [ -f "$STAMP" ] && [ "$(cat "$STAMP")" = "$want_stamp" ]; then
  echo "==> proot: third_party/proot is up to date"
  exit 0
fi

fetch() { # <pool path> <sha256> -> path in the cache
  local path=$1 sum=$2 file
  file=$CACHE/$(basename "$path")
  if [ -f "$file" ] && echo "$sum  $file" | sha256sum -c --status; then
    echo "$file"
    return
  fi
  for m in "${MIRRORS[@]}"; do
    if curl -fsSL --retry 3 -o "$file.part" "$m/$path" 2>/dev/null; then
      if echo "$sum  $file.part" | sha256sum -c --status; then
        mv "$file.part" "$file"
        echo "$file"
        return
      fi
      echo "fetch-proot: checksum mismatch for $m/$path" >&2
    fi
  done
  rm -f "$file.part"
  die "cannot fetch $path with the pinned sha256 from any mirror.
  Termux drops superseded versions from its pool; run scripts/fetch-proot.sh --update-pins,
  paste the output into PINS in this script and commit it."
}

extract() { # <deb> <dir>
  local deb=$1 dir=$2 tmp
  tmp=$(mktemp -d "$ROOT/target/proot-ar.XXXXXX")
  ( cd "$tmp" && ar x "$deb" )
  local data
  data=$(ls "$tmp"/data.tar.* | head -1)
  case "$data" in
    *.xz) tar -xJf "$data" -C "$dir" ;;
    *.gz) tar -xzf "$data" -C "$dir" ;;
    *.zst) tar --zstd -xf "$data" -C "$dir" ;;
    *) tar -xf "$data" -C "$dir" ;;
  esac
  rm -rf "$tmp"
}

check_elf() { # <file>
  local f=$1 lib
  if readelf -d "$f" | grep -qE '\((RPATH|RUNPATH)\)'; then
    die "$f still has a RUNPATH/RPATH"
  fi
  for lib in $(readelf -d "$f" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p'); do
    case " $ALLOWED_NEEDED " in
      *" $lib "*) ;;
      *) die "$f needs $lib, which is neither an Android system library nor shipped" ;;
    esac
  done
}

rm -rf "$OUT"
mkdir -p "$OUT"
for arch in aarch64 arm x86_64 i686; do
  abi=$(abi_of "$arch")
  echo "==> proot for $abi"
  stage=$(mktemp -d "$ROOT/target/proot-stage.XXXXXX")
  while read -r a pkg path sum; do
    [ "$a" = "$arch" ] || continue
    deb=$(fetch "$path" "$sum")
    extract "$deb" "$stage"
    echo "    $pkg $(basename "$path")"
  done <<<"$(printf '%s\n' "$PINS" | sed '/^$/d')"
  usr=$stage/data/data/com.termux/files/usr
  dest=$OUT/$abi
  mkdir -p "$dest"
  install -m 755 "$usr/bin/proot" "$dest/libproot.so"
  install -m 755 "$usr/libexec/proot/loader" "$dest/libproot-loader.so"
  if [ -f "$usr/libexec/proot/loader32" ]; then
    install -m 755 "$usr/libexec/proot/loader32" "$dest/libproot-loader32.so"
  fi
  install -m 755 "$(readlink -f "$usr/lib/libtalloc.so.2")" "$dest/libtalloc.so"
  install -m 755 "$usr/lib/libandroid-shmem.so" "$dest/libandroid-shmem.so"

  patchelf --replace-needed libtalloc.so.2 libtalloc.so "$dest/libproot.so"
  patchelf --remove-rpath "$dest/libproot.so"
  patchelf --set-soname libtalloc.so "$dest/libtalloc.so"
  patchelf --remove-rpath "$dest/libtalloc.so"
  patchelf --remove-rpath "$dest/libandroid-shmem.so"
  for f in "$dest/libproot.so" "$dest/libtalloc.so" "$dest/libandroid-shmem.so"; do
    check_elf "$f"
  done
  rm -rf "$stage"
  ls "$dest" | sed 's/^/    -> /'
done
echo "$want_stamp" > "$STAMP"
echo "==> third_party/proot ready"
