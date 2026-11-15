#!/bin/bash
# Lumis for a ferros device as a ferros process, a kist (docs/KISTS.md "Lumis", docs/CAMERA.md): the std/lumis package (Lumis's own core, the lumis_core library, under fluor's ferros host, with its platform layer for ferros) on the ferros target, std from the ferros-rust fork, then the flat image the kernel loads.
# std/lumis names Lumis, fluor, vsf and kete by path (siblings of this tree), so Lumis's own manifest and lockfile stay as its Android build wants them; the only patch is libc with a ferros arm. Lumis's port lives on its branch `ferros-port` until merged: build from a tree whose sibling `lumis` is that branch (a farm of the siblings, as /mnt/Harbor/tmp/lumis-port).
# Pass extra cargo arguments after the script name.
set -e
here=$(cd "$(dirname "$0")/.." && pwd)
code=$(cd "$here/.." && pwd)
# In a worktree of ferros (.claude/worktrees/<name>) the siblings are four levels up.
[ -d "$code/ferros-rust" ] || code=$(cd "$here/../../../.." && pwd)
pkg="$here/std/lumis"
export __CARGO_TESTS_ONLY_SRC_ROOT="$code/ferros-rust/library"
export CC_aarch64_unknown_ferros=aarch64-linux-gnu-gcc
export AR_aarch64_unknown_ferros=aarch64-linux-gnu-ar
# No branch protection in the C: Fedora's cross gcc defaults to pac-ret, and its PACIASP/AUTIASP would sign with the pointer-authentication keys, which hold shares of the ira and wairua (the kernel keeps them switched off for EL0 as well, cachemap.rs).
export CFLAGS_aarch64_unknown_ferros="-ffreestanding -fno-stack-protector -fno-builtin -mbranch-protection=none -O2 -isystem $here/tools/std-sysroot/include"
export CARGO_TARGET_AARCH64_UNKNOWN_FERROS_RUSTFLAGS='--cfg getrandom_backend="custom"'
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$pkg/target/ferros}"
# Release by default: Lumis's preview develops every frame on the CPU across the cores, and an unoptimised build would show it (Mahere, 2026-11-11); LUMIS_PROFILE=debug for a development build.
profile=${LUMIS_PROFILE:-release}
profile_flag=$( [ "$profile" = release ] && echo --release )
cd "$pkg"
cargo +nightly build -Zjson-target-spec -Zbuild-std=core,alloc,std,panic_abort -Zbuild-std-features=compiler-builtins-mem \
    --target "$here/targets/aarch64-unknown-ferros.json" \
    --config "patch.crates-io.libc.path='$code/ferros-libc'" \
    $profile_flag "$@"
out="$CARGO_TARGET_DIR/aarch64-unknown-ferros/$profile/lumis"
aarch64-linux-gnu-objcopy -O binary "$out" "$out.bin" && echo "$out.bin: $(stat -c %s "$out.bin") bytes"
