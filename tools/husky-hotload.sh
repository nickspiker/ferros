#!/usr/bin/env bash
# Hot-load a ferros shim build onto husky (Pixel 8 Pro) with no flashing: push, stage, `adb reboot ferros`, read the report.
#
# Needs the runtime-trigger husky kernel (husky-kernel aosp: ferros_handoff.c /sys/kernel/ferros/stage + the ufshcd.c reboot notifier) flashed once, plus root (Magisk) and pKVM disabled.
# The kernel copies the staged image into the 0x92400000 carveout; `adb reboot ferros` lets Android shut down cleanly (sync + unmount), then the reboot notifier wakes and drains the UFS host and jumps to ferros before device_shutdown() can suspend the link.
# ferros reports into ramoops (0xFD60_0000) and PSCI warm-resets; the next Linux boot prints the report in dmesg.
# Nothing is armed at boot, so every reset — report, fault sink, panic reporter, watchdog — lands back in Android with adb, and `adb reboot bootloader` is always one command away. Measured round trip: about one minute.
#
# The default shim build is the USB service (ferros_kernel/src/shim_usb.rs): it adopts UFS, probes the vault, then serves Photon Transport on the inherited USB controller, so `ferros-bridge diag|reload|run|reboot` work with the disk live. It leaves (report + reset to Android) on `bridge reboot`, 64 s without enumerating, 512 s without a command, or both volume buttons. `bridge reboot fastboot` asks the next Linux boot to drop into the bootloader.
#
# Usage:
#   tools/husky-hotload.sh            # build the USB-service shim, hot-load it, wait for it on USB, print DIAG
#   tools/husky-hotload.sh --vault    # build the one-shot vault probe instead (reports + resets, prints the report)
#   tools/husky-hotload.sh <flat.bin> # hot-load a prebuilt flat binary
#   tools/husky-hotload.sh --read     # print the last report again
#   tools/husky-hotload.sh --live ... # hand off from RUNNING Android (screen woken and kept on) via /sys/kernel/ferros/go instead of `adb reboot ferros`: no Android shutdown, so the display pipeline is still live for ferros to adopt. Filesystems are synced first; otherwise it is a sudden power loss to Android.
set -euo pipefail

REPO=/mnt/Harbor/Code/ferros
DEV_BIN=/data/local/tmp/ferros.bin
STAGE=/sys/kernel/ferros/stage

report() {
    echo "-- previous boot (pstore console): the handoff side"
    adb shell su -c "'cat /sys/fs/pstore/console-ramoops-0 2>/dev/null | grep -E \"ferros:\"'" 2>/dev/null || echo "(no pstore console)"
    echo "-- this boot (dmesg): ferros's report"
    adb shell su -c "'dmesg | grep -E \"ferros:|ferros \"'" 2>/dev/null || echo "(no ferros lines in dmesg)"
    echo "bootreason: $(adb shell getprop ro.boot.bootreason | tr -d '\r')"
}

if [ "${1:-}" = "--read" ]; then report; exit 0; fi

ENTRY=usb
LIVE=0
if [ "${1:-}" = "--live" ]; then LIVE=1; shift; fi
if [ "${1:-}" = "--vault" ]; then ENTRY=vault; shift; fi
if [ -n "${1:-}" ]; then
    BIN=$1
else
    BIN=$REPO/target/aarch64-unknown-none/release/ferros_kernel.bin
    ( cd "$REPO" && FERROS_SHIM_HANDOFF=1 FERROS_SHIM_ENTRY=$ENTRY cargo build -p ferros_kernel --target aarch64-unknown-none --release )
    if command -v aarch64-linux-gnu-objcopy >/dev/null; then OBJ=aarch64-linux-gnu-objcopy; else OBJ=rust-objcopy; fi
    "$OBJ" -O binary "$REPO/target/aarch64-unknown-none/release/ferros_kernel" "$BIN"
fi
echo "== image: $BIN ($(stat -c%s "$BIN") bytes)"

adb wait-for-device
if [ "$(adb shell su -c "'cat /proc/cmdline'" | grep -o 'kvm-arm.mode=[a-z]*' | tail -1)" = "kvm-arm.mode=protected" ]; then
    echo "pKVM is active — it cages the EL2 handoff. Run: adb reboot bootloader && fastboot oem pkvm disable && fastboot reboot" >&2
    exit 1
fi
adb shell su -c "'test -e $STAGE'" || { echo "no $STAGE — flash the runtime-trigger husky kernel first" >&2; exit 1; }

adb push -q "$BIN" "$DEV_BIN"
adb shell su -c "'echo $DEV_BIN > $STAGE'"
[ "$(adb shell su -c "'cat $STAGE'" | tr -d '\r')" = "1" ] || { echo "staging failed:" >&2; adb shell su -c "'dmesg | grep ferros: | tail -3'" >&2; exit 1; }
t0=$(date +%s)
if [ $LIVE = 1 ]; then
    # Screen on and kept on; DECON hibernation off (in command mode the driver powers the whole DPU down a few idle frames after each update, so without this ferros finds DECON reading all zeros).
    adb shell su -c "'svc power stayon true; input keyevent KEYCODE_WAKEUP; echo 0 > /sys/devices/platform/19470000.drmdecon/hibernation'"
    sleep 2
    # stayon is a persistent Android setting; drop it again now that the screen is lit (it stays on well past the handoff), so Android doesn't keep the panel on forever after ferros exits.
    adb shell su -c "'svc power stayon false'"
    echo "== staged; live handoff (screen on)"
    timeout 10 adb shell su -c "'echo 1 > /sys/kernel/ferros/go'" || true
else
    echo "== staged; adb reboot ferros"
    adb reboot ferros
fi
sleep 8
while :; do
    if lsusb -d 1209:4665 >/dev/null 2>&1; then
        echo "== ferros on USB after $(( $(date +%s) - t0 ))s"
        ( cd "$REPO" && cargo run -q -p ferros-bridge -- diag ) || echo "(diag failed — retry: cargo run -p ferros-bridge -- diag)"
        exit 0
    fi
    if [ "$(timeout 3 adb shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = "1" ]; then
        echo "== back in Android after $(( $(date +%s) - t0 ))s"
        report
        exit 0
    fi
    [ $(( $(date +%s) - t0 )) -gt 900 ] && { echo "neither ferros on USB nor Android after 15 min — phone needs hands (Power+VolDown)" >&2; exit 1; }
    sleep 1
done
