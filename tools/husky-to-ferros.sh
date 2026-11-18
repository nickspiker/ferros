#!/usr/bin/env bash
# Bring husky back from Android (or fastboot) to ferros with no buttons.
# Both slots carry the same signed gate (tools/husky-cold.sh), so a plain reboot lands on "Ready?" and from there on ferros (BOOT.md "A reboot from Android lands on slot a's gate again").
# No slot switch: `fastboot --set-active` strands husky in fastboot (BOOT.md, 2026-10-23), and the boot LUN attribute this used to write is a slot switch too.
# Usage: tools/husky-to-ferros.sh
set -u
# Only husky: every fastboot and adb call goes to it, and fastboot refuses a phone that is not (tools/lib/devices.sh).
. /mnt/Harbor/Code/ferros/tools/lib/devices.sh
device_pin husky || exit 1
cd "$(dirname "$0")/.."
wait_ferros() { for i in $(seq 60); do lsusb -d 1209:4665 >/dev/null 2>&1 && { echo "== ferros on USB after $((i*2)) s"; return 0; }; sleep 2; done; echo "== ferros did not enumerate in 120 s"; return 1; }
if adb devices 2>/dev/null | grep -qE "device$"; then
  adb reboot
elif fastboot devices | grep -q .; then
  timeout 30 fastboot reboot >/dev/null 2>&1
else
  echo "no husky on adb or fastboot"; exit 1
fi
wait_ferros
