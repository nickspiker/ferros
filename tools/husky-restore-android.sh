#!/usr/bin/env bash
# Stock Android back on slot a (no hypervisor): from fastboot, adb or ferros, flash the boot_a backup, make slot a active (fresh retries), reboot, confirm adb.
set -u
cd "$(dirname "$0")/.."
BACKUP=${1:-/mnt/Harbor/tmp/husky-backup-2026-10-13/boot_a.img}
[ -f "$BACKUP" ] || { echo "no backup at $BACKUP"; exit 1; }
if adb devices 2>/dev/null | grep -qE "device$"; then adb reboot bootloader; elif lsusb -d 1209:4665 >/dev/null 2>&1; then timeout 90 target/debug/ferros-bridge reboot fastboot >/dev/null 2>&1; fi
for i in $(seq 60); do lsusb -d 18d1:4ee0 >/dev/null 2>&1 && break; sleep 3; done
lsusb -d 18d1:4ee0 >/dev/null 2>&1 || { echo "no fastboot device"; exit 1; }
timeout 120 fastboot flash boot_a "$BACKUP" 2>&1 | grep -E "OKAY|FAIL" | tail -1
timeout 30 fastboot --set-active=a 2>&1 | tail -1
timeout 30 fastboot reboot >/dev/null 2>&1
for i in $(seq 80); do sleep 3; adb devices 2>/dev/null | grep -qE "device$|unauthorized" && { echo "== stock Android on adb after $((i*3)) s"; exit 0; }; done
echo "== no adb after 240 s"; exit 1
