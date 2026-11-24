#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

: "${JAVA_HOME:?Set JAVA_HOME to JDK 17}"
: "${ANDROID_HOME:?Set ANDROID_HOME to the Android SDK directory}"
tools="$ANDROID_HOME/build-tools/35.0.1"
gradle="${GRADLE_BIN:-$PWD/android/gradlew}"
"$gradle" -p android :app:assembleRelease --console=plain

# Preserve the signing identity across updates. Never include these files in an APK.
signing="${WISE_SIGNING_DIR:-$PWD/.android-signing}"
mkdir -p -m 700 "$signing"
store="$signing/wise-softphone.p12"
passfile="$signing/store.pass"
if [[ ! -f "$store" ]]; then
    (
        umask 077
        if [[ ! -f "$passfile" ]]; then openssl rand -hex 32 > "$passfile"; fi
        "$JAVA_HOME/bin/keytool" -genkeypair -keystore "$store" -storetype PKCS12 \
            -storepass:file "$passfile" -alias wise-softphone -keyalg RSA -keysize 3072 \
            -validity 10000 -dname "CN=Wise Softphone" -noprompt
    )
fi

unsigned=android/app/build/outputs/apk/release/app-release-unsigned.apk
aligned=android/app/build/wise-softphone-aligned.apk
out=dist/wise-softphone-0.2.0-arm64-v8a.apk
mkdir -p dist
"$tools/zipalign" -P 16 -f 4 "$unsigned" "$aligned"
"$tools/apksigner" sign --ks "$store" --ks-key-alias wise-softphone \
    --ks-pass "file:$passfile" --v4-signing-enabled false --out "$out" "$aligned"
"$tools/apksigner" verify --verbose "$out"
"$tools/zipalign" -c -P 16 4 "$out"
echo "$PWD/$out"
