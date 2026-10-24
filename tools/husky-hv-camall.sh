#!/usr/bin/env bash
# One marked session through every camera under an oracle hv build on slot a: open the camera app (main), tap ultrawide, tele, then flip to the front camera, a ferros-mark before each, then flush with a panic and pull the hv's lines.
# Screen coordinates for the Pixel Camera app on husky at 1008 x 2244 (2026-10-22 screenshot): UW G#18C,G#59D (396,1437), T G#267,G#59D (615,1437), flip G#30C,G#693 (780,1683).
# Usage: tools/husky-hv-camall.sh <out.log> [seconds per camera, default 5] [step ...]
# A step is name:x:y, a marked tap; the default steps are ultrawide:396:1437 tele:615:1437 front:780:1683. One capture should hold one camera's bring-up: the ultrawide alone fills a 36000-line cap, and the console ring loses lines under that load.
set -u
OUT=$1; HOLD=${2:-5}; shift 2 2>/dev/null || shift $#
STEPS=("$@"); [ ${#STEPS[@]} -gt 0 ] || STEPS=(ultrawide:396:1437 tele:615:1437 front:780:1683)
usbserial() { adb devices | awk '$2=="device" && $1 !~ /[:.]/ {print $1; exit}'; }
until [ -n "$(usbserial)" ]; do sleep 3; "$(dirname "$0")/usb-kick.sh" --if-wedged >/dev/null 2>&1; done
S=$(usbserial)
until adb -s $S shell 'test $(cut -d. -f1 /proc/uptime) -ge 62 && test "$(getprop sys.boot_completed)" = 1' 2>/dev/null; do sleep 3; done
mark() { echo "su -c 'echo \"<3>ferros-mark: $1\" > /dev/kmsg';"; }
TAPS=""; for st in "${STEPS[@]}"; do IFS=: read -r n x y <<< "$st"; TAPS="$TAPS $(mark "$n") input tap $x $y; sleep $HOLD;"; done
adb -s $S shell "su -c 'echo 4 > /proc/sys/kernel/printk'; $(mark 'camera open main') input keyevent KEYCODE_WAKEUP; am start -a android.media.action.STILL_IMAGE_CAMERA >/dev/null; sleep $HOLD; $TAPS input keyevent KEYCODE_HOME; sleep 2; $(mark closed) su -c 'echo c > /proc/sysrq-trigger'" >/dev/null 2>&1
sleep 10
until [ -n "$(usbserial)" ]; do sleep 3; "$(dirname "$0")/usb-kick.sh" --if-wedged >/dev/null 2>&1; done
S=$(usbserial)
until adb -s $S shell 'su -c "ls /sys/fs/pstore/console-ramoops-0"' >/dev/null 2>&1; do sleep 3; done
adb -s $S shell 'su -c "grep -aE \"ferros hv|ferros-mark\" /sys/fs/pstore/console-ramoops-0"' > "$OUT"
echo "$(wc -l < "$OUT") lines -> $OUT"
