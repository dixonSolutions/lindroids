#!/usr/bin/env bash
# Go back from a dev build (scripts/build.sh) to an official release: the latest or a given one.
# Run it where Lindroids is installed: in Termux, or in the Linux system (proot distro,
# container or PC).
#
#   scripts/use-release.sh [latest | <version>]     e.g. 0.1.130 (or v0.1.130); default latest
#   scripts/use-release.sh --list                   the releases, newest first
#
# It goes through whatever installed `lindroids`, found by which package owns it:
#   apt (Termux, Debian, Ubuntu)   from the APT repository while it has the version (the newest
#                                  three, signed), else the GitHub release's .deb, checked
#                                  against its SHA256SUMS
#   pacman (Termux, Arch)          from the pacman repository (the newest three; pacman checks
#                                  the package's signature)
#   dnf, yum, zypper (rpm)         from the RPM repository (the newest three, signed
#                                  metadata), else the GitHub release's .rpm (over HTTPS)
#   apk (Alpine & co.)             from the apk repository (the newest three, signed index);
#                                  else the release's static binary in /usr/local/bin
#   a static install               that release's static binary, through install.sh
# With no lindroids yet, the system's package manager is used as for apt above.
# The version is installed over whatever is there, a newer one too, and the usual upgrades take it
# on from there. Then an idle server is stopped, so the next program starts the release.
#
# Environment: LINDROIDS_REPO_URL (default https://dixonsolutions.github.io/lindroids),
# LINDROIDS_GITHUB_REPO (default dixonSolutions/lindroids).
set -euo pipefail

REPO_URL=${LINDROIDS_REPO_URL:-https://dixonsolutions.github.io/lindroids}
GITHUB_REPO=${LINDROIDS_GITHUB_REPO:-dixonSolutions/lindroids}
RELEASES=https://github.com/$GITHUB_REPO/releases/download

WANT=latest LIST=0
while [ $# -gt 0 ]; do
  case "$1" in
    --list) LIST=1; shift ;;
    -h|--help) awk 'NR > 1 && !/^#/ {exit} NR > 1 {sub(/^# ?/, ""); print}' "$0"; exit 0 ;;
    -*) echo "use-release: unknown argument $1 (--help)" >&2; exit 2 ;;
    *) WANT=${1#v}; shift ;;
  esac
done
die() { echo "use-release: $*" >&2; exit 1; }
command -v curl >/dev/null || die "curl is required"

IN_TERMUX=0
if [ -n "${TERMUX_VERSION:-}" ] || [ "${PREFIX:-}" = /data/data/com.termux/files/usr ]; then IN_TERMUX=1; fi
SUDO=''
if [ $IN_TERMUX = 0 ] && [ "$(id -u)" != 0 ]; then SUDO=sudo; fi

# Which package manager owns the installed lindroids: apt pacman rpm apk xbps static, or the
# system's own (apt pacman rpm apk xbps) when there is none.
installed_by() {
  local exe
  exe=$(command -v lindroids 2>/dev/null || true)
  if [ -n "$exe" ]; then
    exe=$(readlink -f "$exe")
    if command -v dpkg-query >/dev/null && dpkg-query -S "$exe" 2>/dev/null | grep -q '^lindroids:'; then echo apt
    elif command -v pacman >/dev/null && pacman -Qqo "$exe" >/dev/null 2>&1; then echo pacman
    elif command -v rpm >/dev/null && rpm -qf "$exe" >/dev/null 2>&1; then echo rpm
    elif command -v apk >/dev/null && apk info --who-owns "$exe" 2>/dev/null | grep -q lindroids; then echo apk
    elif command -v xbps-query >/dev/null && xbps-query -o "$exe" >/dev/null 2>&1; then echo xbps
    else echo static
    fi
    return
  fi
  if [ $IN_TERMUX = 1 ]; then
    if command -v apt-get >/dev/null; then echo apt; else echo pacman; fi
    return
  fi
  local c
  for c in apt-get pacman dnf zypper yum apk xbps-install; do
    if command -v $c >/dev/null; then
      case $c in apt-get) echo apt ;; dnf|zypper|yum) echo rpm ;; xbps-install) echo xbps ;; *) echo $c ;; esac
      return
    fi
  done
  echo static
}
HOW=$(installed_by)

if [ $LIST = 1 ]; then
  current=$(lindroids version 2>/dev/null | awk '{print $2}' || true)
  echo "Installed: ${current:-none} ($HOW)"
  case $HOW in
    pacman|apk) echo "($HOW: the repository keeps the newest three releases)" ;;
  esac
  curl -fsSL "https://api.github.com/repos/$GITHUB_REPO/releases?per_page=30" |
    sed -n 's/^ *"tag_name": *"v\{0,1\}\([^"]*\)".*/\1/p' || die "cannot list the releases"
  exit 0
fi

if [ "$WANT" = latest ]; then
  WANT=$(curl -fsSL "$REPO_URL/VERSION" | tr -d '[:space:]') || die "cannot fetch $REPO_URL/VERSION"
fi
printf '%s\n' "$WANT" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || die "'$WANT' is not a version like 0.1.130 (--list)"
if [ -n "$SUDO" ] && ! command -v sudo >/dev/null; then die "installing needs root: run it as root"; fi

restart_idle() {
  local st
  st=$(lindroids status 2>/dev/null) || return 0
  if printf '%s\n' "$st" | grep -q '^window '; then
    echo "The server has windows open: close them and run \`lindroids stop\` to use $WANT."
  elif printf '%s\n' "$st" | grep -q '^server '; then
    lindroids stop >/dev/null
  fi
}

TMP=$(mktemp -d "${TMPDIR:-/tmp}/use-release.XXXXXX")
trap 'rm -rf "$TMP"' EXIT
current=$(lindroids version 2>/dev/null | awk '{print $2}' || true)
echo "==> lindroids ${current:-not installed} -> $WANT ($HOW)"

case $HOW in
  apt)
    $SUDO apt-get update -q >/dev/null 2>&1 || echo "use-release: apt-get update failed; trying the GitHub release" >&2
    if apt-cache madison lindroids 2>/dev/null | awk -F'|' '{gsub(/ /, "", $2); print $2}' | grep -qxF "$WANT"; then
      $SUDO apt-get install --reinstall --allow-downgrades -y "lindroids=$WANT"
    else
      deb=lindroids_${WANT}_$(dpkg --print-architecture).deb
      echo "==> $deb from the GitHub release v$WANT"
      curl -fsSL -o "$TMP/SHA256SUMS" "$RELEASES/v$WANT/SHA256SUMS" || die "no release v$WANT (--list)"
      curl -fsSL -o "$TMP/$deb" "$RELEASES/v$WANT/$deb" || die "release v$WANT has no $deb"
      # SHA256SUMS names it debs/<deb>
      sum=$(awk -v f="$deb" '$2 == f || $2 == "debs/" f {print $1}' "$TMP/SHA256SUMS")
      [ -n "$sum" ] && [ "$(sha256sum "$TMP/$deb" | cut -d' ' -f1)" = "$sum" ] || die "$deb does not match the release's SHA256SUMS"
      chmod 755 "$TMP"; chmod 644 "$TMP/$deb"
      $SUDO apt-get install --reinstall --allow-downgrades -y "$TMP/$deb"
    fi ;;
  pacman)
    m=$(uname -m)
    if [ $IN_TERMUX = 1 ]; then
      flavour=termux
      case $m in armv7l|armv8l) arch=arm ;; *) arch=$m ;; esac
    else
      flavour=arch
      case $m in armv7l) arch=armv7h ;; *) arch=$m ;; esac
    fi
    url=$REPO_URL/pacman/$flavour/$arch/lindroids-$WANT-1-$arch.pkg.tar.xz
    curl -fsSI "$url" >/dev/null 2>&1 || die "the pacman repository has no $WANT any more (it keeps the newest three; --list)"
    $SUDO pacman -U --noconfirm "$url" ;;
  rpm)
    m=$(uname -m)
    case $m in armv7l|armv8l) arch=armv7hl ;; *) arch=$m ;; esac
    ok=0
    if command -v dnf >/dev/null; then
      $SUDO dnf -q makecache >/dev/null 2>&1 || true
      { $SUDO dnf install -y "lindroids-$WANT" || $SUDO dnf downgrade -y "lindroids-$WANT"; } >/dev/null 2>&1 && ok=1
    elif command -v zypper >/dev/null; then
      $SUDO zypper --non-interactive --quiet refresh >/dev/null 2>&1 || true
      $SUDO zypper --non-interactive install --oldpackage "lindroids=$WANT" >/dev/null 2>&1 && ok=1
    elif command -v yum >/dev/null; then
      { $SUDO yum install -y "lindroids-$WANT" || $SUDO yum downgrade -y "lindroids-$WANT"; } >/dev/null 2>&1 && ok=1
    fi
    if [ $ok = 1 ] && rpm -q --qf '%{VERSION}\n' lindroids | grep -qxF "$WANT"; then
      echo "==> from the RPM repository"
    else
      rpmf=lindroids-$WANT-1.$arch.rpm
      echo "==> $rpmf from the GitHub release v$WANT"
      curl -fsSL -o "$TMP/$rpmf" "$RELEASES/v$WANT/$rpmf" || die "release v$WANT has no $rpmf (--list)"
      $SUDO rpm -U --oldpackage --replacepkgs "$TMP/$rpmf"
    fi ;;
  apk)
    $SUDO apk update -q >/dev/null 2>&1 || true
    # lindroids=<version> pins it in /etc/apk/world; `apk add lindroids` takes the pin off again.
    if $SUDO apk add -q "lindroids=$WANT" 2>/dev/null; then
      $SUDO apk add -q lindroids
    else
      echo "==> the apk repository has no $WANT: the release's static binary instead, in /usr/local/bin"
      if apk info -e lindroids >/dev/null 2>&1; then $SUDO apk del -q lindroids; fi
      curl -fsSL "$REPO_URL/install.sh" | $SUDO env LINDROIDS_METHOD=static LINDROIDS_VERSION="$WANT" \
        LINDROIDS_INSTALL_DIR=/usr/local/bin LINDROIDS_FLATPAK=no sh
      hash -r
    fi ;;
  xbps)
    die "Void (xbps): \`xbps-install -Su lindroids\` brings the latest release; older ones are not kept" ;;
  static)
    exe=$(command -v lindroids || true)
    dir=$HOME/.local/bin
    [ -n "$exe" ] && dir=$(dirname "$(readlink -f "$exe")")
    echo "==> static lindroids in $dir"
    w=''
    mkdir -p "$dir" 2>/dev/null || true
    [ -w "$dir" ] || w=$SUDO
    curl -fsSL "$REPO_URL/install.sh" | $w env LINDROIDS_METHOD=static LINDROIDS_VERSION="$WANT" \
      LINDROIDS_INSTALL_DIR="$dir" LINDROIDS_FLATPAK=no sh ;;
esac
lindroids version
restart_idle
