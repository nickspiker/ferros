. /mnt/Harbor/Code/ferros/tools/lib/profile.sh
#!/usr/bin/env bash
# ferros hv on husky (HV.md): build the EL2 layer, pack it in front of the slot-a Linux Image as one boot.img v4 kernel, and (with --flash) hand it to tools/husky-cold.sh to sign, keep and flash.
# Blob = [hv flat binary padded to G#18_0000][Linux Image], LZ4-legacy compressed as ABL expects; ABL loads it at G#8008_0000, so the Image lands 2 MiB aligned at G#8020_0000.
# The Linux Image defaults to the one unpacked from the 2026-10-13 boot_a backup (the kernel Android runs today).
# The boot gate (hv/src/gate.rs): by default the blob also carries the cold ferros image, packed past Linux's image_size (rounded up to 2 MiB) behind a 4 KiB header {"FERROSGT", u64 length}; the hv then shows "Ready?" before Linux: power (or 1 s on dev builds) = ferros, volume down = Android, volume up = hold. --no-ferros leaves it out (the hv boots Android straight away, as before: oracle captures).
# Usage: tools/husky-hv.sh [--image <Linux Image>] [--no-ferros] [--flash]
set -eu
# Only husky: every fastboot and adb call goes to it, and fastboot refuses a phone that is not (tools/lib/devices.sh).
. /mnt/Harbor/Code/ferros/tools/lib/devices.sh
device_pin husky || exit 1
cd "$(dirname "$0")/.."
BACKUP=/mnt/Harbor/tmp/husky-backup-2026-10-13/boot_a.img
W=target/hv; mkdir -p $W
IMAGE=""; FLASH=0; FERROS=1
while [ $# -gt 0 ]; do case $1 in --image) IMAGE=$2; shift 2;; --flash) FLASH=1; shift;; --no-ferros) FERROS=0; shift;; *) echo "unknown arg $1"; exit 2;; esac; done
cargo build -q -p ferros-mkimg
MKIMG=target/debug/ferros-mkimg
if [ -z "$IMAGE" ]; then
  # target/hv/Image is the only copy left since /mnt/Harbor/tmp was cleared (2026-11-17); the backup it came from is gone.
  [ -f $W/Image ] || [ -f $BACKUP ] || { echo "no Linux Image: neither $W/Image nor $BACKUP (pass --image)"; exit 1; }
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
  cargo build -q -p ferros_kernel --target aarch64-unknown-none --release "${PROFILE_FLAGS[@]}"
  aarch64-linux-gnu-objcopy -O binary target/aarch64-unknown-none/release/ferros_kernel $W/ferros.bin
  FERROS_BIN=$W/ferros.bin
fi
# The blob and the boot image in Rust (ferros-mkimg gate, 2026-10-28; Python is not allowed here): the same bytes before compression as the old Python packer and mkbootimg.py made, checked byte for byte.
# HV_CMDLINE: extra kernel command line in the boot image header (ABL appends it). Oracle captures of boot-time drivers use HV_CMDLINE=loglevel=4 so the kernel's boot log stays out of the pstore console ring the captures travel through.
$MKIMG gate --hv $W/hv.bin --image $IMAGE ${FERROS_BIN:+--ferros $FERROS_BIN} --cmdline "${HV_CMDLINE:-}" -o $W/boot_a_hv.img
echo "== image: $W/boot_a_hv.img ($(stat -c %s $W/boot_a_hv.img) bytes)"
[ $FLASH = 1 ] || { echo "not flashing (add --flash, or: tools/husky-cold.sh $PWD/$W/boot_a_hv.img)"; exit 0; }
# One flash path for husky: tools/husky-cold.sh signs the gate, keeps it in keys/husky/images, and flashes the booting slot, then the other once ferros has booted from it (no slot switch: BOOT.md, 2026-10-23).
# A gate built --no-ferros boots Android, so the other slot keeps its build (FERROS_BOTH_AT_ONCE=1 writes both).
exec tools/husky-cold.sh "$PWD/$W/boot_a_hv.img"
