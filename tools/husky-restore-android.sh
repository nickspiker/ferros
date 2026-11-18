#!/usr/bin/env bash
# Stock Android back on the booting slot (no hypervisor): from fastboot, adb or ferros, flash the boot backup onto the slot ABL boots, reboot, confirm adb.
# The default backup was in /mnt/Harbor/tmp, cleared 2026-11-17: pass a stock boot image as the first argument.
set -u
# Only husky: every fastboot and adb call goes to it, and fastboot refuses a phone that is not (tools/lib/devices.sh).
. /mnt/Harbor/Code/ferros/tools/lib/devices.sh
device_pin husky || exit 1
cd "$(dirname "$0")/.."
BACKUP=${1:-/mnt/Harbor/tmp/husky-backup-2026-10-13/boot_a.img}
[ -f "$BACKUP" ] || { echo "no backup at $BACKUP"; exit 1; }
if adb devices 2>/dev/null | grep -qE "device$"; then adb reboot bootloader; elif lsusb -d 1209:4665 >/dev/null 2>&1; then timeout 90 target/debug/ferros-bridge reboot fastboot >/dev/null 2>&1; fi
for i in $(seq 60); do lsusb -d 18d1:4ee0 >/dev/null 2>&1 && break; sleep 3; done
lsusb -d 18d1:4ee0 >/dev/null 2>&1 || { echo "no fastboot device"; exit 1; }
# The booting slot (the UFS boot LUN's), never a slot switch: `fastboot --set-active` strands husky in fastboot (BOOT.md, 2026-10-23).
SLOT=$(fastboot getvar current-slot 2>&1 | awk '/current-slot/ {print $2}')
case "$SLOT" in a|b) ;; *) echo "fastboot did not name the current slot ('$SLOT')"; exit 1;; esac
timeout 120 fastboot flash "boot_$SLOT" "$BACKUP" 2>&1 | grep -E "OKAY|FAIL" | tail -1
timeout 30 fastboot reboot >/dev/null 2>&1
for i in $(seq 80); do sleep 3; adb devices 2>/dev/null | grep -qE "device$|unauthorized" && { echo "== stock Android on adb after $((i*3)) s"; exit 0; }; done
echo "== no adb after 240 s"; exit 1
