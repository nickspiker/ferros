#!/usr/bin/env bash
# Log the rear spectral sensor (VD6282, the AoC's "VD6282 Spectral Sensor") to CSV for characterising its channels, e.g. one laser line at a time.
# Subscribes the sensor (USF type 12) and polls the newest sample over the bridge (~4 per second), writing each new one once: host time, the AoC's timestamp, then the sample's 16 floats as the sensor sends them (channel counts, per-channel gains, timing, the ADC ceiling, rear lux, ...; SENSORS.md "Spectral").
# Usage: tools/vd6282-log.sh [out.csv, default target/vd6282/<time>.csv] — Ctrl-C to stop.
set -uo pipefail
REPO=/mnt/Harbor/Code/ferros
B=$REPO/target/debug/ferros-bridge
OUT=${1:-$REPO/target/vd6282/$(date +%Y%m%d-%H%M%S).csv}
mkdir -p "$(dirname "$OUT")"
timeout 30 "$B" diag "usf subh 18 200 1" 2>&1 | grep -a "usf: subscribe"
echo "host_time,aoc_ts,v0,v1,v2,v3,v4,v5,v6,v7,v8,v9,v10,v11,v12,v13,v14,v15" > "$OUT"
echo "== logging to $OUT"
last=""
while :; do
    line=$(timeout 10 "$B" diag "usf ev" 2>/dev/null | grep -a -A1 "^type 12:" | tail -1)
    ts=$(echo "$line" | grep -o "ts G#[0-9A-F]*" | cut -c6-)
    if [ -n "$ts" ] && [ "$ts" != "$last" ]; then
        last=$ts
        vals=$(echo "$line" | grep -oE " -?[0-9]+\.[0-9]+ \(" | tr -d ' (' | paste -sd,)
        row="$(date +%s.%N | cut -c1-14),$ts,$vals"
        echo "$row" >> "$OUT"
        echo "$row" | cut -d, -f1,3-8 | tr ',' '\t'
    fi
    sleep 0.2
done
