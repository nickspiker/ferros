#!/usr/bin/env bash
# Put stock Android back on slot a (no hypervisor): wait for the phone in any mode, get to fastboot, flash the boot_a backup, make slot a active (fresh retries), reboot, confirm adb. The backup is the slot-a boot image saved before any hv work (BOOT.md).
set -u
cd "$(dirname "$0")/.."
BACKUP=${1:-/mnt/Harbor/tmp/husky-backup-2026-10-13/boot_a.img}
[ -f "$BACKUP" ] || { echo "no backup at $BACKUP"; exit 1; }
echo "== waiting for the phone ($(date +%T))"
while :; do
  if lsusb -d 18d1:4ee0 >/dev/null 2>&1; then echo "fastboot at $(date +%T)"; break; fi
  if adb devices 2>/dev/null | grep -qE "device$"; then echo "android at $(date +%T), rebooting to bootloader"; adb reboot bootloader; sleep 15; continue; fi
  if lsusb -d 1209:4665 >/dev/null 2>&1; then echo "ferros at $(date +%T), rebooting to bootloader"; timeout 90 target/debug/ferros-bridge reboot fastboot >/dev/null 2>&1; sleep 30; continue; fi
  sleep 5
done
sha256sum "$BACKUP" | cut -c1-16
timeout 120 fastboot flash boot_a "$BACKUP" 2>&1 | grep -E "OKAY|FAIL" | tail -1
timeout 30 fastboot --set-active=a 2>&1 | tail -1
timeout 30 fastboot reboot >/dev/null 2>&1
for i in $(seq 80); do sleep 3; adb devices 2>/dev/null | grep -qE "device$|unauthorized" && { echo "== ANDROID (stock boot_a) on adb after $((i*3)) s"; adb shell 'getprop ro.boot.slot_suffix' 2>/dev/null; exit 0; }; done
echo "== no adb after 240 s"; exit 1
