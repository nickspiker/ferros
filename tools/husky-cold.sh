. /mnt/Harbor/Code/ferros/tools/lib/profile.sh
#!/usr/bin/env bash
# Build the hv gate with this ferros packed in, sign it, keep it, and flash it onto both of husky's boot slots, the booting slot first.
#
# Both slots carry the same signed gate (HV.md: the slots stay identical): ABL loads it, the gate shows "Ready?", power (or 1 s on a dev build) = ferros, volume down = Android once (BOOT.md "As built on husky").
# ABL boots the slot of the UFS boot LUN whatever devinfo says, and `fastboot --set-active` strands husky in fastboot (BOOT.md, 2026-10-23), so nothing here switches slots: the booting slot (`getvar current-slot`) is flashed and booted first, and only once it has booted ferros does the other slot get the same image.
# If the new build dies on the booting slot, ABL counts its retries down and falls back to the other slot, which still holds the last good build (the A/B fallback, proven 2026-10-28).
#
# Signing (BOOT.md "Signed and unlocked"): `ferros-mkimg avb-footer` makes the gate a whole boot partition with its own vbmeta, signed with the owner's key, which the device's top-level vbmeta already chains `boot` to.
# The template is a boot partition signed earlier (target/avb/boot.img, else the newest image kept below): it gives the partition size, Google's boot properties and the rollback index; its own contents do not matter.
# The key goes in by path only and is never printed.
# Every image flashed is kept with its blake3 in keys/husky/images/ (`<name>.img` + `<name>.img.b3`), so any of them can go back on with `tools/husky-cold.sh <kept image>`.
#
# No buttons needed: from Android it goes `adb reboot bootloader`; from ferros `bridge reboot fastboot` (EL3 reboot-mode stamp + watchdog reset).
#
# Usage:
#   tools/husky-cold.sh              build, sign, keep, flash the booting slot, boot it, then the other slot (FERROS_BOTH_AT_ONCE=1: both slots, one boot; FERROS_ONE_SLOT=1: the booting slot only)
#   tools/husky-cold.sh <img>        same with a prebuilt gate image (signed here), or a kept signed partition image (flashed as is, after its blake3 is checked)
#   tools/husky-cold.sh --android    leave ferros for Android once, by the gate's own door (`bridge diag android`), no slot switch
#   tools/husky-cold.sh --read       last ferros report, from Android's dmesg
# Iterate without reflashing: `ferros-bridge reload target/aarch64-unknown-none/release/ferros_kernel.bin` (objcopy'd flat image).
set -euo pipefail
# Only husky: every fastboot and adb call goes to it, and fastboot refuses a phone that is not (tools/lib/devices.sh).
. /mnt/Harbor/Code/ferros/tools/lib/devices.sh
device_pin husky || exit 1

for v in FERROS_SLOT_B_ONLY FERROS_NO_SLOT_A FERROS_PURE_B FERROS_UNSIGNED; do
    if [ -n "${!v:-}" ]; then
        echo "$v is gone (2026-11-18): slot b cannot be chosen without a slot switch, and every flash is the signed gate; FERROS_ONE_SLOT=1 flashes the booting slot only" >&2
        exit 2
    fi
done

REPO=/mnt/Harbor/Code/ferros
BRIDGE=$REPO/target/debug/ferros-bridge
MKIMG=$REPO/target/debug/ferros-mkimg
AVB_KEY=${AVB_KEY:-/mnt/Harbor/Code/keys/avb.vsf}
KEEP=/mnt/Harbor/Code/keys/husky/images
AVB_TEMPLATE=${AVB_TEMPLATE:-$REPO/target/avb/boot.img}
# Other devices may share this host's adb (2026-10-23: a Pixel 3a); every adb call here means the husky. Its serial is read live, never written down.
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
    echo "no fastboot after the Android route either: the device needs hands (Power+VolDown)" >&2
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

# Wait for the boot after a `fastboot reboot`: 0 = ferros on USB, 1 = fastboot, 2 = Android, 3 = nothing for $1 seconds.
wait_boot() {
    local t0 limit=$1
    t0=$(date +%s)
    while :; do
        # With FERROS_BUS set (two ferros devices on the host) only that bus counts: the other device answered at once and the slot check ran against a Pro still booting (2026-11-10).
        # FERROS_PORT (the sysfs name, e.g. 1-2.2) picks the socket when both devices share a bus behind a hub (2026-11-12).
        if { [ -n "${FERROS_PORT:-}" ] && [ "$(cat /sys/bus/usb/devices/$FERROS_PORT/idProduct 2>/dev/null)" = 4665 ]; } || { [ -z "${FERROS_PORT:-}" ] && lsusb -d 1209:4665 2>/dev/null | grep -q "^Bus 0*${FERROS_BUS:-[0-9]*} "; }; then
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

# fastboot's own verdict on every write (Claude memories/husky_flash_paths.md: a night of captures once ran on a stale build behind a failed flash the script hid).
flash() {
    local out
    out=$(fastboot flash "$1" "$2" 2>&1) || true
    echo "$out" | grep -E "Sending|Writing|FAIL" | sed "s/^/   $1: /"
    if echo "$out" | grep -q FAIL || ! echo "$out" | grep -q "^Writing.*OKAY"; then
        echo "== $1 was NOT written" >&2
        return 1
    fi
}

case "${1:-}" in
    --read)
        read_report
        exit 0
        ;;
    --android)
        # The gate's development door: `diag android` leaves the marker and resets; the gate then boots Android once (BOOT.md "Back to Android"). From fastboot, boot the gate first.
        if fastboot devices | grep -q .; then
            fastboot reboot >/dev/null 2>&1
            wait_boot 600 || { echo "ferros did not come up to take the request" >&2; exit 1; }
        fi
        timeout 60 "$BRIDGE" diag android >/dev/null 2>&1 || true
        android_up && echo "== Android up" || { echo "Android did not come up" >&2; exit 1; }
        read_report
        exit 0
        ;;
esac

( cd "$REPO" && cargo build -q -p ferros-mkimg && cargo build -q -p ferros-bridge )
if [ -n "${1:-}" ]; then
    GATE=$1
else
    # The hv gate with this ferros packed in (tools/husky-hv.sh), the image both slots boot.
    ( cd "$REPO" && tools/husky-hv.sh >/dev/null )
    GATE=$REPO/target/hv/boot_a_hv.img
fi
[ -f "$GATE" ] || { echo "no image at $GATE" >&2; exit 1; }
mkdir -p "$KEEP"

# The template: target/avb/boot.img, else the newest kept image (each is a signed partition with the same size and properties).
if [ ! -f "$AVB_TEMPLATE" ]; then
    AVB_TEMPLATE=$(ls -t "$KEEP"/*.img 2>/dev/null | head -1)
    [ -n "$AVB_TEMPLATE" ] || { echo "no signing template: neither target/avb/boot.img nor a kept image in $KEEP" >&2; exit 1; }
fi
PART_SIZE=$(stat -c%s "$AVB_TEMPLATE")

if [ "$(stat -c%s "$GATE")" = "$PART_SIZE" ]; then
    # Already a whole signed partition: a kept image goes back on as it is, its blake3 checked first.
    PART=$GATE
    if [ -f "$PART.b3" ]; then
        ( cd "$(dirname "$PART")" && b3sum -c --quiet "$(basename "$PART").b3" ) || { echo "$PART does not match its blake3" >&2; exit 1; }
        echo "== kept image: $PART (blake3 checked)"
    else
        echo "== whole-partition image with no .b3 beside it: $PART, flashed as is"
    fi
else
    [ -f "$AVB_KEY" ] || { echo "no AVB key at $AVB_KEY: refusing to flash an unsigned gate" >&2; exit 1; }
    BRANCH=$(git -C "$REPO" rev-parse --abbrev-ref HEAD | tr '/' '-')
    COMMIT=$(git -C "$REPO" rev-parse --short=8 HEAD)
    git -C "$REPO" diff --quiet HEAD -- || COMMIT=${COMMIT}_dirty
    TMP=$KEEP/.signing.img
    rm -f "$TMP"
    if ! "$MKIMG" avb-footer --key "$AVB_KEY" --google-boot "$AVB_TEMPLATE" --image "$GATE" -o "$TMP" 2>&1 | sed 's/^/   avb: /' || [ ! -s "$TMP" ]; then
        echo "== signing failed; nothing flashed" >&2
        rm -f "$TMP"
        exit 1
    fi
    # The name carries the build and the start of the image's own blake3: every signing has a fresh salt, so the same build signed twice is two images.
    B3=$(b3sum --no-names "$TMP")
    NAME=boot_${BRANCH}_${COMMIT}_${B3:0:8}.img
    mv "$TMP" "$KEEP/$NAME"
    echo "$B3  $NAME" > "$KEEP/$NAME.b3"
    PART=$KEEP/$NAME
    echo "== kept: $PART (blake3 ${B3:0:16}..)"
fi

to_fastboot
HOME_SLOT=$(fastboot getvar current-slot 2>&1 | awk '/current-slot/ {print $2}')
case "$HOME_SLOT" in
    a) OTHER=b ;;
    b) OTHER=a ;;
    *) echo "fastboot did not name the current slot ('$HOME_SLOT')" >&2; exit 1 ;;
esac
echo "== booting slot: $HOME_SLOT"

if [ "${FERROS_BOTH_AT_ONCE:-0}" = 1 ]; then
    flash "boot_$HOME_SLOT" "$PART"
    flash "boot_$OTHER" "$PART"
    fastboot reboot
    if wait_boot 600; then sleep 1; "$BRIDGE" diag 2>/dev/null || echo "(diag failed: retry $BRIDGE diag)"; exit 0; fi
    exit 1
fi

# The booting slot first, the other slot only once it has booted ferros (BOOT.md "write slot B, verify, only then slot A"): a build that dies early stops here, with the other slot still the last good one.
flash "boot_$HOME_SLOT" "$PART"
fastboot reboot
if wait_boot 900; then
    SLOT=$(booted_slot)
    if [ "$SLOT" != "$HOME_SLOT" ]; then
        echo "== slot $HOME_SLOT did not boot ferros (ABL booted slot '${SLOT:-?}'); slot $OTHER left as it was" >&2
        exit 1
    fi
else
    echo "== no ferros from slot $HOME_SLOT (an Android-only gate, or the build died); slot $OTHER left as it was" >&2
    exit 1
fi
if [ "${FERROS_ONE_SLOT:-0}" = 1 ]; then
    echo "== slot $HOME_SLOT boots this build; slot $OTHER left as it was (FERROS_ONE_SLOT)"
    "$BRIDGE" diag 2>/dev/null | head -3 || true
    exit 0
fi
echo "== slot $HOME_SLOT boots this build; now slot $OTHER"
to_fastboot
flash "boot_$OTHER" "$PART"
fastboot reboot
if wait_boot 600; then
    echo "== both slots carry $(basename "$PART"); slot $(booted_slot) booted"
    sleep 1
    "$BRIDGE" diag 2>/dev/null || echo "(diag failed: retry $BRIDGE diag)"
    exit 0
fi
exit 1
