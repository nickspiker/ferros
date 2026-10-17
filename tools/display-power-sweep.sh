#!/usr/bin/env bash
# Display power profile under ferros: for each panel state and scene, the system's draw from the battery (charger buck off during the sample, `bridge display power … battery`) and the TE rate that proves the state. Prints a markdown table; DISPLAY-POWER.md keeps the results.
# Usage: tools/display-power-sweep.sh [samples=8] [gap sixteenths=4]
set -u
cd "$(dirname "$0")/.."
B=target/debug/ferros-bridge
N=${1:-8}
GAP=${2:-4}

run() { timeout 120 $B "$@" 2>/dev/null; }
power() { run display power "$N" "$GAP" battery | grep -oE "MEAN over [0-9]+: -?[0-9]+ mA at [0-9]+ mV = -?[0-9]+ mW" | sed -E 's/MEAN over [0-9]+: //'; }
te() { run display terate | grep -oE "TE: [0-9]+ Hz on the pin \([0-9]+ us high\)" | sed -E 's/TE: ([0-9]+) Hz on the pin \(([0-9]+) us high\)/\1 Hz, \2 us/' | grep . || echo "?"; }
fill() { run display fill "$1" >/dev/null; }
bright() { run display dcs "f0 5a 5a / 51 $1 / f0 a5 a5" >/dev/null; }
settle() { sleep 1; }

row() { # label
  local label=$1; settle
  printf "| %s | %s | %s |\n" "$label" "$(te)" "$(power)"
}

echo "| state | TE Hz | mean draw |"
echo "|---|---|---|"

# WQHD 120 Hz, 10 bpc blend, 1346 Mbps: scenes at the default 140 nits (DBV 1353).
run display modeset wqhd 10 1346 >/dev/null
bright "05 49"
fill 000000; row "WQHD 120, 10 bpc, black, 140 nits"
fill 808080; row "WQHD 120, 10 bpc, grey 50%, 140 nits"
fill ffffff; row "WQHD 120, 10 bpc, white, 140 nits"
bright "00 c4"; row "WQHD 120, 10 bpc, white, 2 nits"
bright "0c eb"; row "WQHD 120, 10 bpc, white, 1000 nits"
run display dcs "f0 5a 5a / 53 e0 / 51 0f ff / f0 a5 a5" >/dev/null; row "WQHD 120, 10 bpc, white, HBM 1600 nits"
run display dcs "f0 5a 5a / 53 20 / 51 05 49 / f0 a5 a5" >/dev/null; settle

# 8 bpc blend, same scene, for the DQE's cost.
run display modeset wqhd 8 >/dev/null
fill ffffff; row "WQHD 120, 8 bpc, white, 140 nits"
fill 000000; row "WQHD 120, 8 bpc, black, 140 nits"

# 1368 Mbps link.
run display modeset wqhd 1368 >/dev/null
fill 000000; row "WQHD 120, 8 bpc, 1368 Mbps, black, 140 nits"
run display modeset wqhd 1346 >/dev/null

# 60 Hz (HS 60 via the panel's refresh set), then back.
run display mode 60 >/dev/null
fill 000000; row "WQHD 60, black, 140 nits"
fill ffffff; row "WQHD 60, white, 140 nits"
run display mode 120 >/dev/null

# Idle floors at WQHD (auto rate; TE changeable so the pin shows the floor).
for f in 30 10 1; do
  run display idle $f changeable >/dev/null
  fill 000000; row "WQHD idle floor $f Hz, black, 140 nits"
  fill ffffff; row "WQHD idle floor $f Hz, white, 140 nits"
done
run display idle 0 >/dev/null

# FHD 120.
run display modeset fhd 10 >/dev/null
fill 000000; row "FHD 120, 10 bpc, black, 140 nits"
fill ffffff; row "FHD 120, 10 bpc, white, 140 nits"
run display modeset wqhd 10 >/dev/null
fill 000000; row "WQHD 120, 10 bpc, black, 140 nits (end check)"
