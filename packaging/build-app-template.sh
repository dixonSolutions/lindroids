#!/usr/bin/env bash
# Build the template of a program's own app: the window app's code and resources with placeholder
# package, name, start page and icons (packaging/webview-app/AndroidManifest.program.xml). It is
# unsigned and committed as crates/lindroids-apk/template/window-app.apk, so the Lindroids binary
# carries it and `lindroids export` builds each program's app from it on the phone, in Rust, with no
# Android SDK, aapt or Java there (crates/lindroids-apk). Run this after changing
# packaging/webview-app, as scripts/build-shortcut-dex.sh is run after changing Shortcut.java.
#
#   packaging/build-app-template.sh [--out crates/lindroids-apk/template/window-app.apk]
#
# Needs a JDK (javac) and the Android SDK's platform android.jar and build-tools' aapt2, d8 and
# zipalign. ANDROID_HOME defaults to ~/Android/Sdk.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
OUT=$ROOT/crates/lindroids-apk/template/window-app.apk
while [ $# -gt 0 ]; do
  case "$1" in
    --out) OUT=$2; shift 2 ;;
    -h|--help) awk 'NR > 1 && !/^#/ {exit} NR > 1 {sub(/^# ?/, ""); print}' "$0"; exit 0 ;;
    *) echo "build-app-template: unknown argument $1" >&2; exit 2 ;;
  esac
done

: "${ANDROID_HOME:=$HOME/Android/Sdk}"
JAR=$(ls -d "$ANDROID_HOME"/platforms/android-*/android.jar 2>/dev/null | sort -V | tail -1)
TOOLS=$(ls -d "$ANDROID_HOME"/build-tools/* 2>/dev/null | sort -V | tail -1)
[ -f "${JAR:-/nonexistent}" ] || { echo "build-app-template: no android.jar in $ANDROID_HOME/platforms" >&2; exit 1; }
[ -x "${TOOLS:-/nonexistent}/aapt2" ] || { echo "build-app-template: no aapt2 in $ANDROID_HOME/build-tools" >&2; exit 1; }
command -v javac >/dev/null || { echo "build-app-template: javac is not installed (a JDK is needed)" >&2; exit 1; }

SRC=$ROOT/packaging/webview-app
WORK=$ROOT/target/app-template
rm -rf "$WORK"
mkdir -p "$WORK/res" "$WORK/classes" "$WORK/dex" "$WORK/gen"
cp -a "$SRC/res/." "$WORK/res/"
cp -a "$SRC/res-program/." "$WORK/res/"
mkdir -p "$WORK/res/mipmap-xxxhdpi"
# Placeholders, replaced per program: the plain icon, the adaptive icon's two layers.
cp "$ROOT/media/icon-512.png" "$WORK/res/mipmap-xxxhdpi/ic_launcher.png"
cp "$ROOT/media/icon-512.png" "$WORK/res/mipmap-xxxhdpi/ic_launcher_foreground.png"
python3 - "$WORK/res/mipmap-xxxhdpi/ic_launcher_background.png" <<'EOF'
import struct, sys, zlib
def chunk(t, d): return struct.pack('>I', len(d)) + t + d + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
raw = b''.join(b'\0' + b'\x0f\x17\x20' * 16 for _ in range(16))
png = b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', 16, 16, 8, 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b'')
open(sys.argv[1], 'wb').write(png)
EOF

# --no-crunch: the PNGs are swapped byte for byte; keep the names aapt2 gives them predictable.
"$TOOLS/aapt2" compile --no-crunch --dir "$WORK/res" -o "$WORK/res.zip"
"$TOOLS/aapt2" link -o "$WORK/unsigned.apk" --manifest "$SRC/AndroidManifest.program.xml" \
  -I "$JAR" --min-sdk-version 24 --target-sdk-version 35 \
  -A "$SRC/assets" --java "$WORK/gen" --custom-package org.lindroids "$WORK/res.zip"
javac --release 8 -Xlint:-options -cp "$JAR" -d "$WORK/classes" \
  $(find "$WORK/gen" "$SRC/src" -name '*.java')
"$TOOLS/d8" --release --min-api 24 --lib "$JAR" --output "$WORK/dex" $(find "$WORK/classes" -name '*.class')
cp "$WORK/dex/classes.dex" "$WORK/classes.dex"
(cd "$WORK" && zip -q -j unsigned.apk classes.dex)
mkdir -p "$(dirname "$OUT")"
"$TOOLS/zipalign" -f -p 4 "$WORK/unsigned.apk" "$OUT"
echo "    $OUT ($(du -h "$OUT" | cut -f1)), unsigned"
unzip -l "$OUT" | awk 'NR > 3 && $4 != "" {print "      " $4}' | grep -v '^      -' || true
