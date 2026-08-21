#!/usr/bin/env bash
# Build the PocketGuard priv-app APK and assemble the Magisk module zip.
#
# This is the husky/Android STOPGAP for the ferros native wake policy: screen lights
# only when the power button is pressed AND the raw proximity sensor reads far.
# See README.md. The proper implementation lives in ferros itself, not here.
#
# Prerequisites (paths overridable by env var):
#   BT            Android build-tools dir with aapt2/d8/zipalign/apksigner   (default: $ANDROID_HOME/build-tools/34.0.0)
#   ANDROID_JAR   android.jar for the target platform                        (default: $ANDROID_HOME/platforms/android-34/android.jar)
#   FRAMEWORK_RES framework-res.apk pulled from the device                   (default: ./framework-res.apk)
#   KEYSTORE      signing keystore (kept in the keys/ vault, NOT committed)  (default: ../../../keys/pocketguard.jks)
#   KS_PASS       keystore password                                          (default: android)
#   KS_ALIAS      key alias                                                  (default: rro)
#
# framework-res.apk: adb pull /system/framework/framework-res.apk
set -euo pipefail
cd "$(dirname "$0")"

BT="${BT:-${ANDROID_HOME:-$HOME/Android/Sdk}/build-tools/34.0.0}"
ANDROID_JAR="${ANDROID_JAR:-${ANDROID_HOME:-$HOME/Android/Sdk}/platforms/android-34/android.jar}"
FRAMEWORK_RES="${FRAMEWORK_RES:-framework-res.apk}"
KEYSTORE="${KEYSTORE:-../../../keys/pocketguard.jks}"
KS_PASS="${KS_PASS:-android}"
KS_ALIAS="${KS_ALIAS:-rro}"

rm -rf build && mkdir -p build

echo "== javac =="
# NOTE: flat top-level classes only — the bundled d8 NPEs on inner classes here.
javac -source 8 -target 8 -cp "$ANDROID_JAR" -d build $(find src -name '*.java')

echo "== d8 =="
"$BT/d8" --min-api 30 --lib "$ANDROID_JAR" --output build $(find build -name '*.class')

echo "== aapt2 link =="
"$BT/aapt2" link -I "$FRAMEWORK_RES" --manifest AndroidManifest.xml \
    --min-sdk-version 30 --target-sdk-version 34 -o build/pg-base.apk

echo "== package + align + sign =="
(cd build && zip -qj pg-base.apk classes.dex)
"$BT/zipalign" -f 4 build/pg-base.apk build/PocketGuard.apk
"$BT/apksigner" sign --ks "$KEYSTORE" --ks-pass "pass:$KS_PASS" --ks-key-alias "$KS_ALIAS" build/PocketGuard.apk

echo "== assemble module =="
rm -rf build/module && cp -r module build/module
mkdir -p build/module/system/priv-app/PocketGuard
cp build/PocketGuard.apk build/module/system/priv-app/PocketGuard/PocketGuard.apk
(cd build/module && zip -qr ../pocketguard-module.zip .)

echo "built: build/PocketGuard.apk  +  build/pocketguard-module.zip"
