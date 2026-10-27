#!/usr/bin/env bash
# End-to-end test of every package repository and of install.sh's detection, with throwaway keys.
#
#   tests/install-repos.sh [--bin <static x86_64 lindroids>] [--xwls target/xwls/out] [distro...]
#
# Makes the x86_64 packages of every kind from one static musl CLI (build-debs.sh, build-pacman.sh,
# build-rpm.sh, build-apk.sh, build-xbps.sh), a throwaway GPG key and RSA key, the repositories
# with packaging/repo/build-repo.sh, serves them on 127.0.0.1:$LINDROIDS_TEST_PORT (8790), and in a
# container of each distro runs `curl .../install.sh | sh` twice (the second run is an update):
# it must choose the expected method, install the package from the repository (or the static
# build, where it should fall back to that), and `lindroids version` must run.
#
# Distros (default: all): debian ubuntu kali archlinux fedora rocky opensuse-tumbleweed
#   opensuse-leap alpine alpine-edge chimera void-glibc void-musl amazonlinux2 busybox
# (the last two must fall back to the static build, served here as a release tarball)
#
# --bin: default target/x86_64-unknown-linux-musl/release/lindroids, built if missing.
# --xwls: the xwayland-satellite builds (debian-amd64/ is used; scripts/build-xwayland-satellite.sh).
# Needs podman or docker (LINDROIDS_CONTAINER_ENGINE), and what the packaging scripts need
# (dpkg-deb, bsdtar, gpg, openssl, python3).
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
# shellcheck source=packaging/container.sh
. "$ROOT/packaging/container.sh"

BIN=$ROOT/target/x86_64-unknown-linux-musl/release/lindroids
XWLS=$ROOT/target/xwls/out
PORT=${LINDROIDS_TEST_PORT:-8790}
DISTROS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --bin) BIN=$(readlink -f "$2"); shift 2 ;;
    --xwls) XWLS=$(readlink -f "$2"); shift 2 ;;
    -h|--help) awk 'NR > 1 && !/^#/ {exit} NR > 1 {sub(/^# ?/, ""); print}' "$0"; exit 0 ;;
    -*) echo "install-repos: unknown option $1" >&2; exit 2 ;;
    *) DISTROS+=("$1"); shift ;;
  esac
done
[ ${#DISTROS[@]} -gt 0 ] || DISTROS=(debian ubuntu kali archlinux fedora rocky opensuse-tumbleweed
  opensuse-leap alpine alpine-edge chimera void-glibc void-musl amazonlinux2 busybox)

# distro -> image | expected method | preparation (curl, where the image has none) | check
spec() {
  case "$1" in
    debian) echo "docker.io/library/debian:stable@apt-debian@apt-get update -q >/dev/null && apt-get install -y -q curl >/dev/null@dpkg -s lindroids >/dev/null && ! ldd /usr/libexec/lindroids/xwayland-satellite | grep 'not found' && /usr/libexec/lindroids/xwayland-satellite -help >/dev/null 2>&1 && command -v Xwayland" ;;
    ubuntu) echo "docker.io/library/ubuntu:latest@apt-debian@apt-get update -q >/dev/null && apt-get install -y -q curl >/dev/null@dpkg -s lindroids >/dev/null" ;;
    kali) echo "docker.io/kalilinux/kali-rolling:latest@apt-debian@apt-get update -q >/dev/null && apt-get install -y -q curl >/dev/null@dpkg -s lindroids >/dev/null" ;;
    archlinux) echo "docker.io/library/archlinux:latest@pacman-arch@true@pacman -Qi lindroids >/dev/null" ;;
    fedora) echo "registry.fedoraproject.org/fedora:latest@dnf@true@rpm -q lindroids && test -x /usr/libexec/lindroids/xwayland-satellite" ;;
    rocky) echo "docker.io/rockylinux/rockylinux:9@dnf@true@rpm -q lindroids" ;;
    opensuse-tumbleweed) echo "registry.opensuse.org/opensuse/tumbleweed:latest@zypper@true@rpm -q lindroids" ;;
    opensuse-leap) echo "registry.opensuse.org/opensuse/leap:15.6@zypper@true@rpm -q lindroids" ;;
    alpine) echo "docker.io/library/alpine:latest@apk@apk add -q curl@apk info -e lindroids && test ! -e /usr/libexec/lindroids/xwayland-satellite" ;;
    alpine-edge) echo "docker.io/library/alpine:edge@apk@apk add -q curl@apk info -e lindroids" ;;
    chimera) echo "docker.io/chimeralinux/chimera:latest@apk@apk add -q curl < /dev/null@apk info -e lindroids" ;;
    void-glibc) echo "ghcr.io/void-linux/void-glibc-full:latest@xbps@xbps-install -Syu xbps >/dev/null 2>&1; xbps-install -Sy curl >/dev/null@xbps-query lindroids >/dev/null && test -x /usr/libexec/lindroids/xwayland-satellite" ;;
    amazonlinux2) echo "docker.io/library/amazonlinux:2@static@true@true" ;;
    busybox) echo "docker.io/library/busybox:latest@static@true@true" ;;
    void-musl) echo "ghcr.io/void-linux/void-musl-full:latest@xbps@xbps-install -Syu xbps >/dev/null 2>&1; xbps-install -Sy curl >/dev/null@xbps-query lindroids >/dev/null && test ! -e /usr/libexec/lindroids/xwayland-satellite" ;;
    *) return 1 ;;
  esac
}
for d in "${DISTROS[@]}"; do spec "$d" >/dev/null || { echo "install-repos: unknown distro $d" >&2; exit 2; }; done

if [ ! -x "$BIN" ]; then
  echo "==> building the static CLI"
  cargo build -p lindroids --release --target x86_64-unknown-linux-musl
  BIN=$ROOT/target/x86_64-unknown-linux-musl/release/lindroids
fi
[ -f "$XWLS/debian-amd64/xwayland-satellite" ] || { echo "install-repos: no $XWLS/debian-amd64 (scripts/build-xwayland-satellite.sh debian)" >&2; exit 1; }

eval "$(scripts/version.sh)"
VER=$LINDROIDS_VERSION
T=$ROOT/target/install-repos
rm -rf "$T"
mkdir -p "$T/dist/bin/x86_64-unknown-linux-musl" "$T/keys"
install -m 755 "$BIN" "$T/dist/bin/x86_64-unknown-linux-musl/lindroids"
server=
cleanup() {
  [ -z "$server" ] || kill "$server" 2>/dev/null || true
  gpgconf --homedir "$T/keys/gnupg" --kill gpg-agent >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "==> packages $VER (x86_64)"
packaging/build-debs.sh --version "$VER" --dist "$T/dist" --out "$T/dist/debs" --arches amd64 --xwls "$XWLS"
packaging/build-pacman.sh --debs "$T/dist/debs" --out "$T/dist/pacman"
packaging/build-rpm.sh --version "$VER" --dist "$T/dist" --arches x86_64 --xwls "$XWLS"
packaging/build-apk.sh --version "$VER" --dist "$T/dist" --arches x86_64
packaging/build-xbps.sh --version "$VER" --dist "$T/dist" --arches "x86_64 x86_64-musl" --xwls "$XWLS"

echo "==> throwaway keys"
export GNUPGHOME=$T/keys/gnupg
mkdir -p "$GNUPGHOME" && chmod 700 "$GNUPGHOME"
gpg --batch --quiet --pinentry-mode loopback --passphrase '' \
  --quick-generate-key "lindroids test <test@example.invalid>" ed25519 sign never
gpg --batch --export > "$T/keys/test.gpg"
openssl genrsa -out "$T/keys/test.rsa" 2048 2>/dev/null
openssl pkey -in "$T/keys/test.rsa" -pubout -out "$T/keys/test.rsa.pub"

echo "==> release tarball (static installs; served as LINDROIDS_RELEASE_URL)"
name=lindroids-$VER-x86_64-linux-musl
mkdir -p "$T/tarball/$name/bin" "$T/tarball/$name/libexec/lindroids" "$T/releases/v$VER"
install -m 755 "$BIN" "$T/tarball/$name/bin/lindroids"
install -m 755 "$XWLS/debian-amd64/xwayland-satellite" "$T/tarball/$name/libexec/lindroids/"
tar -C "$T/tarball" -czf "$T/releases/v$VER/$name.tar.gz" "$name"
(cd "$T/releases/v$VER" && sha256sum "$name.tar.gz" > SHA256SUMS)

echo "==> repository"
env -u LINDROIDS_GPG_PRIVATE_KEY -u LINDROIDS_GNUPGHOME -u LINDROIDS_APK_RSA_KEY LINDROIDS_APK_RSA_KEY_FILE="$T/keys/test.rsa" \
  packaging/repo/build-repo.sh "$T/site" --debs "$T/dist/debs" --pacman "$T/dist/pacman" \
  --rpm "$T/dist/rpm" --apk "$T/dist/apk" --xbps "$T/dist/xbps" \
  --public-key "$T/keys/test.gpg" --rsa-public-key "$T/keys/test.rsa.pub" \
  --base-url "http://127.0.0.1:$PORT"

mv "$T/releases" "$T/site/releases"
python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$T/site" >/dev/null 2>&1 &
server=$!
sleep 1

engine=$(container_engine)
declare -A RESULT
mkdir -p "$T/logs"
for d in "${DISTROS[@]}"; do
  IFS='@' read -r image method prep check <<<"$(spec "$d")"
  log=$T/logs/$d.log
  echo "==> $d ($image): expecting $method"
  # shellcheck disable=SC2086
  if $engine run --rm --network host "$image" sh -c "
      set -e
      { $prep; } || { echo 'FAIL: preparing the container'; exit 1; }
      url=http://127.0.0.1:$PORT/install.sh
      export LINDROIDS_FLATPAK=no LINDROIDS_RELEASE_URL=http://127.0.0.1:$PORT/releases
      if command -v curl >/dev/null; then get='curl -fsSL'; else get='wget -qO-'; fi
      \$get \$url | sh > /tmp/1.log 2>&1 || { cat /tmp/1.log; exit 1; }
      cat /tmp/1.log
      grep -q 'installing with: $method\$' /tmp/1.log || { echo 'FAIL: not installed with $method'; exit 1; }
      echo '--- second run (update)'
      \$get \$url | sh
      if [ $method = static ]; then
        bin=\$HOME/.local/bin/lindroids
      else
        # From the package, not a static install into ~/.local/bin.
        bin=/usr/bin/lindroids
        test ! -e \"\$HOME/.local/bin/lindroids\" || { echo 'FAIL: a static install'; exit 1; }
      fi
      \$bin version
      { $check; } || { echo 'FAIL: $d check'; exit 1; }
      echo 'PASS'" > "$log" 2>&1; then
    RESULT[$d]="PASS ($method; $(grep -m1 '^lindroids [0-9]' "$log" || true))"
  else
    RESULT[$d]="FAIL (log: $log)"
    tail -15 "$log" | sed 's/^/    /'
  fi
  echo "    ${RESULT[$d]}"
done

echo "==> results"
fail=0
for d in "${DISTROS[@]}"; do
  printf '    %-20s %s\n' "$d" "${RESULT[$d]}"
  case "${RESULT[$d]}" in PASS*) ;; *) fail=1 ;; esac
done
exit $fail
