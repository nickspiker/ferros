#!/usr/bin/env bash
# Run a ferros shim/probe on husky (Pixel 8) via the custom-kernel handoff.
#
# The shim (entry_vault / the ira instrumentation) does NOT boot standalone on husky.
# A custom -dirty android14 GKI kernel fires it from inside ufshcd_async_scan (so it
# inherits the live UFS link), staged at phys 0x92400000, one-shot, then soft-restarts
# (warm reset) back to Linux which reports the ramoops result region (phys 0xFD60_0000)
# to dmesg. This script builds the flat binary, injects it as /ferros.bin into the
# vendor_kernel_boot ramdisk, strips the power-domain/pKVM modules that would otherwise
# swallow the EL2 handoff, and repacks. Flashing is gated behind --flash.
#
# Reconstructed from session 873bc7fe (see memory husky_ferros_probe_boot). This is THE
# durable copy of the procedure — keep it in sync if the mechanism changes.
#
# Prereqs: bootloader UNLOCKED (no AVB re-sign needed), device rooted (Magisk) for dmesg,
# pKVM disabled once via `fastboot oem pkvm disable` (this script reminds you).
#
# Usage:
#   tools/husky-probe-boot.sh            # build + repack image only; prints flash steps
#   tools/husky-probe-boot.sh --flash    # also: fastboot flash vendor_kernel_boot + reboot
#   tools/husky-probe-boot.sh --read     # read the ferros scan line back from dmesg/pstore
set -euo pipefail

HK=/mnt/Harbor/husky-kernel
DIST=$HK/out/shusky/dist
MKB=$HK/tools/mkbootimg
LZ4=$HK/prebuilts/kernel-build-tools/linux-x86/bin/lz4
W=/mnt/Harbor/tmp/vkb_probe
REPO=/mnt/Harbor/Code/ferros
BIN=$REPO/target/aarch64-unknown-none/release/ferros_kernel.bin
# DO NOT strip modules. The canonical kernel-tree repack (husky-kernel/repack-ferros-vkb.sh)
# injects /ferros.bin into the STOCK ramdisk and strips nothing. An earlier reconstruction of
# this script stripped the power-domain/PMIC/regulator modules — which breaks Android boot
# entirely (kernel sticks at the Google logo, cool), poisoning every handoff result. Proven
# 2026-08-21: stock modules + /ferros.bin boots, fires ferros, round-trips a result; the same
# ramdisk with those modules removed can't reach Android even with ferros absent.

if [ "${1:-}" = "--read" ]; then
    echo "== ferros scan line (dmesg) =="
    adb shell su -c "dmesg" 2>/dev/null | grep -iE ferros || echo "(no ferros lines — did it fire? is /ferros.bin armed + pKVM off?)"
    echo "== pstore =="
    adb shell su -c "cat /sys/fs/pstore/console-ramoops-0 2>/dev/null" 2>/dev/null | grep -iE ferros || true
    echo "Interpret the scan values via ferros/IRA-ENTROPY-SOURCES.md 'ramoops offset map'."
    exit 0
fi

echo "== build shim-handoff kernel =="
( cd "$REPO" && FERROS_SHIM_HANDOFF=1 cargo build -p ferros_kernel --target aarch64-unknown-none --release )
if command -v aarch64-linux-gnu-objcopy >/dev/null; then OBJ=aarch64-linux-gnu-objcopy; else OBJ=rust-objcopy; fi
"$OBJ" -O binary "$REPO/target/aarch64-unknown-none/release/ferros_kernel" "$BIN"
echo "  bin: $(stat -c%s "$BIN") bytes"

echo "== unpack vendor_kernel_boot + extract ramdisk =="
rm -rf "$W"; mkdir -p "$W"
python3 "$MKB/unpack_bootimg.py" --boot_img "$DIST/vendor_kernel_boot.img" --out "$W/unpacked" --format=mkbootimg 2>/dev/null > "$W/mkargs.txt"
mkdir -p "$W/rd"; ( cd "$W/rd" && "$LZ4" -dc "$W/unpacked/vendor_ramdisk00" | cpio -id 2>/dev/null )

echo "== inject /ferros.bin (NO module strip — stock ramdisk, matches repack-ferros-vkb.sh) =="
cp "$BIN" "$W/rd/ferros.bin"

echo "== repack ramdisk + vendor_kernel_boot =="
( cd "$W/rd" && find . | cpio -o -H newc 2>/dev/null | "$LZ4" -l -9 > "$W/unpacked/vendor_ramdisk00.new" )
mv "$W/unpacked/vendor_ramdisk00.new" "$W/unpacked/vendor_ramdisk00"
python3 "$MKB/mkbootimg.py" $(cat "$W/mkargs.txt") --vendor_boot "$W/vendor_kernel_boot.img"
echo "  image: $W/vendor_kernel_boot.img ($(stat -c%s "$W/vendor_kernel_boot.img") bytes)"

if [ "${1:-}" = "--flash" ]; then
    echo "== flashing (device must be in fastboot) =="
    adb reboot bootloader 2>/dev/null || true
    echo "  waiting for fastboot..."; fastboot getvar product 2>/dev/null || true
    echo "  NOTE: run 'fastboot oem pkvm disable' ONCE if /proc/cmdline still shows kvm-arm.mode=protected"
    fastboot flash vendor_kernel_boot "$W/vendor_kernel_boot.img"
    fastboot reboot
    echo "== after boot (fire -> warm-reset -> report -> Android), read with: $0 --read =="
else
    echo
    echo "Image built. To flash + fire:"
    echo "  adb reboot bootloader"
    echo "  fastboot oem pkvm disable      # ONCE, if kvm-arm.mode=protected still in /proc/cmdline"
    echo "  fastboot flash vendor_kernel_boot $W/vendor_kernel_boot.img"
    echo "  fastboot reboot"
    echo "  $0 --read                      # after it boots back to Android"
fi
