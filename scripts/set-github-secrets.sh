#!/usr/bin/env bash
# Set the GitHub Actions secrets the release workflow needs, from the local key material.
# Run once by a maintainer after the GitHub repository exists (needs `gh auth login`).
#
#   scripts/set-github-secrets.sh [--release] [owner/repo]   (default: dixonSolutions/lindroids)
#
# --release: then run the release workflow, wait for it, and turn GitHub Pages on for gh-pages
# (the APT repository and install.sh), so one command takes a fresh repository to a published
# release.
#
# Secrets:
#   LINDROIDS_GPG_PRIVATE_KEY  armored secret key of the APT repository signing key, exported
#                              from the GNUPGHOME ~/.local/share/lindroids/gnupg (also signs the
#                              pacman and RPM repositories)
#   LINDROIDS_APP_KEYSTORE     base64 PKCS#12 keystore that signs the window app (org.lindroids),
#                              from $DIR/lindroids-app.p12 (or LINDROIDS_APP_KEYSTORE_FILE); its
#                              password from $DIR/lindroids-app.pass (or
#                              LINDROIDS_APP_KEYSTORE_PASS_FILE), alias lindroids by default.
#                              Losing this key means installed window apps cannot be updated.
#   LINDROIDS_APK_RSA_KEY      PEM private RSA key that signs the apk (Alpine) and xbps (Void)
#                              repositories: ~/.local/share/lindroids/lindroids.rsa. If neither it
#                              nor packaging/repo/lindroids.rsa.pub exists yet, a 4096-bit key is
#                              generated there and its public half written to
#                              packaging/repo/lindroids.rsa.pub, which must then be committed (the
#                              release checks the secret against it, and publishes it).
#
# The values are piped to `gh secret set` and never printed.
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

# The RSA key (apk, xbps): made once, here; its public half is committed.
command -v openssl >/dev/null || die "openssl is required"
RSA=$DIR/lindroids.rsa
RSA_PUB=packaging/repo/lindroids.rsa.pub
if [ ! -f "$RSA" ]; then
  [ ! -f "$RSA_PUB" ] || die "$RSA_PUB exists, but its private key is not in $RSA"
  echo "Generating the apk/xbps signing key $RSA"
  mkdir -p "$DIR"
  (umask 077 && openssl genrsa -out "$RSA" 4096 2>/dev/null)
  echo "Back it up offline, with $GNUPGHOME: losing it means every Alpine and Void user re-trusts a new key."
fi
if [ ! -f "$RSA_PUB" ]; then
  openssl pkey -in "$RSA" -pubout -out "$RSA_PUB"
  echo "Wrote $RSA_PUB: commit it (releases check the key against it and publish it)."
fi
[ "$(openssl pkey -in "$RSA" -pubout)" = "$(openssl pkey -pubin -in "$RSA_PUB")" ] \
  || die "$RSA does not match $RSA_PUB"

echo "Setting the secrets on $REPO (APT key $FPR, RSA key $RSA)"
gpg --batch --pinentry-mode loopback --passphrase '' --armor --export-secret-keys "$FPR" \
  | gh secret set LINDROIDS_GPG_PRIVATE_KEY --repo "$REPO"
gh secret set LINDROIDS_APK_RSA_KEY --repo "$REPO" < "$RSA"

# The window app (org.lindroids): one key for every release, so Android accepts each update.
APP_FILE=${LINDROIDS_APP_KEYSTORE_FILE:-$DIR/lindroids-app.p12}
APP_PASS_FILE=${LINDROIDS_APP_KEYSTORE_PASS_FILE:-$DIR/lindroids-app.pass}
APP_ALIAS=${LINDROIDS_APP_KEY_ALIAS:-lindroids}
if [ -f "$APP_FILE" ] && [ -f "$APP_PASS_FILE" ]; then
  base64 -w0 "$APP_FILE" | gh secret set LINDROIDS_APP_KEYSTORE --repo "$REPO"
  gh secret set LINDROIDS_APP_KEYSTORE_PASS --repo "$REPO" --body "$(cat "$APP_PASS_FILE")"
  gh secret set LINDROIDS_APP_KEY_ALIAS --repo "$REPO" --body "$APP_ALIAS"
  echo "Window app signing key: $APP_FILE (alias $APP_ALIAS)"
elif [ -n "${LINDROIDS_APP_KEYSTORE:-}" ]; then
  printf '%s' "$LINDROIDS_APP_KEYSTORE" | gh secret set LINDROIDS_APP_KEYSTORE --repo "$REPO"
  gh secret set LINDROIDS_APP_KEYSTORE_PASS --repo "$REPO" --body "${LINDROIDS_APP_KEYSTORE_PASS:?set LINDROIDS_APP_KEYSTORE_PASS}"
  gh secret set LINDROIDS_APP_KEY_ALIAS --repo "$REPO" --body "$APP_ALIAS"
  echo "Window app signing key: from LINDROIDS_APP_KEYSTORE"
else
  echo "set-github-secrets: warning: no window app keystore in $APP_FILE (or LINDROIDS_APP_KEYSTORE): releases will fail" >&2
fi
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
