#!/usr/bin/env bash
# akita's glass, measured on the glass itself (display op 52, payloads/display; docs/BOARDS.md "akita's glass, measured"): the shell's publishing is held and every phase shows one drawing until `off`.
#   corner <g>            the corners at glass radius g px (fractions allowed): grey inside, orange where the mask would cut. Orange on the glass: the mask cuts glass there (g too big).
#   cutout <cx> <cy> <r>  black, a white ring 16 px wide from radius r round (cx, cy) and a red crosshair beyond it. The ring's inner edge touching the camera hole all round, with no black between: the hole is r about that centre.
#   off                   back to the shell.
# The values in pixels of the 1080 x 2400 panel; sent in 16ths. The display's console line says what it drew (diag console).
set -euo pipefail
cd /mnt/Harbor/Code/ferros
R=target/el0/aarch64-unknown-none/release
B=target/debug/ferros-bridge
[ -f $R/capture.bin ] || { cargo el0 -q -p ferros-payload-capture && aarch64-linux-gnu-objcopy -O binary $R/capture $R/capture.bin; }
# A pixel value as a u16 in 16ths, little-endian hex (awk rounds; no Python).
u16() { awk -v v="$1" 'BEGIN { n = int(v * 16 + 0.5); printf "%02X%02X", n % 256, int(n / 256) }'; }
case "${1:-}" in
    corner) msg=3401$(u16 "$2");;
    cutout) msg=3402$(u16 "$2")$(u16 "$3")$(u16 "$4");;
    off) msg=3400;;
    *) sed -n '2,6p' "$0"; exit 1;;
esac
timeout 60 "$B" spawn $R/capture.bin "$msg" --use display 2>&1 | grep -a "^proc: [0-9A-F]" | head -1
sleep 1
timeout 30 "$B" diag console 2>&1 | grep -a "op 52" | tail -1
