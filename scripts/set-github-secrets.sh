#!/usr/bin/env bash
# Set the GitHub Actions secrets the release workflow needs, from the local key material.
# Run once by a maintainer after the GitHub repository exists (needs `gh auth login`).
#
#   scripts/set-github-secrets.sh [owner/repo]        (default: dixonSolutions/lindroids)
#
# Secrets:
#   LINDROIDS_KEYSTORE_B64     base64 of ~/.local/share/lindroids/release.keystore (APK signing)
#   LINDROIDS_KEYSTORE_PASS    contents of ~/.local/share/lindroids/release.pass
#   LINDROIDS_KEY_ALIAS        key alias in the keystore (default: lindroids)
#   LINDROIDS_GPG_PRIVATE_KEY  armored secret key of the APT repository signing key, exported
#                              from the GNUPGHOME ~/.local/share/lindroids/gnupg
#
# Values are piped to `gh secret set` and never printed or written to disk.
set -euo pipefail
cd "$(dirname "$0")/.."

REPO=${1:-${LINDROIDS_GITHUB_REPO:-dixonSolutions/lindroids}}
DIR=${LINDROIDS_KEYS_DIR:-$HOME/.local/share/lindroids}
KEYSTORE=$DIR/release.keystore
PASSFILE=$DIR/release.pass
ALIAS=${LINDROIDS_KEY_ALIAS:-lindroids}
export GNUPGHOME=${LINDROIDS_GNUPGHOME:-$DIR/gnupg}

die() { echo "set-github-secrets: $*" >&2; exit 1; }
command -v gh >/dev/null || die "gh (GitHub CLI) is required"
gh auth status >/dev/null 2>&1 || die "run 'gh auth login' first"
[ -f "$KEYSTORE" ] || die "no keystore at $KEYSTORE"
[ -f "$PASSFILE" ] || die "no keystore password at $PASSFILE"

FPR=$(gpg --batch --show-keys --with-colons packaging/repo/lindroids.gpg | awk -F: '/^fpr/ {print $10; exit}')
[ -n "$FPR" ] || die "cannot read packaging/repo/lindroids.gpg"
gpg --batch --list-secret-keys "$FPR" >/dev/null 2>&1 \
  || die "the secret key $FPR is not in $GNUPGHOME; it must match packaging/repo/lindroids.gpg"

echo "Setting secrets on $REPO (APT key $FPR)"
base64 -w0 "$KEYSTORE" | gh secret set LINDROIDS_KEYSTORE_B64 --repo "$REPO"
tr -d '\r\n' < "$PASSFILE" | gh secret set LINDROIDS_KEYSTORE_PASS --repo "$REPO"
printf '%s' "$ALIAS" | gh secret set LINDROIDS_KEY_ALIAS --repo "$REPO"
gpg --batch --pinentry-mode loopback --passphrase '' --armor --export-secret-keys "$FPR" \
  | gh secret set LINDROIDS_GPG_PRIVATE_KEY --repo "$REPO"
gh secret list --repo "$REPO"
