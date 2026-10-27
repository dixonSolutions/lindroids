#!/usr/bin/env bash
# Rebuild crates/lindroids-cli/shortcut/shortcut.dex from Shortcut.java. The .dex is checked in and
# embedded in the CLI, so releases need neither Java nor the Android SDK; run this only after
# changing Shortcut.java.
#
# Needs a JDK (javac) and, from the Android SDK, a platform's android.jar (compile-time stubs) and
# build-tools' d8: ANDROID_HOME (default ~/Android/Sdk).
set -euo pipefail
cd "$(dirname "$0")/.."
: "${ANDROID_HOME:=$HOME/Android/Sdk}"
JAR=$(ls -d "$ANDROID_HOME"/platforms/android-*/android.jar | sort -V | tail -1)
D8=$(ls -d "$ANDROID_HOME"/build-tools/*/d8 | sort -V | tail -1)
SRC=crates/lindroids-cli/shortcut
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
javac --release 11 -Xlint:-options -cp "$JAR" -d "$TMP/classes" "$SRC/Shortcut.java"
"$D8" --release --min-api 26 --lib "$JAR" --output "$TMP" $(find "$TMP/classes" -name '*.class')
cp "$TMP/classes.dex" "$SRC/shortcut.dex"
echo "$SRC/shortcut.dex: $(stat -c %s "$SRC/shortcut.dex") bytes"
