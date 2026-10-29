#!/usr/bin/env bash
# Make slot b's Android boot chain a copy of slot a's (BOOT.md "Dual Boot"). Both slots carry the same signed gate (tools/husky-cold.sh writes boot and vbmeta to both), and the gate starts Android on slot a's dynamic partitions from either slot. But ABL hands Linux the *booted* slot's own init_boot, dtbo, vendor_boot, vendor_kernel_boot and pvmfw, and the Android kernel the gate carries is slot a's own (not the factory one), so those must be slot a's on both slots. 2026-10-28: slot b had the factory ones, and Android from slot b hung on missing modules (vh_mm.ko).
#
# Copies from Android with root (Magisk): each partition is dd'd a -> b only if it differs, then both are hashed (sha256) and must match. Untouched: the bootloader chain on LU1/LU2 (identical already, and a torn write there is a brick), dram_train (the bootloader's own per-slot DRAM calibration), and Android's dynamic partitions (one super, only slot a's are real).
#
# Usage:
#   tools/husky-mirror-slot-b.sh             Android must be up with root (from ferros: bridge diag android)
#   tools/husky-mirror-slot-b.sh --factory   the old behaviour, slot b from the factory zip: only for a stock slot a (Android from slot b then needs a factory kernel in the gate)
set -eu
REPO=$(cd "$(dirname "$0")/.." && pwd)
PARTS="init_boot dtbo vendor_boot vendor_kernel_boot pvmfw vbmeta_system vbmeta_vendor"

husky() {
    local s
    s=$(adb devices -l 2>/dev/null | awk '/product:husky/{print $1; exit}')
    [ -n "$s" ] && export ANDROID_SERIAL=$s
    [ -n "${ANDROID_SERIAL:-}" ]
}

if [ "${1:-}" = "--factory" ]; then
    ZIP=${2:-/mnt/Harbor/tmp/husky-factory/husky-cp1a.260505.005/image-husky-cp1a.260505.005.zip}
    W=$(mktemp -d)
    trap 'rm -rf "$W"' EXIT
    for p in $PARTS; do unzip -q -o "$ZIP" "$p.img" -d "$W" 2>/dev/null || echo "   (no $p.img in the zip; skipped)"; done
    husky && adb reboot bootloader
    for _ in $(seq 60); do fastboot devices | grep -q . && break; sleep 2; done
    for p in $PARTS; do [ -f "$W/$p.img" ] && fastboot flash "${p}_b" "$W/$p.img" 2>&1 | grep -E "Writing|FAIL" | sed "s/^/   ${p}_b: /"; done
    fastboot reboot
    exit 0
fi

husky || { echo "no Android on adb: boot it (from ferros: target/debug/ferros-bridge diag android)" >&2; exit 1; }
adb shell su -c id 2>/dev/null | grep -q "uid=0" || { echo "no root in Android" >&2; exit 1; }
for p in $PARTS; do
    out=$(adb shell su -c "'a=/dev/block/by-name/${p}_a; b=/dev/block/by-name/${p}_b; [ -e \$a ] && [ -e \$b ] || { echo missing; exit; }; [ \$(blockdev --getsize64 \$a) = \$(blockdev --getsize64 \$b) ] || { echo sizes-differ; exit; }; ha=\$(sha256sum \$a | cut -c1-16); hb=\$(sha256sum \$b | cut -c1-16); if [ \"\$ha\" = \"\$hb\" ]; then echo \"same \$ha\"; exit; fi; dd if=\$a of=\$b bs=1M 2>/dev/null; sync; hb=\$(sha256sum \$b | cut -c1-16); [ \"\$ha\" = \"\$hb\" ] && echo \"copied \$ha\" || echo \"COPY-MISMATCH a=\$ha b=\$hb\"'" | tr -d '\r')
    echo "   ${p}: $out"
    case "$out" in same*|copied*) ;; *) echo "stopping: ${p} not mirrored" >&2; exit 1 ;; esac
done
echo "== slot b's boot chain is slot a's"
