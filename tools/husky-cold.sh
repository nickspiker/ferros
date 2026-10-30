#!/usr/bin/env bash
# Boot ferros straight from the bootloader on slot b (pure cold boot, no Linux) and drive it over USB.
#
# Slot a stays Android + the handoff kernel (dev host and escape hatch); slot b's boot partition holds ferros. ABL loads the boot.img kernel at G#8008_0000 (the cold link base, ferros_kernel/aarch64-pixel8.ld) with the MMU off and jumps to it; ferros arms its fault sink + watchdogs, tunes the eUSB repeater and serves Photon Transport on USB (ferros_kernel/src/service.rs). It brings up the disk itself (cold_ufs.rs) and marks slot b successful in devinfo (slot.rs), so a plain reset or power cycle boots ferros again; every exit except a plain `bridge reboot` goes to fastboot, where this script (or you) decides what boots next.
#
# No buttons needed: from Android it goes `adb reboot bootloader`; from ferros `bridge reboot fastboot` (EL3 reboot-mode stamp + watchdog reset). `--android` is the only way back to slot a: slot b no longer falls back on its own.
#
# Usage:
#   tools/husky-cold.sh            build, flash slot b (the gate + this ferros, signed), boot it, and only once slot b boots ferros flash slot a and boot the home slot (FERROS_BOTH_AT_ONCE=1: both at once, as before; FERROS_SLOT_B_ONLY=1: slot b only, slot a left as it is)
#   tools/husky-cold.sh <img>      same with a prebuilt boot image
#   tools/husky-cold.sh --android  leave ferros: fastboot, make a active, reboot into Android
#   tools/husky-cold.sh --read     last ferros report, from Android's dmesg (slot a handoff kernel prints the ramoops block)
# Iterate without reflashing: `ferros-bridge reload target/aarch64-unknown-none/release/ferros_kernel.bin` (objcopy'd flat image).
set -euo pipefail

REPO=/mnt/Harbor/Code/ferros
BRIDGE=$REPO/target/debug/ferros-bridge
ELF=$REPO/target/aarch64-unknown-none/release/ferros_kernel
IMG=$REPO/target/aarch64-unknown-none/release/ferros_cold.img
# Other phones may share this host's adb (2026-10-23: a Pixel 3a); every adb call here means the husky. Its serial is read live, never written down.
husky() {
    if [ -z "${ANDROID_SERIAL:-}" ]; then
        local s
        s=$(adb devices -l 2>/dev/null | awk '/product:husky/{print $1; exit}')
        [ -n "$s" ] && export ANDROID_SERIAL=$s
    fi
    return 0
}
husky

to_fastboot() {
    if fastboot devices | grep -q .; then return; fi
    if lsusb -d 1209:4665 >/dev/null 2>&1; then
        echo "== ferros -> fastboot"
        timeout 30 "$BRIDGE" reboot fastboot >/dev/null 2>&1 || true
        for _ in $(seq 40); do fastboot devices | grep -q . && return; sleep 2; "$REPO/tools/usb-kick.sh" --if-wedged; done
        # `bridge reboot fastboot` lands in fastboot ~40 s after the command (fixed 2026-10-24: the vault's watchdog pets, from the map's writes on the worker cores, had been holding off the reset; platform.rs RESET_ORDERED). Should it not take, the route through Android: its `adb reboot bootloader` always lands in fastboot; the gate boots Android without asking on the marker `diag android` leaves (BOOT.md).
        if lsusb -d 1209:4665 >/dev/null 2>&1; then
            echo "== fastboot did not take; ferros -> Android -> fastboot"
            timeout 60 "$BRIDGE" diag android >/dev/null 2>&1 || true
            for _ in $(seq 60); do husky; adb get-state >/dev/null 2>&1 && break; sleep 2; done
        fi
    fi
    husky
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
        husky
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

# Signed with the owner's AVB key when it is there (BOOT.md, "Signed and unlocked"): the gate as a whole boot partition with its own signed footer, and the top-level vbmeta whose boot chain names that key, both made by one command from the installed build's Google images. Without the key (or with FERROS_UNSIGNED=1) the gate goes on bare, as before; ABL logs the verification error and boots it, unlocked.
AVB_KEY=${AVB_KEY:-/mnt/Harbor/Code/keys/avb.vsf}
AVB_ZIP=${AVB_ZIP:-/mnt/Harbor/tmp/husky-factory/husky-cp1a.260505.005/image-husky-cp1a.260505.005.zip}
# The slot's own dtbo, vendor_boot, vendor_kernel_boot and init_boot (`<name>_a.img`): the top vbmeta hashes those as installed and chains init_boot to the owner's key, so both slots verify end to end (BOOT.md). Must be what the phone carries: the 2026-10-30 snapshot of slot a (the post-wipe Magisk init_boot; the rest as on 2026-10-28), also mirrored onto slot b.
AVB_INSTALLED=${AVB_INSTALLED:-/mnt/Harbor/tmp/husky-backup-2026-10-30}
SIGNED=""
if [ -n "$HV_IMG" ] && [ -f "$AVB_KEY" ] && [ "${FERROS_UNSIGNED:-0}" != 1 ]; then
    G=$(mktemp -d)
    INST=""
    [ -f "$AVB_INSTALLED/init_boot_a.img" ] && INST="--installed $AVB_INSTALLED --google-init-boot $G/init_boot.img --out-init-boot $REPO/target/avb/init_boot.img"
    if unzip -q -o "$AVB_ZIP" vbmeta.img boot.img init_boot.img -d "$G" && ( cd "$REPO" && cargo build -q -p ferros-mkimg ) && mkdir -p "$REPO/target/avb" \
        && OUT=$("$REPO/target/debug/ferros-mkimg" avb-sign --key "$AVB_KEY" --google-vbmeta "$G/vbmeta.img" --google-boot "$G/boot.img" --gate "$HV_IMG" $INST --out-vbmeta "$REPO/target/avb/vbmeta.img" --out-boot "$REPO/target/avb/boot.img" 2>&1); then
        SIGNED=1
    fi
    echo "${OUT:-}" | grep -E "^boot|^key|failed" | sed 's/^/   avb: /' || true
    rm -rf "$G"
    [ -n "$SIGNED" ] && HV_IMG=$REPO/target/avb/boot.img || echo "== signing failed; the gate goes on unsigned"
fi

to_fastboot
HOME_SLOT=$(fastboot getvar current-slot 2>&1 | awk '/current-slot/ {print $2}')

# Wait for the boot after a `fastboot reboot`: 0 = ferros on USB, 1 = fastboot, 2 = Android, 3 = nothing for $1 seconds.
wait_boot() {
    local t0 limit=$1
    t0=$(date +%s)
    while :; do
        if lsusb -d 1209:4665 >/dev/null 2>&1; then
            echo "== ferros on USB after $(( $(date +%s) - t0 ))s"
            return 0
        fi
        if [ $(( $(date +%s) - t0 )) -gt 20 ] && fastboot devices | grep -q .; then
            echo "== landed in fastboot after $(( $(date +%s) - t0 ))s"
            return 1
        fi
        husky
        # `adb get-state` can answer for a moment after `fastboot reboot` (2026-10-23: "Android came up after 0s" while ferros was booting); only a real adb device on the bus counts.
        if [ "$(adb get-state 2>/dev/null)" = device ] && lsusb | grep -q "18d1:4ee[1-9a-f]"; then
            echo "== Android came up after $(( $(date +%s) - t0 ))s"
            return 2
        fi
        [ $(( $(date +%s) - t0 )) -gt "$limit" ] && { echo "== nothing on USB after ${limit}s"; return 3; }
        "$REPO/tools/usb-kick.sh" --if-wedged
        sleep 1
    done
}

# The slot ABL booted, from its own log in RAM (G#FD80_0000): "[RIPCURRENT] boot slot: a|b".
booted_slot() {
    local f
    f=$(mktemp)
    for _ in $(seq 20); do timeout 5 "$BRIDGE" status >/dev/null 2>&1 && break; sleep 1; done
    timeout 60 "$BRIDGE" read FD800000 10000 "$f" >/dev/null 2>&1 || true
    strings "$f" | awk '/boot slot:/ {s=$NF} END {print s}'
    rm -f "$f"
}

if [ -z "$HV_IMG" ] || [ "${FERROS_PURE_B:-0}" = 1 ] || [ "${FERROS_BOTH_AT_ONCE:-0}" = 1 ]; then
    # One pass, both slots at once (FERROS_BOTH_AT_ONCE=1, or a slot-b-only bare image): the old flow.
    if [ -n "$HV_IMG" ] && [ "${FERROS_PURE_B:-0}" != 1 ]; then
        fastboot flash boot_b "$HV_IMG" 2>&1 | grep -E "Writing|FAIL" | sed 's/^/   slot b: /'
    else
        fastboot flash boot_b "$IMG"
    fi
    [ -n "$HV_IMG" ] && fastboot flash boot_a "$HV_IMG" 2>&1 | grep -E "Writing|FAIL" | sed 's/^/   slot a: /'
    if [ -n "$SIGNED" ]; then
        for s in a b; do fastboot flash "vbmeta_$s" "$REPO/target/avb/vbmeta.img" 2>&1 | grep -E "Writing|FAIL" | sed "s/^/   vbmeta $s: /"; done
    fi
    # Re-written as active so a stray de-activated bit on it is cleared (the other slot's is cleared by ferros when it marks, slot.rs).
    fastboot --set-active="$HOME_SLOT" >/dev/null 2>&1 || true
    fastboot reboot
    if wait_boot 600; then sleep 1; "$BRIDGE" diag 2>/dev/null || echo "(diag failed — retry: $BRIDGE diag)"; exit 0; fi
    exit 1
fi

# Slot b first, slot a only once b has booted (BOOT.md "write slot B, verify, only then slot A"): a build that dies early stops here, with slot a still the last good one. If b fails, ABL counts its retries down and falls back to slot a by itself (the A/B fallback, proven 2026-10-28), so no hands are needed either way.
fastboot flash boot_b "$HV_IMG" 2>&1 | grep -E "Writing|FAIL" | sed 's/^/   slot b: /'
[ -n "$SIGNED" ] && fastboot flash vbmeta_b "$REPO/target/avb/vbmeta.img" 2>&1 | grep -E "Writing|FAIL" | sed 's/^/   vbmeta b: /'
fastboot --set-active=b >/dev/null 2>&1
fastboot reboot
wait_boot 900 || true
SLOT=$(booted_slot)
if [ "$SLOT" != b ]; then
    echo "== slot b did not boot ferros (ABL booted slot '${SLOT:-?}'); slot a left as it was" >&2
    exit 1
fi
# FERROS_SLOT_B_ONLY=1: stop here, slot a as it was (an Android left on it for captures, say) — the gate on b, verified.
if [ "${FERROS_SLOT_B_ONLY:-0}" = 1 ]; then
    echo "== slot b boots this build; slot a left as it was (FERROS_SLOT_B_ONLY)"
    "$BRIDGE" diag 2>/dev/null | head -3 || true
    exit 0
fi
echo "== slot b boots this build; now slot a"
to_fastboot
fastboot flash boot_a "$HV_IMG" 2>&1 | grep -E "Writing|FAIL" | sed 's/^/   slot a: /'
[ -n "$SIGNED" ] && fastboot flash vbmeta_a "$REPO/target/avb/vbmeta.img" 2>&1 | grep -E "Writing|FAIL" | sed 's/^/   vbmeta a: /'
fastboot --set-active="$HOME_SLOT" >/dev/null 2>&1 || true
fastboot reboot
if wait_boot 600; then
    echo "== both slots carry this build; slot $(booted_slot) booted"
    sleep 1
    "$BRIDGE" diag 2>/dev/null || echo "(diag failed — retry: $BRIDGE diag)"
    exit 0
fi
exit 1
