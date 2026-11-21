#!/usr/bin/env bash
# The ferros hv on akita (Pixel 8a), as on husky (tools/husky-hv.sh, HV.md): the hv, akita's own Android kernel and ferros packed into one boot image, the same image for either slot (BOARDS.md "akita's slots"). The hv goes on into Android only on ferros's word (ANDROID_ONCE, owner 2026-11-21: Android starts only when the attested identity chose it); every other boot is ferros. On development builds it shows the gate first: power (or 1 s untouched) = ferros, volume down = Android by way of ferros. Under Android it keeps the chords (power + volume up = ferros, both volumes = off) with FERROS_HV_CORE=1, the default here.
# Built for akita's memory (FERROS_HV_BOARD=akita: the hv's 64 MiB at G#9_0000_0000, the middle of its high bank (the top of DRAM is out of reach at the handoff on the August boot chain)) and its key rows.
# The Linux Image is slot a's Android (Android 17, CP3A.261005.005, since 2026-11-18), unpacked from Google's boot_a kept in keys/akita/images (read twice from ferros, B3SUMS), which is also the footer's template. Back to stock: `fastboot flash boot_a <that boot_a.img>`.
# Slot a's boot must carry ferros as well (`SLOT=a`): with the hv alone there (`--no-ferros`, as until 2026-11-21), every reboot of Android lands in Android again, which the owner's 2026-11-21 decision closes. A hv with no ferros packed now goes to fastboot without the word; only a capture build (FERROS_HV_ORACLE) still runs Android without it.
# pKVM must be off (the hv takes EL2): `fastboot oem pkvm disable`, which a `fastboot -w` turns back on; checked and done here before flashing.
# Usage: SLOT=a|b tools/akita-hv.sh [--no-ferros] [--flash]   (SLOT defaults to b; --flash writes boot_$SLOT whole and never switches slots: slot b was made active by hand before 2026-11-18, slot a is Android's)   (FERROS_HV_UFS=1 adds the UFS firewall; off until Android has booted under the hv)
set -eu
. /mnt/Harbor/Code/ferros/tools/lib/devices.sh
SLOT=${SLOT:-b}
case $SLOT in a|b) ;; *) echo "SLOT must be a or b"; exit 2;; esac
cd "$(dirname "$0")/.."
GOOGLE=/mnt/Harbor/Code/keys/akita/images/slot-a-CP3A.261005.005/boot_a.img
W=target/hv-akita; mkdir -p $W
FLASH=0; FERROS=1
for a in "$@"; do case $a in --flash) FLASH=1;; --no-ferros) FERROS=0;; *) echo "unknown arg $a"; exit 2;; esac; done
[ $FLASH = 0 ] || { device_pin akita || exit 1; }
S=${ANDROID_SERIAL:-}
cargo build -q -p ferros-mkimg
MKIMG=target/debug/ferros-mkimg
[ -f $W/Image-CP3A ] || $MKIMG unpack-kernel --boot $GOOGLE -o $W/Image-CP3A
FERROS_HV_BOARD=akita FERROS_HV_CORE=${FERROS_HV_CORE:-1} cargo build -q -p ferros_hv --target aarch64-unknown-none-softfloat --release
aarch64-linux-gnu-objcopy -O binary target/aarch64-unknown-none-softfloat/release/ferros_hv $W/hv.bin
# ferros as tools/akita-boot.sh builds it: the usb capsule the kernel carries on akita, then the kernel.
R=target/el0/aarch64-unknown-none/release
cargo el0 -q -p ferros-payload-usb
aarch64-linux-gnu-objcopy -O binary $R/usb $R/usb.bin
cargo build -q -p ferros-bridge
target/debug/ferros-bridge capsule $R/usb.bin target/akita-usb.capsule.vsf >/dev/null
cargo build -q -p ferros_kernel --target aarch64-unknown-none --release
aarch64-linux-gnu-objcopy -O binary target/aarch64-unknown-none/release/ferros_kernel $W/ferros.bin
$MKIMG gate --hv $W/hv.bin --image $W/Image-CP3A $([ $FERROS = 1 ] && echo --ferros $W/ferros.bin) --cmdline "${HV_CMDLINE:-}" -o $W/boot_gate.img
echo "== image: $W/boot_gate.img ($(stat -c %s $W/boot_gate.img) bytes)"
# The gate image with its own AVB footer, as the whole partition (mkimg avb-footer): bare, it would leave Google's footer at the partition's end pointing at G#FD_C000, inside this image, and ABL refuses the slot ("invalid vbmeta header", 2026-11-06).
$MKIMG avb-footer --key /mnt/Harbor/Code/keys/avb.vsf --google-boot $GOOGLE --image $W/boot_gate.img -o $W/boot_gate_part.img
b3sum $W/boot_gate_part.img 2>/dev/null || true
[ $FLASH = 1 ] || { echo "not flashing (add --flash)"; exit 0; }
# Which ferros device is the 8a: with the Pro on the same bus behind a hub (2026-11-12), "a ferros device" matched it too, and the line below rebooted whichever came first into fastboot. FERROS_PORT (the sysfs name, e.g. 1-2.4) names the 8a's socket; without it, two ferros devices on the host refuse to go on.
ferros_here() {
    if [ -n "${FERROS_PORT:-}" ]; then [ "$(cat /sys/bus/usb/devices/$FERROS_PORT/idProduct 2>/dev/null)" = 4665 ]; return; fi
    [ "$(lsusb -d 1209:4665 2>/dev/null | wc -l)" -ge 1 ]
}
if [ -z "${FERROS_PORT:-}" ] && [ "$(lsusb -d 1209:4665 2>/dev/null | wc -l)" -gt 1 ]; then echo "two ferros devices on the host: set FERROS_PORT to the 8a's socket (lsusb -t; e.g. 1-2.4)"; exit 1; fi
# To fastboot from wherever the 8a is.
if ferros_here; then timeout 60 target/debug/ferros-bridge reboot fastboot >/dev/null 2>&1 || true
elif adb devices | grep -q "^$S"; then adb -s $S reboot bootloader || true; fi
for i in $(seq 90); do command fastboot devices | grep -q "$S" && break; sleep 2; done
command fastboot devices | grep -q "$S" || { echo "no fastboot"; exit 1; }
[ "${SKIP_PKVM_CHECK:-0}" = 1 ] || if fastboot oem pkvm status 2>&1 | grep -q "pKVM is enabled"; then fastboot oem pkvm disable 2>&1 | grep -i pkvm || true; fi
fastboot -s "$S" flash boot_$SLOT $W/boot_gate_part.img 2>&1 | tail -1
fastboot -s "$S" reboot >/dev/null 2>&1
echo "== slot $SLOT under the hv (if it is the active slot): waiting for ferros (the gate's dev timer) or Android"
sleep 6
for i in $(seq 60); do
    if ferros_here; then echo "== FERROS on USB after $((i * 2 + 6)) s (the gate chose ferros)"; exit 0; fi
    if adb devices | grep -q "^$S.*device$"; then echo "== ANDROID on adb after $((i * 2 + 6)) s"; exit 0; fi
    if command fastboot devices | grep -q "$S"; then echo "== back in fastboot after $((i * 2 + 6)) s (the slot did not boot)"; exit 1; fi
    sleep 2
done
echo "== nothing on USB after 126 s"
exit 1
