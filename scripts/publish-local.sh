#!/usr/bin/env bash
# Publish a release from a maintainer's machine, where the signing key is: the same release the
# release workflow makes when its secrets are set (scripts/set-github-secrets.sh), for when they are
# not. The key never leaves this machine; only signed artifacts are uploaded.
#
#   scripts/publish-local.sh [--dry-run]
#
# 1. Checks that HEAD is clean and pushed (the release tag points at it).
# 2. scripts/build-release.sh.
# 3. Fetches the published site (gh-pages), adds the new packages with packaging/repo/build-repo.sh,
#    signed with the APT key in ~/.local/share/lindroids/gnupg and (apk, xbps) the RSA key
#    ~/.local/share/lindroids/lindroids.rsa; without the RSA key those two are left as they are.
# 4. Creates the GitHub release v<version> with the tarballs, debs and SHA256SUMS.
# 5. Pushes the site to gh-pages as a single commit, and turns GitHub Pages on for it if needed.
#    The release comes first, so the site's VERSION never names a release that does not exist.
#
# Needs: gh (logged in), git, and everything build-release.sh and build-repo.sh need.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
DRY=0
[ "${1:-}" = "--dry-run" ] && DRY=1

KEYS=${LINDROIDS_KEYS_DIR:-$HOME/.local/share/lindroids}
REPO=${LINDROIDS_GITHUB_REPO:-dixonSolutions/lindroids}
OWNER=${REPO%%/*}
NAME=${REPO#*/}
BASE_URL="https://${OWNER,,}.github.io/$NAME"

die() { echo "publish-local: $*" >&2; exit 1; }
[ -d "$KEYS/gnupg" ] || die "no APT signing key in $KEYS/gnupg"
[ -f "$KEYS/lindroids.rsa" ] || echo "publish-local: warning: no $KEYS/lindroids.rsa: the apk and xbps repositories are not updated" >&2
# Inside a distrobox (where the toolchains are), the host's gh will do (LINDROIDS_HOST_GH, else
# the usual places; Homebrew's is not on the PATH distrobox-host-exec uses).
if ! command -v gh >/dev/null && command -v distrobox-host-exec >/dev/null; then
  for HOST_GH in ${LINDROIDS_HOST_GH:-} /home/linuxbrew/.linuxbrew/bin/gh /usr/bin/gh /usr/local/bin/gh; do
    if distrobox-host-exec test -x "$HOST_GH" 2>/dev/null; then
      gh() { distrobox-host-exec "$HOST_GH" "$@"; }
      break
    fi
  done
fi
command -v gh >/dev/null || declare -F gh >/dev/null || die "gh is required"
[ -z "$(git status --porcelain --untracked-files=no)" ] || die "the working tree has uncommitted changes"
git fetch -q origin main
[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || die "HEAD is not origin/main: push first"

eval "$(scripts/version.sh)"
VER=$LINDROIDS_VERSION
TAG="v$VER"
SHA=$(git rev-parse HEAD)
if gh release view "$TAG" -R "$REPO" >/dev/null 2>&1; then
  die "release $TAG already exists"
fi
echo "==> publishing Lindroids $VER ($SHA) to $REPO and $BASE_URL"

LINDROIDS_GITHUB_REPO="$REPO" scripts/build-release.sh

SITE="$ROOT/target/publish-site"
rm -rf "$SITE"
mkdir -p "$SITE"
if git fetch -q --depth 1 origin gh-pages 2>/dev/null; then
  git archive FETCH_HEAD | tar -x -C "$SITE"
else
  echo "no gh-pages branch yet: starting an empty repository"
fi
# Flatpak for Termux: the packages of the newest termux-flatpak-* release, if any
# (.github/workflows/termux-flatpak.yml).
rm -rf dist/termux-flatpak
mkdir -p dist/termux-flatpak
FLATPAK_TAG=$(gh release list -R "$REPO" --limit 100 --json tagName --jq '.[].tagName | select(startswith("termux-flatpak-"))' | head -1)
[ -z "$FLATPAK_TAG" ] || gh release download "$FLATPAK_TAG" -R "$REPO" -p 'lindroids-flatpak_*.deb' -D dist/termux-flatpak
LINDROIDS_GNUPGHOME="$KEYS/gnupg" LINDROIDS_APK_RSA_KEY_FILE="$KEYS/lindroids.rsa" \
  packaging/repo/build-repo.sh "$SITE" --debs dist/debs --debs dist/termux-flatpak --pacman dist/pacman \
  --rpm dist/rpm --apk dist/apk --xbps dist/xbps --keep 3 \
  --base-url "$BASE_URL" --github-repo "$REPO"

if [ $DRY = 1 ]; then
  echo "==> dry run: release files in dist/, site in $SITE; nothing uploaded"
  exit 0
fi

echo "==> GitHub release $TAG"
gh release create "$TAG" -R "$REPO" --target "$SHA" --title "Lindroids $VER" --generate-notes --latest \
  dist/lindroids-*.tar.gz dist/debs/*.deb dist/SHA256SUMS

echo "==> gh-pages"
PUB="$ROOT/target/publish-git"
rm -rf "$PUB"
mkdir -p "$PUB"
cp -a "$SITE/." "$PUB/"
git -C "$PUB" init -q -b gh-pages
git -C "$PUB" add -A
git -C "$PUB" -c user.name="$(git config user.name)" -c user.email="$(git config user.email)" \
  commit -q -m "Publish Lindroids $VER [skip ci]"
git -C "$PUB" push -q --force "$(git remote get-url origin)" gh-pages:gh-pages
rm -rf "$PUB"

if ! gh api "repos/$REPO/pages" >/dev/null 2>&1; then
  echo "==> turning on GitHub Pages (gh-pages, /)"
  gh api -X POST "repos/$REPO/pages" -f "source[branch]=gh-pages" -f "source[path]=/" >/dev/null
fi
echo "==> done: $BASE_URL (Pages can take a minute to update)"
