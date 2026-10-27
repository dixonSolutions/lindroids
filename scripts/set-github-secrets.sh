#!/usr/bin/env bash
# Set the GitHub Actions secret the release workflow needs, from the local key material.
# Run once by a maintainer after the GitHub repository exists (needs `gh auth login`).
#
#   scripts/set-github-secrets.sh [--release] [owner/repo]   (default: dixonSolutions/lindroids)
#
# --release: then run the release workflow, wait for it, and turn GitHub Pages on for gh-pages
# (the APT repository and install.sh), so one command takes a fresh repository to a published
# release.
#
# Secret:
#   LINDROIDS_GPG_PRIVATE_KEY  armored secret key of the APT repository signing key, exported
#                              from the GNUPGHOME ~/.local/share/lindroids/gnupg
#
# The value is piped to `gh secret set` and never printed or written to disk.
set -euo pipefail
cd "$(dirname "$0")/.."

RELEASE=0
if [ "${1:-}" = "--release" ]; then RELEASE=1; shift; fi
REPO=${1:-${LINDROIDS_GITHUB_REPO:-dixonSolutions/lindroids}}
DIR=${LINDROIDS_KEYS_DIR:-$HOME/.local/share/lindroids}
export GNUPGHOME=${LINDROIDS_GNUPGHOME:-$DIR/gnupg}

die() { echo "set-github-secrets: $*" >&2; exit 1; }
command -v gh >/dev/null || die "gh (GitHub CLI) is required"
gh auth status >/dev/null 2>&1 || die "run 'gh auth login' first"

FPR=$(gpg --batch --show-keys --with-colons packaging/repo/lindroids.gpg | awk -F: '/^fpr/ {print $10; exit}')
[ -n "$FPR" ] || die "cannot read packaging/repo/lindroids.gpg"
gpg --batch --list-secret-keys "$FPR" >/dev/null 2>&1 \
  || die "the secret key $FPR is not in $GNUPGHOME; it must match packaging/repo/lindroids.gpg"

echo "Setting the secret on $REPO (APT key $FPR)"
gpg --batch --pinentry-mode loopback --passphrase '' --armor --export-secret-keys "$FPR" \
  | gh secret set LINDROIDS_GPG_PRIVATE_KEY --repo "$REPO"
gh secret list --repo "$REPO"

[ $RELEASE = 1 ] || exit 0
echo "Running the release workflow on $REPO"
before=$(gh run list --repo "$REPO" --workflow release.yml --limit 1 --json databaseId -q '.[0].databaseId' 2>/dev/null || true)
gh workflow run release.yml --repo "$REPO"
run=$before
for _ in $(seq 30); do
  sleep 4
  run=$(gh run list --repo "$REPO" --workflow release.yml --limit 1 --json databaseId -q '.[0].databaseId')
  [ "$run" != "$before" ] && break
done
[ "$run" != "$before" ] || die "the workflow run did not start"
gh run watch "$run" --repo "$REPO" --exit-status || die "the release failed: gh run view $run --repo $REPO --log-failed"
if ! gh api "repos/$REPO/pages" >/dev/null 2>&1; then
  echo "Turning on GitHub Pages (gh-pages, /)"
  gh api -X POST "repos/$REPO/pages" -f "source[branch]=gh-pages" -f "source[path]=/" >/dev/null
fi
owner=${REPO%%/*}
echo "Published: https://github.com/$REPO/releases/latest and https://${owner,,}.github.io/${REPO#*/}/install.sh"
