#!/usr/bin/env bash
# Build (or update) the static, signed Lindroids package repositories served by GitHub Pages:
# APT, pacman, RPM (dnf/yum/zypper), Alpine (apk) and Void (xbps).
#
#   packaging/repo/build-repo.sh <out-dir> [--debs dist/debs]... [--pacman dist/pacman]
#                                [--rpm dist/rpm] [--apk dist/apk] [--xbps dist/xbps] [--keep 3]
#                                [--base-url URL] [--github-repo owner/repo]
#                                [--public-key packaging/repo/lindroids.gpg]
#                                [--rsa-public-key packaging/repo/lindroids.rsa.pub]
#
# <out-dir> may already hold the published site (CI copies the gh-pages branch into it): the
# packages already in its pool are kept, the new ones are added, and only the newest --keep
# versions per architecture survive. Everything under dists/ is regenerated.
#
# Layout:
#   dists/termux/{Release,InRelease,Release.gpg}
#   dists/termux/main/binary-{aarch64,arm,x86_64,i686}/Packages(.gz)
#   dists/stable/{Release,InRelease,Release.gpg}
#   dists/stable/main/binary-{arm64,armhf,amd64,i386}/Packages(.gz)
#   pool/{termux,stable}/main/l/lindroids/lindroids_<version>_<arch>.deb
#   pacman/termux/{aarch64,arm,x86_64,i686}/        Termux with pacman (packaging/build-pacman.sh)
#   pacman/arch/{aarch64,armv7h,x86_64,i686}/       Arch Linux and derivatives
#     lindroids.db{,.tar.gz}{,.sig}, lindroids-<version>-<rel>-<arch>.pkg.tar.xz{,.sig}
#   rpm/lindroids-<version>-1.{aarch64,armv7hl,x86_64,i686}.rpm  dnf, yum and zypper
#   rpm/repodata/repomd.xml{,.asc,.key} (+ the files it lists), rpm/lindroids.repo
#   apk/{aarch64,armv7,x86_64,x86}/APKINDEX.tar.gz, .../lindroids-<version>-r0.apk   Alpine & co.
#   xbps/<arch>-repodata, xbps/lindroids-<version>_1.<arch>.xbps{,.sig2}            Void Linux
#     (<arch>: aarch64 aarch64-musl armv7l armv7l-musl x86_64 x86_64-musl i686)
#   lindroids.gpg (binary public key), lindroids.asc, lindroids.rsa.pub, install.sh, VERSION,
#   .nojekyll, and index.html when there is none yet (the website is site/, published by site.yml)
#
# Signing key, in this order:
#   LINDROIDS_GPG_PRIVATE_KEY  armored secret key (CI secret), imported into a throwaway GNUPGHOME
#   LINDROIDS_GNUPGHOME / GNUPGHOME, else ~/.local/share/lindroids/gnupg
# The key must be the one whose public half is committed as packaging/repo/lindroids.gpg (or
# --public-key, for tests), or nobody could verify the result; the script checks that, and
# verifies its own signatures with that public key alone. The site gets that public key.
#
# The apk and xbps repositories are signed with an RSA key instead (apk and xbps know no OpenPGP):
#   LINDROIDS_APK_RSA_KEY       PEM private key (CI secret)
#   LINDROIDS_APK_RSA_KEY_FILE, else ~/.local/share/lindroids/lindroids.rsa
# Its public half must be packaging/repo/lindroids.rsa.pub (or --rsa-public-key); the site gets it.
# Without that key the apk and xbps repositories are left as they are (nothing new is published
# there, with a warning); everything else is still built.
#
# Needs: dpkg-deb, gpg, gpgv, gzip, md5sum, sha1sum, sha256sum; for the pacman repositories also
# bsdtar (libarchive-tools) and base64; for rpm, apk and xbps podman or docker (createrepo_c,
# apk and xbps-rindex run in fedora, alpine and void containers; packaging/container.sh) and,
# for apk and xbps, openssl.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)

OUTDIR=
DEBS=()
PKGS=$ROOT/dist/pacman
KEEP=3
BASE_URL=${LINDROIDS_REPO_URL:-}
GITHUB_REPO=${LINDROIDS_GITHUB_REPO:-}
RPMS=$ROOT/dist/rpm
APKS=$ROOT/dist/apk
XBPS=$ROOT/dist/xbps
PUBKEY=$HERE/lindroids.gpg
RSA_PUB=$HERE/lindroids.rsa.pub
while [ $# -gt 0 ]; do
  case "$1" in
    --debs) DEBS+=("$2"); shift 2 ;;
    --pacman) PKGS=$2; shift 2 ;;
    --rpm) RPMS=$2; shift 2 ;;
    --apk) APKS=$2; shift 2 ;;
    --xbps) XBPS=$2; shift 2 ;;
    --rsa-public-key) RSA_PUB=$(readlink -f "$2"); shift 2 ;;
    --keep) KEEP=$2; shift 2 ;;
    --base-url) BASE_URL=$2; shift 2 ;;
    --github-repo) GITHUB_REPO=$2; shift 2 ;;
    --public-key) PUBKEY=$(readlink -f "$2"); shift 2 ;;
    -h|--help) awk 'NR > 1 && !/^#/ {exit} NR > 1 {sub(/^# ?/, ""); print}' "$0"; exit 0 ;;
    -*) echo "build-repo: unknown option $1" >&2; exit 2 ;;
    *) OUTDIR=$1; shift ;;
  esac
done
[ -n "$OUTDIR" ] || { echo "usage: build-repo.sh <out-dir> [--debs dir] [--pacman dir] [--keep N]" >&2; exit 2; }
for tool in dpkg-deb gpg gpgv gzip md5sum sha1sum sha256sum; do
  command -v "$tool" >/dev/null || { echo "build-repo: $tool is required" >&2; exit 1; }
done
mkdir -p "$OUTDIR"
OUTDIR=$(cd "$OUTDIR" && pwd)

TERMUX_ARCHES="aarch64 arm x86_64 i686"
DEBIAN_ARCHES="arm64 armhf amd64 i386"
suite_of() {
  case "$1" in
    aarch64|arm|x86_64|i686) echo termux ;;
    arm64|armhf|amd64|i386) echo stable ;;
    *) return 1 ;;
  esac
}

# --- signing key ---------------------------------------------------------------------------
WANT_FPR=$(gpg --batch --show-keys --with-colons "$PUBKEY" 2>/dev/null | awk -F: '/^fpr/ {print $10; exit}')
[ -n "$WANT_FPR" ] || { echo "build-repo: cannot read $PUBKEY" >&2; exit 1; }
CLEANUP_GNUPG=
if [ -n "${LINDROIDS_GPG_PRIVATE_KEY:-}" ]; then
  GNUPGHOME=$(mktemp -d)
  CLEANUP_GNUPG=$GNUPGHOME
  chmod 700 "$GNUPGHOME"
  export GNUPGHOME
  printf '%s\n' "$LINDROIDS_GPG_PRIVATE_KEY" | gpg --batch --quiet --import
else
  GNUPGHOME=${LINDROIDS_GNUPGHOME:-${GNUPGHOME:-$HOME/.local/share/lindroids/gnupg}}
  export GNUPGHOME
fi
RSA_DIR=
cleanup() {
  if [ -n "$CLEANUP_GNUPG" ]; then
    gpgconf --kill gpg-agent >/dev/null 2>&1 || true
    rm -rf "$CLEANUP_GNUPG"
  fi
  [ -z "$RSA_DIR" ] || rm -rf "$RSA_DIR"
}
trap cleanup EXIT
if ! gpg --batch --list-secret-keys --with-colons "$WANT_FPR" >/dev/null 2>&1; then
  echo "build-repo: the secret key for $WANT_FPR (packaging/repo/lindroids.gpg) is not available" >&2
  echo "            set LINDROIDS_GPG_PRIVATE_KEY or GNUPGHOME" >&2
  exit 1
fi
GPG_SIGN=(gpg --batch --yes --pinentry-mode loopback --passphrase "" --local-user "$WANT_FPR" --digest-algo SHA512)

# The RSA key for apk and xbps, copied into a private directory (the containers mount it).
RSA_KEY=
rsa_file=${LINDROIDS_APK_RSA_KEY_FILE:-${LINDROIDS_KEYS_DIR:-$HOME/.local/share/lindroids}/lindroids.rsa}
if [ -n "${LINDROIDS_APK_RSA_KEY:-}" ] || [ -f "$rsa_file" ]; then
  command -v openssl >/dev/null || { echo "build-repo: openssl is required" >&2; exit 1; }
  RSA_DIR=$(mktemp -d)
  chmod 700 "$RSA_DIR"
  RSA_KEY=$RSA_DIR/lindroids.rsa
  if [ -n "${LINDROIDS_APK_RSA_KEY:-}" ]; then
    (umask 077 && printf '%s\n' "$LINDROIDS_APK_RSA_KEY" > "$RSA_KEY")
  else
    install -m 600 "$rsa_file" "$RSA_KEY"
  fi
  [ -f "$RSA_PUB" ] || { echo "build-repo: the RSA public key $RSA_PUB is missing (scripts/set-github-secrets.sh writes it)" >&2; exit 1; }
  if [ "$(openssl pkey -in "$RSA_KEY" -pubout 2>/dev/null)" != "$(openssl pkey -pubin -in "$RSA_PUB")" ]; then
    echo "build-repo: the RSA key does not match $RSA_PUB" >&2
    exit 1
  fi
  RSA_DIR=$(cd "$RSA_DIR" && pwd)
fi

# Termux's architecture "x86_64" makes stock dpkg-deb warn on every read; only show real errors.
ctl() { dpkg-deb -f "$@" 2>/dev/null || dpkg-deb -f "$@"; }

# --- pool ----------------------------------------------------------------------------------
added=0
[ ${#DEBS[@]} -gt 0 ] || DEBS=("$ROOT/dist/debs")
for debs in "${DEBS[@]}"; do
  [ -d "$debs" ] || continue
  for deb in "$debs"/*.deb; do
    [ -e "$deb" ] || continue
    pkg=$(ctl "$deb" Package)
    arch=$(ctl "$deb" Architecture)
    ver=$(ctl "$deb" Version)
    suite=$(suite_of "$arch") || { echo "build-repo: $deb has unknown architecture $arch" >&2; exit 1; }
    dir=$OUTDIR/pool/$suite/main/${pkg:0:1}/$pkg
    mkdir -p "$dir"
    cp -f "$deb" "$dir/${pkg}_${ver}_${arch}.deb"
    added=$((added + 1))
  done
done
echo "==> pool: $added package(s) added from ${DEBS[*]}"

# Keep the newest $KEEP versions of every package per architecture.
for pkgdir in "$OUTDIR"/pool/*/main/*/*; do
  [ -d "$pkgdir" ] || continue
  for arch in $TERMUX_ARCHES $DEBIAN_ARCHES; do
    mapfile -t files < <(find "$pkgdir" -maxdepth 1 -name "*_${arch}.deb" -printf '%f\n' \
      | awk -F_ '{print $2 "\t" $0}' | sort -V -k1,1 | cut -f2)
    n=${#files[@]}
    if [ "$n" -gt "$KEEP" ]; then
      for ((i = 0; i < n - KEEP; i++)); do
        echo "    prune ${files[$i]}"
        rm -f "${pkgdir:?}/${files[$i]}"
      done
    fi
  done
done

# --- indices -------------------------------------------------------------------------------
# One Packages stanza: the control fields, then Filename/Size/hashes, then the Description.
stanza() { # <deb> <path relative to OUTDIR>
  local deb=$1 rel=$2
  ctl "$deb" | awk -v fn="$rel" -v size="$(stat -c %s "$deb")" \
    -v md5="$(md5sum "$deb" | cut -d' ' -f1)" -v sha1="$(sha1sum "$deb" | cut -d' ' -f1)" \
    -v sha256="$(sha256sum "$deb" | cut -d' ' -f1)" '
    /^Description:/ { desc = 1 }
    desc { d = d $0 "\n"; next }
    NF { print }
    END {
      print "Filename: " fn
      print "Size: " size
      print "MD5sum: " md5
      print "SHA1: " sha1
      print "SHA256: " sha256
      printf "%s", d
      print ""
    }'
}

DATE=$(LC_ALL=C date -u -R)
build_suite() { # <suite> <arches...>
  local suite=$1; shift
  local dist=$OUTDIR/dists/$suite arch
  rm -rf "$dist"
  for arch in "$@"; do
    local bdir=$dist/main/binary-$arch
    mkdir -p "$bdir"
    : > "$bdir/Packages"
    while IFS= read -r rel; do
      stanza "$OUTDIR/$rel" "$rel" >> "$bdir/Packages"
    done < <(cd "$OUTDIR" && if [ -d "pool/$suite" ]; then find "pool/$suite" -name "*_${arch}.deb" | sort -V; fi)
    gzip -9nc "$bdir/Packages" > "$bdir/Packages.gz"
    echo "    $suite/$arch: $(grep -c '^Package:' "$bdir/Packages" || true) package(s)"
  done
  {
    echo "Origin: Lindroids"
    echo "Label: Lindroids"
    echo "Suite: $suite"
    echo "Codename: $suite"
    echo "Date: $DATE"
    echo "Architectures: $*"
    echo "Components: main"
    echo "Acquire-By-Hash: no"
    echo "Description: Lindroids: Linux apps in Android windows"
    local algo
    for algo in MD5Sum:md5sum SHA1:sha1sum SHA256:sha256sum; do
      echo "${algo%%:*}:"
      (cd "$dist" && find main -type f -name 'Packages*' | sort | while read -r f; do
        printf ' %s %16d %s\n' "$("${algo#*:}" "$f" | cut -d' ' -f1)" "$(stat -c %s "$f")" "$f"
      done)
    done
  } > "$dist/Release"
  "${GPG_SIGN[@]}" --clearsign -o "$dist/InRelease" "$dist/Release"
  "${GPG_SIGN[@]}" --armor --detach-sign -o "$dist/Release.gpg" "$dist/Release"
  # Check with the committed public key only, the way apt will.
  gpgv --quiet --keyring "$PUBKEY" "$dist/InRelease" 2>/dev/null \
    || { echo "build-repo: InRelease of $suite does not verify with lindroids.gpg" >&2; exit 1; }
  gpgv --quiet --keyring "$PUBKEY" "$dist/Release.gpg" "$dist/Release" 2>/dev/null \
    || { echo "build-repo: Release.gpg of $suite does not verify" >&2; exit 1; }
}
echo "==> indices"
# shellcheck disable=SC2086
build_suite termux $TERMUX_ARCHES
# shellcheck disable=SC2086
build_suite stable $DEBIAN_ARCHES

# --- pacman repositories -------------------------------------------------------------------
# pacman/<flavour>/<arch>/: the packages with detached binary signatures, and the database
# lindroids.db (a gzipped tar of <name>-<version>/desc), signed too. pacman wants the signature of
# every package (SigLevel Required); install.sh trusts the same key with pacman-key.
pacman_desc() { # <pkg>: the database entry, from the package's .PKGINFO
  local pkg=$1 info
  info=$(bsdtar -xOf "$pkg" .PKGINFO)
  val() { printf '%s\n' "$info" | sed -n "s/^$1 = //p"; }
  section() { # <NAME> <values...>
    local v printed=
    for v in "${@:2}"; do
      [ -n "$v" ] || continue
      [ -n "$printed" ] || { echo "%$1%"; printed=1; }
      echo "$v"
    done
    [ -z "$printed" ] || echo
  }
  local IFS=$'\n'
  # shellcheck disable=SC2046
  {
    section FILENAME "$(basename "$pkg")"
    section NAME $(val pkgname)
    section BASE $(val pkgbase)
    section VERSION $(val pkgver)
    section DESC "$(val pkgdesc)"
    section CSIZE "$(stat -c %s "$pkg")"
    section ISIZE $(val size)
    section MD5SUM "$(md5sum "$pkg" | cut -d' ' -f1)"
    section SHA256SUM "$(sha256sum "$pkg" | cut -d' ' -f1)"
    section PGPSIG "$(base64 -w0 "$pkg.sig")"
    section URL $(val url)
    section LICENSE $(val license)
    section ARCH $(val arch)
    section BUILDDATE $(val builddate)
    section PACKAGER "$(val packager)"
    section DEPENDS $(val depend)
    section OPTDEPENDS $(val optdepend)
  }
}
pacman_sign() { # <file>: <file>.sig, binary, checked with the committed key alone
  "${GPG_SIGN[@]}" --detach-sign -o "$1.sig" "$1"
  gpgv --quiet --keyring "$PUBKEY" "$1.sig" "$1" 2>/dev/null \
    || { echo "build-repo: $1.sig does not verify with lindroids.gpg" >&2; exit 1; }
}
if [ -d "$PKGS" ] || [ -d "$OUTDIR/pacman" ]; then
  for tool in bsdtar base64; do
    command -v "$tool" >/dev/null || { echo "build-repo: $tool is required" >&2; exit 1; }
  done
  echo "==> pacman"
  added=0
  if [ -d "$PKGS" ]; then
    while IFS= read -r pkg; do
      rel=${pkg#"$PKGS"/}
      mkdir -p "$OUTDIR/pacman/$(dirname "$rel")"
      cp -f "$pkg" "$OUTDIR/pacman/$rel"
      rm -f "$OUTDIR/pacman/$rel.sig"
      added=$((added + 1))
    done < <(find "$PKGS" -mindepth 3 -maxdepth 3 -name '*.pkg.tar.*' ! -name '*.sig' | sort)
  fi
  echo "    $added package(s) added from $PKGS"
  for dir in "$OUTDIR"/pacman/*/*; do
    [ -d "$dir" ] || continue
    # Newest $KEEP versions (lindroids-<pkgver>-<pkgrel>-<arch>.pkg.tar.xz).
    mapfile -t files < <(find "$dir" -maxdepth 1 -name 'lindroids-*.pkg.tar.*' ! -name '*.sig' -printf '%f\n' \
      | awk '{v = $0; sub(/^lindroids-/, "", v); sub(/-[^-]*\.pkg\.tar\..*$/, "", v); print v "\t" $0}' \
      | sort -V -k1,1 | cut -f2)
    n=${#files[@]}
    for ((i = 0; i < n - KEEP; i++)); do
      echo "    prune ${files[$i]}"
      rm -f "${dir:?}/${files[$i]}" "${dir:?}/${files[$i]}.sig"
    done
    db=$(mktemp -d)
    for pkg in "$dir"/*.pkg.tar.*; do
      case "$pkg" in *.sig) continue ;; esac
      [ -e "$pkg" ] || continue
      [ -f "$pkg.sig" ] || pacman_sign "$pkg"
      entry=$(bsdtar -xOf "$pkg" .PKGINFO | awk -F' = ' '$1 == "pkgname" {n = $2} $1 == "pkgver" {v = $2} END {print n "-" v}')
      mkdir -p "$db/$entry"
      pacman_desc "$pkg" > "$db/$entry/desc"
    done
    rm -f "$dir"/lindroids.db* "$dir"/lindroids.files*
    # shellcheck disable=SC2035
    (cd "$db" && LANG=C bsdtar -czf "$dir/lindroids.db.tar.gz" --uid 0 --gid 0 --uname root --gname root *)
    rm -rf "$db"
    # GitHub Pages serves no symlinks: lindroids.db is a copy.
    cp "$dir/lindroids.db.tar.gz" "$dir/lindroids.db"
    pacman_sign "$dir/lindroids.db.tar.gz"
    pacman_sign "$dir/lindroids.db"
    echo "    pacman/${dir#"$OUTDIR"/pacman/}: $(bsdtar -tf "$dir/lindroids.db" | grep -c '/desc$' || true) package(s)"
  done
fi

# --- helpers for the rpm, apk and xbps repositories -----------------------------------------
# shellcheck source=packaging/container.sh
. "$ROOT/packaging/container.sh"
FEDORA_IMAGE=${LINDROIDS_FEDORA_IMAGE:-registry.fedoraproject.org/fedora:latest}
ALPINE_IMAGE=${LINDROIDS_ALPINE_IMAGE:-docker.io/library/alpine:3.22}
VOID_IMAGE=${LINDROIDS_VOID_IMAGE:-ghcr.io/void-linux/void-glibc-full:latest}

armored_pubkey() { # the APT/pacman/rpm key, armored
  if [ "$PUBKEY" = "$HERE/lindroids.gpg" ]; then cat "$HERE/lindroids.asc"; else gpg --batch --armor --export "$WANT_FPR"; fi
}

# prune <dir> <file glob> <sed expression: file name -> version>: keep the newest $KEEP, with
# their signatures.
prune() {
  local dir=$1 glob=$2 expr=$3 files n i
  mapfile -t files < <(find "$dir" -maxdepth 1 -name "$glob" -printf '%f\n' \
    | while read -r f; do printf '%s\t%s\n' "$(printf '%s\n' "$f" | sed "$expr")" "$f"; done \
    | sort -V -k1,1 | cut -f2)
  n=${#files[@]}
  for ((i = 0; i < n - KEEP; i++)); do
    echo "    prune ${files[$i]}"
    rm -f "${dir:?}/${files[$i]}" "${dir:?}/${files[$i]}.sig2"
  done
}

# --- RPM repository (dnf, yum, zypper) -------------------------------------------------------
# rpm/: the packages of every architecture and one repodata/ (createrepo_c, gzip: yum and older
# zypper read no zstd). repomd.xml is signed with the same key as APT (repomd.xml.asc, armored,
# detached; the key also as repomd.xml.key, where zypper looks); dnf, yum and zypper check it
# (repo_gpgcheck=1), and through it the checksum of every package.
if [ -d "$RPMS" ] || [ -d "$OUTDIR/rpm" ]; then
  echo "==> rpm"
  mkdir -p "$OUTDIR/rpm"
  added=0
  for f in "$RPMS"/*.rpm; do
    [ -e "$f" ] || continue
    cp -f "$f" "$OUTDIR/rpm/"
    added=$((added + 1))
  done
  echo "    $added package(s) added from $RPMS"
  for arch in aarch64 armv7hl x86_64 i686; do
    prune "$OUTDIR/rpm" "lindroids-*.$arch.rpm" 's/^lindroids-\(.*\)\.[^.]*\.rpm$/\1/'
  done
  rm -rf "$OUTDIR/rpm/repodata"
  # shellcheck disable=SC2016
  in_container "$FEDORA_IMAGE" "$OUTDIR" -- sh -ec '
    dnf -q -y install createrepo_c >/dev/null 2>&1 || dnf -y install createrepo_c
    createrepo_c --quiet --no-database --general-compress-type=gz "$1"' sh "$OUTDIR/rpm"
  repomd=$OUTDIR/rpm/repodata/repomd.xml
  [ -f "$repomd" ] || { echo "build-repo: createrepo_c made no $repomd" >&2; exit 1; }
  "${GPG_SIGN[@]}" --armor --detach-sign -o "$repomd.asc" "$repomd"
  gpgv --quiet --keyring "$PUBKEY" "$repomd.asc" "$repomd" 2>/dev/null \
    || { echo "build-repo: repomd.xml.asc does not verify with lindroids.gpg" >&2; exit 1; }
  armored_pubkey > "$repomd.key"
  echo "    rpm: $(find "$OUTDIR/rpm" -maxdepth 1 -name '*.rpm' | wc -l) package(s)"
fi

# --- Alpine repository (apk) -----------------------------------------------------------------
# apk/<arch>/: the packages and APKINDEX.tar.gz, made by `apk index` and signed by abuild-sign
# with the RSA key (.SIGN.RSA.lindroids.rsa.pub: apk looks for /etc/apk/keys/lindroids.rsa.pub).
# The packages themselves are unsigned; apk checks each against the checksum in the signed index.
# v2 formats, which apk-tools 2 and 3 both read.
APK_ARCHES="aarch64 armv7 x86_64 x86"
if [ -d "$APKS" ] || [ -d "$OUTDIR/apk" ]; then
  echo "==> apk"
  if [ -z "$RSA_KEY" ]; then
    echo "build-repo: warning: no RSA key (LINDROIDS_APK_RSA_KEY): the apk repository is not updated" >&2
  else
    added=0
    for f in "$APKS"/*/*.apk; do
      [ -e "$f" ] || continue
      arch=$(basename "$(dirname "$f")")
      case " $APK_ARCHES " in *" $arch "*) ;; *) echo "build-repo: $f: unknown apk architecture $arch" >&2; exit 1 ;; esac
      mkdir -p "$OUTDIR/apk/$arch"
      cp -f "$f" "$OUTDIR/apk/$arch/"
      added=$((added + 1))
    done
    echo "    $added package(s) added from $APKS"
    for arch in $APK_ARCHES; do
      [ -d "$OUTDIR/apk/$arch" ] || continue
      prune "$OUTDIR/apk/$arch" 'lindroids-*.apk' 's/^lindroids-\(.*\)\.apk$/\1/'
      rm -f "$OUTDIR/apk/$arch/APKINDEX.tar.gz"
    done
    cp "$RSA_PUB" "$RSA_DIR/lindroids.rsa.pub"
    # Index and sign, then check the signature with the public key alone, as apk will.
    # shellcheck disable=SC2016
    in_container "$ALPINE_IMAGE" "$OUTDIR" "$RSA_DIR" -- sh -ec '
      apk add -q abuild >/dev/null
      repo=$1 keys=$2
      for dir in "$repo"/*/; do
        arch=$(basename "$dir")
        cd "$dir"
        ls lindroids-*.apk >/dev/null 2>&1 || continue
        apk index --quiet --allow-untrusted --rewrite-arch "$arch" -d "Lindroids" -o APKINDEX.tar.gz lindroids-*.apk
        abuild-sign -q -k "$keys/lindroids.rsa" -p lindroids.rsa.pub APKINDEX.tar.gz
        mkdir -p /tmp/keys
        cp "$keys/lindroids.rsa.pub" /tmp/keys/
        apk --keys-dir /tmp/keys --repositories-file /dev/null verify APKINDEX.tar.gz >/dev/null \
          || { echo "build-repo: apk/$arch/APKINDEX.tar.gz does not verify with lindroids.rsa.pub" >&2; exit 1; }
      done' sh "$OUTDIR/apk" "$RSA_DIR"
    for arch in $APK_ARCHES; do
      [ -f "$OUTDIR/apk/$arch/APKINDEX.tar.gz" ] && echo "    apk/$arch: $(find "$OUTDIR/apk/$arch" -name '*.apk' | wc -l) package(s)"
    done
  fi
fi

# --- Void repository (xbps) ------------------------------------------------------------------
# xbps/: the packages of every architecture with their signatures (.sig2), and <arch>-repodata
# for each, signed: xbps-rindex --sign / --sign-pkg with the RSA key. xbps imports the key on
# the first sync (install.sh confirms it).
XBPS_ARCHES="aarch64 aarch64-musl armv7l armv7l-musl x86_64 x86_64-musl i686"
if [ -d "$XBPS" ] || [ -d "$OUTDIR/xbps" ]; then
  echo "==> xbps"
  if [ -z "$RSA_KEY" ]; then
    echo "build-repo: warning: no RSA key (LINDROIDS_APK_RSA_KEY): the xbps repository is not updated" >&2
  else
    mkdir -p "$OUTDIR/xbps"
    added=0
    for f in "$XBPS"/*.xbps; do
      [ -e "$f" ] || continue
      cp -f "$f" "$OUTDIR/xbps/"
      rm -f "$OUTDIR/xbps/$(basename "$f").sig2"
      added=$((added + 1))
    done
    echo "    $added package(s) added from $XBPS"
    for arch in $XBPS_ARCHES; do
      prune "$OUTDIR/xbps" "lindroids-*.$arch.xbps" 's/^lindroids-\(.*\)\.[^.]*\.xbps$/\1/'
    done
    rm -f "$OUTDIR"/xbps/*-repodata "$OUTDIR"/xbps/*.sig2
    # shellcheck disable=SC2016,SC2086
    in_container "$VOID_IMAGE" "$OUTDIR" "$RSA_DIR" -- sh -ec '
      repo=$1 key=$2/lindroids.rsa; shift 2
      cd "$repo"
      for arch in "$@"; do
        set -- "$repo"/lindroids-*."$arch".xbps
        [ -e "$1" ] || continue
        XBPS_TARGET_ARCH=$arch xbps-rindex -a "$@" >/dev/null
        XBPS_TARGET_ARCH=$arch xbps-rindex --sign --signedby "Lindroids <lindroids@users.noreply.github.com>" \
          --privkey "$key" "$repo" >/dev/null
      done
      xbps-rindex --sign-pkg --privkey "$key" "$repo"/*.xbps >/dev/null' sh "$OUTDIR/xbps" "$RSA_DIR" $XBPS_ARCHES
    for arch in $XBPS_ARCHES; do
      [ -f "$OUTDIR/xbps/$arch-repodata" ] && echo "    xbps/$arch: $(find "$OUTDIR/xbps" -name "*.$arch.xbps" | wc -l) package(s)"
    done
  fi
fi

# --- site root -----------------------------------------------------------------------------
cp "$PUBKEY" "$OUTDIR/lindroids.gpg"
if [ "$PUBKEY" = "$HERE/lindroids.gpg" ]; then
  cp "$HERE/lindroids.asc" "$OUTDIR/lindroids.asc"
else
  gpg --batch --armor --export "$WANT_FPR" > "$OUTDIR/lindroids.asc"
fi
[ -f "$RSA_PUB" ] && install -m 644 "$RSA_PUB" "$OUTDIR/lindroids.rsa.pub"
sed_args=(-e "s|^DEFAULT_KEY_FPR=.*|DEFAULT_KEY_FPR=\"$WANT_FPR\"|")
[ -n "$BASE_URL" ] && sed_args+=(-e "s|^DEFAULT_REPO_URL=.*|DEFAULT_REPO_URL=\"${BASE_URL%/}\"|")
[ -n "$GITHUB_REPO" ] && sed_args+=(-e "s|^DEFAULT_GITHUB_REPO=.*|DEFAULT_GITHUB_REPO=\"$GITHUB_REPO\"|")
sed "${sed_args[@]}" "$ROOT/install.sh" > "$OUTDIR/install.sh"
chmod 644 "$OUTDIR/install.sh"
# Native Flatpak for Termux: the build kit install.sh fetches (packaging/termux-flatpak).
rm -rf "$OUTDIR/termux-flatpak"
mkdir -p "$OUTDIR/termux-flatpak"
install -m 644 "$ROOT"/packaging/termux-flatpak/* "$OUTDIR/termux-flatpak/"
# VERSION: the newest lindroids in the pool (the static installer downloads that release).
latest=$(find "$OUTDIR/pool" -name 'lindroids_*.deb' -printf '%f\n' | awk -F_ '{print $2}' | sort -uV | tail -1)
[ -n "$latest" ] && echo "$latest" > "$OUTDIR/VERSION"
touch "$OUTDIR/.nojekyll"
site_url=${BASE_URL:-$(sed -n 's/^DEFAULT_REPO_URL="\(.*\)"/\1/p' "$ROOT/install.sh")}
site_url=${site_url%/}
if [ -d "$OUTDIR/rpm" ]; then
  # For dnf config-manager / zypper addrepo. Packages are checked through the signed metadata.
  cat > "$OUTDIR/rpm/lindroids.repo" <<EOF
[lindroids]
name=Lindroids
baseurl=$site_url/rpm
enabled=1
gpgcheck=0
repo_gpgcheck=1
gpgkey=$site_url/lindroids.asc
EOF
fi
# A plain page for a fresh repository; the published site has its own (site/, site.yml).
[ -e "$OUTDIR/index.html" ] || cat > "$OUTDIR/index.html" <<EOF
<!doctype html>
<meta charset="utf-8">
<title>Lindroids package repository</title>
<style>body{font:16px/1.5 system-ui,sans-serif;max-width:46rem;margin:3rem auto;padding:0 1rem}code,pre{background:#f3f3f3;padding:.1rem .3rem}pre{padding:.8rem;overflow:auto}</style>
<h1>Lindroids</h1>
<p>Linux GUI programs on Android, in windows and apps of their own. Latest version: <b>${latest:-none}</b>.</p>
<p>Install or update (Termux, or any Linux distribution, also under proot-distro, UserLAnd or Andronix):</p>
<pre>curl -fsSL $site_url/install.sh | sh</pre>
<p>Manual APT setup: key <a href="lindroids.gpg">lindroids.gpg</a> (<a href="lindroids.asc">armored</a>), fingerprint <code>$WANT_FPR</code>.</p>
<pre># Termux
deb [signed-by=\$PREFIX/etc/apt/keyrings/lindroids.gpg] $site_url termux main
# Debian / Ubuntu
deb [signed-by=/etc/apt/keyrings/lindroids.gpg] $site_url stable main</pre>
<p>pacman (after <code>pacman-key --add lindroids.asc &amp;&amp; pacman-key --lsign-key $WANT_FPR</code>):</p>
<pre># Termux with pacman: \$PREFIX/etc/pacman.conf
[lindroids]
Server = $site_url/pacman/termux/\$arch
# Arch Linux and derivatives: /etc/pacman.conf
[lindroids]
Server = $site_url/pacman/arch/\$arch</pre>
<p>dnf / yum (Fedora, RHEL, Rocky, Alma, CentOS) and zypper (openSUSE): <a href="rpm/lindroids.repo">rpm/lindroids.repo</a> into <code>/etc/yum.repos.d/</code>, or <code>zypper addrepo $site_url/rpm/lindroids.repo</code>; metadata signed with the key above.</p>
<p>apk (Alpine, postmarketOS, Chimera): key <a href="lindroids.rsa.pub">lindroids.rsa.pub</a> into <code>/etc/apk/keys/</code>, then</p>
<pre>echo $site_url/apk &gt;&gt; /etc/apk/repositories
apk update &amp;&amp; apk add lindroids</pre>
<p>xbps (Void Linux; imports the same RSA key on the first sync):</p>
<pre>echo repository=$site_url/xbps &gt; /etc/xbps.d/lindroids.conf
xbps-install -S lindroids</pre>
EOF
echo "==> repository ready in $OUTDIR (latest ${latest:-none}, key $WANT_FPR)"
