#!/usr/bin/env bash
# Measure the husky panel with an i1 Pro through ArgyllCMS, patches driven through ferros: luminance per DBV on full white, a grey ramp at 140 nits for the EOTF, R/G/B/W for the primaries. Writes a CSV (patch, dbv, rgb, Y cd/m², x, y) the colour profile and the gamma2tanh mapping are built from (DISPLAY-DESIGN.md §6).
# Needs: the instrument readable (sudo chmod a+rw /dev/bus/usb/<bus>/<dev>, or the colord group), its white-tile calibration done once (`spotread -e -O` by hand, or the first run here prompts), the i1 face-down on the panel over the fill, a dark room.
# Usage: tools/display-calib.sh [out.csv] [--skip-cal]
set -uo pipefail
REPO=/mnt/Harbor/Code/ferros
B=$REPO/target/debug/ferros-bridge
OUT=${1:-$REPO/oracle/display/husky-hk3/calib-$(date +%Y%m%d-%H%M).csv}
run() { timeout 60 "$B" "$@" >/dev/null 2>&1; }
dbv() { local v=$1; run display dcs "f0 5a 5a / 51 $(printf '%02x %02x' $((v >> 8)) $((v & 255))) / f0 a5 a5"; }
read_yxy() {
    # One emissive reading, Yxy: "Result is Yxy: 139.52 0.3127 0.3290" (Argyll's wording varies slightly by version; take the three numbers after "Yxy:").
    timeout 60 spotread -e -N -O -x 2>&1 | grep -oE "Yxy: *[-0-9.]+ +[-0-9.]+ +[-0-9.]+" | head -1 | sed -E 's/Yxy: *//'
}
patch() { # label dbv rrggbb
    local label=$1 d=$2 rgb=$3
    dbv "$d"; run display fill "$rgb"; sleep 1.5
    local r; r=$(read_yxy)
    [ -z "$r" ] && r="? ? ?"
    echo "$label,$d,$rgb,${r// /,}" | tee -a "$OUT"
}
if [[ "${2:-}" != "--skip-cal" ]]; then
    echo "Instrument calibration: put the i1 Pro on its white tile, then press a key in this terminal."
    spotread -e -O >/dev/null 2>&1 </dev/tty || { echo "calibration failed (permissions? sudo chmod a+rw /dev/bus/usb/<bus>/<dev>)"; exit 1; }
fi
echo "Now the i1 face-down on the panel, centred. Starting in 5 s."; sleep 5
echo "patch,dbv,rgb,Y_cdm2,x,y" > "$OUT"
# 1. Luminance per DBV, full white (the driver's table says 196 = 2 nits, 1353 = 140, 3307 = 1000; HBM 4095 = 1600).
for d in 196 400 853 1353 2000 3307; do patch "white" $d ffffff; done
run display dcs "f0 5a 5a / 53 e0 / 51 0f ff / f0 a5 a5"; run display fill ffffff; sleep 1.5
echo "white-hbm,4095,ffffff,$(read_yxy | tr ' ' ',')" | tee -a "$OUT"
run display dcs "f0 5a 5a / 53 20 / 51 05 49 / f0 a5 a5"
# 2. The EOTF at 140 nits: 17 greys, code 0 to 255.
for c in 0 16 32 48 64 80 96 112 128 144 160 176 192 208 224 240 255; do h=$(printf '%02x' $c); patch "grey$c" 1353 "$h$h$h"; done
# 3. Primaries and white at 140 nits.
patch red 1353 ff0000; patch green 1353 00ff00; patch blue 1353 0000ff; patch white 1353 ffffff
run display fill 000000
echo "done: $OUT"
