#!/usr/bin/env bash
# The capture of Android's own long exposure on the Pro's main camera (docs/CAMERA.md "Long exposures", 2026-11-17): Lumis on Android at 16 s, the main sensor's I2C traffic trapped by the hv's register oracle, so ferros learns how the sensor is told to integrate past the 16 bits of its CCS frame length (the vendor's shift words G#0702 and G#0704, which Android writes as 0 in every ordinary hold).
# One controller, one target: hsi2c_1 (G#1088_0000), and on it only the main sensor (boitata, G#3D), its transfers alone (FERROS_HV_ORACLE_I2C_TARGET), so the lens and OIS polling on the same bus (seven of every eight transfers after stream on) never crowd the console ring and the whole session fits.
#
# Steps, each its own command, each after the coordinator's go:
#   tools/husky-hv-longexp.sh build              the oracle hv in front of Android's kernel, no ferros, footered and signed with the owner's AVB key: target/hv/boot_a_longexp.img
#   tools/husky-hv-longexp.sh flash              that image to boot_a (from ferros by `bridge reboot fastboot`, or from Android by adb), then Android boots under it
#   tools/husky-hv-longexp.sh start              waits for Android past the arming time, quiets the kernel console (pstore integrity), marks the start; then the owner, by hand: open Lumis, the main camera, shutter to 16 s, one shot, wait for it to land
#   tools/husky-hv-longexp.sh end <out.log>      marks the end, flushes the console ring with a panic (a crash reset keeps it), waits for Android again, pulls the hv's lines, decodes them (tools/oracle-i2c): <out.log>, <out>.holds, <out>.xfers
#   tools/husky-hv-longexp.sh restore <image>    the image boot_a held before (the coordinator names it: the signed gate with the flashed ferros), back to boot_a
# The boot after the capture traps nothing (the hv's one-shot marker), so `end` reads the ring from a normal Android boot.
# Nothing here writes anything but boot_a, and only in `flash` and `restore`.
set -eu
. /mnt/Harbor/Code/ferros/tools/lib/devices.sh
cd "$(dirname "$0")/.."
W=target/hv
mkdir -p $W
IMG=$W/boot_a_longexp.img
# Android's kernel, as slot a runs it (the boot backup it was unpacked from went with Harbor tmp on 2026-11-17; the main checkout keeps the unpacked Image).
LINUX=${LINUX:-/mnt/Harbor/Code/ferros/target/hv/Image}
AVB_KEY=${AVB_KEY:-/mnt/Harbor/Code/keys/avb.vsf}
# The footer's template: the signed boot image of 2026-11-17 morning, as the b929 flash used it (Claude memories/stored_capsules_need_flashed_kernel.md).
AVB_TEMPLATE=${AVB_TEMPLATE:-/mnt/Harbor/Code/ferros/target/avb/boot.img}
# Android's clock runs about 20 s behind the hv's counter (docs/oracle/CAMERA.md), so the oracle arms about 40 s into Android's uptime: past the camera service's boot probe, before anyone can open Lumis.
ARM_MS=60000
# fastboot's own words, all of them, and a stop on FAILED: a whole night of captures once ran on a stale build because a script hid a failed flash (Claude memories/husky_flash_paths.md; a 64 MiB send that fails is the cable).
flash_boot_a() {
    local out
    out=$(fastboot flash boot_a "$1" 2>&1)
    echo "$out"
    if echo "$out" | grep -q FAILED || ! echo "$out" | grep -q OKAY; then echo "== boot_a NOT written"; exit 1; fi
}
usbserial() { adb devices | awk '$2=="device" && $1 !~ /[:.]/ {print $1; exit}'; }
wait_android() {
    until [ -n "$(usbserial)" ]; do sleep 3; tools/usb-kick.sh --if-wedged >/dev/null 2>&1 || true; done
    device_pin husky
    until adb shell "test \$(cut -d. -f1 /proc/uptime) -ge $((ARM_MS / 1000 + 2)) && test \"\$(getprop sys.boot_completed)\" = 1" 2>/dev/null; do sleep 3; done
}
case "${1:-}" in
build)
    [ -f "$LINUX" ] || { echo "no Android kernel at $LINUX"; exit 1; }
    # The capture's knobs: stage 2 with the UFS firewall as every gate runs it (Android may not write ferros's slot), the oracle on hsi2c_1's page, its sensor alone, logging never capped (the ring keeps the newest), armed after ARM_MS; no PMU reads.
    # Built here as tools/husky-hv.sh builds (host only: no device is asked anything): the hv, its flat binary in front of Android's kernel, no ferros packed, the kernel's console at level 4 from boot (pstore integrity).
    cargo build -q -p ferros-mkimg
    FERROS_HV_STAGE2=1 FERROS_HV_UFS=1 FERROS_HV_ORACLE=1 FERROS_HV_NO_PMU_READ=1 \
    FERROS_HV_ORACLE_PAGES="G#1088_0000" FERROS_HV_ORACLE_I2C_TARGET=G#3D \
    FERROS_HV_ORACLE_CAP=$((1 << 20)) FERROS_HV_ORACLE_ARM_AFTER_MS=$ARM_MS \
    cargo build -q -p ferros_hv --target aarch64-unknown-none-softfloat --release
    aarch64-linux-gnu-objcopy -O binary target/aarch64-unknown-none-softfloat/release/ferros_hv $W/hv-longexp.bin
    [ "$(stat -c %s $W/hv-longexp.bin)" -le $((0x180000)) ] || { echo "the hv is over its G#18_0000 pad"; exit 1; }
    target/debug/ferros-mkimg gate --hv $W/hv-longexp.bin --image "$LINUX" --cmdline "loglevel=4" -o $W/boot_a_longexp_bare.img
    target/debug/ferros-mkimg avb-footer --key "$AVB_KEY" --google-boot "$AVB_TEMPLATE" --image $W/boot_a_longexp_bare.img -o $IMG
    echo "== $IMG ($(stat -c %s $IMG) bytes), blake3 $(b3sum --no-names $IMG 2>/dev/null || echo '?')"
    ;;
flash)
    [ -f "$IMG" ] || { echo "no $IMG: run build first"; exit 1; }
    # From ferros (the Pro on USB as 1209:4665): the bridge's fastboot; from Android: adb.
    if lsusb -d 1209:4665 >/dev/null 2>&1; then timeout 60 target/debug/ferros-bridge reboot fastboot >/dev/null 2>&1 || true
    elif [ -n "$(usbserial)" ]; then adb -s "$(usbserial)" reboot bootloader || true; fi
    for i in $(seq 60); do fastboot devices 2>/dev/null | grep -q . && break; sleep 2; tools/usb-kick.sh --if-wedged || true; done
    device_pin husky
    # No set-active: ABL boots slot a whatever fastboot says, and a set-active of b strands the phone in fastboot (Claude memories/husky_cold_slot_b.md).
    flash_boot_a $IMG
    fastboot reboot >/dev/null 2>&1
    echo "== boot_a holds the capture image; Android boots under the oracle (no gate, no ferros). Then: $0 start"
    ;;
start)
    wait_android
    adb shell "su -c 'echo 4 > /proc/sys/kernel/printk; echo \"<3>ferros-mark: long exposure: start\" > /dev/kmsg'; input keyevent KEYCODE_WAKEUP" >/dev/null 2>&1
    # The capture image is the one booted: its command line carries loglevel=4 (the gate's carries none), and the hv's region is reserved.
    adb shell 'grep -q loglevel=4 /proc/cmdline && ls /sys/firmware/devicetree/base/reserved-memory/ | grep -q hv' || { echo "== this boot is not the capture image (no loglevel=4 or no hv): stop"; exit 1; }
    echo "== marked. The owner now: open Lumis, the main camera, shutter to 16 s, take one shot, wait for it to land (about 35 s). Then: $0 end <out.log>"
    ;;
end)
    OUT=${2:?usage: $0 end <out.log>}
    S=$(usbserial)
    [ -n "$S" ] || { echo "no Android on adb"; exit 1; }
    device_pin husky
    adb shell "su -c 'echo \"<3>ferros-mark: long exposure: end\" > /dev/kmsg; echo c > /proc/sysrq-trigger'" >/dev/null 2>&1 || true
    sleep 10
    wait_android
    until adb shell 'su -c "ls /sys/fs/pstore/console-ramoops-0"' >/dev/null 2>&1; do sleep 3; done
    adb shell 'su -c "grep -aE \"ferros hv|ferros-mark\" /sys/fs/pstore/console-ramoops-0"' > "$OUT"
    echo "== $(wc -l < "$OUT") lines -> $OUT"
    ( cd tools/oracle-i2c && cargo build -q --release )
    D=tools/oracle-i2c/target/release/oracle-i2c
    $D "$OUT" --max-bytes 32 > "${OUT%.log}.xfers"
    $D "$OUT" --max-bytes 32 --holds G#3D > "${OUT%.log}.holds"
    echo "== ${OUT%.log}.xfers, ${OUT%.log}.holds (the vendor upload, if any, stands as its length and blake3; keep both local)"
    grep -E "MARK|0702=0[^0]|0702=00[^0]|0704=0[^0]|0704=00[^0]|^W 0100" "${OUT%.log}.holds" | head -40
    ;;
restore)
    R=${2:?usage: $0 restore <the image boot_a held before>}
    [ -f "$R" ] || { echo "no $R"; exit 1; }
    S=$(usbserial)
    [ -n "$S" ] && { device_pin husky; adb -s "$S" reboot bootloader || true; }
    for i in $(seq 60); do fastboot devices 2>/dev/null | grep -q . && break; sleep 2; tools/usb-kick.sh --if-wedged || true; done
    device_pin husky
    flash_boot_a "$R"
    fastboot reboot >/dev/null 2>&1
    echo "== boot_a restored from $R"
    ;;
*)
    sed -n '2,15p' "$0"
    exit 2
    ;;
esac
