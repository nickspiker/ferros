#!/usr/bin/env bash
# The ferros hv on akita (Pixel 8a), as on husky (tools/husky-hv.sh, HV.md): the hv, akita's own Android kernel and ferros packed into one boot image on Android's slot, b. The hv shows the gate before Linux: power (or 1 s on dev builds) = ferros, volume down = Android; under Android it keeps the chords (power + volume up = ferros, both volumes = off) with FERROS_HV_CORE=1, the default here.
# Built for akita's memory (FERROS_HV_BOARD=akita: the hv's 64 MiB at G#9_0000_0000, the middle of its high bank (the top of DRAM is out of reach at the handoff on the August boot chain)) and its key rows. Slot a keeps the bare ferros of tools/akita-boot.sh as the escape: if slot b does not boot, ABL falls back to it.
# The Linux Image is the one slot b's Android runs, unpacked from its backup (2026-11-06, `/mnt/Harbor/tmp/akita-boot-b-2026-11-06/`, hashes checked against the device). Restore: `fastboot flash boot_b /mnt/Harbor/tmp/akita-boot-b-2026-11-06/boot_b.img`.
# pKVM must be off (the hv takes EL2): `fastboot oem pkvm disable`, which a `fastboot -w` turns back on; checked and done here before flashing.
# Usage: tools/akita-hv.sh [--no-ferros] [--flash]   (--no-ferros: the hv alone in front of Android, no gate, as husky-hv.sh's)   (FERROS_HV_UFS=1 adds the UFS firewall; off until Android has booted under the hv)
set -eu
. /mnt/Harbor/Code/ferros/tools/lib/devices.sh
device_pin akita || exit 1
S=$ANDROID_SERIAL
cd "$(dirname "$0")/.."
BACKUP=/mnt/Harbor/tmp/akita-boot-b-2026-11-06/boot_b.img
W=target/hv-akita; mkdir -p $W
FLASH=0; FERROS=1
for a in "$@"; do case $a in --flash) FLASH=1;; --no-ferros) FERROS=0;; *) echo "unknown arg $a"; exit 2;; esac; done
cargo build -q -p ferros-mkimg
MKIMG=target/debug/ferros-mkimg
[ -f $W/Image ] || $MKIMG unpack-kernel --boot $BACKUP -o $W/Image
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
$MKIMG gate --hv $W/hv.bin --image $W/Image $([ $FERROS = 1 ] && echo --ferros $W/ferros.bin) --cmdline "${HV_CMDLINE:-}" -o $W/boot_b_hv.img
echo "== image: $W/boot_b_hv.img ($(stat -c %s $W/boot_b_hv.img) bytes)"
[ $FLASH = 1 ] || { echo "not flashing (add --flash)"; exit 0; }
# To fastboot from wherever the 8a is.
if lsusb -d 1209:4665 >/dev/null 2>&1; then timeout 60 target/debug/ferros-bridge reboot fastboot >/dev/null 2>&1 || true
elif adb devices | grep -q "^$S"; then adb -s $S reboot bootloader || true; fi
for i in $(seq 90); do command fastboot devices | grep -q "$S" && break; sleep 2; done
command fastboot devices | grep -q "$S" || { echo "no fastboot"; exit 1; }
if fastboot oem pkvm status 2>&1 | grep -q "pKVM is enabled"; then fastboot oem pkvm disable 2>&1 | grep -i pkvm || true; fi
fastboot flash boot_b $W/boot_b_hv.img 2>&1 | tail -1
fastboot --set-active=b 2>&1 | tail -1
fastboot reboot >/dev/null 2>&1
echo "== slot b under the hv: waiting for ferros (the gate's dev timer) or Android"
sleep 6
for i in $(seq 60); do
    if lsusb -d 1209:4665 >/dev/null 2>&1; then echo "== FERROS on USB after $((i * 2 + 6)) s (the gate chose ferros)"; exit 0; fi
    if adb devices | grep -q "^$S.*device$"; then echo "== ANDROID on adb after $((i * 2 + 6)) s"; exit 0; fi
    if command fastboot devices | grep -q "$S"; then echo "== back in fastboot after $((i * 2 + 6)) s (slot b did not boot)"; exit 1; fi
    sleep 2
done
echo "== nothing on USB after 126 s"
exit 1
