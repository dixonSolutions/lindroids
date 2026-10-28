#!/data/data/com.termux/files/usr/bin/sh
# Build ostree and Flatpak natively for Termux (bionic) and install them into $PREFIX, with no
# root and no proot distro. Termux has no flatpak package; this is how Lindroids gets one.
#
#   sh build-in-termux.sh          (in Termux; about 20 minutes, ~600 MB of build tools)
#   sh build-in-termux.sh --package <dir>
#                                  the same, built afresh, and also packed as <dir>/
#                                  lindroids-flatpak_<version>_<arch>.deb, which install.sh
#                                  installs in seconds instead of building (the release publishes
#                                  it in the termux suite; its Depends are the Termux packages of
#                                  the libraries its programs link)
#
# What makes it work on Android (all in this directory):
#   bionic-compat.h                 glibc functions bionic lacks at API 24 (strdupa, syncfs,
#                                   strverscmp, explicit_bzero, qsort_r), force-included
#   flatpak-android.patch           temp files in $TMPDIR instead of /tmp; Termux's resolv.conf
#                                   when Android has no /etc/resolv.conf
#   libglnx-android-seccomp.patch   no openat2/open_tree: Android's app seccomp filter kills them
#                                   (SIGSYS) instead of failing, so libglnx's own fallback is used
#   ostree-android-nolink.patch     a hard link refused with EACCES falls back to a copy, like
#   libglnx-android-nolink.patch    EPERM; and libglnx renames instead of linking. Apps that target
#                                   Android 10 or later may not make hard links (SELinux), and
#                                   Termux from Google Play does (F-Droid's targets Android 9)
# Flatpak's bubblewrap is $PREFIX/libexec/lindroids/bwrap (`lindroids bwrap`), which leaves out
# the namespaces the kernel lacks; it only matters for root (`tsu`), since an app user cannot
# create namespaces at all.
#
# Without root: `flatpak --user install flathub <app>` installs (its post-install triggers need
# bubblewrap and are skipped with a warning), and `lindroids run <app>` runs it under proot.
# With root (tsu): `flatpak install` and `flatpak run` work too, sandboxed by bubblewrap.
set -eu

OSTREE_VERSION=2026.5
OSTREE_SHA256=1829428ed2d982dc684211ff18525b57ea627056a68fa5b04f674f0ca6c9c19b
FLATPAK_VERSION=1.18.4
FLATPAK_SHA256=b899a7a00c48d2c626cb8ec33fe556720b376c25805d7222430c0fdca4c6ac8d
FLATHUB=https://dl.flathub.org/repo/flathub.flatpakrepo

[ -n "${PREFIX:-}" ] && [ -d "$PREFIX" ] || { echo "build-in-termux: run this in Termux" >&2; exit 1; }
HERE=$(cd "$(dirname "$0")" && pwd)
PKGDIR=
case "${1:-}" in
  --package)
    [ -n "${2:-}" ] || { echo "build-in-termux: --package <dir>" >&2; exit 2; }
    mkdir -p "$2"
    PKGDIR=$(cd "$2" && pwd)
    ;;
  '') ;;
  *) echo "build-in-termux: unknown option $1" >&2; exit 2 ;;
esac
WORK=${TMPDIR:-$PREFIX/tmp}/lindroids-flatpak-build
# Few parallel jobs: on a phone, Android 12+'s phantom process killer ends an app's children when
# there are too many of them (and big parallel builds run out of memory); either kills the build
# half way. LINDROIDS_BUILD_JOBS overrides.
JOBS=${LINDROIDS_BUILD_JOBS:-2}
say() { printf '\033[1m==> %s\033[0m\n' "$*"; }

# step <log> <command...>: run it with its output in <log>; if it fails, say which step, where
# its log is and how the log ends, and stop.
step() {
  log=$1
  shift
  "$@" > "$log" 2>&1 && return 0
  echo "build-in-termux: failed: $* (in $(pwd); the whole log: $log)" >&2
  tail -40 "$log" >&2
  exit 1
}

# retry <log> <command...>: a step that downloads (a mobile network drops out now and then): up to
# three tries, then as step.
retry() {
  log=$1
  shift
  for _ in 1 2; do
    "$@" > "$log" 2>&1 && return 0
    echo "  (failed, perhaps the network: trying again in 10 s)"
    sleep 10
  done
  step "$log" "$@"
}

# build <log> <make|ninja> [args...]: build with $JOBS jobs; if that fails, once more with one
# job (it carries on where it stopped), which gets past processes killed for being too many.
build() {
  log=$1
  tool=$2
  shift 2
  if [ "$tool" = ninja ]; then
    ninja -j"$JOBS" "$@" > "$log" 2>&1 && return 0
    echo "  (failed with $JOBS jobs: trying again with one)"
    step "$log.retry" ninja -j1 "$@"
  else
    make -j"$JOBS" "$@" > "$log" 2>&1 && return 0
    echo "  (failed with $JOBS jobs: trying again with one)"
    step "$log.retry" make -j1 "$@"
  fi
}

# Each part is built once: a run that stopped after ostree starts again at Flatpak
# (LINDROIDS_REBUILD=1, and --package, build everything again). A part counts as built only when
# this kit built it (or its package brought it, with these stamps), at this revision: KIT_REV goes
# up whenever the patches change, so what an older kit built is built again.
KIT_REV=2
STAMPS=$PREFIX/share/lindroids/termux-flatpak
built() { [ -z "${LINDROIDS_REBUILD:-}$PKGDIR" ] && [ "$(cat "$STAMPS/$1" 2>/dev/null)" = "$2 $KIT_REV" ]; }
stamp() { mkdir -p "$STAMPS" && echo "$2 $KIT_REV" > "$STAMPS/$1"; }

# Flatpak's bubblewrap: `lindroids bwrap` (the Termux package ships this too). meson runs its
# `--version` when Flatpak is configured, before the CLI is installed (the build container has none).
bwrap_shim() {
  if [ ! -x "$PREFIX/libexec/lindroids/bwrap" ]; then
    mkdir -p "$PREFIX/libexec/lindroids"
    cat > "$PREFIX/libexec/lindroids/bwrap" <<EOF
#!$PREFIX/bin/sh
if [ "\$1" = --version ] && [ ! -x "$PREFIX/bin/lindroids" ]; then echo "bubblewrap (lindroids)"; exit 0; fi
exec "$PREFIX/bin/lindroids" bwrap "\$@"
EOF
    chmod 755 "$PREFIX/libexec/lindroids/bwrap"
  fi
}

# The last step of every run: the Flathub remote.
finish() {
  say "Flathub (per user)"
  flatpak --user remote-add --if-not-exists flathub "$FLATHUB"
  say "$(flatpak --version) is installed"
  echo "  Install apps:  flatpak --user install flathub org.gnome.Calculator"
  echo "  Run them:      lindroids run org.gnome.Calculator"
}

# All of it current: no build tools needed (600 MB to fetch on a fresh Termux).
if built ostree "$OSTREE_VERSION" && built flatpak "$FLATPAK_VERSION" && [ -x "$PREFIX/bin/flatpak" ]; then
  say "ostree $OSTREE_VERSION and flatpak $FLATPAK_VERSION: built already"
  finish
  exit 0
fi

mkdir -p "$WORK"
say "build tools and libraries (pkg; log: $WORK/pkg.log)"
# bubblewrap is in root-repo: only root uses it, but it is part of a complete Flatpak.
yes | pkg install -y root-repo > /dev/null 2>&1 || true
# xz-utils, tar and patch: the sources are .tar.xz, and a fresh Termux has no xz program.
# libxau with xorgproto: Flatpak's configure looks xau up, and xau.pc needs xproto.pc.
# git: meson fetches Flatpak's dbus-proxy subproject (xdg-dbus-proxy) with it.
retry "$WORK/pkg.log" sh -c 'yes | pkg install -y build-essential xz-utils tar patch git ninja pkg-config python python-pip \
  bison gettext autoconf automake libtool glib libarchive libcurl gpgme json-glib libxml2 libseccomp appstream \
  gdk-pixbuf zstd libyaml libgpg-error libassuan liblzma libcap e2fsprogs libfuse3 dbus bubblewrap libxau xorgproto'
retry "$WORK/pip.log" pip install meson pyparsing

cd "$WORK"
fetch() { # fetch <url> <sha256>
  f=${1##*/}
  [ -f "$f" ] && echo "$2  $f" | sha256sum -c - > /dev/null 2>&1 && return 0
  retry "$WORK/download.log" curl -fsSL -o "$f" "$1"
  echo "$2  $f" | sha256sum -c - > /dev/null || { echo "build-in-termux: $f: checksum mismatch" >&2; exit 1; }
}
fetch "https://github.com/ostreedev/ostree/releases/download/v$OSTREE_VERSION/libostree-$OSTREE_VERSION.tar.xz" "$OSTREE_SHA256"
fetch "https://github.com/flatpak/flatpak/releases/download/$FLATPAK_VERSION/flatpak-$FLATPAK_VERSION.tar.xz" "$FLATPAK_SHA256"
COMPAT="-include $HERE/bionic-compat.h"
# --package: each part is also installed into $STAGE, which becomes the package.
STAGE=$WORK/stage
rm -rf "$STAGE"

if built ostree "$OSTREE_VERSION"; then
  say "ostree $OSTREE_VERSION: built already"
else
  say "ostree $OSTREE_VERSION (logs: $WORK/ostree-*.log)"
  rm -rf "libostree-$OSTREE_VERSION"
  tar xf "libostree-$OSTREE_VERSION.tar.xz"
  cd "libostree-$OSTREE_VERSION"
  patch -p1 < "$HERE/ostree-android-nolink.patch"
  patch -p1 -d libglnx < "$HERE/libglnx-android-nolink.patch"
  # The compat header only at build time: in configure's checks it hides what bionic really has.
  step "$WORK/ostree-configure.log" ./configure --prefix="$PREFIX" --with-curl --without-soup --without-libsystemd \
    --without-selinux --without-avahi --disable-man --disable-gtk-doc --disable-rofiles-fuse
  build "$WORK/ostree-make.log" make CFLAGS="-g -O2 $COMPAT"
  # (libtool's "ldconfig: not found" while installing is harmless: Android has no ldconfig.)
  step "$WORK/ostree-install.log" make install
  [ -z "$PKGDIR" ] || step "$WORK/ostree-stage.log" make install DESTDIR="$STAGE"
  stamp ostree "$OSTREE_VERSION"
  cd "$WORK"
fi

if built flatpak "$FLATPAK_VERSION"; then
  say "flatpak $FLATPAK_VERSION: built already"
else
  say "flatpak $FLATPAK_VERSION (logs: $WORK/flatpak-*.log)"
  bwrap_shim
  rm -rf "flatpak-$FLATPAK_VERSION"
  tar xf "flatpak-$FLATPAK_VERSION.tar.xz"
  cd "flatpak-$FLATPAK_VERSION"
  patch -p1 < "$HERE/flatpak-android.patch"
  patch -p1 < "$HERE/libglnx-android-seccomp.patch"
  patch -p1 -d subprojects/libglnx < "$HERE/libglnx-android-nolink.patch"
  # Termux's gettext makes translated format strings non-literal for clang: not errors here.
  # meson clones the dbus-proxy subproject with git: from scratch on every try.
  retry "$WORK/flatpak-setup.log" sh -c 'rm -rf build subprojects/dbus-proxy && exec "$@"' sh meson setup build --prefix="$PREFIX" \
    -Dsystem_bubblewrap="$PREFIX/libexec/lindroids/bwrap" -Dsystem_install_dir="$PREFIX/var/lib/flatpak" \
    -Dsystemd=disabled -Dsystem_helper=disabled -Dmalcontent=disabled -Dselinux_module=disabled \
    -Dgir=disabled -Ddocbook_docs=disabled -Dman=disabled -Dgtkdoc=disabled -Dtests=false \
    -Dinstalled_tests=false -Dwayland_security_context=disabled -Ddconf=disabled \
    -Dc_args="$COMPAT -Wno-error=format-nonliteral -Wno-error=format-security -Wno-format-nonliteral -Wno-format-security"
  build "$WORK/flatpak-build.log" ninja -C build
  step "$WORK/flatpak-install.log" ninja -C build install
  [ -z "$PKGDIR" ] || step "$WORK/flatpak-stage.log" env DESTDIR="$STAGE" ninja -C build install
  stamp flatpak "$FLATPAK_VERSION"
  cd "$WORK"
fi

if [ -n "$PKGDIR" ]; then
  arch=$(dpkg --print-architecture)
  version=$FLATPAK_VERSION+ostree$OSTREE_VERSION-$KIT_REV
  say "package lindroids-flatpak $version ($arch)"
  root=$STAGE$PREFIX
  # What runs, only: no headers, static or libtool archives, documentation, debug symbols.
  rm -rf "$root/include" "$root/lib/pkgconfig" "$root/share/aclocal" "$root/share/gtk-doc" \
    "$root/share/gir-1.0" "$root/share/man" "$root/share/doc"
  find "$root" \( -name '*.a' -o -name '*.la' \) -delete
  # revokefs-fuse serves only the system helper (disabled here; FUSE needs root on Android), and
  # it would make the package need libfuse3, from root-repo, which a fresh Termux does not have.
  rm -f "$root/libexec/revokefs-fuse"
  is_elf() { [ "$(head -c 4 "$1" | od -An -tx1 | tr -d ' \n')" = 7f454c46 ]; }
  find "$root" -type f | while read -r f; do
    if is_elf "$f"; then strip --strip-unneeded "$f"; fi
  done
  # The stamps: the kit counts what the package brought as built.
  mkdir -p "$root/share/lindroids/termux-flatpak"
  echo "$OSTREE_VERSION $KIT_REV" > "$root/share/lindroids/termux-flatpak/ostree"
  echo "$FLATPAK_VERSION $KIT_REV" > "$root/share/lindroids/termux-flatpak/flatpak"
  # Depends: the Termux package of every library linked that the package does not bring itself
  # and that is not Android's own (libc, libdl, liblog, ...); and gnupg, which gpgme runs to check
  # Flathub's signatures.
  find "$root" -type f | while read -r f; do
    if is_elf "$f"; then readelf -d "$f" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p'; fi
  done | sort -u > "$WORK/needed"
  : > "$WORK/depends"
  while read -r lib; do
    [ -z "$(find "$root" -name "$lib")" ] || continue
    [ -e "$PREFIX/lib/$lib" ] || continue
    owner=$(dpkg -S "$PREFIX/lib/$lib" 2>/dev/null || dpkg -S "$(readlink -f "$PREFIX/lib/$lib")" 2>/dev/null) \
      || { echo "build-in-termux: no Termux package has $PREFIX/lib/$lib" >&2; exit 1; }
    echo "${owner%%:*}" | tr ',' '\n' | sed 's/^ *//' >> "$WORK/depends"
  done < "$WORK/needed"
  echo gnupg >> "$WORK/depends"
  depends=$(grep -vx lindroids-flatpak "$WORK/depends" | sort -u | paste -sd, - | sed 's/,/, /g')
  # Every one from Termux's main repository: root-repo and x11-repo are not there on a fresh Termux.
  for d in $(echo "$depends" | tr -d ','); do
    apt-cache policy "$d" | grep -q 'termux-main' \
      || { echo "build-in-termux: the package would need $d, which is not in Termux's main repository" >&2; exit 1; }
  done
  mkdir -p "$STAGE/DEBIAN"
  {
    echo "Package: lindroids-flatpak"
    echo "Version: $version"
    echo "Architecture: $arch"
    echo "Maintainer: dixonSolutions <lindroids@users.noreply.github.com>"
    echo "Installed-Size: $(du -sk "$root" | cut -f1)"
    echo "Depends: $depends"
    echo "Recommends: lindroids, proot"
    echo "Provides: flatpak, ostree"
    echo "Conflicts: flatpak, ostree"
    echo "Section: admin"
    echo "Priority: optional"
    echo "Homepage: https://github.com/dixonSolutions/lindroids"
    echo "Description: Flatpak and ostree for Termux, as Lindroids builds them"
    echo " Flatpak $FLATPAK_VERSION and ostree $OSTREE_VERSION built natively for Termux (bionic) with"
    echo " Lindroids' Android patches: no root and no Linux distribution needed. Apps"
    echo " install per user (flatpak --user install flathub <app>) and run with"
    echo " lindroids run <app>."
  } > "$STAGE/DEBIAN/control"
  # Termux's umask is 077; dpkg-deb wants 755 directories, and files are as packages have them.
  chmod -R u+rwX,go+rX,go-w "$STAGE"
  deb=$PKGDIR/lindroids-flatpak_${version}_$arch.deb
  step "$WORK/dpkg-deb.log" dpkg-deb --build --root-owner-group -Zxz "$STAGE" "$deb"
  say "$deb ($(du -h "$deb" | cut -f1)); Depends: $depends"
fi

finish
cd /
rm -rf "$WORK"
