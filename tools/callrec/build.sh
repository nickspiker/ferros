#!/usr/bin/env bash
# Build the CallRec priv-app APK and assemble the Magisk module.
#
# Auto call recorder: captures both directions of a VoIP call on separate channels
# (VOICE_UPLINK = you = left, VOICE_DOWNLINK = far end = right) and writes stereo Opus
# to the shared Recordings folder. Route-independent (earpiece/speaker/wired/BT), because
# the tap sits at the AudioFlinger VoIP endpoints, upstream of physical output routing.
# See README.md.
#
# Prerequisites (paths overridable by env var):
#   BT            Android build-tools dir (aapt2/d8/zipalign/apksigner)   (default: $ANDROID_HOME/build-tools/34.0.0)
#   ANDROID_JAR   a REAL android.jar (API 34+) — must include MediaCodec/MediaMuxer/
#                 getInputBuffer/KEY_PCM_ENCODING. A stripped jar will NOT compile.
#                                                                          (default: $ANDROID_HOME/platforms/android-34/android.jar)
#   FRAMEWORK_RES framework-res.apk pulled from the device                 (default: ./framework-res.apk)
#   KEYSTORE      signing keystore (kept in keys/, NOT committed)          (default: ../../../keys/callrec.jks)
#   KS_PASS / KS_ALIAS
#
# framework-res.apk: adb pull /system/framework/framework-res.apk
set -euo pipefail
cd "$(dirname "$0")"

BT="${BT:-${ANDROID_HOME:-$HOME/Android/Sdk}/build-tools/34.0.0}"
ANDROID_JAR="${ANDROID_JAR:-${ANDROID_HOME:-$HOME/Android/Sdk}/platforms/android-34/android.jar}"
FRAMEWORK_RES="${FRAMEWORK_RES:-framework-res.apk}"
KEYSTORE="${KEYSTORE:-../../../keys/callrec.jks}"
KS_PASS="${KS_PASS:-android}"
KS_ALIAS="${KS_ALIAS:-rro}"

rm -rf build && mkdir -p build
# NOTE: flat top-level classes only — the bundled d8 NPEs on inner classes here.
javac -source 8 -target 8 -cp "$ANDROID_JAR" -d build $(find src -name '*.java')
"$BT/d8" --min-api 30 --lib "$ANDROID_JAR" --output build $(find build -name '*.class')
"$BT/aapt2" link -I "$FRAMEWORK_RES" --manifest AndroidManifest.xml \
    --min-sdk-version 30 --target-sdk-version 34 -o build/cr-base.apk
(cd build && zip -qj cr-base.apk classes.dex)
"$BT/zipalign" -f 4 build/cr-base.apk build/CallRec.apk
"$BT/apksigner" sign --ks "$KEYSTORE" --ks-pass "pass:$KS_PASS" --ks-key-alias "$KS_ALIAS" build/CallRec.apk

rm -rf build/module && cp -r module build/module
mkdir -p build/module/system/priv-app/CallRec
cp build/CallRec.apk build/module/system/priv-app/CallRec/CallRec.apk
(cd build/module && zip -qr ../callrec-module.zip .)
echo "built: build/CallRec.apk  +  build/callrec-module.zip"
