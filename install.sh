#!/bin/sh
# Install or update Lindroids.
#
#   curl -fsSL https://dixonsolutions.github.io/lindroids/install.sh | sh
#
# It looks at what the system really has (the package managers that are present and run, with
# /etc/os-release as the tie-breaker) and installs the way that fits:
#   Termux with apt (the default)   -> APT repository, suite `termux` (bionic build)
#   Termux with pacman              -> pacman repository pacman/termux (bionic build)
#   Debian, Ubuntu, Kali, Mint, ... -> APT repository, suite `stable` (static build)
#   Arch, Manjaro, Artix, EndeavourOS -> pacman repository pacman/arch (static build)
#   Fedora, RHEL 8+, Rocky, Alma, CentOS Stream, Oracle, Amazon 2023 -> RPM repository rpm/ (dnf)
#   openSUSE, SLE                   -> RPM repository rpm/ with zypper
#   Alpine, postmarketOS, Adélie, Chimera -> apk repository apk/ (static build, no glibc parts)
#   Void (glibc and musl)           -> xbps repository xbps/
#   anything else, or a repository that is not there -> static binary from the latest GitHub
#                                      release into ~/.local/bin
# The same works under proot (proot-distro, UserLAnd, Andronix), where the shell is root. Outside
# Termux, system changes need root: it runs them directly as root, else with sudo or doas.
# It also checks for Flatpak and, when it is missing, asks to install it (with Flathub): with the
# system's package manager, or in Termux (which has no Flatpak package) in a proot-distro distro —
# one you have, or, if you say yes, a Debian it sets up for that.
#
# Safe to run again: it rewrites the same source and key files and upgrades the package.
#
# Environment (all optional):
#   LINDROIDS_REPO_URL      repository / site base URL (a mirror, or a local test repository)
#   LINDROIDS_GITHUB_REPO   owner/repo for release downloads
#   LINDROIDS_RELEASE_URL   base for release assets (default https://github.com/<repo>/releases/download)
#   LINDROIDS_VERSION       version for the static install (default: <repo url>/VERSION)
#   LINDROIDS_INSTALL_DIR   static install target (default ~/.local/bin)
#   LINDROIDS_METHOD        force apt-termux | pacman-termux | apt-debian | pacman-arch | dnf | yum |
#                           zypper | apk | xbps | static
#   LINDROIDS_FLATPAK       yes | no: install Flatpak (with Flathub) without asking, or skip it
#   LINDROIDS_DISTRO        proot-distro distro for Flatpak in Termux (default: the first one installed;
#                           with none, Debian is set up only when you answer yes)
set -eu

# build-repo.sh rewrites these lines for the repository it publishes.
DEFAULT_REPO_URL="https://dixonsolutions.github.io/lindroids"
DEFAULT_GITHUB_REPO="dixonSolutions/lindroids"
DEFAULT_KEY_FPR="CF71814EDB63C911B430479C6FFD91C90F43BF7B"

REPO_URL=${LINDROIDS_REPO_URL:-$DEFAULT_REPO_URL}
REPO_URL=${REPO_URL%/}
GITHUB_REPO=${LINDROIDS_GITHUB_REPO:-$DEFAULT_GITHUB_REPO}
RELEASE_URL=${LINDROIDS_RELEASE_URL:-https://github.com/$GITHUB_REPO/releases/download}
KEY_FPR=${LINDROIDS_KEY_FPR:-$DEFAULT_KEY_FPR}

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

TERMUX=
case "${PREFIX:-}" in
  *com.termux*) TERMUX=1 ;;
esac

# /etc/os-release, as a hint: which package manager this distribution is built around.
OS_ID='' OS_LIKE='' OS_NAME=''
if [ -z "$TERMUX" ] && [ -r /etc/os-release ]; then
  # shellcheck disable=SC1091
  OS_ID=$(. /etc/os-release && echo "${ID:-}")
  # shellcheck disable=SC1091
  OS_LIKE=$(. /etc/os-release && echo "${ID_LIKE:-}")
  # shellcheck disable=SC1091
  OS_NAME=$(. /etc/os-release && echo "${PRETTY_NAME:-${NAME:-}}")
fi
os_family() {
  for id in $OS_ID $OS_LIKE; do
    case "$id" in
      debian|ubuntu|kali|linuxmint|raspbian|pop|elementary|zorin|neon|deepin|openkylin|pardus|trisquel|devuan|parrot|mx) echo apt; return ;;
      arch|archarm|archlinux|manjaro|manjaro-arm|artix|endeavouros|garuda|cachyos|arcolinux) echo pacman; return ;;
      alpine|postmarketos|adelie|chimera|wolfi) echo apk; return ;;
      fedora|rhel|centos|rocky|almalinux|ol|amzn|nobara|ultramarine|circle|eurolinux|virtuozzo) echo dnf; return ;;
      opensuse|opensuse-*|suse|sles|sled|sle-micro) echo zypper; return ;;
      void) echo xbps; return ;;
    esac
  done
}

# pm_works <pm>: the package manager is installed and runs (not just a stray file of that name).
pm_works() {
  case "$1" in
    apt) have apt-get && have dpkg && apt-get --version >/dev/null 2>&1 ;;
    pacman) have pacman && pacman --version >/dev/null 2>&1 ;;
    dnf) have dnf && dnf --version >/dev/null 2>&1 ;;
    yum) have yum && yum --version >/dev/null 2>&1 ;;
    zypper) have zypper && zypper --version >/dev/null 2>&1 ;;
    apk) have apk && apk --version 2>/dev/null | grep -q apk-tools ;;
    xbps) have xbps-install && xbps-install --version >/dev/null 2>&1 ;;
    *) return 1 ;;
  esac
}

# The package manager in use: apt, pacman, dnf, yum, zypper, apk, xbps or none. Termux has apt or,
# with the pacman bootstrap, pacman; Termux itself says which in TERMUX_MAIN_PACKAGE_FORMAT.
# Elsewhere the one /etc/os-release points to, if it works (Debian can have pacman installed, Arch
# can have apt), else the first that works.
detect_pm() {
  if [ -n "$TERMUX" ]; then
    case "${TERMUX_MAIN_PACKAGE_FORMAT:-}" in
      pacman) have pacman && { echo pacman; return; } ;;
      debian) have apt && { echo apt; return; } ;;
    esac
    if have apt; then echo apt; elif have pacman; then echo pacman; else echo none; fi
    return
  fi
  hint=$(os_family)
  if [ "$hint" = dnf ] && ! pm_works dnf && pm_works yum; then hint=yum; fi
  if [ -n "$hint" ] && pm_works "$hint"; then echo "$hint"; return; fi
  for m in apt pacman dnf yum zypper apk xbps; do
    pm_works "$m" && { echo "$m"; return; }
  done
  echo none
}

# SUDO: what runs a command as root. Empty in Termux (no root there; its package managers run as
# the app's user) and as root; else sudo or doas. Fails when there is neither.
find_sudo() {
  SUDO=
  [ -n "$TERMUX" ] && return 0
  [ "$(id -u)" = 0 ] && return 0
  if have sudo; then SUDO=sudo
  elif have doas; then SUDO=doas
  else return 1
  fi
}

# How system changes are made, for the messages: "as root", "with sudo" or "with doas".
as_who() {
  if [ -n "$TERMUX" ]; then echo "as the Termux user"; elif [ -z "${SUDO:-}" ]; then echo "as root"; else echo "with $SUDO"; fi
}

# need_root <what it writes>: set SUDO, or stop with what to do.
need_root() {
  find_sudo || die "this writes to $1, which needs root: run it as root, or install sudo (or doas)"
}

# repo_has <path>: the repository at $REPO_URL has that file (it is published).
repo_has() {
  if have curl; then
    curl -fsSL -o /dev/null "$REPO_URL/$1" 2>/dev/null
  elif have wget; then
    wget -q -O /dev/null "$REPO_URL/$1" 2>/dev/null
  else
    return 1
  fi
}

# pm_install <package...>: install with $PM (and $SUDO).
APT_UPDATED=
pm_install() {
  case "$PM" in
    apt)
      if [ -n "$TERMUX" ]; then
        apt install -y "$@"
      else
        [ -n "$APT_UPDATED" ] || { $SUDO apt-get update -q || true; APT_UPDATED=1; }
        $SUDO env DEBIAN_FRONTEND=noninteractive apt-get install -y -q "$@"
      fi
      ;;
    pacman) $SUDO pacman -S --needed --noconfirm "$@" ;;
    dnf) $SUDO dnf install -y "$@" < /dev/null ;;
    yum) $SUDO yum install -y "$@" < /dev/null ;;
    zypper) $SUDO zypper --non-interactive install "$@" < /dev/null ;;
    # stdin is this script under `curl | sh`: nothing may read it (apk and xbps can ask).
    apk) $SUDO apk add "$@" < /dev/null ;;
    xbps) $SUDO xbps-install -Sy "$@" < /dev/null ;;
    *) return 1 ;;
  esac
}

# pacman's architecture (what $arch in a Server line becomes).
pacman_arch() {
  a=$(pacman-conf Architecture 2>/dev/null | head -1)
  case "$a" in
    ''|auto) uname -m ;;
    *) echo "$a" ;;
  esac
}

# next_steps <how to update>
next_steps() {
  echo
  say "Lindroids is installed: $(lindroids version 2>/dev/null || echo "run 'lindroids version'")"
  echo
  echo "Next: lindroids run <program>   # e.g. lindroids run foot; X11 programs work too"
  echo "  The windows show in the web browser: lindroids run opens it."
  if [ -n "${FLATPAK_DISTRO:-}" ]; then
    flatpak_hint " (inside: proot-distro login $FLATPAK_DISTRO)"
  elif have flatpak; then
    flatpak_hint ""
  fi
  echo
  echo "Update: $1"
}

FLATHUB=https://dl.flathub.org/repo/flathub.flatpakrepo

# ask <question> [yes|no]: yes or no from the terminal, also under `curl | sh` (whose stdin is the
# script); the second argument is what Enter means (default yes). LINDROIDS_FLATPAK answers it;
# with no terminal the answer is no.
ask() {
  case "${LINDROIDS_FLATPAK:-}" in
    y|yes|1|true) return 0 ;;
    n|no|0|false) return 1 ;;
  esac
  # In a subshell: a failed redirection on a special builtin exits a POSIX shell.
  (: < /dev/tty) 2>/dev/null || return 1
  if [ "${2:-yes}" = no ]; then printf '%s [y/N] ' "$1" > /dev/tty; else printf '%s [Y/n] ' "$1" > /dev/tty; fi
  read -r answer < /dev/tty || return 1
  case "$answer" in
    '') [ "${2:-yes}" != no ] ;;
    y|Y|yes|Yes|YES) return 0 ;;
    *) return 1 ;;
  esac
}

flatpak_hint() {
  echo "  Flatpak apps: lindroids run <app-id>, e.g. lindroids run org.gnome.Calculator"
  echo "     after: flatpak install flathub org.gnome.Calculator$1"
}

# proot-distro's root filesystem of a distro (newer proot-distro: containers/<name>/rootfs).
distro_rootfs() {
  base=$PREFIX/var/lib/proot-distro
  if [ -d "$base/containers/$1/rootfs" ]; then echo "$base/containers/$1/rootfs"; else echo "$base/installed-rootfs/$1"; fi
}

# Termux has no flatpak package: Flatpak goes into a proot-distro distro the user already has, where
# `lindroids run` finds its apps on its own. Sets FLATPAK_DISTRO to the distro that has it.
flatpak_termux() {
  for d in "$PREFIX"/var/lib/proot-distro/installed-rootfs/* "$PREFIX"/var/lib/proot-distro/containers/*/rootfs; do
    if [ -x "$d/usr/bin/flatpak" ]; then
      case "$d" in
        */rootfs) d=${d%/rootfs} ;;
      esac
      FLATPAK_DISTRO=${d##*/}
      say "Flatpak: found in the proot-distro distro $FLATPAK_DISTRO"
      return 0
    fi
  done
  # Flatpak has no Termux build: it goes into a proot-distro distro, with that distro's own
  # package manager — one the user has, or a Debian set up for it, only when the user says so.
  distro=''
  rootfs=''
  created=''
  if [ -n "${LINDROIDS_DISTRO:-}" ] && [ -d "$(distro_rootfs "$LINDROIDS_DISTRO")/usr" ]; then
    distro=$LINDROIDS_DISTRO
    rootfs=$(distro_rootfs "$distro")
  else
    for d in "$PREFIX"/var/lib/proot-distro/containers/*/rootfs "$PREFIX"/var/lib/proot-distro/installed-rootfs/*; do
      [ -d "$d/usr" ] || continue
      rootfs=$d
      case "$d" in
        */rootfs) d=${d%/rootfs} ;;
      esac
      distro=${d##*/}
      break
    done
  fi
  if [ -z "$distro" ]; then
    echo
    echo "Flatpak: not installed. Termux has no Flatpak package: Flatpak needs a Linux distro, which"
    echo "  proot-distro runs inside Termux, without root (Debian: about 300 MB, more with apps)."
    echo "  lindroids runs Termux programs without it; it is only for Flatpak apps."
    if ! ask "Set up proot-distro with Debian and install Flatpak (with Flathub) in it?" no; then
      echo "Skipping Flatpak (to add it later: LINDROIDS_FLATPAK=yes, then run this installer again)."
      return 0
    fi
    if ! have proot-distro; then
      say "installing proot-distro"
      pm_install proot-distro < /dev/null || { warn "installing proot-distro failed"; return 0; }
    fi
    say "proot-distro: installing Debian"
    proot-distro install debian < /dev/null || { warn "proot-distro could not install Debian"; return 0; }
    distro=debian
    rootfs=$(distro_rootfs debian)
    created=1
  fi
  # The distro's package manager, from its root filesystem.
  dpm=
  for m in apt-get pacman dnf zypper apk xbps-install; do
    if [ -e "$rootfs/usr/bin/$m" ] || [ -e "$rootfs/bin/$m" ] || [ -e "$rootfs/sbin/$m" ] || [ -e "$rootfs/usr/sbin/$m" ]; then
      dpm=$m
      break
    fi
  done
  case "$dpm" in
    apt-get) inst="export DEBIAN_FRONTEND=noninteractive; apt-get update -q && apt-get install -y -q flatpak" ;;
    pacman) inst="pacman -Sy --needed --noconfirm flatpak" ;;
    dnf) inst="dnf install -y flatpak" ;;
    zypper) inst="zypper --non-interactive install flatpak" ;;
    apk) inst="apk add flatpak" ;;
    xbps-install) inst="xbps-install -Sy flatpak" ;;
    *) echo "Flatpak: not installed (no known package manager in the distro $distro)."; return 0 ;;
  esac
  if ! have proot-distro; then
    echo "Flatpak: not installed (proot-distro is missing, so the distro $distro cannot be entered)."
    return 0
  fi
  if [ -z "$created" ]; then
    echo
    echo "Flatpak: not installed. Termux has no Flatpak package; it can go into your proot-distro"
    echo "  distro $distro, where lindroids finds its apps by itself."
    if ! ask "Install Flatpak (with Flathub) in $distro, with ${dpm%-install}?"; then
      echo "Skipping Flatpak (to add it later: LINDROIDS_FLATPAK=yes, then run this installer again)."
      return 0
    fi
  fi
  say "installing Flatpak in $distro (${dpm%-install}) and adding Flathub"
  proot-distro login "$distro" -- sh -c "$inst && flatpak remote-add --if-not-exists flathub $FLATHUB" \
    || { warn "installing Flatpak in $distro failed"; return 0; }
  FLATPAK_DISTRO=$distro
}

# Flatpak with the system's package manager, whichever it is.
flatpak_system() {
  if have flatpak; then
    say "Flatpak: found ($(flatpak --version 2>/dev/null || echo flatpak))"
    if ! flatpak remotes 2>/dev/null | grep -q '^flathub'; then
      if find_sudo; then
        $SUDO flatpak remote-add --if-not-exists flathub "$FLATHUB" || true
      else
        flatpak remote-add --user --if-not-exists flathub "$FLATHUB" || true
      fi
    fi
    return 0
  fi
  if [ "$PM" = none ]; then
    echo "Flatpak is not installed; for Flatpak apps install it with this system's package manager."
    return 0
  fi
  if ! find_sudo; then
    echo "Flatpak is not installed; for Flatpak apps install it as root ($PM)."
    return 0
  fi
  if ! ask "Install Flatpak (with Flathub) with $PM, to run Flatpak apps?"; then
    echo "Skipping Flatpak (LINDROIDS_FLATPAK=yes, or run this installer again, to install it later)."
    return 0
  fi
  say "$PM: installing flatpak"
  pm_install flatpak || { warn "installing flatpak with $PM failed"; return 0; }
  $SUDO flatpak remote-add --if-not-exists flathub "$FLATHUB" || warn "cannot add Flathub"
}

# add_pacman_repo <pacman.conf> <flavour>: trust the Lindroids key in pacman's keyring and add the
# [lindroids] repository. Its settings are in pacman.d/lindroids.conf, rewritten on every run, so
# pacman.conf itself only gets two lines, once.
add_pacman_repo() {
  conf=$1 flavour=$2
  confd=$(dirname "$conf")/pacman.d
  [ -f "$conf" ] || die "$conf not found"
  have pacman-key || die "pacman-key not found"
  say "pacman: trusting the Lindroids key $KEY_FPR"
  gpgdir=$(pacman-conf GPGDir 2>/dev/null | head -1)
  if [ ! -d "${gpgdir:-$confd/gnupg}" ]; then
    $SUDO pacman-key --init || die "pacman-key --init failed"
  fi
  tmp=$(mktemp)
  download "$REPO_URL/lindroids.asc" "$tmp"
  $SUDO pacman-key --add "$tmp" >/dev/null 2>&1 || die "pacman-key --add failed"
  rm -f "$tmp"
  # Signing needs the keyring's local master key, which container images ship without;
  # --init makes one when it is missing (and keeps what is there).
  if ! $SUDO pacman-key --lsign-key "$KEY_FPR" >/dev/null 2>&1; then
    $SUDO pacman-key --init >/dev/null 2>&1 || true
    $SUDO pacman-key --lsign-key "$KEY_FPR" >/dev/null \
      || die "cannot trust $KEY_FPR (is it the key at $REPO_URL/lindroids.asc?)"
  fi
  say "pacman: adding the Lindroids repository ($REPO_URL/pacman/$flavour)"
  tmp=$(mktemp)
  {
    echo "# Written by the Lindroids installer; included by [lindroids] in $conf."
    echo "SigLevel = Required DatabaseOptional"
    printf 'Server = %s/pacman/%s/$arch\n' "$REPO_URL" "$flavour"
  } > "$tmp"
  $SUDO mkdir -p "$confd"
  $SUDO cp "$tmp" "$confd/lindroids.conf"
  $SUDO chmod 644 "$confd/lindroids.conf"
  rm -f "$tmp"
  if ! grep -q '^\[lindroids\]' "$conf"; then
    printf '\n[lindroids]\nInclude = %s\n' "$confd/lindroids.conf" | $SUDO tee -a "$conf" >/dev/null
  elif ! grep -q "^Include *= *$confd/lindroids.conf" "$conf"; then
    warn "$conf already has a [lindroids] section of its own; it is left as it is"
  fi
  say "pacman -Sy lindroids"
  $SUDO pacman -Sy --needed --noconfirm lindroids || die "pacman -Sy lindroids failed"
}

install_termux_apt() {
  [ -n "${PREFIX:-}" ] || die "PREFIX is not set; is this Termux?"
  have apt || die "apt not found in Termux"
  PM=apt
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
  FLATPAK_DISTRO=
  flatpak_termux
  next_steps "pkg upgrade (or: apt update && apt install lindroids)"
}

install_termux_pacman() {
  [ -n "${PREFIX:-}" ] || die "PREFIX is not set; is this Termux?"
  have pacman || die "pacman not found in Termux"
  PM=pacman
  SUDO=
  say "Termux with pacman"
  add_pacman_repo "$PREFIX/etc/pacman.conf" termux
  FLATPAK_DISTRO=
  flatpak_termux
  next_steps "pkg upgrade (or: pacman -Syu)"
}

install_arch() {
  have pacman || die "pacman not found"
  PM=pacman
  find_sudo || die "run this as root (or install sudo): it writes to /etc/pacman.conf"
  say "Arch Linux ($(pacman_arch))"
  add_pacman_repo /etc/pacman.conf arch
  flatpak_system
  next_steps "pacman -Syu (or run this installer again)"
}

install_debian() {
  PM=apt
  find_sudo || die "run this as root (or install sudo): it writes to /etc/apt"
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
  APT_UPDATED=1
  say "apt-get install lindroids"
  $SUDO apt-get install -y -q lindroids || die "apt-get install lindroids failed"
  flatpak_system
  next_steps "apt update && apt upgrade (or run this installer again)"
}

# The rpm architecture: aarch64, armv7hl, x86_64 or i686, or empty if there is no package for it.
rpm_arch() {
  a=$(rpm --eval '%{_arch}' 2>/dev/null || uname -m)
  case "$a" in
    x86_64|aarch64|i686) echo "$a" ;;
    armv7hl|armv7l|armv7hnl|armv8l) echo armv7hl ;;
  esac
}

# dnf / yum (Fedora and the RHEL family) and zypper (openSUSE, SLE): one RPM repository. Its
# metadata is signed (repo_gpgcheck), and lists the checksum of every package.
install_rpm() {
  PM=$1
  case "$PM" in
    zypper) repofile=/etc/zypp/repos.d/lindroids.repo ;;
    *) repofile=/etc/yum.repos.d/lindroids.repo ;;
  esac
  need_root "$repofile"
  say "${OS_NAME:-RPM system} ($PM, $(rpm_arch)): adding the Lindroids RPM repository $REPO_URL/rpm"
  say "  as $repofile, and installing lindroids $(as_who)"
  tmp=$(mktemp)
  {
    echo "# Written by the Lindroids installer."
    echo "[lindroids]"
    echo "name=Lindroids"
    echo "baseurl=$REPO_URL/rpm"
    echo "enabled=1"
    echo "gpgcheck=0"
    echo "repo_gpgcheck=1"
    echo "gpgkey=$REPO_URL/lindroids.asc"
    if [ "$PM" = zypper ]; then
      echo "type=rpm-md"
      echo "autorefresh=1"
      echo "pkg_gpgcheck=0"
    fi
  } > "$tmp"
  $SUDO mkdir -p "$(dirname "$repofile")"
  $SUDO install -m 644 "$tmp" "$repofile"
  rm -f "$tmp"
  case "$PM" in
    dnf)
      say "dnf install lindroids"
      $SUDO dnf -y --refresh install lindroids < /dev/null || die "dnf install lindroids failed"
      update="dnf upgrade lindroids (or run this installer again)"
      ;;
    yum)
      say "yum install lindroids"
      $SUDO yum -y makecache < /dev/null >/dev/null 2>&1 || true
      $SUDO yum -y install lindroids < /dev/null || die "yum install lindroids failed"
      update="yum update lindroids (or run this installer again)"
      ;;
    zypper)
      # zypper trusts the repository key on first use only when told to; it is the key published
      # next to the repository (as apt and pacman get it), fetched over the same connection.
      say "zypper refresh lindroids && zypper install lindroids"
      $SUDO zypper --non-interactive --gpg-auto-import-keys refresh lindroids < /dev/null \
        || die "zypper refresh lindroids failed"
      $SUDO zypper --non-interactive install lindroids < /dev/null || die "zypper install lindroids failed"
      update="zypper update lindroids (or run this installer again)"
      ;;
  esac
  flatpak_system
  next_steps "$update"
}

# Alpine and the other apk systems: the signing key into /etc/apk/keys, the repository into
# /etc/apk/repositories.d/lindroids.list where apk-tools 3 reads that directory (Chimera), else a
# line in /etc/apk/repositories.
install_apk() {
  PM=apk
  arch=$(apk --print-arch)
  need_root /etc/apk
  if [ -d /etc/apk/repositories.d ]; then where=/etc/apk/repositories.d/lindroids.list; else where=/etc/apk/repositories; fi
  say "${OS_NAME:-apk system} (apk, $arch): adding the Lindroids apk repository $REPO_URL/apk"
  say "  to $where, its key as /etc/apk/keys/lindroids.rsa.pub, and installing lindroids $(as_who)"
  tmp=$(mktemp)
  download "$REPO_URL/lindroids.rsa.pub" "$tmp"
  $SUDO mkdir -p /etc/apk/keys
  $SUDO install -m 644 "$tmp" /etc/apk/keys/lindroids.rsa.pub
  if [ "$where" = /etc/apk/repositories ]; then
    if ! grep -qxF "$REPO_URL/apk" /etc/apk/repositories 2>/dev/null; then
      echo "$REPO_URL/apk" | $SUDO tee -a /etc/apk/repositories >/dev/null
    fi
  else
    echo "$REPO_URL/apk" > "$tmp"
    $SUDO install -m 644 "$tmp" "$where"
  fi
  rm -f "$tmp"
  say "apk update && apk add lindroids"
  $SUDO apk update < /dev/null >/dev/null || die "apk update failed"
  $SUDO apk add lindroids < /dev/null || die "apk add lindroids failed"
  flatpak_system
  next_steps "apk upgrade (or run this installer again)"
}

# Void: /etc/xbps.d/lindroids.conf. xbps asks whether to trust a repository's key on its first
# sync; the answer is yes for this repository only (-i: no other repository is synced).
install_xbps() {
  PM=xbps
  need_root /etc/xbps.d
  say "${OS_NAME:-Void} (xbps, $(xbps-uhelper arch 2>/dev/null)): adding the Lindroids xbps repository $REPO_URL/xbps"
  say "  as /etc/xbps.d/lindroids.conf, trusting its key, and installing lindroids $(as_who)"
  tmp=$(mktemp)
  echo "repository=$REPO_URL/xbps" > "$tmp"
  $SUDO mkdir -p /etc/xbps.d
  $SUDO install -m 644 "$tmp" /etc/xbps.d/lindroids.conf
  rm -f "$tmp"
  yes | $SUDO xbps-install -S -i -R "$REPO_URL/xbps" >/dev/null 2>&1 || true
  say "xbps-install -S lindroids"
  $SUDO xbps-install -Sy lindroids < /dev/null || die "xbps-install lindroids failed"
  flatpak_system
  next_steps "xbps-install -Su (or run this installer again)"
}

install_static() {
  case "$(uname -m)" in
    x86_64|amd64) arch=x86_64 ;;
    armv8l|armv7*|arm) arch=armv7 ;;   # armv8l: 32-bit userspace on a 64-bit CPU
    aarch64|arm64|armv8*) arch=aarch64 ;;
    i386|i486|i586|i686|x86) arch=i686 ;;
    *) die "no Lindroids build for $(uname -m)" ;;
  esac
  # Minimal images (Amazon Linux, some proot rootfs) lack tar or gzip; the package manager has them.
  need=
  have tar || need=" tar"
  have gzip || need="$need gzip"
  if [ -n "$need" ] && [ "$PM" != none ] && find_sudo; then
    say "$PM: installing$need"
    # shellcheck disable=SC2086
    pm_install $need || warn "installing$need failed"
  fi
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
  # Not when `lindroids update` runs this: that is about the CLI only.
  [ -n "${LINDROIDS_INSTALL_DIR:-}" ] || flatpak_system
  next_steps "run this installer again"
}

# fallback <why>: the static build instead of a repository.
fallback() {
  say "$1: installing the static build instead"
  method=static
}

PM=$(detect_pm)
method=${LINDROIDS_METHOD:-}
if [ -z "$method" ]; then
  if [ -n "$TERMUX" ]; then
    case "$PM" in
      apt) method=apt-termux ;;
      pacman) method=pacman-termux ;;
      *) die "this Termux has neither apt nor pacman" ;;
    esac
  else
    case "$PM" in
      apt)
        method=static
        if [ -f /etc/debian_version ]; then
          case "$(dpkg --print-architecture)" in
            arm64|armhf|amd64|i386) method=apt-debian ;;
          esac
        fi
        ;;
      pacman)
        case "$(pacman_arch)" in
          x86_64|aarch64|armv7h|i686) method=pacman-arch ;;
          *) method=static ;;
        esac
        ;;
      dnf|yum|zypper)
        method=$PM
        if [ "$PM" = yum ]; then
          # yum without dnf is yum 3 (RHEL/CentOS 7, Amazon Linux 2): its GnuPG 2.0 cannot check
          # the repository's ed25519 signature.
          fallback "yum without dnf cannot check the Lindroids repository's signature"
        elif [ -z "$(rpm_arch)" ]; then
          fallback "no Lindroids RPM for $(uname -m)"
        elif ! repo_has rpm/repodata/repomd.xml; then
          fallback "the Lindroids RPM repository is not at $REPO_URL/rpm"
        fi
        ;;
      apk)
        method=apk
        arch=$(apk --print-arch)
        case "$arch" in
          aarch64|armv7|x86_64|x86)
            repo_has "apk/$arch/APKINDEX.tar.gz" || fallback "the Lindroids apk repository is not at $REPO_URL/apk"
            ;;
          *) fallback "no Lindroids apk for $arch" ;;
        esac
        ;;
      xbps)
        method=xbps
        arch=$(xbps-uhelper arch 2>/dev/null || true)
        case "$arch" in
          aarch64|aarch64-musl|armv7l|armv7l-musl|x86_64|x86_64-musl|i686)
            repo_has "xbps/$arch-repodata" || fallback "the Lindroids xbps repository is not at $REPO_URL/xbps"
            ;;
          *) fallback "no Lindroids xbps package for ${arch:-this architecture}" ;;
        esac
        ;;
      *) method=static ;;
    esac
  fi
fi
# A repository needs root (or sudo/doas); a user without either still gets the static build.
if [ -z "${LINDROIDS_METHOD:-}" ] && [ -z "$TERMUX" ] && [ "$method" != static ] && ! find_sudo; then
  fallback "not root, and neither sudo nor doas is installed"
fi
say "${OS_NAME:+$OS_NAME; }package manager: $PM; installing with: $method"

case "$method" in
  apt-termux) install_termux_apt ;;
  pacman-termux) install_termux_pacman ;;
  apt-debian) install_debian ;;
  pacman-arch) install_arch ;;
  dnf|yum|zypper)
    pm_works "$method" || die "LINDROIDS_METHOD=$method, but $method does not work here"
    install_rpm "$method"
    ;;
  apk) install_apk ;;
  xbps) install_xbps ;;
  static) install_static ;;
  *) die "unknown LINDROIDS_METHOD $method" ;;
esac
