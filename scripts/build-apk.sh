#!/usr/bin/env bash
# Build the Lindroids APK without Gradle: cargo-ndk for the Rust library, aapt2 for resources,
# javac + d8 for the thin Java layer, zipalign + apksigner to finish.
#
#   scripts/build-apk.sh [--abis "arm64-v8a armeabi-v7a x86_64 x86"] [--out dist/lindroids.apk]
#
# Environment:
#   ANDROID_HOME              Android SDK (build-tools 35, platforms;android-35)
#   ANDROID_NDK_HOME          Android NDK r27
#   LINDROIDS_VERSION         versionName (default: workspace version)
#   LINDROIDS_VERSION_CODE    versionCode (default: commit count)
#   LINDROIDS_KEYSTORE / LINDROIDS_KEYSTORE_PASS / LINDROIDS_KEY_ALIAS
#                             release signing key; without them a throwaway debug key is used,
#                             and such an APK cannot update one signed with the release key.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD

ABIS="arm64-v8a armeabi-v7a x86_64 x86"
OUT=dist/lindroids.apk
while [ $# -gt 0 ]; do
  case "$1" in
    --abis) ABIS="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

: "${ANDROID_HOME:=$HOME/Android/Sdk}"
: "${ANDROID_NDK_HOME:=$(ls -d "$ANDROID_HOME"/ndk/* 2>/dev/null | sort -V | tail -1)}"
export ANDROID_HOME ANDROID_NDK_HOME
BT=$(ls -d "$ANDROID_HOME"/build-tools/* | sort -V | tail -1)
PLATFORM=$(ls -d "$ANDROID_HOME"/platforms/android-* | sort -V | tail -1)/android.jar
VERSION=${LINDROIDS_VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)}
VERSION_CODE=${LINDROIDS_VERSION_CODE:-$(git rev-list --count HEAD 2>/dev/null || echo 1)}
export LINDROIDS_BUILD_ID=${LINDROIDS_BUILD_ID:-$(git rev-parse --short HEAD 2>/dev/null || echo dev)}

WORK=target/apk
rm -rf "$WORK" && mkdir -p "$WORK"/{res,gen,classes,dex,stage/lib}

echo "==> Rust library ($ABIS)"
NDK_ARGS=()
for abi in $ABIS; do NDK_ARGS+=(-t "$abi"); done
cargo ndk "${NDK_ARGS[@]}" -P 24 -o "$WORK/stage/lib" build -p lindroids-android --release

echo "==> proot"
for abi in $ABIS; do
  if [ -d "third_party/proot/$abi" ]; then
    cp third_party/proot/"$abi"/*.so "$WORK/stage/lib/$abi/"
  else
    echo "    (no proot for $abi: run scripts/fetch-proot.sh; exported bundles will not start on $abi)"
  fi
done

echo "==> resources"
"$BT/aapt2" compile --dir android/res -o "$WORK/res/res.zip"
"$BT/aapt2" link -o "$WORK/base.apk" -I "$PLATFORM" \
  --manifest android/AndroidManifest.xml \
  --min-sdk-version 24 --target-sdk-version 28 \
  --version-code "$VERSION_CODE" --version-name "$VERSION" \
  --java "$WORK/gen" "$WORK/res/res.zip"

echo "==> java"
find android/java "$WORK/gen" -name '*.java' > "$WORK/sources.txt"
javac -nowarn -Xlint:-options --release 11 -encoding UTF-8 -classpath "$PLATFORM" -d "$WORK/classes" @"$WORK/sources.txt"
"$BT/d8" --release --min-api 24 --lib "$PLATFORM" --output "$WORK/dex" $(find "$WORK/classes" -name '*.class')

echo "==> package"
cp "$WORK/base.apk" "$WORK/unaligned.apk"
( cd "$WORK/dex" && zip -q -X "$ROOT/$WORK/unaligned.apk" classes.dex )
# Native libraries stay uncompressed: the server loads liblindroids.so straight out of the APK.
( cd "$WORK/stage" && zip -q -X -0 -r "$ROOT/$WORK/unaligned.apk" lib )
"$BT/zipalign" -f -P 16 4 "$WORK/unaligned.apk" "$WORK/aligned.apk"

mkdir -p "$(dirname "$OUT")"
if [ -n "${LINDROIDS_KEYSTORE:-}" ]; then
  "$BT/apksigner" sign --ks "$LINDROIDS_KEYSTORE" --ks-pass env:LINDROIDS_KEYSTORE_PASS \
    --ks-key-alias "${LINDROIDS_KEY_ALIAS:-lindroids}" --out "$OUT" "$WORK/aligned.apk"
else
  DEBUG_KS=$HOME/.android/lindroids-debug.keystore
  if [ ! -f "$DEBUG_KS" ]; then
    mkdir -p "$(dirname "$DEBUG_KS")"
    keytool -genkeypair -keystore "$DEBUG_KS" -storepass android -keypass android -alias debug \
      -keyalg RSA -keysize 2048 -validity 10000 -dname "CN=Lindroids Debug" >/dev/null 2>&1
  fi
  echo "    (signed with the debug key)"
  "$BT/apksigner" sign --ks "$DEBUG_KS" --ks-pass pass:android --ks-key-alias debug --out "$OUT" "$WORK/aligned.apk"
fi
rm -f "$OUT.idsig"
echo "==> $OUT ($VERSION, code $VERSION_CODE, $(du -h "$OUT" | cut -f1))"
