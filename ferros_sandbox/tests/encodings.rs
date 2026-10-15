//! Every case is assembled by the GNU assembler (aarch64-linux-gnu-as), so the checker is tested against real encodings, not hand arithmetic.

use ferros_sandbox::{check, Refusal};
use std::process::Command;

fn assemble(src: &str) -> Vec<u32> {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("ferros_sandbox_{}_{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let s = dir.join("t.s");
    let o = dir.join("t.o");
    let b = dir.join("t.bin");
    std::fs::write(&s, format!(".arch armv8.5-a+lse\n{src}\n")).unwrap();
    let ok = Command::new("aarch64-linux-gnu-as").arg("-o").arg(&o).arg(&s).status().unwrap().success();
    assert!(ok, "assembler rejected: {src}");
    let ok = Command::new("aarch64-linux-gnu-objcopy").args(["-O", "binary", "-j", ".text"]).arg(&o).arg(&b).status().unwrap().success();
    assert!(ok);
    std::fs::read(&b).unwrap().chunks_exact(4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])).collect()
}

fn one(src: &str) -> u32 {
    let w = assemble(src);
    assert_eq!(w.len(), 1, "{src}");
    w[0]
}

fn accepts(src: &str) {
    let i = one(src);
    assert_eq!(check(i), Ok(()), "{src} ({i:08x}) should be accepted");
}

fn refuses(src: &str, why: Refusal) {
    let i = one(src);
    assert_eq!(check(i), Err(why), "{src} ({i:08x}) should be refused as {why:?}");
}

#[test]
fn invariant_register_writers() {
    accepts("and x26, x3, x27");
    accepts("and x26, x26, x27");
    accepts("add x25, x28, x26");
    accepts("add sp, x28, x26, uxtx");
    assert_eq!(one("and x26, x3, x27"), 0x8A1B_007A);
    assert_eq!(one("add x25, x28, x26"), ferros_sandbox::ADD_X25);
    assert_eq!(one("add sp, x28, x26, uxtx"), ferros_sandbox::ADD_SP);
}

#[test]
fn writes_to_reserved_registers() {
    refuses("mov x28, x0", Refusal::WritesBaseOrMask);
    refuses("add x27, x27, #1", Refusal::WritesBaseOrMask);
    refuses("ldr x28, [sp, #8]", Refusal::WritesBaseOrMask);
    refuses("mov x26, x1", Refusal::WritesInvariantRegister);
    refuses("and x26, x1, x2", Refusal::WritesInvariantRegister);
    refuses("and x26, x1, #0xffff", Refusal::WritesInvariantRegister);
    refuses("add x25, x28, x1", Refusal::WritesInvariantRegister);
    refuses("add x25, x25, #8", Refusal::WritesInvariantRegister);
    refuses("ldp x25, x26, [sp, #16]", Refusal::WritesInvariantRegister);
    refuses("mov sp, x0", Refusal::WritesInvariantRegister);
    refuses("sub sp, sp, #16", Refusal::WritesInvariantRegister);
    refuses("add sp, x28, x26, lsl #1", Refusal::WritesInvariantRegister);
    refuses("and sp, x0, #0xfff", Refusal::WritesInvariantRegister);
    refuses("umov x26, v0.d[0]", Refusal::WritesInvariantRegister);
    refuses("fmov x25, d0", Refusal::WritesInvariantRegister);
    refuses("fcvtzs x28, d1", Refusal::WritesBaseOrMask);
    refuses("mrs x26, cntvct_el0", Refusal::WritesInvariantRegister);
}

#[test]
fn ordinary_computation() {
    for s in [
        "add x0, x1, x2",
        "sub x3, x4, #100",
        "mov x0, #0x1234",
        "movk x0, #0x5678, lsl #16",
        "madd x0, x1, x2, x3",
        "udiv x0, x1, x2",
        "csel x0, x1, x2, eq",
        "cmp x0, x1",
        "ccmp x0, x1, #4, ne",
        "lsl x0, x1, #3",
        "ubfx x0, x1, #4, #8",
        "adrp x0, .",
        "adr x1, .",
        "and x0, x1, #0xff",
        "eor x2, x3, x4",
        "fadd d0, d1, d2",
        "add v25.4s, v26.4s, v27.4s",
        "mov v28.16b, v0.16b",
        "fmov d0, x1",
        "ucvtf d0, x1",
        "umov w0, v1.s[1]",
    ] {
        accepts(s);
    }
}

#[test]
fn memory_forms() {
    accepts("ldr x0, [x28, x26]");
    accepts("strb w1, [x28, x26]");
    accepts("ldr q0, [x28, x26]");
    accepts("ldr x0, [x25, #8]");
    accepts("str x0, [x25, #32760]");
    accepts("ldr q1, [x25, #65520]");
    accepts("ldur x0, [x25, #-8]");
    accepts("ldr x0, [sp, #16]");
    accepts("stp x29, x30, [sp, #16]");
    accepts("ldp q0, q1, [x25, #32]");
    accepts("ldxr x0, [x25]");
    accepts("stlr x1, [sp]");
    accepts("ldadd x1, x2, [x25]");
    accepts("ld1 {v0.16b}, [x25]");
    accepts("prfm pldl1keep, [x25, #64]");

    refuses("ldr x0, [x1]", Refusal::AddressForm);
    refuses("ldr x0, [x1, #8]", Refusal::AddressForm);
    refuses("str x0, [x28, #8]", Refusal::AddressForm);
    refuses("ldr x0, [x26, #8]", Refusal::AddressForm);
    refuses("ldr x0, [x28, x1]", Refusal::AddressForm);
    refuses("ldr x0, [x28, x26, lsl #3]", Refusal::AddressForm);
    refuses("ldr x0, [x28, w26, uxtw]", Refusal::AddressForm);
    refuses("ldp x0, x1, [x2, #16]", Refusal::AddressForm);
    refuses("ldxr x0, [x1]", Refusal::AddressForm);
    refuses("ldadd x1, x2, [x3]", Refusal::AddressForm);
    refuses("ld1 {v0.16b}, [x2]", Refusal::AddressForm);
    refuses("ldtr x0, [x25]", Refusal::AddressForm);
    refuses("stp x29, x30, [sp, #-16]!", Refusal::Writeback);
    refuses("ldp x29, x30, [sp], #16", Refusal::Writeback);
    refuses("ldr x0, [x25, #8]!", Refusal::Writeback);
    refuses("str x0, [sp], #16", Refusal::Writeback);
    refuses("ld1 {v0.16b}, [x25], #16", Refusal::Writeback);
    refuses("ldr x0, .", Refusal::LiteralLoad);
}

#[test]
fn control_flow() {
    for s in ["b .", "bl .", "b.eq .", "cbz x0, .", "tbnz x1, #3, .", "br x0", "blr x1", "ret", "svc #0", "brk #1", "nop", "isb", "dsb sy", "dmb ish", "bti c", "paciasp", "autiasp", "mrs x0, cntvct_el0", "mrs x1, cntfrq_el0"] {
        accepts(s);
    }
    refuses("svc #1", Refusal::Exception);
    refuses("hvc #0", Refusal::Exception);
    refuses("smc #0", Refusal::Exception);
    refuses("eret", Refusal::System);
    refuses("braa x0, x1", Refusal::System);
    refuses("msr daifset, #2", Refusal::System);
    refuses("mrs x0, midr_el1", Refusal::System);
    refuses("mrs x0, tpidr_el0", Refusal::System);
    refuses("msr tpidr_el0, x0", Refusal::System);
    refuses("dc zva, x25", Refusal::System);
    refuses("ic ivau, x0", Refusal::System);
}
