#!/usr/bin/env bash
# Build a payload as a sandboxed image (SANDBOX.md): every crate — the payload, core, alloc, compiler_builtins — compiled from source to assembly (nightly -Z build-std), rewritten by ferros-mask into the allowed forms, assembled (llvm-mc), linked with the payload's own linker script (ld.lld), and refused unless the sandbox checker accepts every instruction.
# Usage: tools/ferros-masked-build.sh <payload dir under payloads/> [bin name]   → target/masked/<bin>/<bin>.bin (flat image) and .elf
set -euo pipefail
REPO=/mnt/Harbor/Code/ferros
# --fixed K anywhere after the payload: fixed mode at arc size 2^K (SANDBOX.md, "Modes"): every address made safe by `bfi x26, xN, #0, #K`, the capsule sealed at that size; without it, ring mode.
FIXED=""
ARGS=()
while [ $# -gt 0 ]; do case $1 in --fixed) FIXED=$2; shift 2;; *) ARGS+=("$1"); shift;; esac; done
set -- "${ARGS[@]}"
P=$1
BIN=${2:-$1}
CRATE=$(grep -m1 '^name' "$REPO/payloads/$P/Cargo.toml" | sed 's/.*"\(.*\)"/\1/')
OUT=$REPO/target/masked/$BIN
TD=$OUT/build
mkdir -p "$OUT/s"
# The codegen rules come from the target itself (targets/aarch64-ferros.json): the process ABI's reserved registers and the CPU's measured features are properties of the target, so core, alloc and compiler_builtins inherit them by being built for it.
# They used to be scraped out of .cargo/config.toml with sed. That worked, but a change to the shape of that line would have silently produced a different feature set — and a core built without x27/x28 reserved breaks the masking invariant (KERNEL.md: the arc base and mask must hold at *every* instruction, which callee-saved does not give).
TARGET=$REPO/targets/aarch64-ferros.json
( cd "$REPO" && RUSTFLAGS="-C codegen-units=1 --emit=asm,link${BTI:+ -Z branch-protection=bti}" cargo +nightly build -q -p "$CRATE" --target "$TARGET" --release -Z json-target-spec -Z build-std=core,alloc,compiler_builtins -Z build-std-features=compiler-builtins-mem --target-dir "$TD" )
DEPS=$TD/aarch64-ferros/release/deps
# The payload's own (post-LTO) module holds everything it uses; compiler_builtins is never part of LTO.
SRCS=$(ls -t "$DEPS"/${BIN}-*.s | head -1; ls -t "$DEPS"/compiler_builtins-*.s | head -1)
# llvm-mc's own spellings of the same set: LLVM and rustc disagree on several names (+fullfp16 for +fp16, +fp16fml for +fhm, +complxnum for +fcma, +fptoint for +frintts), so this list cannot be shared with the target's and is kept beside it deliberately.
ATTR="+neon,+aes,+sha2,+sha3,+sm4,+crc,+lse,+rdm,+dotprod,+fullfp16,+fp16fml,+i8mm,+bf16,+rcpc,+jsconv,+complxnum,+flagm,+sb,+ssbs,+fptoint,+dit"
OBJS=()
for s in $SRCS; do
    b=$(basename "$s" .s)
    "$REPO/target/debug/ferros-mask" "$s" "$OUT/s/$b.s" ${FIXED:+--fixed $FIXED} ${BTI:+--bti}
    llvm-mc -triple=aarch64-unknown-none -mattr=$ATTR -filetype=obj -o "$OUT/s/$b.o" "$OUT/s/$b.s"
    OBJS+=("$OUT/s/$b.o")
done
# Position-independent, as the payloads' build.rs links them: every absolute address in data becomes an R_AARCH64_RELATIVE entry for ferros_sys::relocate() (a static link baked link-time offsets into vtables and slice pointers, and the first masked process that formatted a number branched to an arc offset, 2026-11-03).
# The data on its own pages: the sandbox makes the text pages read-only and executable, and a small image whose .rodata shared the last text page faulted on its first relocation write (touch, 2026-11-03).
sed 's/^\( *\)\.rodata : /\1. = ALIGN(4096);\n\1.rodata : /' "$REPO/payloads/$P/payload.ld" > "$OUT/payload.ld"
ld.lld -T "$OUT/payload.ld" --gc-sections -e _entry -pie --no-dynamic-linker -z norelro -z notext -o "$OUT/$BIN.elf" "${OBJS[@]}"
aarch64-linux-gnu-objcopy -O binary "$OUT/$BIN.elf" "$OUT/$BIN.bin"
aarch64-linux-gnu-objcopy -O binary -j .text "$OUT/$BIN.elf" "$OUT/$BIN.text"
# The capsule declares how much of the image is instructions (the kernel's checker covers exactly that; .text comes first, payload.ld).
stat -c %s "$OUT/$BIN.text" > "$OUT/$BIN.textlen"
( cd "$REPO" && cargo run -q -p ferros_sandbox --example scan -- "$OUT/$BIN.text" ${FIXED:+--fixed $FIXED} ${BTI:+--bti} ) | tee "$OUT/scan.txt"
if [ -n "${BTI:-}" ]; then touch "$OUT/$BIN.bti"; else rm -f "$OUT/$BIN.bti"; fi
# Fixed mode: the arc is exactly 2^K, so the image, its zeroed tail, the grants page and the 64 KiB stack must fit (process.rs `build`: CODE + image + tail + STACK).
if [ -n "$FIXED" ]; then
    TAIL=0
    if [ "$(dd if="$OUT/$BIN.bin" bs=1 skip=8 count=8 2>/dev/null)" = ferrosRT ]; then TAIL=$(od -An -tu8 -j 16 -N 8 "$OUT/$BIN.bin" | tr -d ' '); fi
    NEED=$(( 4096 + $(stat -c %s "$OUT/$BIN.bin") + TAIL + 65536 ))
    [ $NEED -le $((1 << FIXED)) ] || { echo "REFUSED: $BIN needs $NEED bytes, over a fixed arc of 2^$FIXED"; exit 1; }
    echo $FIXED > "$OUT/$BIN.fixed"
else
    rm -f "$OUT/$BIN.fixed"
fi
grep -q " 0 refused" "$OUT/scan.txt" || { echo "REFUSED: $BIN does not pass the sandbox checker"; exit 1; }
echo "$BIN: sandboxed image $(stat -c %s "$OUT/$BIN.bin") bytes → $OUT/$BIN.bin"
