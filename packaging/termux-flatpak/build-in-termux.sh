#!/data/data/com.termux/files/usr/bin/sh
# Build ostree and Flatpak natively for Termux (bionic) and install them into $PREFIX, with no
# root and no proot distro. Termux has no flatpak package; this is how Lindroids gets one.
#
#   sh build-in-termux.sh          (in Termux; about 20 minutes, ~600 MB of build tools)
#
# What makes it work on Android (all in this directory):
#   bionic-compat.h                 glibc functions bionic lacks at API 24 (strdupa, syncfs,
#                                   strverscmp, explicit_bzero, qsort_r), force-included
#   flatpak-android.patch           temp files in $TMPDIR instead of /tmp; Termux's resolv.conf
#                                   when Android has no /etc/resolv.conf
#   libglnx-android-seccomp.patch   no openat2/open_tree: Android's app seccomp filter kills them
#                                   (SIGSYS) instead of failing, so libglnx's own fallback is used
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
WORK=${TMPDIR:-$PREFIX/tmp}/lindroids-flatpak-build
JOBS=$(nproc 2>/dev/null || echo 2)
say() { printf '\033[1m==> %s\033[0m\n' "$*"; }

say "build tools and libraries (pkg)"
# bubblewrap is in root-repo: only root uses it, but it is part of a complete Flatpak.
yes | pkg install -y root-repo > /dev/null 2>&1 || true
# xz-utils, tar and patch: the sources are .tar.xz, and a fresh Termux has no xz program.
# libxau with xorgproto: Flatpak's configure looks xau up, and xau.pc needs xproto.pc.
# git: meson fetches Flatpak's dbus-proxy subproject (xdg-dbus-proxy) with it.
yes | pkg install -y build-essential xz-utils tar patch git ninja pkg-config python python-pip bison gettext autoconf automake libtool \
  glib libarchive libcurl gpgme json-glib libxml2 libseccomp appstream gdk-pixbuf zstd libyaml \
  libgpg-error libassuan liblzma libcap e2fsprogs libfuse3 dbus bubblewrap libxau xorgproto
pip install --quiet meson pyparsing

mkdir -p "$WORK"
cd "$WORK"
fetch() { # fetch <url> <sha256>
  f=${1##*/}
  [ -f "$f" ] && echo "$2  $f" | sha256sum -c - > /dev/null 2>&1 && return 0
  curl -fsSL -o "$f" "$1"
  echo "$2  $f" | sha256sum -c - > /dev/null || { echo "build-in-termux: $f: checksum mismatch" >&2; exit 1; }
}
fetch "https://github.com/ostreedev/ostree/releases/download/v$OSTREE_VERSION/libostree-$OSTREE_VERSION.tar.xz" "$OSTREE_SHA256"
fetch "https://github.com/flatpak/flatpak/releases/download/$FLATPAK_VERSION/flatpak-$FLATPAK_VERSION.tar.xz" "$FLATPAK_SHA256"
COMPAT="-include $HERE/bionic-compat.h"

say "ostree $OSTREE_VERSION"
rm -rf "libostree-$OSTREE_VERSION"
tar xf "libostree-$OSTREE_VERSION.tar.xz"
cd "libostree-$OSTREE_VERSION"
# The compat header only at build time: in configure's checks it hides what bionic really has.
./configure --prefix="$PREFIX" --with-curl --without-soup --without-libsystemd --without-selinux \
  --without-avahi --disable-man --disable-gtk-doc --disable-rofiles-fuse > ../ostree-configure.log
make -j"$JOBS" CFLAGS="-g -O2 $COMPAT" > ../ostree-make.log 2>&1 || { tail -30 ../ostree-make.log >&2; exit 1; }
make install > /dev/null
cd ..

say "flatpak $FLATPAK_VERSION"
rm -rf "flatpak-$FLATPAK_VERSION"
tar xf "flatpak-$FLATPAK_VERSION.tar.xz"
cd "flatpak-$FLATPAK_VERSION"
patch -p1 < "$HERE/flatpak-android.patch"
patch -p1 < "$HERE/libglnx-android-seccomp.patch"
# Termux's gettext makes translated format strings non-literal for clang: not errors here.
meson setup build --prefix="$PREFIX" \
  -Dsystem_bubblewrap="$PREFIX/libexec/lindroids/bwrap" -Dsystem_install_dir="$PREFIX/var/lib/flatpak" \
  -Dsystemd=disabled -Dsystem_helper=disabled -Dmalcontent=disabled -Dselinux_module=disabled \
  -Dgir=disabled -Ddocbook_docs=disabled -Dman=disabled -Dgtkdoc=disabled -Dtests=false \
  -Dinstalled_tests=false -Dwayland_security_context=disabled -Ddconf=disabled \
  -Dc_args="$COMPAT -Wno-error=format-nonliteral -Wno-error=format-security -Wno-format-nonliteral -Wno-format-security" \
  > ../flatpak-setup.log 2>&1 || { tail -30 ../flatpak-setup.log >&2; exit 1; }
ninja -C build > ../flatpak-build.log 2>&1 || { grep -E 'error' ../flatpak-build.log | head -20 >&2; exit 1; }
ninja -C build install > /dev/null
cd ..

# Flatpak's bubblewrap: `lindroids bwrap` (the Termux package ships this too).
if [ ! -x "$PREFIX/libexec/lindroids/bwrap" ]; then
  mkdir -p "$PREFIX/libexec/lindroids"
  printf '#!%s/bin/sh\nexec %s/bin/lindroids bwrap "$@"\n' "$PREFIX" "$PREFIX" > "$PREFIX/libexec/lindroids/bwrap"
  chmod 755 "$PREFIX/libexec/lindroids/bwrap"
fi

say "Flathub (per user)"
flatpak --user remote-add --if-not-exists flathub "$FLATHUB"
cd /
rm -rf "$WORK"
say "$(flatpak --version) is installed"
echo "  Install apps:  flatpak --user install flathub org.gnome.Calculator"
echo "  Run them:      lindroids run org.gnome.Calculator"
