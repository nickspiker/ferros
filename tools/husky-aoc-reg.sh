#!/usr/bin/env bash
# Put the sensor registry scripts into ferros's vault as `aoc.reg.0`, `aoc.reg.1`, ... in the order Android's sensor HAL loads them (SENSORS.md "The registry"): /vendor/etc/sensors/registry, then its append/, then the phone's own factory calibration from /mnt/vendor/persist/sensors/registry. The AoC finds no sensor until these are pushed (usf.rs `registry`); it parses each script itself and filters its sections on the board's CDT.
# The scripts are Google's (vendor.img) and this phone's calibration (persist): they live on Harbor, never in the repo. The order is the phone's readdir order (ext4 hash order for the vendor directories, f2fs dentry order for persist), worked out from the images on 2026-10-24.
# Usage: tools/husky-aoc-reg.sh [dir, default /mnt/Harbor/firmware/husky-aoc]
set -euo pipefail
REPO=/mnt/Harbor/Code/ferros
BRIDGE=$REPO/target/debug/ferros-bridge
D=${1:-/mnt/Harbor/firmware/husky-aoc}
FILES=(
    registry/husky_proto_1_0.reg registry/husky_common.reg registry/husky_dvt.reg registry/husky_evt.reg registry/husky_proto_1_1.reg
    registry/append/motion_state.reg registry/append/power.reg
    persist/icm45631_accel_fac_cal.reg persist/icm45631_gyro_fac_cal.reg persist/icp20100_baro_fac_cal.reg persist/fusion_mag_mag_fac_cal.reg
    persist/vd6282_rls_fac_cal.reg persist/vd6282_spectral_fac_cal.reg persist/tmd3719_prox_fac_cal.reg persist/tmd3719_als_fac_cal.reg
)
n=0
for f in "${FILES[@]}"; do
    [ -r "$D/$f" ] || { echo "== skip $f (missing)"; continue; }
    printf '== aoc.reg.%d %s (%d bytes): ' $n "$f" "$(stat -c%s "$D/$f")"
    timeout 60 "$BRIDGE" store put "aoc.reg.$n" "@$D/$f" 2>&1 | tail -1
    n=$((n + 1))
done
for k in $(seq $n $((n + 8))); do timeout 30 "$BRIDGE" store rm "aoc.reg.$k" >/dev/null 2>&1 || true; done
echo "== $n scripts"
