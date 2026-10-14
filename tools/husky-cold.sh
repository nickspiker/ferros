#!/usr/bin/env bash
# Boot ferros straight from the bootloader on slot b (pure cold boot, no Linux) and drive it over USB.
#
# Slot a stays Android + the handoff kernel (dev host and escape hatch); slot b's boot partition holds ferros. ABL loads the boot.img kernel at G#8008_0000 (the cold link base, ferros_kernel/aarch64-pixel8.ld) with the MMU off and jumps to it; ferros arms its fault sink + watchdogs, tunes the eUSB repeater and serves Photon Transport on USB (ferros_kernel/src/service.rs). There is no disk on a cold boot yet (UFS.md), so every exit except a plain `bridge reboot` goes to fastboot, where this script (or you) decides what boots next.
#
# No buttons needed: from Android it goes `adb reboot bootloader`; from ferros `bridge reboot fastboot` (EL3 reboot-mode stamp + watchdog reset). Slot b is never marked successful (that needs the devinfo partition, i.e. a disk), so ABL also falls back to slot a by itself after its retry count runs out.
#
# Usage:
#   tools/husky-cold.sh            build, flash boot_b, make b active, boot, wait for ferros on USB, print DIAG
#   tools/husky-cold.sh <img>      same with a prebuilt boot image
#   tools/husky-cold.sh --android  leave ferros: fastboot, make a active, reboot into Android
#   tools/husky-cold.sh --read     last ferros report, from Android's dmesg (slot a handoff kernel prints the ramoops block)
# Iterate without reflashing: `ferros-bridge reload target/aarch64-unknown-none/release/ferros_kernel.bin` (objcopy'd flat image).
set -euo pipefail

REPO=/mnt/Harbor/Code/ferros
BRIDGE=$REPO/target/debug/ferros-bridge
ELF=$REPO/target/aarch64-unknown-none/release/ferros_kernel
IMG=$REPO/target/aarch64-unknown-none/release/ferros_cold.img

to_fastboot() {
    if fastboot devices | grep -q .; then return; fi
    if lsusb -d 1209:4665 >/dev/null 2>&1; then
        echo "== ferros -> fastboot"
        timeout 30 "$BRIDGE" reboot fastboot >/dev/null 2>&1 || true
    elif adb get-state >/dev/null 2>&1; then
        echo "== Android -> fastboot"
        adb reboot bootloader
    fi
    for _ in $(seq 90); do fastboot devices | grep -q . && return; sleep 2; done
    echo "no fastboot after 3 min — phone needs hands (Power+VolDown)" >&2
    exit 1
}

android_up() {
    for _ in $(seq 120); do
        [ "$(timeout 3 adb shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = "1" ] && return 0
        sleep 3
    done
    return 1
}

read_report() {
    adb shell su -c "'dmesg | grep -E \"ferros: (UFS READ|r\\[0[0-3])\"'" 2>/dev/null || echo "(no ferros report in dmesg)"
    echo "bootreason: $(adb shell getprop ro.boot.bootreason | tr -d '\r')"
}

case "${1:-}" in
    --read)
        read_report
        exit 0
        ;;
    --android)
        to_fastboot
        fastboot --set-active=a >/dev/null 2>&1
        fastboot reboot >/dev/null 2>&1
        android_up && echo "== Android on slot a" || { echo "Android did not come up" >&2; exit 1; }
        read_report
        exit 0
        ;;
esac

if [ -n "${1:-}" ]; then
    IMG=$1
else
    ( cd "$REPO" && cargo build -p ferros_kernel --target aarch64-unknown-none --release && cargo build -q -p ferros-bridge )
    ( cd "$REPO" && cargo run -q -p ferros-mkimg -- boot "$ELF" --pad 16 -o "$IMG" )
fi
echo "== image: $IMG ($(stat -c%s "$IMG") bytes)"

to_fastboot
fastboot flash boot_b "$IMG"
fastboot --set-active=b
t0=$(date +%s)
fastboot reboot
while :; do
    if lsusb -d 1209:4665 >/dev/null 2>&1; then
        echo "== ferros on USB after $(( $(date +%s) - t0 ))s (cold boot, slot b)"
        sleep 1
        "$BRIDGE" diag 2>/dev/null || echo "(diag failed — retry: $BRIDGE diag)"
        exit 0
    fi
    if fastboot devices | grep -q .; then
        echo "== landed in fastboot after $(( $(date +%s) - t0 ))s — ferros left (or never enumerated); boot Android with: $0 --android"
        exit 1
    fi
    if adb get-state >/dev/null 2>&1; then
        echo "== Android came up after $(( $(date +%s) - t0 ))s — ABL fell back to slot a"
        read_report
        exit 1
    fi
    [ $(( $(date +%s) - t0 )) -gt 600 ] && { echo "nothing on USB after 10 min — phone needs hands (Power+VolDown)" >&2; exit 1; }
    sleep 1
done
