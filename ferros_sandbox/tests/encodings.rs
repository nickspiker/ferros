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
    refuses("ldur x0, [x25, #-8]", Refusal::AddressForm);
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

/// Handles (SANDBOX.md): the pair load from the table, the masked offset, and accesses through the pair; nothing else may write x21-x23, and the pair takes no immediate offsets.
#[test]
fn handle_forms() {
    accepts("ldp x23, x22, [x28]");
    accepts("ldp x23, x22, [x28, #16]");
    accepts("ldp x23, x22, [x28, #496]");
    assert_eq!(one("ldp x23, x22, [x28]"), ferros_sandbox::LDP_HANDLE);
    refuses("ldp x23, x22, [x28, #-16]", Refusal::AddressForm);
    refuses("ldp x23, x22, [x28, #8]", Refusal::AddressForm);
    assert_eq!(one("and x21, x0, x22"), ferros_sandbox::AND_X21);
    accepts("and x21, x5, x22");
    accepts("str w3, [x23, x21]");
    accepts("ldr x0, [x23, x21]");
    accepts("strb w3, [x23, x21]");
    accepts("ldr q0, [x23, x21]");
    accepts("str q1, [x23, x21]");
    refuses("ldp x23, x22, [x25, #16]", Refusal::WritesInvariantRegister);
    refuses("ldp x22, x23, [x28, #16]", Refusal::AddressForm);
    refuses("mov x23, x1", Refusal::WritesInvariantRegister);
    refuses("and x21, x5, x27", Refusal::WritesInvariantRegister);
    refuses("add x22, x22, #1", Refusal::WritesInvariantRegister);
    refuses("str w3, [x23, #4]", Refusal::AddressForm);
    refuses("str w3, [x23, x26]", Refusal::AddressForm);
    refuses("str w3, [x28, x21]", Refusal::AddressForm);
    refuses("str w3, [x23, x21, lsl #2]", Refusal::AddressForm);
}

/// An image with the entry header: the branch at 0 is checked, the pad, the magic and the five words to 56 are stepped over, and the code after is checked.
#[test]
fn header_is_skipped() {
    let mut img = Vec::new();
    img.extend_from_slice(&one("b 56").to_le_bytes());
    img.extend_from_slice(&[0u8; 4]);
    img.extend_from_slice(b"ferrosRT");
    img.extend_from_slice(&[0xFFu8; 40]);
    img.extend_from_slice(&one("and x26, x3, x27").to_le_bytes());
    assert_eq!(ferros_sandbox::check_image(&img), Ok(2));
    img.extend_from_slice(&one("mov x28, x0").to_le_bytes());
    assert!(ferros_sandbox::check_image(&img).is_err());
}

/// Nothing below the arc base: negative immediates on x25 and sp are refused (the emitter folds them into the masked address); non-negative ones stay.
#[test]
fn no_negative_offsets() {
    accepts("ldur w0, [x25, #255]");
    accepts("ldr x0, [x25, #8]");
    accepts("ldp x0, x1, [sp, #16]");
    accepts("stur q0, [sp, #1]");
    refuses("ldur w0, [x25, #-8]", Refusal::AddressForm);
    refuses("stur x0, [sp, #-256]", Refusal::AddressForm);
    refuses("ldp x0, x1, [x25, #-16]", Refusal::AddressForm);
    refuses("stp q0, q1, [sp, #-32]", Refusal::AddressForm);
}

fn accepts_fixed(src: &str, k: u32) {
    let i = one(src);
    assert_eq!(ferros_sandbox::check_in(i, ferros_sandbox::Mode::Fixed(k)), Ok(()), "{src} ({i:08x}) should be accepted in fixed {k}");
}

fn refuses_fixed(src: &str, k: u32) {
    let i = one(src);
    assert!(ferros_sandbox::check_in(i, ferros_sandbox::Mode::Fixed(k)).is_err(), "{src} ({i:08x}) should be refused in fixed {k}");
}

/// Fixed mode (SANDBOX.md, "Modes"): x26 written only by `bfi x26, xN, #0, #K` at the sealed K, sp only from x26; accesses through x26 (immediates non-negative), sp and the handle; the ring forms refused.
#[test]
fn fixed_mode() {
    accepts_fixed("bfi x26, x8, #0, #17", 17);
    accepts_fixed("bfi x26, x24, #0, #17", 17);
    assert_eq!(one("bfi x26, x8, #0, #17"), ferros_sandbox::bfi_x26(17) | 8 << 5);
    accepts_fixed("mov sp, x26", 17);
    accepts_fixed("ldr x0, [x26]", 17);
    accepts_fixed("ldr x0, [x26, #16]", 17);
    accepts_fixed("ldp q0, q1, [x26, #32]", 17);
    accepts_fixed("ld4 {v0.8h, v1.8h, v2.8h, v3.8h}, [x26]", 17);
    accepts_fixed("ldur w0, [x26, #3]", 17);
    accepts_fixed("str x0, [sp, #8]", 17);
    accepts_fixed("str w3, [x23, x21]", 17);
    refuses_fixed("bfi x26, x8, #0, #18", 17);
    refuses_fixed("bfi x26, x8, #1, #17", 17);
    refuses_fixed("and x26, x8, x27", 17);
    refuses_fixed("add x25, x28, x26", 17);
    refuses_fixed("add sp, x28, x26, uxtx", 17);
    refuses_fixed("ldr x0, [x28, x26]", 17);
    refuses_fixed("ldr x0, [x25, #8]", 17);
    refuses_fixed("ldur x0, [x26, #-8]", 17);
    refuses_fixed("mov x26, x0", 17);
    // Ring mode refuses the fixed forms.
    refuses("bfi x26, x8, #0, #17", Refusal::WritesInvariantRegister);
    refuses("mov sp, x26", Refusal::WritesInvariantRegister);
}

fn image(src: &str) -> Vec<u8> {
    assemble(src).iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// Exact mode (SANDBOX.md, "Modes"): the four-instruction sequence is the only writer of x26, only in a BTI image, and no direct branch may land inside it.
#[test]
fn exact_mode() {
    use ferros_sandbox::{check_image_with, Mode};
    let seq = "sub x25, x8, x28\nsub x26, x25, x27\norn x26, x25, x26\norr x26, x8, x26, asr #63\n";
    assert_eq!(one("sub x25, x8, x28") & 0xFFFF_FC1F, ferros_sandbox::EXACT_0);
    assert_eq!(one("sub x26, x25, x27"), ferros_sandbox::EXACT_1);
    assert_eq!(one("orn x26, x25, x26"), ferros_sandbox::EXACT_2);
    assert_eq!(one("orr x26, x8, x26, asr #63") & 0xFFFF_FC1F, ferros_sandbox::EXACT_3);
    let ok = image(&format!("{seq}ldr x0, [x26]\nstr q0, [x26, #16]\nmov sp, x26\nbr x30\n"));
    assert!(check_image_with(&ok, Mode::Exact, true).is_ok(), "a sequence and its accesses");
    assert!(check_image_with(&ok, Mode::Exact, false).is_err(), "exact needs BTI");
    let into = image(&format!("b 1f\n{}1:\n{}", "sub x25, x8, x28\nsub x26, x25, x27\n", "orn x26, x25, x26\norr x26, x8, x26, asr #63\nldr x0, [x26]\nbr x30\n"));
    assert!(check_image_with(&into, Mode::Exact, true).is_err(), "a branch into the sequence");
    let partial = image("sub x25, x8, x28\nsub x26, x25, x27\norn x26, x25, x26\nldr x0, [x26]\nbr x30\n");
    assert!(check_image_with(&partial, Mode::Exact, true).is_err(), "a partial sequence");
    let other = image("sub x25, x8, x28\nsub x26, x25, x27\norn x26, x25, x26\norr x26, x9, x26, asr #63\nbr x30\n");
    assert!(check_image_with(&other, Mode::Exact, true).is_err(), "a different register in the last");
    let ret = image(&format!("{seq}ldr x0, [x26]\nret\n"));
    assert!(check_image_with(&ret, Mode::Exact, true).is_err(), "ret in a BTI image");
    let bfi = image("bfi x26, x8, #0, #17\nbr x30\n");
    assert!(check_image_with(&bfi, Mode::Exact, true).is_err(), "the fixed writer in exact mode");
}

/// Words disassembled by GNU objdump, one string each ("mnemonic\toperands", or ".inst ... ; undefined"): the Arm ARM's decode as binutils implements it, the oracle the lane-move tests hold the checker to.
fn disassemble(words: &[u32]) -> Vec<String> {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("ferros_sandbox_dis_{}_{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let b = dir.join("w.bin");
    std::fs::write(&b, words.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>()).unwrap();
    let out = Command::new("aarch64-linux-gnu-objdump").args(["-D", "-b", "binary", "-m", "aarch64"]).arg(&b).output().unwrap();
    assert!(out.status.success());
    let _ = std::fs::remove_dir_all(&dir);
    let text = String::from_utf8(out.stdout).unwrap();
    // "   4:\t2e092f7c \t.inst\t0x2e092f7c ; undefined": an address, the word, then the instruction.
    let lines: Vec<String> = text
        .lines()
        .filter(|l| l.split_once(':').is_some_and(|(a, _)| !a.trim().is_empty() && a.trim().chars().all(|c| c.is_ascii_hexdigit())) && l.contains('\t'))
        .map(|l| l.splitn(3, '\t').nth(2).unwrap_or("").trim().to_string())
        .collect();
    assert_eq!(lines.len(), words.len());
    lines
}

/// What the oracle says a word writes: a general register (its destination spelled w or x), a vector or FP register, or nothing it knows (unallocated).
#[derive(Debug, PartialEq)]
enum Dest {
    General,
    Vector,
    Undefined,
}

fn dest(dis: &str) -> Dest {
    if dis.contains("undefined") {
        return Dest::Undefined;
    }
    let ops = dis.split_once('\t').map_or("", |(_, o)| o).trim();
    match ops.chars().next() {
        Some('w' | 'x') => Dest::General,
        Some('v' | 'b' | 'h' | 's' | 'd' | 'q') => Dest::Vector,
        _ => panic!("no destination in {dis:?}"),
    }
}

/// Holds the checker to the oracle over `words`, each with Rd = `rd`: a vector destination accepted whatever its number, a general one refused as its register says (accepted for an ordinary one), an unallocated word refused.
fn against_oracle(words: &[u32], rd: u32) {
    let why = match rd {
        27 | 28 => Some(Refusal::WritesBaseOrMask),
        21..=26 => Some(Refusal::WritesInvariantRegister),
        _ => None,
    };
    for (w, dis) in words.iter().zip(disassemble(words)) {
        let got = check(*w);
        match dest(&dis) {
            Dest::Vector => assert_eq!(got, Ok(()), "{dis} ({w:08x}) writes a vector register"),
            Dest::General => assert_eq!(got, why.map_or(Ok(()), Err), "{dis} ({w:08x}) writes x{rd}"),
            Dest::Undefined => assert!(got.is_err(), "{dis} ({w:08x}) is unallocated and was accepted"),
        }
    }
}

/// The lane moves the Photon scan found refused (2026-11-12, target/masked-photon): INS (element) writes a vector register, whatever its number, and its imm4 is a lane, never SMOV's or UMOV's opcode.
#[test]
fn lane_moves_from_the_scan() {
    assert_eq!(one("mov v28.b[4], v27.b[5]"), 0x6E09_2F7C);
    assert_eq!(one("mov v22.b[2], v18.b[7]"), 0x6E05_3E56);
    accepts("mov v28.b[4], v27.b[5]");
    accepts("mov v22.b[2], v18.b[7]");
}

/// Every lane move, dup and insert form, by hand: vector destinations numbered as the reserved general registers are accepted; SMOV and UMOV (and `mov` spelled for UMOV) into them are refused; INS (general) and DUP (general) only read a general register.
#[test]
fn lane_moves_and_dups() {
    for s in [
        "mov v28.b[4], v27.b[5]",
        "ins v27.h[1], v0.h[7]",
        "mov v26.s[3], v1.s[1]",
        "mov v25.d[1], v2.d[0]",
        "mov v22.b[15], v18.b[7]",
        "mov v21.h[5], v28.h[3]",
        "mov v28.b[0], w27",
        "ins v27.s[1], w28",
        "mov v26.d[1], x28",
        "mov v23.h[2], w26",
        "dup v28.16b, v27.b[5]",
        "dup v27.8h, v28.h[7]",
        "dup v26.2s, v0.s[3]",
        "dup v25.2d, v1.d[1]",
        "dup v28.16b, w27",
        "dup v27.4s, w28",
        "dup v22.2d, x28",
        "mov b28, v27.b[5]",
        "dup h27, v28.h[7]",
        "mov s26, v0.s[3]",
        "mov d22, v1.d[1]",
        "umov w0, v28.b[7]",
        "smov x1, v27.h[5]",
        "mov x2, v26.d[1]",
    ] {
        accepts(s);
    }
    refuses("umov w28, v0.b[5]", Refusal::WritesBaseOrMask);
    refuses("umov w27, v0.h[7]", Refusal::WritesBaseOrMask);
    refuses("mov w28, v1.s[3]", Refusal::WritesBaseOrMask);
    refuses("mov x27, v1.d[1]", Refusal::WritesBaseOrMask);
    refuses("smov w28, v2.b[5]", Refusal::WritesBaseOrMask);
    refuses("smov x27, v2.h[7]", Refusal::WritesBaseOrMask);
    refuses("smov x28, v2.s[1]", Refusal::WritesBaseOrMask);
    refuses("umov w26, v3.b[7]", Refusal::WritesInvariantRegister);
    refuses("mov x25, v3.d[0]", Refusal::WritesInvariantRegister);
    refuses("smov x22, v3.b[5]", Refusal::WritesInvariantRegister);
    refuses("umov w23, v3.h[2]", Refusal::WritesInvariantRegister);
    refuses("smov w21, v3.h[3]", Refusal::WritesInvariantRegister);
}

/// The whole Advanced SIMD copy group (every Q, op, imm5 and imm4), with Rd a base register, an invariant register and an ordinary one, against binutils' decode.
#[test]
fn simd_copy_group_against_binutils() {
    for rd in [28, 22, 0] {
        let mut words = Vec::new();
        for q in 0..2u32 {
            for op in 0..2u32 {
                for imm5 in 0..32u32 {
                    for imm4 in 0..16u32 {
                        words.push(0x0E00_0400 | q << 30 | op << 29 | imm5 << 16 | imm4 << 11 | 27 << 5 | rd);
                    }
                }
            }
        }
        against_oracle(&words, rd);
    }
}

/// Floating point to fixed point and back (every sf, S, type, rmode and opcode, scales at both ends of each width): FCVTZS and FCVTZU write a general register (until 2026-11-12 the checker let `fcvtzs x28, d0, #3` through as a vector write), SCVTF and UCVTF a vector one.
#[test]
fn fixed_point_conversions_against_binutils() {
    refuses("fcvtzs x28, d0, #3", Refusal::WritesBaseOrMask);
    refuses("fcvtzu w27, s1, #5", Refusal::WritesBaseOrMask);
    refuses("fcvtzs w26, d2, #8", Refusal::WritesInvariantRegister);
    refuses("fcvtzu x22, d3, #64", Refusal::WritesInvariantRegister);
    accepts("fcvtzs x0, d0, #3");
    accepts("scvtf d28, x27, #3");
    accepts("ucvtf s27, w28, #5");
    for rd in [28, 22] {
        let mut words = Vec::new();
        for sf in 0..2u32 {
            for s in 0..2u32 {
                for ty in 0..4u32 {
                    for rmode in 0..4u32 {
                        for opcode in 0..8u32 {
                            for scale in [0u32, 31, 32, 63] {
                                words.push(0x1E00_0000 | sf << 31 | s << 29 | ty << 22 | rmode << 19 | opcode << 16 | scale << 10 | 27 << 5 | rd);
                            }
                        }
                    }
                }
            }
        }
        against_oracle(&words, rd);
    }
}

/// A sweep of the whole SIMD and FP data processing space (op0 x111) with Rd = x28: wherever binutils decodes a general register destination, the checker refuses it. A fixed xorshift, so the words are the same every run.
#[test]
fn simd_fp_space_never_writes_a_reserved_register() {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut words = Vec::new();
    for _ in 0..1 << 16 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        words.push((x as u32 & !(0b111 << 25) & !31) | 0b111 << 25 | 28);
    }
    for (w, dis) in words.iter().zip(disassemble(&words)) {
        if dest(&dis) == Dest::General {
            assert_eq!(check(*w), Err(Refusal::WritesBaseOrMask), "{dis} ({w:08x}) writes x28");
        }
    }
}
