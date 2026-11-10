#!/usr/bin/env bash
# Screen captures into the folder this is run from: each Enter writes one VSF visual (<Eagle time in hex>.vsf), `q` then Enter ends it (tools/screencap --enter).
# Usage: tools/snap.sh [width height]   (default 1344 2992, the Pro; the 8a is 1080 2400)
set -eu
REPO=/mnt/Harbor/Code/ferros
( cd "$REPO" && cargo build -q -p ferros-bridge && cargo build -q --release --manifest-path tools/screencap/Cargo.toml )
exec "$REPO/tools/screencap/target/release/screencap" --enter "$@"
