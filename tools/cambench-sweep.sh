#!/usr/bin/env bash
# A cambench sweep with a driver spawned by hand, one driver an open (docs/CAMERA.md "The slit runs"): a hand-spawned driver serves one stream (it stays, closed, and refuses a second open), so cambench's own sweeps (+14) only sweep the vault's driver; this one spawns the driver and cambench afresh at each step.
# Usage: tools/cambench-sweep.sh <out dir> <driver input hex> <cambench input hex> <field> <value>...
#   field: rows (+2), fll (+4), llp (+10) or cols (+16), each a u16 written little-endian into the cambench input at its place (the input is padded to reach it); values in decimal.
#   e.g. tools/cambench-sweep.sh runs 01 00030000000000000000000002080000 rows 64 48 32 16
# Env: FERROS_PORT (the device), BRIDGE (target/debug/ferros-bridge), BIN (the directory holding camera.bin and cambench.bin, flat binaries), SECS (most seconds a step waits, 120).
# Never ends a streaming camera: the client and the driver are ended only after the driver's own close line; a step whose camera has not closed stops the sweep with them left running.
# Stops at the first step whose frames stopped (the driver's "no frame") unless KEEP is set.
set -uo pipefail
cd "$(dirname "$0")/.."
OUT=$1; DIN=$2; CIN=$3; FIELD=$4; shift 4
B=${BRIDGE:-target/debug/ferros-bridge}; BIN=${BIN:?the directory with camera.bin and cambench.bin}; SECS=${SECS:-120}
case $FIELD in rows) AT=2 ;; fll) AT=4 ;; llp) AT=10 ;; cols) AT=16 ;; *) echo "no such field: $FIELD"; exit 1 ;; esac
mkdir -p "$OUT"
con() { timeout 60 "$B" diag console 2>/dev/null | grep -v -E '^(Connected|session|Sending|  |Command|Received)' >> "$1"; }
for v in "$@"; do
  # The input with the field's two bytes replaced (little-endian), padded with zero bytes to reach it.
  hex=$CIN
  while [ ${#hex} -lt $(( (AT + 2) * 2 )) ]; do hex=${hex}00; done
  le=$(printf '%04X' "$v"); le=${le:2:2}${le:0:2}
  hex=${hex:0:$((AT * 2))}$le${hex:$(( (AT + 2) * 2 ))}
  log=$OUT/$FIELD-$v.con
  : > "$log"; con /dev/null
  echo "== $FIELD $v: cambench $hex"
  timeout 60 "$B" spawn "$BIN/camera.bin" "$DIN" --name camera --serve camera --hw hsi2c1 --hw ispfe --hw csis --hw ispfe-sysreg --store cam --power camera --dma camera --irq ispfe0 --hw hsi2c3 --hw hsi2c2 --hw hsi2c4 > /dev/null 2>&1
  sleep 1
  timeout 60 "$B" spawn "$BIN/cambench.bin" "$hex" --name cambench --use camera > /dev/null 2>&1
  end=$((SECONDS + SECS))
  while [ $SECONDS -lt $end ]; do
    sleep 4; con "$log"
    grep -qE 'cambench: done' "$log" && break
  done
  sleep 2; con "$log"
  grep -oE 'RATE over.{0,160}|A#[0-9]+ slits in.{0,160}|A#[0-9]+ frames mapped, .{0,100}|no frame in A#[0-9]+ ms|link G#[0-9A-F]+: \+14 G#[0-9A-F]+' "$log"
  if [ "$(grep -c 'closed after' "$log")" -lt "$(grep -c 'camera: camera: streaming' "$log")" ]; then
    echo "STOP: the camera has not closed; nothing ended"
    exit 1
  fi
  timeout 30 "$B" diag "kill cambench" > /dev/null 2>&1
  timeout 30 "$B" diag "kill camera" > /dev/null 2>&1
  if grep -q 'camera: no frame' "$log" && [ -z "${KEEP:-}" ]; then
    echo "STOP: the frames stopped at $FIELD $v"
    exit 1
  fi
done
