#!/usr/bin/env bash
# Stage a ferros-flash release (docs/FLASHER.md "Distribution"): the executables, the board manifests and the boot images, each under its BLAKE3, a release document signed with the ferros key naming them all, and the two one-line installers with the executables' SHA-256 written in.
# The host is the owner's choice and not decided, so nothing is uploaded unless asked: FERROS_FLASH_BASE is the URL the installers and the tool fetch from, and FERROS_FLASH_UPLOAD=r2:<bucket>/<path> uploads the staged folder with wrangler (Photon's route to brobdingnagian.holdmyoscilloscope.com).
# Boot images: FERROS_FLASH_BOOT_<BOARD>=<whole signed boot partition image> adds one; the owner's rule (2026-11-24) is that only production builds are hosted, with every Android path compiled out, so the channel is `production` only when every image given is one, else `development`.
# FERROS_FLASH_COMMIT names the commit when building outside a checkout (a scratch export).
# Usage: FERROS_FLASH_BASE=https://<host>/ferros [FERROS_FLASH_CHANNEL=development] [FERROS_FLASH_BOOT_AKITA=img] tools/flash/publish.sh
set -euo pipefail
cd "$(dirname "$0")/../.."
BASE=${FERROS_FLASH_BASE:?the URL the installers fetch from, e.g. https://ferros.org/flash}
CHANNEL=${FERROS_FLASH_CHANNEL:-development}
KEY=${FERROS_KEY:-/mnt/Harbor/Code/keys/ferros.key.vsf}
OUT=${OUT:-target/flash-publish}
T=${CARGO_TARGET_DIR:-target}
rm -rf "$OUT" && mkdir -p "$OUT"

cargo build -q --release -p ferros-flash --target x86_64-pc-windows-gnu
cargo build -q --release -p ferros-flash --target x86_64-unknown-linux-gnu
cargo build -q -p ferros-flash --features publish
FF=$T/debug/ferros-flash
cp "$T/x86_64-pc-windows-gnu/release/ferros-flash.exe" "$OUT/ferros-flash-windows-x86_64.exe"
cp "$T/x86_64-unknown-linux-gnu/release/ferros-flash" "$OUT/ferros-flash-linux-x86_64"
$FF board akita "$OUT/akita.board.vsf"
$FF board husky "$OUT/husky.board.vsf"

FILES=(--file tool - windows-x86_64 ferros-flash.exe "$OUT/ferros-flash-windows-x86_64.exe"
       --file tool - linux-x86_64 ferros-flash "$OUT/ferros-flash-linux-x86_64"
       --file board akita - akita "$OUT/akita.board.vsf"
       --file board husky - husky "$OUT/husky.board.vsf")
for b in akita husky; do
    v=FERROS_FLASH_BOOT_${b^^}
    [ -n "${!v:-}" ] && FILES+=(--file boot "$b" - boot "${!v}")
done
$FF release make --channel "$CHANNEL" --commit "${FERROS_FLASH_COMMIT:-$(git rev-parse HEAD)}" --key "$KEY" --out "$OUT/release.vsf" "${FILES[@]}"
$FF release verify "$OUT/release.vsf"

W=$(sha256sum "$OUT/ferros-flash-windows-x86_64.exe" | cut -c1-64 | tr a-f A-F)
L=$(sha256sum "$OUT/ferros-flash-linux-x86_64" | cut -c1-64)
sed -e "s|__BASE__|$BASE|g" -e "s|__SHA256_WINDOWS_X86_64__|$W|" tools/flash/install/install.ps1 > "$OUT/install.ps1"
sed -e "s|__BASE__|$BASE|g" -e "s|__SHA256_LINUX_X86_64__|$L|" -e "s|__SHA256_MACOS_ARM64__||" tools/flash/install/install.sh > "$OUT/install.sh"

echo "== staged in $OUT ($CHANNEL)"
echo "   Windows: powershell -ExecutionPolicy Bypass -c \"irm $BASE/install.ps1 | iex\""
echo "   Linux:   curl -sSfL $BASE/install.sh | sh"
case "${FERROS_FLASH_UPLOAD:-}" in
    "") echo "   not uploaded (FERROS_FLASH_UPLOAD=r2:<bucket>/<path> to upload)" ;;
    r2:*)
        dest=${FERROS_FLASH_UPLOAD#r2:}
        for f in "$OUT"/*; do
            wrangler r2 object put "$dest/$(basename "$f")" --file "$f" --remote
        done
        ;;
    *) echo "FERROS_FLASH_UPLOAD: only r2:<bucket>/<path> is known"; exit 1 ;;
esac
