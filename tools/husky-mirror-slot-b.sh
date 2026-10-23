#!/usr/bin/env bash
# Make slot b's boot chain identical to slot a's (BOOT.md "Dual Boot": both slots carry the same gate image, and Android lives on slot a's dynamic partitions — but ABL still loads the booted slot's own vendor_boot, init_boot, dtbo, vendor_kernel_boot and pvmfw; slot b's were a stray stock build, so Android chosen from slot b's gate met ramdisks from another build and hung on the logo, 2026-10-23). This flashes those from the factory image of the build slot a runs. boot_b is left alone: tools/husky-cold.sh owns it.
# Usage: tools/husky-mirror-slot-b.sh [factory image zip]   (the phone in fastboot, or on ferros/Android USB — it is taken to fastboot and rebooted after)
set -eu
REPO=$(cd "$(dirname "$0")/.." && pwd)
ZIP=${1:-/mnt/Harbor/tmp/husky-factory/husky-cp1a.260505.005/image-husky-cp1a.260505.005.zip}
BRIDGE=$REPO/target/debug/ferros-bridge
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT
PARTS="dtbo init_boot vendor_boot vendor_kernel_boot pvmfw vbmeta vbmeta_system vbmeta_vendor"
echo "== unpacking $(basename "$ZIP")"
for p in $PARTS; do unzip -q -o "$ZIP" "$p.img" -d "$W" 2>/dev/null || echo "   (no $p.img in the zip; skipped)"; done
to_fastboot() {
    if fastboot devices | grep -q .; then return; fi
    if adb get-state >/dev/null 2>&1; then adb reboot bootloader; else timeout 30 "$BRIDGE" reboot fastboot >/dev/null 2>&1 || true; fi
    for _ in $(seq 90); do fastboot devices | grep -q . && return; sleep 2; "$REPO/tools/usb-kick.sh" --if-wedged >/dev/null 2>&1 || true; done
    echo "no fastboot" >&2; exit 1
}
to_fastboot
for p in $PARTS; do
    [ -f "$W/$p.img" ] || continue
    fastboot flash "${p}_b" "$W/$p.img" 2>&1 | grep -E "Writing|FAIL" | sed "s/^/   ${p}_b: /"
done
# Slot b stays the active slot; the gate on it asks as before.
fastboot --set-active="$(fastboot getvar current-slot 2>&1 | awk '/current-slot/ {print $2}')" >/dev/null 2>&1 || true
fastboot reboot
echo "== slot b mirrored; rebooting (Ready? -> ferros in a second; volume down there = Android, from either slot now)"
