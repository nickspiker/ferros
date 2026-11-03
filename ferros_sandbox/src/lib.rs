//! The install-time checker for ferros's masking invariant (SANDBOX.md). Run once when a capsule is installed — never at spawn: an image that passes is sealed and then runs with no checks at all.
//!
//! Deny by default: an instruction is accepted only if it is positively recognised as one of the allowed forms. Unknown or unallocated encodings, SVE/SME, system registers, cache maintenance, literal loads, writeback, and any write to the invariant registers outside its single allowed form are refused.
//!
//! The invariant (SANDBOX.md): x28 = arc base and x27 = mask are never written; x26 (masked offset) is written only by `and x26, <reg>, x27`; x25 (address in the arc) only by `add x25, x28, x26`; sp only by `add sp, x28, x26, uxtx`. Memory is reached only through `[x28, x26]`, or `[x25|sp, #imm]` with no writeback (immediates stay inside the guard G around every arc). Because each invariant register is re-established by a single instruction, the invariant holds at every instruction boundary and control flow needs no checking.

#![no_std]

/// Why an instruction was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Writes x27 or x28 (base and mask are the kernel's).
    WritesBaseOrMask,
    /// Writes x26, x25 or sp other than through its one allowed form.
    WritesInvariantRegister,
    /// A load/store addressed some other way than `[x28, x26]`, `[x25, #imm]` or `[sp, #imm]`.
    AddressForm,
    /// Pre- or post-index writeback.
    Writeback,
    /// PC-relative literal load.
    LiteralLoad,
    /// A system instruction outside the allowed set (svc #0, EL0 timer reads, barriers, hints).
    System,
    /// Exception generation other than `svc #0`, `brk`, `udf` (hvc, smc, dcps).
    Exception,
    /// An encoding this checker does not recognise (refused by default).
    Unknown,
}

/// How a masked capsule makes a variable address safe (SANDBOX.md, "Modes"). Ring: `and x26, xN, x27` (an offset, the mask a register the kernel sets), then `[x28, x26]` or `add x25, x28, x26`; the arc may grow by doubling. Fixed(K): `bfi x26, xN, #0, #K` (x26 keeps the arc base in its bits above K, so it is always an address inside the 2^K arc), then `[x26, #imm]`; the size is in the code, sealed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Ring,
    Fixed(u32),
}

/// A refusal at a byte offset into the image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Violation {
    pub offset: usize,
    pub insn: u32,
    pub why: Refusal,
}

const BASE: u32 = 28;
const MASK: u32 = 27;
const OFF: u32 = 26;
const ADDR: u32 = 25;
/// The handle pair (SANDBOX.md, "Handles"): x23 a granted region's base, x22 its mask, x21 an offset masked by it. Written only by the pair load from the handle table and the masked AND.
const HBASE: u32 = 23;
const HMASK: u32 = 22;
const HOFF: u32 = 21;
/// Register number 31 as a base or destination: sp in some encodings, xzr in others.
const R31: u32 = 31;

/// The three allowed writes of the invariant registers.
pub const AND_X26: u32 = 0x8A1B_001A; // and x26, xN, x27  (with Rn in bits 9:5)
const AND_X26_MASK: u32 = 0xFFFF_FC1F;
pub const ADD_X25: u32 = 0x8B1A_0399; // add x25, x28, x26
pub const ADD_SP: u32 = 0x8B3A_639F; // add sp, x28, x26, uxtx
/// Fixed mode's writers: `bfi x26, xN, #0, #K` (BFM with immr 0, imms K - 1; Rn in bits 9:5) and `mov sp, x26` (add sp, x26, #0).
pub const fn bfi_x26(k: u32) -> u32 {
    0xB340_001A | (k - 1) << 10
}
pub const MOV_SP_X26: u32 = 0x9100_035F;
/// The handle pair's writers: `ldp x23, x22, [x28, #imm]` with imm a slot's pair in the handle table, the grants page's first 512 bytes (offset form, imm7 even and under 64: slot k at 16k), and `and x21, xN, x22`.
pub const LDP_HANDLE: u32 = 0xA940_5B97; // ldp x23, x22, [x28, #0]  (imm7 in bits 21:15)
const LDP_HANDLE_MASK: u32 = 0xFFC0_7FFF;
pub const AND_X21: u32 = 0x8A16_0015; // and x21, xN, x22  (with Rn in bits 9:5)

fn rd(i: u32) -> u32 {
    i & 31
}
fn rn(i: u32) -> u32 {
    (i >> 5) & 31
}
fn rt2(i: u32) -> u32 {
    (i >> 10) & 31
}
fn rm(i: u32) -> u32 {
    (i >> 16) & 31
}

/// A general-purpose destination: refuse the reserved registers (31 is xzr here, harmless).
fn gpr_write(r: u32) -> Result<(), Refusal> {
    match r {
        BASE | MASK => Err(Refusal::WritesBaseOrMask),
        OFF | ADDR | HBASE | HMASK | HOFF => Err(Refusal::WritesInvariantRegister),
        _ => Ok(()),
    }
}

/// A destination where 31 means sp.
fn gpr_or_sp_write(r: u32) -> Result<(), Refusal> {
    if r == R31 {
        return Err(Refusal::WritesInvariantRegister);
    }
    gpr_write(r)
}

/// Check one instruction (ring mode).
pub fn check(i: u32) -> Result<(), Refusal> {
    check_in(i, Mode::Ring)
}

/// Check one instruction under `m`.
pub fn check_in(i: u32, m: Mode) -> Result<(), Refusal> {
    // The single allowed writers of the invariant registers, by mode; the handle's offset in every mode.
    if i & AND_X26_MASK == AND_X21 {
        return Ok(());
    }
    match m {
        Mode::Ring => {
            if i & AND_X26_MASK == AND_X26 || i == ADD_X25 || i == ADD_SP {
                return Ok(());
            }
        }
        // x26 is an absolute address in the arc in fixed mode: the ring forms (an offset in x26, x25 from it, sp from x28 + x26) would break it, so they are refused with every other write.
        Mode::Fixed(k) => {
            if (16..=40).contains(&k) && (i & AND_X26_MASK == bfi_x26(k) || i == MOV_SP_X26) {
                return Ok(());
            }
        }
    }
    // The handle select: one pair load from the table, so the base and the mask change together and the invariant holds at every boundary.
    if i & LDP_HANDLE_MASK == LDP_HANDLE {
        let imm7 = (i >> 15) & 0x7F;
        return if imm7 & 1 == 0 && imm7 < 64 { Ok(()) } else { Err(Refusal::AddressForm) };
    }
    let op0 = (i >> 25) & 0xF;
    match op0 {
        // Data processing — immediate.
        0b1000 | 0b1001 => dp_imm(i),
        // Branches, exception generation, system.
        0b1010 | 0b1011 => branch_sys(i),
        // Loads and stores.
        0b0100 | 0b0110 | 0b1100 | 0b1110 => load_store(i, m),
        // Data processing — register.
        0b0101 | 0b1101 => dp_reg(i),
        // SIMD and floating point data processing.
        0b0111 | 0b1111 => simd_fp(i),
        // Reserved, SME, SVE, unallocated.
        _ => Err(Refusal::Unknown),
    }
}

fn dp_imm(i: u32) -> Result<(), Refusal> {
    let op = (i >> 23) & 0x7;
    match op {
        // ADR / ADRP: Rd (31 = xzr).
        0b000 | 0b001 => gpr_write(rd(i)),
        // Add/subtract immediate: S (bit 29) = 0 → Rd 31 is sp.
        0b010 => {
            if (i >> 29) & 1 == 0 {
                gpr_or_sp_write(rd(i))
            } else {
                gpr_write(rd(i))
            }
        }
        // Add/subtract immediate with tags (MTE): refused.
        0b011 => Err(Refusal::Unknown),
        // Logical immediate: AND/ORR/EOR (opc != 11) → Rd 31 is sp; ANDS → xzr.
        0b100 => {
            if (i >> 29) & 3 != 3 {
                gpr_or_sp_write(rd(i))
            } else {
                gpr_write(rd(i))
            }
        }
        // Move wide, bitfield, extract: Rd (31 = xzr).
        0b101 | 0b110 | 0b111 => gpr_write(rd(i)),
        _ => Err(Refusal::Unknown),
    }
}

fn branch_sys(i: u32) -> Result<(), Refusal> {
    // B, BL (op 0b00101 in bits 30:26 with bit 31 either).
    if (i >> 26) & 0x1F == 0b00101 {
        return Ok(());
    }
    // Compare-and-branch, test-and-branch.
    if (i >> 25) & 0x3F == 0b011010 || (i >> 25) & 0x3F == 0b011011 {
        return Ok(());
    }
    // Conditional branch (B.cond, BC.cond).
    if (i >> 24) & 0xFF == 0b0101_0100 {
        return Ok(());
    }
    // Exception generation.
    if (i >> 24) & 0xFF == 0b1101_0100 {
        return match i {
            0xD400_0001 => Ok(()), // svc #0
            _ if (i >> 21) & 7 == 0b001 && i & 0x1F == 0 => Ok(()), // brk #imm
            _ => Err(Refusal::Exception),
        };
    }
    // Unconditional branch (register): BR, BLR, RET (plain forms only).
    if (i >> 25) & 0x7F == 0b110_1011 {
        let opc = (i >> 21) & 0xF;
        let plain = (i >> 10) & 0x7FF == 0b11111_000000 && i & 31 == 0;
        return match opc {
            0b0000 | 0b0001 | 0b0010 if plain => Ok(()), // BR, BLR, RET
            _ => Err(Refusal::System),
        };
    }
    // System: hints, barriers, timer reads.
    if (i >> 22) & 0x3FF == 0b1101_0101_00 {
        return system(i);
    }
    Err(Refusal::Unknown)
}

fn system(i: u32) -> Result<(), Refusal> {
    // Hints (NOP, YIELD, BTI, PAC*SP ...): MSR (immediate) space with CRn = 0010, Rt = 31.
    if i & 0xFFFF_F01F == 0xD503_201F {
        return Ok(());
    }
    // Barriers: CLREX, DSB, DMB, ISB, SB (CRn = 0011, op2 any, Rt = 31).
    if i & 0xFFFF_F01F == 0xD503_301F {
        return Ok(());
    }
    // MRS of CNTFRQ_EL0 (3,3,14,0,0) and CNTVCT_EL0 (3,3,14,0,2): Rt written.
    let sysreg = (i >> 5) & 0xFFFF;
    if (i >> 20) & 0xFFF == 0xD53 && (sysreg == 0xDF00 || sysreg == 0xDF02) {
        return gpr_write(rd(i));
    }
    Err(Refusal::System)
}

/// The base register is acceptable for an immediate-offset access: x25 or sp.
fn imm_base(n: u32, m: Mode) -> Result<(), Refusal> {
    let addr = match m {
        Mode::Ring => ADDR,
        Mode::Fixed(_) => OFF,
    };
    if n == addr || n == R31 {
        Ok(())
    } else {
        Err(Refusal::AddressForm)
    }
}

fn load_store(i: u32, m: Mode) -> Result<(), Refusal> {
    let op0 = (i >> 28) & 0xF;
    let op1 = (i >> 26) & 1;
    let op2 = (i >> 23) & 3;
    let op3 = (i >> 16) & 0x3F;
    let op4 = (i >> 10) & 3;
    // Advanced SIMD load/store multiple or single structures (LD1..ST4): only the no-offset forms (op2 = 00 or 10 with op3 = 0) on x25/sp.
    if op0 & 0b1011 == 0b0000 && op1 == 1 {
        if op2 & 1 == 0 && op3 & 0x1F == 0 && (i >> 23) & 1 == 0 {
            imm_base(rn(i), m)?;
            return Ok(());
        }
        return Err(Refusal::Writeback);
    }
    match op0 & 0b0011 {
        // Load/store exclusive, ordered, compare-and-swap: [Xn|SP] only.
        0b00 if op1 == 0 && op2 & 2 == 0 => {
            imm_base(rn(i), m)?;
            // Rs (bits 20:16) is a status/compare register written by store-exclusive and CAS; Rt / Rt2 by loads.
            gpr_write(rm(i))?;
            gpr_write(rd(i))?;
            gpr_write(rt2(i))
        }
        // Load register (literal): PC-relative.
        0b01 if op2 & 2 == 0 => Err(Refusal::LiteralLoad),
        // Load/store pair: offset form only (op2 = 10), base x25/sp, a non-negative offset (nothing below the arc base is the arc's: native arcs sit right under masked ones); loads write Rt and Rt2.
        0b10 => {
            if op2 != 0b10 {
                return Err(Refusal::Writeback);
            }
            imm_base(rn(i), m)?;
            if (i >> 21) & 1 == 1 {
                return Err(Refusal::AddressForm);
            }
            if (i >> 22) & 1 == 1 && op1 == 0 {
                gpr_write(rd(i))?;
                gpr_write(rt2(i))?;
            }
            Ok(())
        }
        0b11 => {
            let load_gpr = op1 == 0 && (i >> 22) & 3 != 0; // opc != 00 → load (or prefetch), for integer registers
            if op2 & 2 == 2 {
                // Unsigned immediate: base x25/sp, offset within G.
                imm_base(rn(i), m)?;
            } else if (i >> 21) & 1 == 0 {
                match op4 {
                    // Unscaled immediate (LDUR/STUR): base x25/sp, non-negative only (0 to 255; the emitter folds a negative offset into the masked address).
                    0b00 => {
                        imm_base(rn(i), m)?;
                        if (i >> 20) & 1 == 1 {
                            return Err(Refusal::AddressForm);
                        }
                    }
                    // Unprivileged (LDTR/STTR): refused.
                    0b10 => return Err(Refusal::AddressForm),
                    // Pre/post-index.
                    _ => return Err(Refusal::Writeback),
                }
            } else {
                match op4 {
                    // Atomic memory operations (LDADD, SWP ...): [Xn|SP], Rs read, Rt written.
                    0b00 => {
                        imm_base(rn(i), m)?;
                        gpr_write(rd(i))?;
                        return Ok(());
                    }
                    // Register offset: only [x28, x26] or the handle's [x23, x21], with LSL/UXTX #0.
                    0b10 => {
                        let option = (i >> 13) & 7;
                        let s = (i >> 12) & 1;
                        // [x28, x26] only in ring mode, where x26 is an offset; in fixed mode x26 is an address and [x28, x26] would leave the arc.
                        let pair = (m == Mode::Ring && rn(i) == BASE && rm(i) == OFF) || (rn(i) == HBASE && rm(i) == HOFF);
                        if !pair || option != 0b011 || s != 0 {
                            return Err(Refusal::AddressForm);
                        }
                    }
                    // Pointer-authenticated loads (LDRAA/LDRAB): refused.
                    _ => return Err(Refusal::AddressForm),
                }
            }
            if load_gpr {
                // Prefetch (size 11, opc 10) has no destination register.
                let prfm = (i >> 30) == 3 && (i >> 22) & 3 == 2;
                if !prfm {
                    gpr_write(rd(i))?;
                }
            }
            Ok(())
        }
        _ => Err(Refusal::Unknown),
    }
}

fn dp_reg(i: u32) -> Result<(), Refusal> {
    // Add/subtract (extended register): bits 28:24 = 01011, bit 21 = 1; S = 0 → Rd 31 is sp.
    if (i >> 24) & 0x1F == 0b01011 && (i >> 21) & 1 == 1 {
        return if (i >> 29) & 1 == 0 { gpr_or_sp_write(rd(i)) } else { gpr_write(rd(i)) };
    }
    // Conditional compare (CCMN/CCMP): bits 28:21 = 11010010, no register destination.
    if (i >> 21) & 0xFF == 0b1101_0010 {
        return Ok(());
    }
    // Evaluate into flags / rotate right into flags (SETF, RMIF): no register destination.
    if (i >> 21) & 0xFF == 0b1101_0000 && (i >> 10) & 0x1F != 0 {
        return Ok(());
    }
    // Everything else in this group writes Rd (31 = xzr): logical and add/sub shifted register, with carry, conditional select, data processing 1/2/3 source.
    gpr_write(rd(i))
}

fn simd_fp(i: u32) -> Result<(), Refusal> {
    // Advanced SIMD copy, SMOV/UMOV (to a general register): 0 Q op 01110000 imm5 0 imm4 1 Rn Rd with imm4 = 0101 / 0111.
    if i & 0x9FE0_8400 == 0x0E00_0400 {
        let imm4 = (i >> 11) & 0xF;
        if imm4 == 0b0101 || imm4 == 0b0111 {
            return gpr_write(rd(i));
        }
        return Ok(());
    }
    // Conversion between floating point and integer: sf 0 S 11110 type 1 rmode opcode 000000 Rn Rd. To-integer opcodes (FCVT*, FMOV to general, FJCVTZS) write a general register.
    if i & 0x5F20_FC00 == 0x1E20_0000 {
        let opcode = (i >> 16) & 7;
        if matches!(opcode, 0b000 | 0b001 | 0b100 | 0b101 | 0b110) {
            return gpr_write(rd(i));
        }
        return Ok(());
    }
    // All other SIMD/FP data processing writes vector/FP registers only.
    Ok(())
}

/// Check a whole image (little-endian A64 words). Returns the first violation, or the instruction count.
pub fn check_image(code: &[u8]) -> Result<usize, Violation> {
    check_image_in(code, Mode::Ring)
}

/// Check a whole image under `m`.
pub fn check_image_in(code: &[u8], m: Mode) -> Result<usize, Violation> {
    let mut n = 0;
    // The image header (ferros_rt start.rs, the payloads' entry stubs): a pad word at 4, "ferrosRT" at 8, then five words to 56, data the kernel reads, never executed (the entry at 0 branches over it).
    let header = code.len() >= 56 && &code[8..16] == b"ferrosRT";
    for (k, w) in code.chunks_exact(4).enumerate() {
        if header && (1..14).contains(&k) {
            continue;
        }
        let insn = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
        check_in(insn, m).map_err(|why| Violation { offset: 4 * k, insn, why })?;
        n += 1;
    }
    Ok(n)
}
