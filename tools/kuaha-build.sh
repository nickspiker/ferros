#!/bin/bash
# Kūaha for a ferros device as a ferros process, a kist (docs/KISTS.md "Kūaha"): the std/kuaha package (its core, std/kuaha/core, under fluor's ferros host, the hold reached thru its `items` grant) on the ferros target, std from the ferros-rust fork, then the flat image the kernel loads.
# std/kuaha names fluor, vsf, kete and tatau by path (siblings of this tree), and the hold's crates (ferros_items, ferros_lang) inside it; the only patch is libc with a ferros arm.
# Pass extra cargo arguments after the script name.
set -e
here=$(cd "$(dirname "$0")/.." && pwd)
code=$(cd "$here/.." && pwd)
# In a worktree of ferros (.claude/worktrees/<name>) the siblings are four levels up.
[ -d "$code/ferros-rust" ] || code=$(cd "$here/../../../.." && pwd)
pkg="$here/std/kuaha"
export __CARGO_TESTS_ONLY_SRC_ROOT="$code/ferros-rust/library"
export CC_aarch64_unknown_ferros=aarch64-linux-gnu-gcc
export AR_aarch64_unknown_ferros=aarch64-linux-gnu-ar
# No branch protection in the C, and the target's reserved registers reserved in it too (tools/lumis-build.sh says why).
export CFLAGS_aarch64_unknown_ferros="-ffreestanding -fno-stack-protector -fno-builtin -mbranch-protection=none -ffixed-x24 -ffixed-x25 -ffixed-x26 -ffixed-x27 -ffixed-x28 -O2 -isystem $here/tools/std-sysroot/include"
export CARGO_TARGET_AARCH64_UNKNOWN_FERROS_RUSTFLAGS='--cfg getrandom_backend="custom"'
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$pkg/target/ferros}"
# Release by default: a find's page of cards is laid out and drawn whole every frame while the column moves; KUAHA_PROFILE=debug for a development build.
profile=${KUAHA_PROFILE:-release}
profile_flag=$( [ "$profile" = release ] && echo --release )
cd "$pkg"
cargo +nightly build -Zjson-target-spec -Zbuild-std=core,alloc,std,panic_abort -Zbuild-std-features=compiler-builtins-mem \
    --target "$here/targets/aarch64-unknown-ferros.json" \
    --config "patch.crates-io.libc.path='$code/ferros-libc'" \
    -p kuaha-ferros $profile_flag "$@"
out="$CARGO_TARGET_DIR/aarch64-unknown-ferros/$profile/kuaha"
aarch64-linux-gnu-objcopy -O binary "$out" "$out.bin" && echo "$out.bin: $(stat -c %s "$out.bin") bytes"
