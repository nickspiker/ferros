#!/usr/bin/env bash
# One marked camera open under an oracle hv build on slot a (tools/husky-hv.sh --no-ferros --flash first), then the console flushed with a panic and the hv's lines pulled from the next boot's pstore.
# Usage: tools/husky-hv-camopen.sh <out.log> [seconds open, default 10]
# CAM_INTENT_EXTRA: extra `am start` arguments, e.g. the front camera: CAM_INTENT_EXTRA="--ez android.intent.extra.USE_FRONT_CAMERA true --ei android.intent.extras.CAMERA_FACING 1"
# Waits for Android past 62 s uptime (oracle builds arm at 60 s), quiets the console to level 4 (pstore integrity), marks open and close in the same ring as the hv's lines.
set -u
OUT=$1; HOLD=${2:-10}
usbserial() { adb devices | awk '$2=="device" && $1 !~ /[:.]/ {print $1; exit}'; }
until [ -n "$(usbserial)" ]; do sleep 3; "$(dirname "$0")/usb-kick.sh" --if-wedged >/dev/null 2>&1; done
S=$(usbserial)
until adb -s $S shell 'test $(cut -d. -f1 /proc/uptime) -ge 62 && test "$(getprop sys.boot_completed)" = 1' 2>/dev/null; do sleep 3; done
adb -s $S shell "su -c 'echo 4 > /proc/sys/kernel/printk; echo \"<3>ferros-mark: camera open\" > /dev/kmsg'; input keyevent KEYCODE_WAKEUP; am start -a android.media.action.STILL_IMAGE_CAMERA ${CAM_INTENT_EXTRA:-} >/dev/null; sleep $HOLD; input keyevent KEYCODE_HOME; sleep 2; su -c 'echo \"<3>ferros-mark: closed\" > /dev/kmsg; echo c > /proc/sysrq-trigger'" >/dev/null 2>&1
sleep 10
until [ -n "$(usbserial)" ]; do sleep 3; "$(dirname "$0")/usb-kick.sh" --if-wedged >/dev/null 2>&1; done
S=$(usbserial)
until adb -s $S shell 'su -c "ls /sys/fs/pstore/console-ramoops-0"' >/dev/null 2>&1; do sleep 3; done
adb -s $S shell 'su -c "grep -aE \"ferros hv|ferros-mark\" /sys/fs/pstore/console-ramoops-0"' > "$OUT"
echo "$(wc -l < "$OUT") lines -> $OUT"
