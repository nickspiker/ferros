#!/usr/bin/env bash
# Overnight soak of Android under the hv (HV.md, the Mahere freeze that stopped reproducing on 2026-10-18): every few minutes launch Mahere, pan, screen off and on, stop it; after any freeze wait for the watchdog reboot, keep the frozen run's pstore log with the hv's stats timeline, go on. At the deadline, put stock Android back on slot a (the boot_a backup, slot a active) so the morning starts without the hv.
# Usage: tools/husky-hv-soak.sh <stop at HH:MM (24 h)>   e.g. 06:30
set -u
cd "$(dirname "$0")/.."
STOP=${1:-06:30}; OUT=target/hv/soak; mkdir -p $OUT; BACKUP=/mnt/Harbor/tmp/husky-backup-2026-10-13/boot_a.img
log() { echo "$(date +%H:%M:%S) $*"; }
alive() { timeout 8 adb shell 'echo ok' 2>/dev/null | grep -q ok; }
past_stop() { [ "$(date +%H%M)" -ge "$(echo $STOP | tr -d :)" ] && [ "$(date +%H)" -lt 12 ]; }
n=0; freezes=0
while ! past_stop; do
  n=$((n+1))
  adb shell 'monkey -p nz.mahere.app -c android.intent.category.LAUNCHER 1' >/dev/null 2>&1
  for i in $(seq 15); do sleep 2; adb shell 'input swipe 300 1500 900 900 150' >/dev/null 2>&1; sleep 1; adb shell 'input swipe 900 900 300 1500 150' >/dev/null 2>&1; done
  adb shell 'input keyevent 26' >/dev/null 2>&1; sleep 30; adb shell 'input keyevent 26' >/dev/null 2>&1; sleep 3; adb shell 'input keyevent 82' >/dev/null 2>&1; sleep 5
  if alive; then
    adb shell 'am force-stop nz.mahere.app' >/dev/null 2>&1
    log "round $n ok, up $(adb shell uptime 2>/dev/null | grep -oE 'up [^,]+')"
    sleep 120
    continue
  fi
  freezes=$((freezes+1)); log "round $n: FROZE (#$freezes), waiting for the watchdog reboot"
  for i in $(seq 200); do adb devices 2>/dev/null | grep -qE "device$" && break; sleep 3; done
  adb devices 2>/dev/null | grep -qE "device$" || { log "phone did not come back on adb (usb: $(lsusb | grep -oE '1209:4665|18d1:[0-9a-f]+' | head -1)); stopping"; break; }
  sleep 40
  L=$OUT/freeze-$freezes-$(date +%H%M).txt
  adb shell 'su -c "getprop ro.boot.bootreason; cat /sys/fs/pstore/console-ramoops-0"' > "$L" 2>&1
  log "saved $L ($(grep -ac '' "$L") lines, $(grep -ac 'ferros hv stats' "$L") stats lines); last stats:"
  grep -a "ferros hv stats" "$L" | tail -3 | cut -c1-300
  for i in $(seq 60); do [ "$(adb shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = "1" ] && break; sleep 3; done; sleep 30
done
log "soak over: $n rounds, $freezes freezes. Restoring stock Android on slot a."
for attempt in 1 2 3; do
  if adb devices 2>/dev/null | grep -qE "device$"; then adb reboot bootloader; fi
  for i in $(seq 60); do lsusb -d 18d1:4ee0 >/dev/null 2>&1 && break; sleep 3; done
  if lsusb -d 18d1:4ee0 >/dev/null 2>&1; then
    timeout 120 fastboot flash boot_a $BACKUP 2>&1 | grep -E "OKAY|FAIL" | tail -1
    timeout 30 fastboot --set-active=a 2>&1 | tail -1; timeout 30 fastboot reboot >/dev/null 2>&1
    for i in $(seq 80); do sleep 3; adb devices 2>/dev/null | grep -qE "device$|unauthorized" && { log "stock Android back on adb"; adb shell 'su -c "dmesg | grep -m1 -E \"started at EL\""' 2>/dev/null; exit 0; }; done
  fi
  log "restore attempt $attempt did not complete (usb: $(lsusb | grep -oE '1209:4665|18d1:[0-9a-f]+' | head -1)); waiting"; sleep 120
done
log "RESTORE FAILED: slot a still has the hv; hold Vol Up + Vol Down at the Google text for fastboot, then tools/husky-restore-android.sh"
