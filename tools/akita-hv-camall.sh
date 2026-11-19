#!/usr/bin/env bash
# One marked camera session on akita (Pixel 8a) under an oracle hv build on slot b (tools/akita-hv.sh --no-ferros with FERROS_HV_ORACLE_*), as tools/husky-hv-camall.sh does on husky: the console quieted to level 4, the Pixel Camera app opened on the main camera, marked taps, home, then a panic to keep the console zone, and the hv's lines pulled from the next boot's pstore.
# Screen positions on akita's Pixel Camera app at 1080 x 2400 (screenshots 2026-11-15): .5 (the ultrawide) (414, 1513), 1x (540, 1513), flip (859, 1796); the app reopens the camera used last, so a session that flips to the front flips back before it closes.
# Usage: tools/akita-hv-camall.sh <out.log> [seconds per step, default 5] [step ...]; a step is name:x:y (a marked tap), e.g. ultrawide:414:1513 front:859:1796 rear:859:1796.
set -u
S=45221JEKB06286
OUT=$1; HOLD=${2:-5}; shift 2 2>/dev/null || shift $#
STEPS=("$@")
A() { adb -s $S "$@"; }
up() { [ "$(A shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = 1 ]; }
# The host's adb server often misses the 8a after it comes back on the USB 3 port (2026-11-15: lsusb shows 18d1:4ee7, `adb devices` lists nothing until the server restarts).
waitup() {
    local k=0
    until up; do
        sleep 3; k=$((k + 1))
        if [ $((k % 10)) = 0 ] && lsusb -d 18d1:4ee7 >/dev/null 2>&1 && ! adb devices | grep -q "^$S"; then adb kill-server; adb start-server >/dev/null 2>&1; fi
    done
}
waitup
# Armed 60 s after the reset (FERROS_HV_ORACLE_ARM_AFTER_MS), the hv's counter some 20 s ahead of Android's: past 62 s of Android's uptime the oracle logs.
until A shell 'test $(cut -d. -f1 /proc/uptime) -ge 62' 2>/dev/null; do sleep 3; done
mark() { echo "su -c 'echo \"<3>ferros-mark: $1\" > /dev/kmsg';"; }
TAPS=""; for st in "${STEPS[@]}"; do IFS=: read -r n x y <<< "$st"; TAPS="$TAPS $(mark "$n") input tap $x $y; sleep $HOLD;"; done
A shell "su -c 'echo 4 > /proc/sys/kernel/printk'; input keyevent KEYCODE_WAKEUP; sleep 1; input swipe 540 2100 540 700 300; sleep 1; $(mark 'camera open main') am start -a android.media.action.STILL_IMAGE_CAMERA >/dev/null; sleep $HOLD; $TAPS input keyevent KEYCODE_HOME; sleep 2; $(mark closed) su -c 'echo c > /proc/sysrq-trigger'" >/dev/null 2>&1
sleep 10
waitup
A shell 'su -c "grep -aE \"ferros hv|ferros-mark\" /sys/fs/pstore/console-ramoops-0"' > "$OUT"
echo "$(wc -l < "$OUT") lines -> $OUT"
