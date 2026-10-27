#!/usr/bin/env bash
# Release notes for v<version>: what Lindroids is, how to install or upgrade it on each system,
# and the commits since the previous release tag.
#
#   scripts/release-notes.sh <version> [<previous tag>]   > notes.md
set -euo pipefail
cd "$(dirname "$0")/.."
VER=${1:?usage: release-notes.sh <version> [previous tag]}
PREV=${2:-$(git describe --tags --abbrev=0 --match 'v*' HEAD^ 2>/dev/null || true)}
SITE=https://dixonsolutions.github.io/lindroids

cat <<NOTES
Lindroids runs Linux GUI programs on Android — from Termux, a proot-distro distro or Flatpak —
and shows their windows in the phone's web browser. It is one package, installed with the
package manager that is already there: no app to install, no permission to grant, no root.
\`lindroids export <program>\` puts a program on the home screen as a shortcut owned by Termux.

## Install or upgrade

In Termux (or any distro under proot, or desktop Linux):

\`\`\`sh
curl -fsSL $SITE/install.sh -o install.sh && sh install.sh
lindroids run mousepad               # its window opens in the browser
\`\`\`

Each program's page has an **Install** button (Chrome, Edge, Samsung Internet; Firefox: menu →
Install): installed, it opens as an app of its own, standalone, with no address bar or tabs.

| System | Repository | Upgrade |
|---|---|---|
| Termux (APT) | \`deb $SITE termux main\` | \`pkg upgrade\` |
| Termux (pacman bootstrap) | \`$SITE/pacman/termux/\$arch\` | \`pacman -Syu\` |
| Debian, Ubuntu, Kali | \`deb $SITE stable main\` | \`apt update && apt upgrade\` |
| Arch, Manjaro, Artix | \`$SITE/pacman/arch/\$arch\` | \`pacman -Syu\` |
| Fedora, RHEL, Rocky, Alma | \`$SITE/rpm/lindroids.repo\` | \`dnf upgrade\` |
| openSUSE | \`$SITE/rpm/lindroids.repo\` | \`zypper update\` |
| Alpine, postmarketOS, Chimera | \`$SITE/apk\` | \`apk upgrade\` |
| Void | \`$SITE/xbps\` | \`xbps-install -Su\` |
| anything else | static binaries below | run install.sh again |

The repositories are signed (\`$SITE/lindroids.asc\`; apk and xbps: \`$SITE/lindroids.rsa.pub\`).
Below: static CLIs for aarch64, armv7, x86_64 and i686 (\`*-linux-musl.tar.gz\`), the .deb and
.rpm packages, and \`SHA256SUMS\`.

NOTES
if [ -n "$PREV" ]; then
  echo "## Changes since $PREV"
  echo
  git log --no-merges --format='- %s (%h)' "$PREV..HEAD"
  echo
  echo "Full diff: https://github.com/dixonSolutions/lindroids/compare/$PREV...v$VER"
fi
