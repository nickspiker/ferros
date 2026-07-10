#![no_std]
#![crate_type = "lib"]
// KEY_REGISTERS derivation probe — see ../KEY_REGISTERS.md.
//
// Question this answers: does LLVM emit the inverted-pair check + zero-all + freeze WITHOUT spilling the secret V-regs to the stack?
// If yes, the generated asm IS the program (Tier 2, ship unmodified). If it spills, we're forced to Tier 3 (modify + prove the diff).
//
// Regenerate the audited asm (keep toolchain pinned — recorded in KEY_REGISTERS.md):
//   rustc --target aarch64-unknown-none -O --emit asm keyreg_probe.rs -o keyreg_probe.s
//
// Tier-2 acceptance: the emitted function body contains NO `str q`/`stp q`/`str x`/`stp x` to `[sp]` and allocates NO stack frame (`sub sp, sp, ...`).
// The secret stays in v0..v4 from load through zero. CI canary greps keyreg_probe.s for SIMD-to-stack stores; a hit fails the build.

use core::arch::asm;

/// Hold a 128-bit secret as an inverted pair (value + complement) in two NEON regs, verify XOR-to-all-ones immediately before use, and on mismatch zero ALL live secret regs then freeze (wfi, never return).
/// One unspillable critical section: mask -> load -> check -> {consume | zero+freeze}.
///
/// The secret's ENTIRE lifetime stays inside this `asm!` block. The no-spill guarantee holds because it never crosses back into Rust.
/// Passing a secret V-reg across a Rust boundary is out of scope and NOT blessed by this probe.
///
/// `secret_ptr`/`comp_ptr` are the load sources. In the shipped path these must themselves be non-named (derived in-block, or from the opaque/wairua store) — the probe validates the hold+check+zero+freeze mechanism, not provenance.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn keyreg_check_or_die(secret_ptr: *const u8, comp_ptr: *const u8) -> u64 {
    let result: u64;
    unsafe {
        asm!(
            // mask all interrupts — seal the critical section
            "msr daifset, #0xF",
            // load value + complement into v0, v1
            "ld1 {{v0.16b}}, [{sp}]",
            "ld1 {{v1.16b}}, [{cp}]",
            // v2 = v0 EOR v1 — all-ones iff pair intact
            "eor v2.16b, v0.16b, v1.16b",
            // v3 = NOT(v2): all-zero iff v2 was all-ones (pair intact)
            "not v3.16b, v2.16b",
            // reduce v3 to a single word: any set bit => corruption
            "umaxv b4, v3.16b",
            "umov w0, v4.s[0]",
            "cbz w0, 2f",            // intact -> continue
            // --- corruption path: zero ALL live secret regs, then freeze ---
            "movi v0.16b, #0",
            "movi v1.16b, #0",
            "movi v2.16b, #0",
            "movi v3.16b, #0",
            "1:",
            "wfi",
            "b 1b",                  // never returns
            // --- intact path: secret is verified-live in v0 ---
            "2:",
            // (real consume happens here, in the same block — the probe just
            //  moves a word out so the reg is observably used)
            "umov {res}, v0.d[0]",
            sp = in(reg) secret_ptr,
            cp = in(reg) comp_ptr,
            res = out(reg) result,
            out("v0") _, out("v1") _, out("v2") _,
            out("v3") _, out("v4") _,
            out("w0") _,
            options(nostack),
        );
    }
    result
}
