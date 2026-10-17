#!/usr/bin/env bash
# Bring husky back from Android to ferros (slot b) with no buttons.
# Preferred path: Android with root — write the UFS Boot LU attribute (2 = Boot LU B) through the Pixel kernel's sysfs node and reboot; the bootloader boots slot b and re-syncs devinfo to it by itself (BOOT.md, "How the bootloader really picks the slot").
# Fallback: fastboot (`adb reboot bootloader`, `--set-active=b`, reboot), which does the same switch from inside the bootloader.
# Usage: tools/husky-to-ferros.sh [--fastboot]
set -u
cd "$(dirname "$0")/.."
wait_ferros() { for i in $(seq 60); do lsusb -d 1209:4665 >/dev/null 2>&1 && { echo "== ferros on USB after $((i*2)) s"; return 0; }; sleep 2; done; echo "== ferros did not enumerate in 120 s"; return 1; }
via_fastboot() {
  if adb devices 2>/dev/null | grep -qE "device$"; then adb reboot bootloader; fi
  for i in $(seq 45); do lsusb -d 18d1:4ee0 >/dev/null 2>&1 && break; sleep 2; done
  lsusb -d 18d1:4ee0 >/dev/null 2>&1 || { echo "no fastboot device"; exit 1; }
  timeout 30 fastboot --set-active=b 2>&1 | tail -1
  timeout 30 fastboot reboot >/dev/null 2>&1
  wait_ferros
}
if [ "${1:-}" = "--fastboot" ] || ! adb devices 2>/dev/null | grep -qE "device$"; then via_fastboot; exit $?; fi
NODE=$(adb shell 'su -c "ls /sys/bus/platform/devices/*.ufs/pixel/boot_lun_enabled 2>/dev/null"' | tr -d '\r' | head -1)
if [ -z "$NODE" ]; then echo "== no boot_lun_enabled node (no root?), using fastboot"; via_fastboot; exit $?; fi
before=$(adb shell "su -c 'cat $NODE'" | tr -d '\r')
adb shell "su -c 'echo 2 > $NODE'" >/dev/null 2>&1
after=$(adb shell "su -c 'cat $NODE'" | tr -d '\r')
echo "== bBootLunEn $before -> $after"
[ "$after" = "2" ] || { echo "== attribute write did not take, using fastboot"; via_fastboot; exit $?; }
adb reboot
wait_ferros
