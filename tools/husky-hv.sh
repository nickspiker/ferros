#!/usr/bin/env bash
# ferros hv on husky (HV.md): build the EL2 layer, pack it in front of the slot-a Linux Image as one boot.img v4 kernel, and (with --flash) put it on boot_a and boot it.
# Blob = [hv flat binary padded to G#18_0000][Linux Image], LZ4-legacy compressed as ABL expects; ABL loads it at G#8008_0000, so the Image lands 2 MiB aligned at G#8020_0000.
# The Linux Image defaults to the one unpacked from the 2026-10-13 boot_a backup (the kernel Android runs today). Restore: `fastboot flash boot_a /mnt/Harbor/tmp/husky-backup-2026-10-13/boot_a.img`.
# The boot gate (ferros_hv/src/gate.rs): by default the blob also carries the cold ferros image, packed past Linux's image_size (rounded up to 2 MiB) behind a 4 KiB header {"FERROSGT", u64 length}; the hv then shows "Ready?" before Linux: power (or 1 s on dev builds) = ferros, volume down = Android, volume up = hold. --no-ferros leaves it out (the hv boots Android straight away, as before: oracle captures).
# Usage: tools/husky-hv.sh [--image <Linux Image>] [--no-ferros] [--flash]
set -eu
cd "$(dirname "$0")/.."
BACKUP=/mnt/Harbor/tmp/husky-backup-2026-10-13/boot_a.img
W=target/hv; mkdir -p $W
IMAGE=""; FLASH=0; FERROS=1
while [ $# -gt 0 ]; do case $1 in --image) IMAGE=$2; shift 2;; --flash) FLASH=1; shift;; --no-ferros) FERROS=0; shift;; *) echo "unknown arg $1"; exit 2;; esac; done
cargo build -q -p ferros-mkimg
MKIMG=target/debug/ferros-mkimg
if [ -z "$IMAGE" ]; then
  [ -f $W/Image ] || $MKIMG unpack-kernel --boot $BACKUP -o $W/Image
  IMAGE=$W/Image
fi
cargo build -q -p ferros_hv --target aarch64-unknown-none-softfloat --release
aarch64-linux-gnu-objcopy -O binary target/aarch64-unknown-none-softfloat/release/ferros_hv $W/hv.bin
HV=$(stat -c %s $W/hv.bin); PAD=$((0x180000))
[ $HV -le $PAD ] || { echo "hv.bin is $HV bytes, over the G#18_0000 pad"; exit 1; }
FERROS_BIN=""
if [ $FERROS = 1 ]; then
  # The cold ferros image (link base G#8008_0000), the same one slot b boots.
  cargo build -q -p ferros_kernel --target aarch64-unknown-none --release
  aarch64-linux-gnu-objcopy -O binary target/aarch64-unknown-none/release/ferros_kernel $W/ferros.bin
  FERROS_BIN=$W/ferros.bin
fi
# The blob and the boot image in Rust (ferros-mkimg gate, 2026-10-28; Python is not allowed here): the same bytes before compression as the old Python packer and mkbootimg.py made, checked byte for byte.
# HV_CMDLINE: extra kernel command line in the boot image header (ABL appends it). Oracle captures of boot-time drivers use HV_CMDLINE=loglevel=4 so the kernel's boot log stays out of the pstore console ring the captures travel through.
$MKIMG gate --hv $W/hv.bin --image $IMAGE ${FERROS_BIN:+--ferros $FERROS_BIN} --cmdline "${HV_CMDLINE:-}" -o $W/boot_a_hv.img
echo "== image: $W/boot_a_hv.img ($(stat -c %s $W/boot_a_hv.img) bytes)"
[ $FLASH = 1 ] || { echo "not flashing (add --flash). Then: fastboot flash boot_a $W/boot_a_hv.img; fastboot --set-active=a; fastboot reboot"; exit 0; }
# Decide from the USB bus, not from `adb devices`: phones reachable over the LAN show up there too, and then a bare `adb reboot bootloader` fails.
if lsusb -d 1209:4665 >/dev/null 2>&1; then timeout 60 target/debug/ferros-bridge reboot fastboot >/dev/null 2>&1 || true
elif lsusb | grep -q "18d1:4ee[0-9a-f]"; then adb -s "$(adb devices | awk '$2=="device" && $1 !~ /[:.]/ {print $1; exit}')" reboot bootloader || true; fi
for i in $(seq 60); do lsusb -d 18d1:4ee0 >/dev/null 2>&1 && break; sleep 2; tools/usb-kick.sh --if-wedged; done
lsusb -d 18d1:4ee0 >/dev/null 2>&1 || { echo "no fastboot device"; exit 1; }
fastboot flash boot_a $W/boot_a_hv.img 2>&1 | tail -2
fastboot --set-active=a 2>&1 | tail -1
fastboot reboot >/dev/null 2>&1
echo "== booting slot a under the hv; waiting for adb (or ferros, if the gate chose it)"
for i in $(seq 80); do sleep 3; tools/usb-kick.sh --if-wedged; if lsusb -d 1209:4665 >/dev/null 2>&1; then echo "== FERROS on USB after $((i*3)) s (the gate chose ferros)"; exit 0; fi; if lsusb | grep -q "18d1:4ee[1-9a-f]"; then echo "== ANDROID on USB after $((i*3)) s"; sleep 5; adb -s "$(adb devices | awk '$2=="device" && $1 !~ /[:.]/ {print $1; exit}')" shell 'dmesg 2>/dev/null | grep -E "started at EL"; ls /sys/firmware/devicetree/base/reserved-memory/ | grep hv' 2>/dev/null; exit 0; fi; lsusb -d 18d1:4ee0 >/dev/null 2>&1 && { echo "== back in fastboot (slot a did not boot)"; exit 1; }; done
echo "== nothing on USB after 240 s"
