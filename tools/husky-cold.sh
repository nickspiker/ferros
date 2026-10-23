#!/usr/bin/env bash
# Boot ferros straight from the bootloader on slot b (pure cold boot, no Linux) and drive it over USB.
#
# Slot a stays Android + the handoff kernel (dev host and escape hatch); slot b's boot partition holds ferros. ABL loads the boot.img kernel at G#8008_0000 (the cold link base, ferros_kernel/aarch64-pixel8.ld) with the MMU off and jumps to it; ferros arms its fault sink + watchdogs, tunes the eUSB repeater and serves Photon Transport on USB (ferros_kernel/src/service.rs). It brings up the disk itself (cold_ufs.rs) and marks slot b successful in devinfo (slot.rs), so a plain reset or power cycle boots ferros again; every exit except a plain `bridge reboot` goes to fastboot, where this script (or you) decides what boots next.
#
# No buttons needed: from Android it goes `adb reboot bootloader`; from ferros `bridge reboot fastboot` (EL3 reboot-mode stamp + watchdog reset). `--android` is the only way back to slot a: slot b no longer falls back on its own.
#
# Usage:
#   tools/husky-cold.sh            build, flash boot_b (and boot_a with the hv gate + the same ferros), make b active, boot, wait for ferros on USB, print DIAG
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
        for _ in $(seq 40); do fastboot devices | grep -q . && return; sleep 2; "$REPO/tools/usb-kick.sh" --if-wedged; done
        # `bridge reboot fastboot` stopped taking on 2026-10-23 (the stamp lost across the watchdog reset, ferros booting again instead); the sure path is through Android, whose `adb reboot bootloader` always lands in fastboot. The gate boots Android without asking on the marker `diag android` leaves (BOOT.md).
        if lsusb -d 1209:4665 >/dev/null 2>&1; then
            echo "== fastboot did not take; ferros -> Android -> fastboot"
            timeout 60 "$BRIDGE" diag android >/dev/null 2>&1 || true
            for _ in $(seq 60); do adb get-state >/dev/null 2>&1 && break; sleep 2; done
        fi
    fi
    if adb get-state >/dev/null 2>&1; then
        echo "== Android -> fastboot"
        adb reboot bootloader
    fi
    for _ in $(seq 60); do fastboot devices | grep -q . && return; sleep 2; "$REPO/tools/usb-kick.sh" --if-wedged; done
    echo "no fastboot after the Android route either — phone needs hands (Power+VolDown)" >&2
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
# Slot a mirrors slot b (BOOT.md "Dual Boot"): its boot image is the hv gate with this same ferros packed in (tools/husky-hv.sh), so every boot of either slot lands on "Ready?" — power or 1 s (dev) = ferros, volume down = Android once — and a reboot from Android comes back to ferros instead of staying in Android (owner, 2026-10-22). FERROS_NO_SLOT_A=1 leaves slot a as it is (an oracle capture build, say).
HV_IMG=""
if [ "${FERROS_NO_SLOT_A:-0}" != 1 ]; then
    ( cd "$REPO" && tools/husky-hv.sh >/dev/null ) && HV_IMG=$REPO/target/hv/boot_a_hv.img || echo "== slot a gate image failed to build; slot a left as it is"
fi

to_fastboot
# Both slots carry the same gate image (owner, 2026-10-23: identical slots, every boot lands on Ready?, Android is a jump from the gate — the gate rewrites bootconfig's slot suffix to _a, the one slot with an Android on it). FERROS_PURE_B=1 puts the bare cold ferros on slot b instead (no gate, no Android from it).
if [ -n "$HV_IMG" ] && [ "${FERROS_PURE_B:-0}" != 1 ]; then
    fastboot flash boot_b "$HV_IMG" 2>&1 | grep -E "Writing|FAIL" | sed 's/^/   slot b: /'
else
    fastboot flash boot_b "$IMG"
fi
[ -n "$HV_IMG" ] && fastboot flash boot_a "$HV_IMG" 2>&1 | grep -E "Writing|FAIL" | sed 's/^/   slot a: /'
# No slot switch: ABL boots the UFS boot LUN's slot regardless, and `--set-active` marks the other slot "de-activated" (bit 3) — ABL then refuses it and sits in fastboot in 12 s (2026-10-23: four bounces, all this). Both slots carry the same image; the current one is re-written as active so a stray bit 3 is cleared.
fastboot --set-active="$(fastboot getvar current-slot 2>&1 | awk '/current-slot/ {print $2}')" >/dev/null 2>&1 || true
t0=$(date +%s)
fastboot reboot
while :; do
    if lsusb -d 1209:4665 >/dev/null 2>&1; then
        echo "== ferros on USB after $(( $(date +%s) - t0 ))s (cold boot, the boot LUN's slot)"
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
    "$REPO/tools/usb-kick.sh" --if-wedged
    sleep 1
done
