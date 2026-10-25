#!/usr/bin/env bash
# Put the Edge TPU's firmware into ferros's vault as `tpu.fw`, for `diag tpu load` (TPU.md). The file is /vendor/firmware/google/edgetpu-rio.fw from the factory image (Google's signed binary: Harbor only, never the repo); 1.1 MB, one piece.
# Usage: tools/husky-tpu-fw.sh [edgetpu-rio.fw]
set -euo pipefail
REPO=/mnt/Harbor/Code/ferros
SRC=${1:-/mnt/Harbor/firmware/husky-tpu/edgetpu-rio.fw}
[ -r "$SRC" ] || { echo "no $SRC (extract it from the factory image's vendor.img)" >&2; exit 1; }
timeout 180 "$REPO/target/debug/ferros-bridge" store put tpu.fw "@$SRC" 2>&1 | tail -1
echo "== tpu.fw: $(stat -c%s "$SRC") bytes, BLAKE3 $(b3sum --no-names "$SRC" | cut -c1-16)…"
