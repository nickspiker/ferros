#!/usr/bin/env bash
# One full hv reproducer cycle (HV.md, the Mahere freeze): flash the current hv build to slot a, wait for Android to finish booting (so ABL will retry slot a after the freeze instead of counting it down), launch Mahere over adb and pan, wait for the watchdog reboot, pull the frozen run's pstore console log and print the hv's timeline from it. Env knobs pass through to the build.
set -u
cd "$(dirname "$0")/.."
S=${HV_CYCLE_OUT:-target/hv/cycle}; mkdir -p "$S"; LOG="$S/console-$(date +%H%M%S).txt"
timeout 900 tools/husky-hv.sh --flash 2>&1 | grep -E "^== |no fastboot|FAIL" | tail -2
if [ "${HV_CYCLE_EARLY:-0}" = 1 ]; then
  # The old harness's timing: launch as soon as adb answers, before Android has finished booting (2026-10-18: testing whether that is what made the freezes).
  for i in $(seq 60); do adb devices 2>/dev/null | grep -qE "device$" && break; sleep 2; done; sleep 5; echo "== early launch, boot_completed=$(adb shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')"
else
  for i in $(seq 100); do [ "$(adb shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = "1" ] && { echo "== boot_completed after $((i*3)) s"; break; }; sleep 3; done
  sleep 30
fi
adb shell 'monkey -p nz.mahere.app -c android.intent.category.LAUNCHER 1' >/dev/null 2>&1; echo "== launched $(date +%T)"
froze=0
for i in $(seq 36); do sleep 2; adb shell 'input swipe 300 1500 900 900 150' >/dev/null 2>&1; sleep 1; adb shell 'input swipe 900 900 300 1500 150' >/dev/null 2>&1; timeout 8 adb shell 'echo ok' 2>/dev/null | grep -q ok || { echo "== FROZE at t+$((i*4)) s"; froze=1; break; }; done
[ $froze = 0 ] && { echo "== ALIVE after 150 s of panning"; exit 0; }
for i in $(seq 160); do adb devices 2>/dev/null | grep -qE "device$" && { echo "== adb back after $((i*3)) s"; break; }; [ $((i%20)) = 0 ] && echo "   t+$((i*3)) s: usb=$(lsusb | grep -oE '1209:4665|18d1:[0-9a-f]+' | head -1)"; sleep 3; done
adb devices 2>/dev/null | grep -qE "device$" || { echo "== phone did not come back on adb"; exit 1; }
sleep 25
adb shell 'su -c "getprop ro.boot.bootreason; cat /sys/fs/pstore/console-ramoops-0"' > "$LOG" 2>&1
echo "== log $LOG: $(grep -ac '' "$LOG") lines, bootreason $(head -1 "$LOG")"
grep -a "ferros hv: EL2 up" "$LOG" | tail -1 | cut -c28-100
echo "== stats lines: $(grep -ac 'ferros hv stats' "$LOG"); last 6 (time, s2mpu):"
grep -a "ferros hv stats" "$LOG" | tail -6 | grep -oE "t=[0-9A-F]+ ms|s2mpu.*" | cut -c1-110
echo "== hv other lines:"; grep -a "ferros hv" "$LOG" | grep -av "stats\|up via hv\|EL2 up" | cut -c1-140 | head -6
echo "== kernel: stall / lockup / panic:"; grep -anE "RT throttling|hardlockup warning|Kernel panic|not released during" "$LOG" | head -4 | cut -c1-120
