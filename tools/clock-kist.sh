#!/bin/bash
# The clock as an installed kist (KISTS.md; owner, 2026-11-10: "can I get my clock widget kist on the homescreen again?"): a widget, drawn live by the display in the kist's circle (payloads/clock says why the capsule has nothing to run), signed into a capsule that wants nothing and installed for the person attested on the device. Its circle appears on the map at the next tap on the plane.
# The option is the clock's identity across versions (ORACLE.md §5: minted once, kept). Minted 2026-11-10; never change it.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
cd "$here"
cargo el0 -q -p ferros-payload-clock
R=target/el0/aarch64-unknown-none/release
tools/payload-check.sh clock
aarch64-linux-gnu-objcopy -O binary $R/clock $R/clock.bin
out=target/clock.capsule.vsf
B=target/debug/ferros-bridge
"$B" capsule $R/clock.bin "$out" --kist Clock --option e6f06ca18ca2027b99ea0f0a3e5e97916ea81f57b227a8d9c51a5632aac5a35b
"$B" kist install "$out"
