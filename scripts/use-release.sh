#!/usr/bin/env bash
# Go back from a dev build (scripts/build.sh) to an official release: the latest or a given one.
# Run it where Lindroids is installed: in Termux, or in the Debian/Ubuntu (proot distro or PC).
#
#   scripts/use-release.sh [latest | <version>]     e.g. 0.1.130 (or v0.1.130); default latest
#   scripts/use-release.sh --list                   the releases, newest first
#
# A version still in the APT repository (the newest three) is installed from it, signed; an older
# one is downloaded from its GitHub release and checked against the release's SHA256SUMS. Either
# way it is installed over whatever `lindroids` package is there (a newer one too), and the usual
# `pkg upgrade` / `apt upgrade` take it on from there. A static install (no package) is replaced
# by that release's static binary, through install.sh.
# Then an idle server is stopped, so the next program starts the release.
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

if [ $LIST = 1 ]; then
  current=$(lindroids version 2>/dev/null | awk '{print $2}' || true)
  echo "Installed: ${current:-none}"
  curl -fsSL "https://api.github.com/repos/$GITHUB_REPO/releases?per_page=30" |
    sed -n 's/^ *"tag_name": *"v\{0,1\}\([^"]*\)".*/\1/p' || die "cannot list the releases"
  exit 0
fi

if [ "$WANT" = latest ]; then
  WANT=$(curl -fsSL "$REPO_URL/VERSION" | tr -d '[:space:]') || die "cannot fetch $REPO_URL/VERSION"
fi
printf '%s\n' "$WANT" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || die "'$WANT' is not a version like 0.1.130 (--list)"

# root for apt on a Debian system; Termux and proot shells are root or need none
SUDO=
if [ -z "${TERMUX_VERSION:-}" ] && [ "${PREFIX:-}" != /data/data/com.termux/files/usr ] && [ "$(id -u)" != 0 ]; then
  command -v sudo >/dev/null || die "apt needs root: run it as root"
  SUDO=sudo
fi

restart_idle() {
  local st
  st=$(lindroids status 2>/dev/null) || return 0
  if printf '%s\n' "$st" | grep -q '^window '; then
    echo "The server has windows open: close them and run \`lindroids stop\` to use $WANT."
  elif printf '%s\n' "$st" | grep -q '^server '; then
    lindroids stop >/dev/null
  fi
}

if command -v dpkg-query >/dev/null && dpkg-query -W -f '${Status}' lindroids 2>/dev/null | grep -q 'ok installed' ||
   { command -v apt-get >/dev/null && ! command -v lindroids >/dev/null; }; then
  installed=$(dpkg-query -W -f '${Version}' lindroids 2>/dev/null || true)
  echo "==> lindroids ${installed:-not installed} -> $WANT"
  $SUDO apt-get update -q >/dev/null 2>&1 || echo "use-release: apt-get update failed; trying the GitHub release" >&2
  if apt-cache madison lindroids 2>/dev/null | awk -F'|' '{gsub(/ /, "", $2); print $2}' | grep -qxF "$WANT"; then
    $SUDO apt-get install --reinstall --allow-downgrades -y "lindroids=$WANT"
  else
    arch=$(dpkg --print-architecture)
    deb=lindroids_${WANT}_$arch.deb
    tmp=$(mktemp -d "${TMPDIR:-/tmp}/use-release.XXXXXX")
    trap 'rm -rf "$tmp"' EXIT
    echo "==> $deb from the GitHub release v$WANT"
    curl -fsSL -o "$tmp/SHA256SUMS" "$RELEASES/v$WANT/SHA256SUMS" || die "no release v$WANT (--list)"
    curl -fsSL -o "$tmp/$deb" "$RELEASES/v$WANT/$deb" || die "release v$WANT has no $deb"
    # SHA256SUMS names it debs/<deb>
    sum=$(awk -v f="$deb" '$2 == f || $2 == "debs/" f {print $1}' "$tmp/SHA256SUMS")
    [ -n "$sum" ] && [ "$(sha256sum "$tmp/$deb" | cut -d' ' -f1)" = "$sum" ] || die "$deb does not match the release's SHA256SUMS"
    chmod 644 "$tmp/$deb"
    $SUDO apt-get install --reinstall --allow-downgrades -y "$tmp/$deb"
  fi
elif command -v lindroids >/dev/null; then
  if command -v pacman >/dev/null && pacman -Qqo "$(command -v lindroids)" >/dev/null 2>&1; then
    die "this lindroids is a pacman package: \`pacman -S lindroids\` installs the latest release (pacman keeps no older ones)"
  fi
  dir=$(dirname "$(readlink -f "$(command -v lindroids)")")
  echo "==> static lindroids in $dir -> $WANT"
  curl -fsSL "$REPO_URL/install.sh" |
    LINDROIDS_METHOD=static LINDROIDS_VERSION=$WANT LINDROIDS_INSTALL_DIR=$dir LINDROIDS_FLATPAK=no sh
else
  die "no lindroids here, and no apt to install it with: see install.sh"
fi
lindroids version
restart_idle
