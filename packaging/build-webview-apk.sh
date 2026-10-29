#!/usr/bin/env bash
# Build the Lindroids window app: a small APK that shows a program's page in Android's own WebView,
# with no browser and no permission but INTERNET. See BROWSER_AGNOSTIC_APK.md.
#
#   packaging/build-webview-apk.sh [--version X.Y.Z] [--out dist] [--debug-sign]
#
# Needs a JDK (javac, keytool) and the Android SDK's platform android.jar, build-tools' aapt2,
# d8, zipalign and apksigner. ANDROID_HOME defaults to ~/Android/Sdk.
#
# The release build signs with the project's key, from LINDROIDS_APP_KEYSTORE (the PKCS#12
# keystore, base64) or LINDROIDS_APP_KEYSTORE_FILE (a path), with LINDROIDS_APP_KEY_ALIAS (default
# lindroids) and LINDROIDS_APP_KEYSTORE_PASS. Without a key, a throwaway test key is made under
# target/ and the APK is marked as a test build: never publish it as Lindroids.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD

VERSION=
OUT=$ROOT/dist
DEBUG_SIGN=0
while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION=$2; shift 2 ;;
    --out) OUT=$2; shift 2 ;;
    --debug-sign) DEBUG_SIGN=1; shift ;;
    -h|--help) awk 'NR > 1 && !/^#/ {exit} NR > 1 {sub(/^# ?/, ""); print}' "$0"; exit 0 ;;
    *) echo "build-webview-apk: unknown argument $1" >&2; exit 2 ;;
  esac
done
if [ -z "$VERSION" ]; then
  eval "$(scripts/version.sh)"
  VERSION=$LINDROIDS_VERSION
fi
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)

: "${ANDROID_HOME:=$HOME/Android/Sdk}"
JAR=$(ls -d "$ANDROID_HOME"/platforms/android-*/android.jar 2>/dev/null | sort -V | tail -1)
TOOLS=$(ls -d "$ANDROID_HOME"/build-tools/* 2>/dev/null | sort -V | tail -1)
[ -f "${JAR:-/nonexistent}" ] || { echo "build-webview-apk: no android.jar in $ANDROID_HOME/platforms" >&2; exit 1; }
[ -x "${TOOLS:-/nonexistent}/aapt2" ] || { echo "build-webview-apk: no aapt2 in $ANDROID_HOME/build-tools" >&2; exit 1; }
for t in javac keytool zip; do
  command -v "$t" >/dev/null || { echo "build-webview-apk: $t is not installed (a JDK is needed)" >&2; exit 1; }
done

SRC=$ROOT/packaging/webview-app
WORK=$ROOT/target/webview-app
rm -rf "$WORK"
mkdir -p "$WORK/res" "$WORK/classes" "$WORK/dex"
cp -a "$SRC/res/." "$WORK/res/"
mkdir -p "$WORK/res/mipmap-xxxhdpi"
cp "$ROOT/media/icon.png" "$WORK/res/mipmap-xxxhdpi/ic_launcher.png"

# 0.1.42 -> 10042; the version name is what the release and `lindroids app` speak.
CODE=$(printf '%s' "$VERSION" | awk -F. '{printf "%d", ($1 * 1000000) + ($2 * 10000) + $3}')
echo "==> Lindroids window app $VERSION ($CODE)"
mkdir -p "$WORK/gen"
"$TOOLS/aapt2" compile --dir "$WORK/res" -o "$WORK/res.zip"
"$TOOLS/aapt2" link -o "$WORK/unsigned.apk" --manifest "$SRC/AndroidManifest.xml" \
  -I "$JAR" --min-sdk-version 24 --target-sdk-version 35 \
  --version-code "$CODE" --version-name "$VERSION" --replace-version \
  -A "$SRC/assets" --java "$WORK/gen" "$WORK/res.zip"
javac --release 8 -Xlint:-options -cp "$JAR" -d "$WORK/classes" \
  $(find "$WORK/gen" "$SRC/src" -name '*.java')
"$TOOLS/d8" --release --min-api 24 --lib "$JAR" --output "$WORK/dex" $(find "$WORK/classes" -name '*.class')
cp "$WORK/dex/classes.dex" "$WORK/classes.dex"
(cd "$WORK" && zip -q -j unsigned.apk classes.dex)
"$TOOLS/zipalign" -f -p 4 "$WORK/unsigned.apk" "$WORK/aligned.apk"

KEYSTORE=
ALIAS=${LINDROIDS_APP_KEY_ALIAS:-lindroids}
PASS=${LINDROIDS_APP_KEYSTORE_PASS:-lindroids}
if [ -n "${LINDROIDS_APP_KEYSTORE_FILE:-}" ]; then
  KEYSTORE=$LINDROIDS_APP_KEYSTORE_FILE
elif [ -n "${LINDROIDS_APP_KEYSTORE:-}" ]; then
  KEYSTORE=$WORK/release.p12
  printf '%s' "$LINDROIDS_APP_KEYSTORE" | base64 -d >"$KEYSTORE"
elif [ $DEBUG_SIGN = 1 ] || [ -z "${LINDROIDS_APP_KEYSTORE_FILE:-}" ]; then
  KEYSTORE=$ROOT/target/webview-app-test.p12
  if [ ! -f "$KEYSTORE" ]; then
    echo "build-webview-apk: no release key: making a throwaway test key ($KEYSTORE)" >&2
    keytool -genkeypair -keystore "$KEYSTORE" -storetype PKCS12 -alias "$ALIAS" \
      -storepass "$PASS" -keypass "$PASS" -keyalg RSA -keysize 2048 -validity 10000 \
      -dname "CN=Lindroids test, O=Lindroids, C=US" >/dev/null 2>&1
  fi
fi
[ -f "$KEYSTORE" ] || { echo "build-webview-apk: no keystore at $KEYSTORE" >&2; exit 1; }

APK=$OUT/lindroids-$VERSION.apk
"$TOOLS/apksigner" sign --ks "$KEYSTORE" --ks-key-alias "$ALIAS" \
  --ks-pass "pass:$PASS" --key-pass "pass:$PASS" --out "$APK" "$WORK/aligned.apk"
"$TOOLS/apksigner" verify --print-certs "$APK" >/dev/null

echo "    $APK ($(du -h "$APK" | cut -f1))"
if [ "$KEYSTORE" = "$ROOT/target/webview-app-test.p12" ]; then
  echo "    signed with a test key: do not publish this APK"
fi
