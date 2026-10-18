#!/usr/bin/env bash
# The Mahere reproducer for the hv (HV.md): flash the current hv build to slot a, boot, launch Mahere over adb, and say whether Android is still answering 40 s later. Env knobs pass through to the build (FERROS_HV_NO_WINDOW, FERROS_HV_NO_ATA, FERROS_HV_ACTLR_KEEP).
set -u
cd "$(dirname "$0")/.."
timeout 900 tools/husky-hv.sh --flash 2>&1 | grep -E "^== |no fastboot|FAIL|error" | tail -3
adb devices 2>/dev/null | grep -qE "device$" || { echo "RESULT: no adb after flash"; exit 1; }
sleep 8
if [ "${HV_TEST_NO_IDLE:-0}" = 1 ]; then
  adb shell 'su -c "for c in /sys/devices/system/cpu/cpu[0-8]; do for st in $c/cpuidle/state*; do [ \"$(cat $st/name)\" != WFI ] && echo 1 > $st/disable; done; done; echo idle-off"' 2>/dev/null | tail -1
  sleep 20
fi
adb shell 'monkey -p nz.mahere.app -c android.intent.category.LAUNCHER 1' >/dev/null 2>&1
t0=$(date +%s)
for i in $(seq 8); do sleep 5; if ! timeout 8 adb shell 'echo ok' 2>/dev/null | grep -q ok; then echo "RESULT: FROZE $(( $(date +%s)-t0 )) s after launch"; exit 2; fi; done
echo "RESULT: ALIVE 40 s after launch ($(adb shell uptime 2>/dev/null | grep -oE 'up [0-9]+ min'))"
