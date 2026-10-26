#!/bin/sh
# Install or update Lindroids.
#
#   curl -fsSL https://dixonsolutions.github.io/lindroids/install.sh | sh
#
# Termux                       -> APT repository, suite `termux` (bionic build)
# Debian/Ubuntu (also proot)   -> APT repository, suite `stable` (static build)
# anything else                -> static binary from the latest GitHub release into ~/.local/bin
#
# Safe to run again: it rewrites the same source and key files and upgrades the package.
#
# Environment (all optional):
#   LINDROIDS_REPO_URL      APT repository / site base URL
#   LINDROIDS_GITHUB_REPO   owner/repo for release downloads
#   LINDROIDS_RELEASE_URL   base for release assets (default https://github.com/<repo>/releases/download)
#   LINDROIDS_VERSION       version for the static install (default: <repo url>/VERSION)
#   LINDROIDS_INSTALL_DIR   static install target (default ~/.local/bin)
#   LINDROIDS_METHOD        force apt-termux | apt-debian | static
set -eu

# build-repo.sh rewrites these two lines for the repository it publishes.
DEFAULT_REPO_URL="https://dixonsolutions.github.io/lindroids"
DEFAULT_GITHUB_REPO="dixonSolutions/lindroids"

REPO_URL=${LINDROIDS_REPO_URL:-$DEFAULT_REPO_URL}
REPO_URL=${REPO_URL%/}
GITHUB_REPO=${LINDROIDS_GITHUB_REPO:-$DEFAULT_GITHUB_REPO}
RELEASE_URL=${LINDROIDS_RELEASE_URL:-https://github.com/$GITHUB_REPO/releases/download}

say() { if [ -t 1 ]; then printf '\033[1m==>\033[0m %s\n' "$*"; else printf '==> %s\n' "$*"; fi; }
warn() { printf 'lindroids-install: %s\n' "$*" >&2; }
die() { printf 'lindroids-install: error: %s\n' "$*" >&2; exit 1; }

# download <url> <file>
download() {
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL --retry 3 -o "$2" "$1" || die "download failed: $1"
  elif command -v wget >/dev/null 2>&1; then
    wget -q -O "$2" "$1" || die "download failed: $1"
  else
    die "neither curl nor wget is installed"
  fi
}

have() { command -v "$1" >/dev/null 2>&1; }

next_steps() {
  apk=$1 update=$2
  echo
  say "Lindroids is installed: $(lindroids version 2>/dev/null || echo "run 'lindroids version'")"
  echo
  echo "Next steps:"
  if [ -n "$apk" ]; then
    echo "  1. lindroids setup           # installs the Lindroids app ($apk) and pairs it"
  else
    echo "  1. Install the Lindroids app on the phone: download lindroids-<version>.apk from"
    echo "       https://github.com/$GITHUB_REPO/releases/latest"
    echo "     then: lindroids setup     # pairs it with this terminal"
  fi
  echo "  2. lindroids run <command>   # e.g. lindroids run foot; X11 programs work too"
  echo "     lindroids export <command> puts an app on the Android home screen."
  echo "     lindroids update brings the CLI and the app to the newest release."
  echo
  echo "Update: $update"
}

install_termux() {
  [ -n "${PREFIX:-}" ] || die "PREFIX is not set; is this Termux?"
  have apt || die "apt not found in Termux"
  say "Termux: adding the Lindroids APT repository ($REPO_URL, suite termux)"
  mkdir -p "$PREFIX/etc/apt/sources.list.d" "$PREFIX/etc/apt/keyrings"
  key=$PREFIX/etc/apt/keyrings/lindroids.gpg
  download "$REPO_URL/lindroids.gpg" "$key.tmp"
  mv "$key.tmp" "$key"
  chmod 644 "$key"
  echo "deb [signed-by=$key] $REPO_URL termux main" > "$PREFIX/etc/apt/sources.list.d/lindroids.list"
  say "apt update"
  apt update || die "apt update failed"
  say "apt install lindroids"
  apt install -y lindroids || die "apt install lindroids failed"
  next_steps "$PREFIX/share/lindroids/lindroids.apk" "pkg upgrade (or: apt update && apt install lindroids)"
}

install_debian() {
  if [ "$(id -u)" = 0 ]; then
    SUDO=
  elif have sudo; then
    SUDO=sudo
  else
    die "run this as root (or install sudo): it writes to /etc/apt"
  fi
  export DEBIAN_FRONTEND=noninteractive
  # HTTPS repositories need the CA bundle; minimal images (and some proot rootfs) lack it.
  case "$REPO_URL" in
    https:*)
      if [ ! -e /etc/ssl/certs/ca-certificates.crt ]; then
        say "installing ca-certificates"
        $SUDO apt-get update -q || true
        $SUDO apt-get install -y -q ca-certificates || die "cannot install ca-certificates"
      fi
      ;;
  esac
  say "Debian/Ubuntu: adding the Lindroids APT repository ($REPO_URL, suite stable)"
  $SUDO mkdir -p /etc/apt/keyrings /etc/apt/sources.list.d
  tmp=$(mktemp)
  download "$REPO_URL/lindroids.gpg" "$tmp"
  $SUDO install -m 644 "$tmp" /etc/apt/keyrings/lindroids.gpg
  echo "deb [signed-by=/etc/apt/keyrings/lindroids.gpg] $REPO_URL stable main" > "$tmp"
  $SUDO install -m 644 "$tmp" /etc/apt/sources.list.d/lindroids.list
  rm -f "$tmp"
  say "apt-get update"
  $SUDO apt-get update -q || die "apt-get update failed"
  say "apt-get install lindroids"
  $SUDO apt-get install -y -q lindroids || die "apt-get install lindroids failed"
  next_steps /usr/share/lindroids/lindroids.apk "apt update && apt upgrade (or run this installer again)"
}

install_static() {
  case "$(uname -m)" in
    x86_64|amd64) arch=x86_64 ;;
    armv8l|armv7*|arm) arch=armv7 ;;   # armv8l: 32-bit userspace on a 64-bit CPU
    aarch64|arm64|armv8*) arch=aarch64 ;;
    i386|i486|i586|i686|x86) arch=i686 ;;
    *) die "no Lindroids build for $(uname -m)" ;;
  esac
  have tar || die "tar is required"
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  version=${LINDROIDS_VERSION:-}
  if [ -z "$version" ]; then
    download "$REPO_URL/VERSION" "$tmp/VERSION"
    version=$(tr -d ' \r\n' < "$tmp/VERSION")
  fi
  [ -n "$version" ] || die "cannot determine the latest version"
  name=lindroids-$version-$arch-linux-musl
  say "downloading $name"
  download "$RELEASE_URL/v$version/$name.tar.gz" "$tmp/$name.tar.gz"
  # Verify against the published checksums when they are there.
  if have sha256sum && (download "$RELEASE_URL/v$version/SHA256SUMS" "$tmp/SHA256SUMS") 2>/dev/null; then
    (cd "$tmp" && grep " $name.tar.gz\$" SHA256SUMS | sha256sum -c -) >/dev/null \
      || die "checksum mismatch for $name.tar.gz"
  fi
  tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
  dir=${LINDROIDS_INSTALL_DIR:-$HOME/.local/bin}
  mkdir -p "$dir"
  bin=$tmp/$name/bin/lindroids
  [ -f "$bin" ] || bin=$tmp/$name/lindroids   # releases before the bin/ + libexec/ layout
  cp "$bin" "$dir/lindroids.new"
  chmod 755 "$dir/lindroids.new"
  mv "$dir/lindroids.new" "$dir/lindroids"
  say "installed $dir/lindroids"
  # xwayland-satellite (X11 programs) goes where the CLI looks for it:
  # <bindir>/../libexec/lindroids/. It is a glibc build that uses the distro's libxcb, so a musl
  # system (Alpine) gets its own package instead, through `lindroids setup x11`.
  xwls=$tmp/$name/libexec/lindroids/xwayland-satellite
  libexec=$(dirname "$dir")/libexec/lindroids
  if [ -f "$xwls" ] && ! ls /lib/ld-musl-* >/dev/null 2>&1; then
    mkdir -p "$libexec"
    cp "$xwls" "$libexec/xwayland-satellite.new"
    chmod 755 "$libexec/xwayland-satellite.new"
    mv "$libexec/xwayland-satellite.new" "$libexec/xwayland-satellite"
    say "installed $libexec/xwayland-satellite"
    if have ldd && ldd "$libexec/xwayland-satellite" 2>&1 | grep -qiE 'not found|error'; then
      warn "for X11 programs, install glibc 2.34+, libxcb and libxcb-cursor (lindroids setup x11)"
    fi
  else
    # A glibc build left by an earlier install would only be skipped over.
    rm -f "$libexec/xwayland-satellite"
  fi
  if ! have Xwayland || { ! have xwayland-satellite && [ ! -f "$libexec/xwayland-satellite" ]; }; then
    warn "for X11 programs, run: lindroids setup x11"
  fi
  case ":$PATH:" in
    *":$dir:"*) ;;
    *) warn "$dir is not in PATH; add: export PATH=\"$dir:\$PATH\""; PATH=$dir:$PATH ;;
  esac
  next_steps "" "run this installer again"
}

method=${LINDROIDS_METHOD:-}
if [ -z "$method" ]; then
  case "${PREFIX:-}" in
    *com.termux*) method=apt-termux ;;
  esac
fi
if [ -z "$method" ]; then
  if have apt-get && have dpkg && [ -f /etc/debian_version ]; then
    case "$(dpkg --print-architecture)" in
      arm64|armhf|amd64|i386) method=apt-debian ;;
      *) method=static ;;
    esac
  else
    method=static
  fi
fi

case "$method" in
  apt-termux) install_termux ;;
  apt-debian) install_debian ;;
  static) install_static ;;
  *) die "unknown LINDROIDS_METHOD $method" ;;
esac
