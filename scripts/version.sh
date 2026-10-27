#!/bin/sh
# Print the release version variables for the current checkout, as shell assignments:
#
#   eval "$(scripts/version.sh)"    # sets LINDROIDS_VERSION, LINDROIDS_BUILD_ID
#
# Scheme: <major>.<minor>.<N>, where major.minor come from the workspace Cargo.toml and N is the
# number of commits reachable from HEAD (`git rev-list --count HEAD`), so every commit on main is
# a strictly newer release for apt and pacman.
# Tag: v<version>. Anything already set in the environment wins (CI can pin a version).
set -eu
cd "$(dirname "$0")/.."

cargo_version=$(sed -n 's/^version = "\([0-9][0-9]*\)\.\([0-9][0-9]*\)\..*"/\1.\2/p' Cargo.toml | head -1)
[ -n "$cargo_version" ] || { echo "version.sh: no version in Cargo.toml" >&2; exit 1; }

if [ -e .git ] && ! command -v git >/dev/null 2>&1; then
  echo "version.sh: git is not installed, so the commit count is unknown" >&2
  exit 1
fi
if git rev-parse --git-dir >/dev/null 2>&1; then
  if [ "$(git rev-parse --is-shallow-repository 2>/dev/null)" = true ]; then
    echo "version.sh: shallow clone, the commit count would be wrong (fetch-depth: 0)" >&2
    exit 1
  fi
  count=$(git rev-list --count HEAD)
  sha=$(git rev-parse --short HEAD)
else
  count=0
  sha=dev
fi

echo "LINDROIDS_VERSION=${LINDROIDS_VERSION:-$cargo_version.$count}"
echo "LINDROIDS_BUILD_ID=${LINDROIDS_BUILD_ID:-$sha}"
