#!/bin/bash
# NOAA CO-OPS's published files for tools/tide-station, fetched on the host (the device never reads JSON).
# Usage: tools/tide-station/fetch.sh <dir> <station id>...   each station's harcon.json, datums.json and station.json (metric)
#        tools/tide-station/fetch.sh <dir> --list            every station with harmonic constituents (stations.json), for `tide-station catalog --noaa`
# NOAA CO-OPS data are works of the US Government, public domain in the US (17 USC 105).
set -euo pipefail
dir=${1:?a directory}
shift
mkdir -p "$dir"
api=https://api.tidesandcurrents.noaa.gov/mdapi/prod/webapi
for id in "$@"; do
    if [ "$id" = --list ]; then
        curl -fsS "$api/stations.json?type=harcon" -o "$dir/stations.json"
        continue
    fi
    curl -fsS "$api/stations/$id/harcon.json?units=metric" -o "$dir/$id.harcon.json"
    curl -fsS "$api/stations/$id/datums.json?units=metric" -o "$dir/$id.datums.json"
    curl -fsS "$api/stations/$id.json" -o "$dir/$id.station.json"
done
