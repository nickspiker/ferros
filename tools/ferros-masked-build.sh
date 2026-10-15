#!/usr/bin/env bash
# Build a payload as a sandboxed image (SANDBOX.md): every crate — the payload, core, alloc, compiler_builtins — compiled from source to assembly (nightly -Z build-std), rewritten by ferros-mask into the allowed forms, assembled (llvm-mc), linked with the payload's own linker script (ld.lld), and refused unless the sandbox checker accepts every instruction.
# Usage: tools/ferros-masked-build.sh <payload dir under payloads/> [bin name]   → target/masked/<bin>/<bin>.bin (flat image) and .elf
set -euo pipefail
REPO=/mnt/Harbor/Code/ferros
P=$1
BIN=${2:-$1}
CRATE=$(grep -m1 '^name' "$REPO/payloads/$P/Cargo.toml" | sed 's/.*"\(.*\)"/\1/')
OUT=$REPO/target/masked/$BIN
TD=$OUT/build
mkdir -p "$OUT/s"
# The same codegen rules as every other aarch64 build (reserved registers + CPU features), from .cargo/config.toml.
FLAGS=$(grep -m1 '^rustflags' "$REPO/.cargo/config.toml" | sed 's/.*target-feature=\([^"]*\)".*/\1/')
( cd "$REPO" && RUSTFLAGS="-C target-feature=$FLAGS -C codegen-units=1 --emit=asm,link" cargo +nightly build -q -p "$CRATE" --target aarch64-unknown-none --release -Z build-std=core,alloc,compiler_builtins -Z build-std-features=compiler-builtins-mem --target-dir "$TD" )
DEPS=$TD/aarch64-unknown-none/release/deps
# The payload's own (post-LTO) module holds everything it uses; compiler_builtins is never part of LTO.
SRCS=$(ls -t "$DEPS"/${BIN}-*.s | head -1; ls -t "$DEPS"/compiler_builtins-*.s | head -1)
ATTR="+neon,+aes,+sha2,+sha3,+sm4,+crc,+lse,+rdm,+dotprod,+fullfp16,+fp16fml,+i8mm,+bf16,+rcpc,+jsconv,+complxnum,+flagm,+sb,+ssbs,+fptoint,+dit"
OBJS=()
for s in $SRCS; do
    b=$(basename "$s" .s)
    "$REPO/target/debug/ferros-mask" "$s" "$OUT/s/$b.s"
    llvm-mc -triple=aarch64-unknown-none -mattr=$ATTR -filetype=obj -o "$OUT/s/$b.o" "$OUT/s/$b.s"
    OBJS+=("$OUT/s/$b.o")
done
ld.lld -T "$REPO/payloads/$P/payload.ld" --gc-sections -e _entry -o "$OUT/$BIN.elf" "${OBJS[@]}"
aarch64-linux-gnu-objcopy -O binary "$OUT/$BIN.elf" "$OUT/$BIN.bin"
aarch64-linux-gnu-objcopy -O binary -j .text "$OUT/$BIN.elf" "$OUT/$BIN.text"
( cd "$REPO" && cargo run -q -p ferros_sandbox --example scan -- "$OUT/$BIN.text" ) | tee "$OUT/scan.txt"
grep -q " 0 refused" "$OUT/scan.txt" || { echo "REFUSED: $BIN does not pass the sandbox checker"; exit 1; }
echo "$BIN: sandboxed image $(stat -c %s "$OUT/$BIN.bin") bytes → $OUT/$BIN.bin"
