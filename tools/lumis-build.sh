#!/bin/bash
# Lumis for a ferros device as a ferros process, a kist (docs/KISTS.md "Lumis", docs/CAMERA.md): Lumis's ferros front (rust/lumis-ferros on Lumis's `ferros` branch: the camera list, the live preview through each sensor's own IDT, the manual controls) on the ferros target, std from the ferros-rust fork, then the flat image the kernel loads. The crate names fluor and vsf by path (siblings of the Lumis tree), so the only patch is libc with a ferros arm; its lockfile is its own (rust/lumis-ferros/Cargo.lock), so Lumis's Android build is untouched.
# LUMIS names another Lumis tree (a worktree of the `ferros` branch: /mnt/Harbor/Code/lumis-ferros until it is merged). Pass extra cargo arguments after the script name.
set -e
here=$(cd "$(dirname "$0")/.." && pwd)
code=$(cd "$here/.." && pwd)
# In a worktree of ferros (.claude/worktrees/<name>) the siblings are four levels up.
[ -d "$code/ferros-rust" ] || code=$(cd "$here/../../../.." && pwd)
lumis=${LUMIS:-$code/lumis}
crate="$lumis/rust/lumis-ferros"
[ -d "$crate" ] || { echo "no $crate: Lumis's ferros front is on its \`ferros\` branch (LUMIS=/mnt/Harbor/Code/lumis-ferros)" >&2; exit 1; }
export __CARGO_TESTS_ONLY_SRC_ROOT="$code/ferros-rust/library"
export CC_aarch64_unknown_ferros=aarch64-linux-gnu-gcc
export AR_aarch64_unknown_ferros=aarch64-linux-gnu-ar
export CFLAGS_aarch64_unknown_ferros="-ffreestanding -fno-stack-protector -fno-builtin -O2 -isystem $here/tools/std-sysroot/include"
export CARGO_TARGET_AARCH64_UNKNOWN_FERROS_RUSTFLAGS='--cfg getrandom_backend="custom"'
export CARGO_TARGET_DIR="$crate/target/ferros"
# Release by default: the preview develops a frame a pass on every core, and an unoptimised build would show it (Mahere, 2026-11-11); LUMIS_PROFILE=debug for a development build.
profile=${LUMIS_PROFILE:-release}
profile_flag=$( [ "$profile" = release ] && echo --release )
cd "$crate"
cargo +nightly build -Zjson-target-spec -Zbuild-std=core,alloc,std,panic_abort -Zbuild-std-features=compiler-builtins-mem \
    --target "$here/targets/aarch64-unknown-ferros.json" \
    --config "patch.crates-io.libc.path='$code/ferros-libc'" \
    $profile_flag "$@"
out="$CARGO_TARGET_DIR/aarch64-unknown-ferros/$profile/lumis"
aarch64-linux-gnu-objcopy -O binary "$out" "$out.bin" && echo "$out.bin: $(stat -c %s "$out.bin") bytes"
