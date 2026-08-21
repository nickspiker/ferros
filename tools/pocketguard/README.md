# PocketGuard — raw-proximity screen-off (husky stopgap)

Wake policy the owner wants: **the screen turns on only when the power button is pressed AND nothing is near the proximity sensor.**
Calls, notifications, plug/unplug, bumps, ambient-light changes must never light the panel. Any orientation counts (flat on a table is "covered"); no on-ear heuristic.

This module is the **husky/Android stopgap** for that policy. The proper implementation belongs in **ferros**, where the power-button handler reads the raw proximity sensor and only drives the display on if far — no wake to race, no flash, because nothing else can turn the panel on. This directory exists so the intent and the working stopgap aren't lost before then.

## How it works

A persistent priv-app (`com.pocketguard`) binds the **raw** proximity sensor and forces the screen back off whenever something is near:

- Binds `TYPE_PROXIMITY` — on husky this is `TMD3719 Proximity (wake-up)`, `type=android.sensor.proximity(8)`, `maxRange=5.0`, near reads `0.0`.
  This is the RAW sensor, deliberately **not** Google's fused `com.google.sensor.prox_voice_call` (`0x0101002a`), which releases screen-off when orientation leaves the ~0.87 rad ear cone — exactly the "flat on a table → screen wakes" failure we reject.
- On `ACTION_SCREEN_ON` (plus re-checks at 80 ms / 220 ms to cover sensor lag), if near it execs `su -c "input keyevent 223"` (`KEYCODE_SLEEP`).
- `onSensorChanged` also sleeps immediately if it observes near while the screen is on.

It is **reactive**: the power button wakes the display in policy/hardware before any app code runs, so there is a ~150 ms flash before it drops. That latency is mostly the `input` JVM cold-start. Zero-flash requires intercepting the power key before the wake — a framework hook (LSPosed) on Android, or native policy in ferros. Accepted as daily-driver as-is.

Proven in logcat: three covered power presses → straight back to sleep; one uncovered press → stays on.

## Layout

- `src/com/pocketguard/` — `PocketApp` (Application + sensor listener), `ScreenReceiver` (SCREEN_ON), `Recheck` (deferred re-check).
  Flat top-level classes on purpose — the bundled `d8` NPEs on inner classes here.
- `AndroidManifest.xml` — `android:persistent="true"`, no permissions (root via `su`).
- `module/` — Magisk module skeleton (`module.prop`); the APK is dropped into `system/priv-app/PocketGuard/` by the build.
- `build.sh` — compiles, dexes, links, signs, and zips the module. See the header for env-var prerequisites (SDK build-tools, `android.jar`, `framework-res.apk`, and a signing keystore kept in `keys/`, never committed).

## Build & install

```bash
# framework-res.apk from the device (once):
adb pull /system/framework/framework-res.apk tools/pocketguard/framework-res.apk

# build (see build.sh header for BT / ANDROID_JAR / KEYSTORE overrides):
tools/pocketguard/build.sh

# install: flash build/pocketguard-module.zip in Magisk, or hot-swap the APK:
adb push build/PocketGuard.apk /data/local/tmp/PocketGuard.apk
adb shell 'su -c "cp /data/local/tmp/PocketGuard.apk \
  /data/adb/modules/pocketguard/system/priv-app/PocketGuard/PocketGuard.apk && \
  chmod 0644 /data/adb/modules/pocketguard/system/priv-app/PocketGuard/PocketGuard.apk"'
adb reboot   # priv-app updates are read at boot
```

Grant `su` to the app's uid in Magisk before first real use, or the sleep exec is denied.
