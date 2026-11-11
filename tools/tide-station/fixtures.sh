#!/bin/bash
# The tide engine's station fixtures (ferros_tide/stations/): four NOAA CO-OPS stations (US public domain), semidiurnal to strongly mixed, and their catalog.
# Usage: tools/tide-station/fixtures.sh <dir with each station's harcon.json, datums.json and station.json, as fetch.sh writes them>
set -euo pipefail
here=$(cd "$(dirname "$0")/../.." && pwd)
cd "$here"
src=${1:?a directory fetch.sh wrote}
cargo build -q --release --manifest-path tools/tide-station/Cargo.toml
tool=tools/tide-station/target/release/tide-station
out=ferros_tide/stations
mkdir -p "$out"
for id in 9414290 8518750 9447130 1612340; do
    "$tool" station --harcon "$src/$id.harcon.json" --datums "$src/$id.datums.json" --station "$src/$id.station.json" --out "$out/$id.tide.vsf"
done
"$tool" catalog --out "$out/catalog.vsf" "$out"/*.tide.vsf
