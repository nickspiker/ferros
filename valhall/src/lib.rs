//! A typed instruction builder for the Mali-G715's Valhall ISA (architecture v11).
//!
//! GPU kernels are written as Rust calls on a [`Kernel`], one method per instruction form, and come out as the 64-bit words the shader core executes.
//! There is no text anywhere: registers, uniforms and LUT immediates are distinct types, so a uniform can never be passed where a register pair is meant.
//! Only the forms the device-verified kernels use are here (the 3-instruction compute shader, the Mandelbrot loop, the present stage); the encodings are hand-written from Mesa's ISA.xml, each citing its lines.
//!
//! # Word layout (ISA.xml, va_pack.c)
//! src0 7:0, src1 15:8, src2 23:16; destination 47:40 (register 45:40, write mask 47:46); opcode 56:48; FAU page 58:57; flow 62:59.
//! A source byte is a register (G#00 | r), a uniform word (G#80 | slot << 1 | word) or a LUT immediate (G#C0 | index).
//!
//! # The flow rule
//! The flow field (bits 62:59) acts AFTER the instruction that carries it, never before.
//! Measured on the device: a `.wait0` placed on a load's consumer came one instruction too late, so the consumer read the register before the load landed.
//! To let instruction N read what a load on slot S wrote, put `.wait(S)` on instruction N - 1 (the load itself is fine when the consumer follows it directly).
//! Mesa does the same: va_insert_flow.c inserts a `NOP.wait` before the consumer and va_merge_flow.c folds it into the previous instruction.
//! The `.reconverge` flow goes on the last instruction before any change to the active-thread mask: every divergent branch, and the last instruction of a block whose fallthrough is a branch target.
//! The `.end` flow goes on the last instruction of the shader.
//! Every method returns a [`Placed`] handle that sets these on the instruction just emitted: `k.store_i32(r(0), pair(2), 0, Slot::S0).end();`.
//!
//! # The FAU rule
//! An ALU instruction may read one 64-bit uniform slot, and at most two distinct 32-bit words of uniforms and LUT immediates together (va_validate.c:10-17).
//! Breaking it is a construction-time panic: the hardware has one 64-bit FAU port per instruction, and an over-subscribed word would silently read the wrong constant.
//!
//! # Example
//! ```
//! use valhall::*;
//! let mut k = Kernel::<8>::new();
//! k.iadd_imm_i32(r(0), r(60), 0xC0FF_EE00);
//! k.shaddx_u64(pair(2), u(0), pair(60), 2);
//! k.store_i32(r(0), pair(2), 0, Slot::S0).end();
//! assert_eq!(k.finish(), &[0x0110_C0C0_FFEE_003C, 0x01A3_C200_2027_3C80, 0x7861_4002_1800_0002]);
//! ```

#![cfg_attr(not(test), no_std)]
#[cfg(feature = "ssa")]
extern crate alloc;
#[cfg(feature = "ssa")]
pub mod ssa;

// ---------------------------------------------------------------------------
// Operands
// ---------------------------------------------------------------------------

/// A 32-bit general register, r0 to r63.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Reg(u8);

/// Register `n`.
pub const fn r(n: u8) -> Reg {
    // WHY: the register field is 6 bits and bits 7:6 of a source byte are its type, so r64 would silently encode as a uniform or a discard; a construction-time panic keeps that out of the shader.
    assert!(n < 64, "valhall: registers are r0 to r63");
    Reg(n)
}

impl Reg {
    /// The register number.
    pub const fn index(self) -> u8 {
        self.0
    }
}

/// An aligned 64-bit register pair `[rN:rN+1]`, N even, low word in rN.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Pair(u8);

/// The pair whose low register is `lo`.
pub const fn pair(lo: u8) -> Pair {
    // WHY: Valhall encodes a pair by its low register and requires it even (va_pack.c:46-63); an odd one would name a pair the hardware does not have.
    assert!(lo < 64 && lo & 1 == 0, "valhall: a register pair starts at an even register below r64");
    Pair(lo)
}

impl Pair {
    /// The low register.
    pub const fn lo(self) -> Reg {
        Reg(self.0)
    }
    /// The high register.
    pub const fn hi(self) -> Reg {
        Reg(self.0 + 1)
    }
}

/// Which 32-bit half of a 64-bit uniform slot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Word {
    W0 = 0,
    W1 = 1,
}

/// A 64-bit fast-access uniform slot `uN` (the FAU buffer is an array of 64-bit entries).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Uniform64(u8);

/// Uniform slot `slot` (0 to 127; slots 32 and up go through the FAU page field).
pub const fn u(slot: u8) -> Uniform64 {
    // WHY: a slot index is 7 bits, 5 in the source byte and 2 in the page field (va_compiler.h:30-42); slot 128 would wrap to slot 0.
    assert!(slot < 128, "valhall: uniform slots are u0 to u127");
    Uniform64(slot)
}

impl Uniform64 {
    /// The low 32-bit word, `uN.w0`.
    pub const fn w0(self) -> Uniform {
        Uniform { slot: self.0, word: Word::W0 }
    }
    /// The high 32-bit word, `uN.w1`.
    pub const fn w1(self) -> Uniform {
        Uniform { slot: self.0, word: Word::W1 }
    }
    /// The slot index.
    pub const fn slot(self) -> u8 {
        self.0
    }
}

/// One 32-bit word of a uniform slot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Uniform {
    slot: u8,
    word: Word,
}

/// The hardware's immediate lookup table (ISA.xml:8-48): any entry is a source without a register or a uniform slot.
pub const LUT: [u32; 32] = [
    0x0000_0000, 0xFFFF_FFFF, 0x7FFF_FFFF, 0xFAFC_FDFE, 0x0100_0000, 0x8000_2000, 0x7060_5030, 0xC0B0_A090,
    0x0302_0100, 0x0706_0504, 0x0B0A_0908, 0x0F0E_0D0C, 0x1312_1110, 0x1716_1514, 0x1B1A_1918, 0x1F1E_1D1C,
    0x3F80_0000, 0x3DCC_CCCD, 0x3EA2_F983, 0x3F31_7218, 0x4049_0FDB, 0x0000_0000, 0x477F_FF00, 0x5C00_5BF8,
    0x2E66_0000, 0x3400_0000, 0x3800_0000, 0x3C00_0000, 0x4000_0000, 0x4400_0000, 0x4800_0000, 0x4248_0000,
];

/// An entry of the immediate lookup table, by index.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Lut(u8);

/// LUT entry `index` (0 to 31).
pub const fn lut(index: u8) -> Lut {
    // WHY: the table has 32 entries; index 32 and up encode the special FAU pages (ISA.xml:87-160), a different source entirely.
    assert!(index < 32, "valhall: the LUT has entries 0 to 31");
    Lut(index)
}

impl Lut {
    /// G#0000_0000: integer 0, float +0.0.
    pub const ZERO: Lut = Lut(0);
    /// G#FFFF_FFFF: all ones, integer -1.
    pub const ALL_ONES: Lut = Lut(1);
    /// Bytes 0, 1, 2, 3.
    pub const BYTES_0_3: Lut = Lut(8);
    /// Bytes 4, 5, 6, 7.
    pub const BYTES_4_7: Lut = Lut(9);
    /// Bytes 8, 9, 10, 11.
    pub const BYTES_8_11: Lut = Lut(10);
    /// Bytes 12, 13, 14, 15.
    pub const BYTES_12_15: Lut = Lut(11);
    /// Bytes 16, 17, 18, 19.
    pub const BYTES_16_19: Lut = Lut(12);
    /// Bytes 20, 21, 22, 23.
    pub const BYTES_20_23: Lut = Lut(13);
    /// Bytes 24, 25, 26, 27.
    pub const BYTES_24_27: Lut = Lut(14);
    /// Bytes 28, 29, 30, 31.
    pub const BYTES_28_31: Lut = Lut(15);
    /// Float 1.0.
    pub const F32_ONE: Lut = Lut(16);

    /// The 32-bit value this entry supplies.
    pub const fn value(self) -> u32 {
        LUT[self.0 as usize]
    }

    /// Entry `i` of [`LUT`] (the SSA compiler finds constants there by value).
    pub const fn index(i: u8) -> Lut {
        // WHY: the table has 32 entries; a larger index would name a source byte that is not an immediate.
        assert!(i < 32, "valhall: the LUT has 32 entries");
        Lut(i)
    }
}

/// A 32-bit source: a register, a uniform word, or a LUT immediate.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Src {
    Reg(Reg),
    Uniform(Uniform),
    Lut(Lut),
}

impl From<Reg> for Src {
    fn from(x: Reg) -> Src {
        Src::Reg(x)
    }
}
impl From<Uniform> for Src {
    fn from(x: Uniform) -> Src {
        Src::Uniform(x)
    }
}
impl From<Lut> for Src {
    fn from(x: Lut) -> Src {
        Src::Lut(x)
    }
}

/// A 64-bit source: a register pair or a whole uniform slot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Src64 {
    Pair(Pair),
    Uniform(Uniform64),
}

impl From<Pair> for Src64 {
    fn from(x: Pair) -> Src64 {
        Src64::Pair(x)
    }
}
impl From<Uniform64> for Src64 {
    fn from(x: Uniform64) -> Src64 {
        Src64::Uniform(x)
    }
}

/// A destination: a full register, or one 16-bit half of it (write mask 47:46, va_pack.c:146-158).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dst {
    /// The whole register (write mask 3).
    Full(Reg),
    /// The low half only (write mask 1); an f32 result is narrowed to f16 on the way (bifrost_compile.c:263-281).
    H0(Reg),
    /// The high half only (write mask 2).
    H1(Reg),
}

impl From<Reg> for Dst {
    fn from(x: Reg) -> Dst {
        Dst::Full(x)
    }
}

/// A 16-bit swizzle of a v2f16 source (ISA.xml:227-244): which halves feed lanes 0 and 1.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Swz16 {
    /// Both lanes from the low half.
    H00 = 0,
    /// Lanes swapped.
    H10 = 1,
    /// Identity.
    H01 = 2,
    /// Both lanes from the high half.
    H11 = 3,
}

/// An f32 operand: a source with optional negate and absolute value (applied as -|x| when both are set).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Float {
    pub src: Src,
    pub neg: bool,
    pub abs: bool,
}

impl Float {
    /// Negated (toggles, so `.neg().neg()` is the plain value).
    pub const fn neg(self) -> Float {
        Float { neg: !self.neg, ..self }
    }
    /// Absolute value (clears a negate taken before it, since |-x| = |x|).
    pub const fn abs(self) -> Float {
        Float { abs: true, neg: false, ..self }
    }
    /// As a v2f16 operand with both lanes from the low half.
    pub const fn h00(self) -> Half2 {
        Half2 { src: self.src, swz: Swz16::H00, neg: self.neg, abs: self.abs }
    }
    /// As a v2f16 operand with the lanes swapped.
    pub const fn h10(self) -> Half2 {
        Half2 { src: self.src, swz: Swz16::H10, neg: self.neg, abs: self.abs }
    }
    /// As a v2f16 operand, identity swizzle.
    pub const fn h01(self) -> Half2 {
        Half2 { src: self.src, swz: Swz16::H01, neg: self.neg, abs: self.abs }
    }
    /// As a v2f16 operand with both lanes from the high half.
    pub const fn h11(self) -> Half2 {
        Half2 { src: self.src, swz: Swz16::H11, neg: self.neg, abs: self.abs }
    }
}

/// A v2f16 operand: a source with a 16-bit swizzle, negate and absolute value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Half2 {
    pub src: Src,
    pub swz: Swz16,
    pub neg: bool,
    pub abs: bool,
}

impl Half2 {
    /// Negated (toggles).
    pub const fn neg(self) -> Half2 {
        Half2 { neg: !self.neg, ..self }
    }
    /// Absolute value (clears a negate taken before it).
    pub const fn abs(self) -> Half2 {
        Half2 { abs: true, neg: false, ..self }
    }
}

impl From<Float> for Half2 {
    fn from(f: Float) -> Half2 {
        f.h01()
    }
}

/// Modifier methods on every plain 32-bit source, so a negated register reads `r(2).neg()` and a broadcast half reads `r(13).h00()`.
pub trait Operand: Copy + Into<Src> {
    /// As an f32 operand, negated.
    fn neg(self) -> Float {
        Float { src: self.into(), neg: true, abs: false }
    }
    /// As an f32 operand, absolute value.
    fn abs(self) -> Float {
        Float { src: self.into(), neg: false, abs: true }
    }
    /// As a v2f16 operand with both lanes from the low half.
    fn h00(self) -> Half2 {
        Half2 { src: self.into(), swz: Swz16::H00, neg: false, abs: false }
    }
    /// As a v2f16 operand with the lanes swapped.
    fn h10(self) -> Half2 {
        Half2 { src: self.into(), swz: Swz16::H10, neg: false, abs: false }
    }
    /// As a v2f16 operand, identity swizzle.
    fn h01(self) -> Half2 {
        Half2 { src: self.into(), swz: Swz16::H01, neg: false, abs: false }
    }
    /// As a v2f16 operand with both lanes from the high half.
    fn h11(self) -> Half2 {
        Half2 { src: self.into(), swz: Swz16::H11, neg: false, abs: false }
    }
}

macro_rules! operand {
    ($($t:ty),*) => {$(
        impl Operand for $t {}
        impl From<$t> for Float {
            fn from(x: $t) -> Float {
                Float { src: x.into(), neg: false, abs: false }
            }
        }
        impl From<$t> for Half2 {
            fn from(x: $t) -> Half2 {
                Half2 { src: x.into(), swz: Swz16::H01, neg: false, abs: false }
            }
        }
    )*};
}
operand!(Reg, Uniform, Lut, Src);

// ---------------------------------------------------------------------------
// Instruction fields
// ---------------------------------------------------------------------------

/// A dependency slot of a message instruction (ISA.xml:591-605); a later reader of its destination waits on it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Slot {
    S0 = 0,
    S1 = 1,
    S2 = 2,
}

/// The flow field, bits 62:59 (ISA.xml:50-85).
/// Values 1 to 7 are the wait sets of slots 0, 1 and 2 as a bit mask, which is how [`Placed::wait`] combines them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Flow {
    None = 0,
    Wait0 = 1,
    Wait1 = 2,
    Wait01 = 3,
    Wait2 = 4,
    Wait02 = 5,
    Wait12 = 6,
    Wait012 = 7,
    Wait0126 = 8,
    /// Slots 0, 1, 2, 6 and 7: barriers.
    Wait = 9,
    Reconverge = 10,
    WaitResource = 12,
    Discard = 13,
    End = 15,
}

/// One 16-bit half of a register, for sources that take a lane select.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Half {
    H0 = 0,
    H1 = 1,
}

/// Which byte of the shift source holds the shift amount (`Lanes (8-bit)`, ISA.xml:184-202; b00 to b33 are encodings 4 to 7).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Byte {
    B0 = 4,
    B1 = 5,
    B2 = 6,
    B3 = 7,
}

/// A compare condition (ISA.xml:461-476).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cond {
    Eq = 0,
    Gt = 1,
    Ge = 2,
    Ne = 3,
    Lt = 4,
    Le = 5,
    GtLt = 6,
    Total = 7,
}

/// How a compare writes its boolean (ISA.xml:395-409).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CmpResult {
    /// 0 or 1.
    I1 = 0,
    /// 0.0 or 1.0.
    F1 = 1,
    /// 0 or all ones.
    M1 = 2,
    /// Low half of a 64-bit compare.
    U1 = 3,
}

/// What `MUX` tests in its mask (ISA.xml:665-680, in the enum's order).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mux {
    /// The mask's sign bit.
    Neg = 0,
    /// The mask is 0x00000000.
    IntZero = 1,
    /// The mask is ±0.0.
    FpZero = 2,
    /// Each bit: a's where the mask's is set, b's elsewhere.
    Bit = 3,
}

/// IEEE 754 rounding (ISA.xml:387-393, in the enum's order).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Round {
    /// To nearest, ties to even.
    Rte = 0,
    /// Towards +∞ (ceil).
    Rtp = 1,
    /// Towards −∞ (floor).
    Rtn = 2,
    /// Towards zero (truncate).
    Rtz = 3,
}

/// A branch target, bound to an instruction index by [`Kernel::bind`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Label(u8);

// ---------------------------------------------------------------------------
// One instruction under construction
// ---------------------------------------------------------------------------

/// Field positions shared by the ALU forms (va_pack.c `va_pack_alu`).
const OPCODE: u32 = 48;
const DEST: u32 = 40;
const PAGE: u32 = 57;
const FLOW: u32 = 59;
/// Write mask in 47:46: full, low half, high half (va_pack.c:146-158).
const MASK_FULL: u64 = 3;
const MASK_H0: u64 = 1;
const MASK_H1: u64 = 2;
/// A staging register's flags in 47:46: read for a store, write for a load (valhall.py:166-172).
const SR_READ: u64 = 1;
const SR_WRITE: u64 = 2;

/// One instruction word plus the FAU it has claimed so far.
struct Ins {
    word: u64,
    /// The one 64-bit uniform slot this instruction reads, if any.
    slot: Option<u8>,
    /// The distinct 32-bit FAU words read: G#0xx = uniform (slot << 1 | word), G#1xx = LUT index.
    words: [u16; 2],
    used: usize,
}

impl Ins {
    fn new(opcode: u64) -> Ins {
        Ins { word: opcode << OPCODE, slot: None, words: [0; 2], used: 0 }
    }

    fn bits(&mut self, value: u64, at: u32) {
        self.word |= value << at;
    }

    /// Claims one 32-bit FAU word, enforcing the two-word budget.
    fn fau_word(&mut self, key: u16) {
        if self.words[..self.used].contains(&key) {
            return;
        }
        // WHY: an instruction reads at most 64 bits of uniforms and LUT immediates combined (va_validate.c:12-13); a third distinct word has no port to come in on.
        assert!(self.used < 2, "valhall: an instruction may read at most two distinct 32-bit uniform/LUT words");
        self.words[self.used] = key;
        self.used += 1;
    }

    /// Claims a uniform slot, enforcing the one-slot rule, and sets its FAU page.
    fn fau_slot(&mut self, slot: u8) {
        match self.slot {
            None => {
                self.slot = Some(slot);
                self.bits((slot >> 5) as u64, PAGE);
            }
            // WHY: the hardware fetches one 64-bit FAU slot per instruction (va_validate.c:10-11); a second slot would read whatever the first slot's port carries.
            Some(s) => assert!(s == slot, "valhall: an instruction may read only one 64-bit uniform slot"),
        }
    }

    /// Encodes a 32-bit source byte at bit `at`.
    fn src(&mut self, s: impl Into<Src>, at: u32) {
        let byte = match s.into() {
            Src::Reg(x) => x.0 as u64,
            Src::Uniform(x) => {
                self.fau_slot(x.slot);
                self.fau_word(((x.slot as u16) << 1) | x.word as u16);
                // The low 5 bits of the slot go in the byte; the top 2 are the page (va_pack.c:117-130).
                0x80 | ((x.slot & 31) as u64) << 1 | x.word as u64
            }
            Src::Lut(x) => {
                self.fau_word(0x100 | x.0 as u16);
                0xC0 | x.0 as u64
            }
        };
        self.bits(byte, at);
    }

    /// Encodes a 64-bit source byte at bit `at`; a whole uniform slot claims both its words.
    fn src64(&mut self, s: impl Into<Src64>, at: u32) {
        let byte = match s.into() {
            Src64::Pair(p) => p.0 as u64,
            Src64::Uniform(x) => {
                self.fau_slot(x.0);
                self.fau_word((x.0 as u16) << 1);
                self.fau_word(((x.0 as u16) << 1) | 1);
                0x80 | ((x.0 & 31) as u64) << 1
            }
        };
        self.bits(byte, at);
    }

    /// An f32 source in ALU position `i`: byte 8i, negate at 38 - 2i, abs at 39 - 2i (va_pack.c `va_pack_alu`); the f32 widen field stays 0 (none).
    fn float(&mut self, f: impl Into<Float>, i: u32) {
        let f = f.into();
        self.src(f.src, 8 * i);
        self.bits(f.neg as u64, 38 - 2 * i);
        self.bits(f.abs as u64, 39 - 2 * i);
    }

    /// A v2f16 source in ALU position `i`: as [`Ins::float`] plus the 16-bit swizzle at 28 - 2i.
    fn half2(&mut self, h: impl Into<Half2>, i: u32) {
        let h = h.into();
        self.src(h.src, 8 * i);
        self.bits(h.swz as u64, 28 - 2 * i);
        self.bits(h.neg as u64, 38 - 2 * i);
        self.bits(h.abs as u64, 39 - 2 * i);
    }

    fn dst(&mut self, d: impl Into<Dst>) {
        let (x, mask) = match d.into() {
            Dst::Full(x) => (x, MASK_FULL),
            Dst::H0(x) => (x, MASK_H0),
            Dst::H1(x) => (x, MASK_H1),
        };
        self.bits(x.0 as u64 | mask << 6, DEST);
    }

    fn dst64(&mut self, p: Pair) {
        self.bits(p.0 as u64 | MASK_FULL << 6, DEST);
    }
}

// ---------------------------------------------------------------------------
// The builder
// ---------------------------------------------------------------------------

/// Branch targets one kernel may declare.
const LABELS: usize = 1 << 5;
/// Branches one kernel may emit.
const BRANCHES: usize = 1 << 6;

/// A kernel under construction, holding at most `N` instruction words with no allocator.
/// Running past `N` words, [`LABELS`] labels or [`BRANCHES`] branches panics on the array index: a kernel that does not fit is a bug in the caller's sizing.
pub struct Kernel<const N: usize> {
    words: [u64; N],
    len: usize,
    labels: [Option<u32>; LABELS],
    nlabels: usize,
    /// (instruction index, label) for every branch, resolved by [`Kernel::finish`].
    branches: [(u32, u8); BRANCHES],
    nbranches: usize,
}

/// The instruction just emitted, for setting its flow field.
pub struct Placed<'k> {
    word: &'k mut u64,
}

impl Placed<'_> {
    fn current(&self) -> u64 {
        (*self.word >> FLOW) & 15
    }

    /// Waits on `slot` after this instruction (so the NEXT instruction sees the slot's result); repeated calls build wait01, wait012 and so on.
    pub fn wait(self, slot: Slot) -> Self {
        let f = self.current();
        // WHY: the flow field holds one value, and only 0 to 7 are wait sets of slots 0-2; OR-ing a slot bit into reconverge or end would make a different flow.
        assert!(f <= 7, "valhall: a wait cannot share the flow field with reconverge, end or a special wait");
        *self.word |= (1u64 << slot as u64) << FLOW;
        self
    }

    /// Marks the last instruction before a possible change of the active-thread mask.
    pub fn reconverge(self) -> Self {
        self.flow(Flow::Reconverge)
    }

    /// Ends the shader after this instruction.
    pub fn end(self) -> Self {
        self.flow(Flow::End)
    }

    /// Sets any flow value.
    pub fn flow(self, f: Flow) -> Self {
        // WHY: the flow field holds one value; overwriting a wait with end (or the reverse) would silently drop the first.
        assert!(self.current() == 0, "valhall: this instruction already carries a flow");
        *self.word |= (f as u64) << FLOW;
        self
    }
}

impl<const N: usize> Default for Kernel<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Kernel<N> {
    /// An empty kernel.
    pub const fn new() -> Self {
        // A branch offset is a 27-bit signed field in instructions (ISA.xml:822); with N at most 2^26, every offset between two of this kernel's instructions fits, so no branch needs a range check.
        const { assert!(N <= 1 << 26, "valhall: a kernel is at most 2^26 instructions") };
        Kernel { words: [0; N], len: 0, labels: [None; LABELS], nlabels: 0, branches: [(0, 0); BRANCHES], nbranches: 0 }
    }

    /// Number of instructions emitted so far.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// True before the first instruction.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn push(&mut self, ins: Ins) -> Placed<'_> {
        let at = self.len;
        self.words[at] = ins.word;
        self.len = at + 1;
        Placed { word: &mut self.words[at] }
    }

    /// A new label, not yet bound.
    pub fn label(&mut self) -> Label {
        let l = self.nlabels;
        // Already None; the write is here so a kernel past LABELS labels panics on this index, at the label's creation.
        self.labels[l] = None;
        self.nlabels = l + 1;
        Label(l as u8)
    }

    /// Binds `l` to the next instruction emitted.
    pub fn bind(&mut self, l: Label) {
        // WHY: binding twice would leave earlier branches aimed at a target the caller no longer means.
        assert!(self.labels[l.0 as usize].is_none(), "valhall: label bound twice");
        self.labels[l.0 as usize] = Some(self.len as u32);
    }

    /// A new label bound to the next instruction emitted (a loop head).
    pub fn here(&mut self) -> Label {
        let l = self.label();
        self.bind(l);
        l
    }

    /// The last instruction emitted, for setting its flow field after the fact (the SSA compiler ends a kernel this way). Panics on an empty kernel: there is nothing to end.
    pub fn last(&mut self) -> Placed<'_> {
        let at = self.len - 1;
        Placed { word: &mut self.words[at] }
    }

    /// Resolves every branch and returns the words.
    pub fn finish(&mut self) -> &[u64] {
        for &(at, l) in &self.branches[..self.nbranches] {
            let target = self.labels[l as usize].expect("valhall: branch to a label never bound");
            // Relative to the next instruction, in instructions (ISA.xml:810-818).
            let offset = target as i64 - (at as i64 + 1);
            self.words[at as usize] |= (offset as u64 & ((1 << 27) - 1)) << 8;
        }
        self.nbranches = 0;
        &self.words[..self.len]
    }

    // --- Integer ALU -------------------------------------------------------

    /// `IADD_IMM.i32 dst, a, #imm`: a + a 32-bit immediate (ISA.xml:3499-3513); with `a` = [`Lut::ZERO`] it is the canonical immediate move.
    pub fn iadd_imm_i32(&mut self, dst: impl Into<Dst>, a: impl Into<Src>, imm: u32) -> Placed<'_> {
        let mut i = Ins::new(0x110);
        i.src(a, 0);
        i.bits(imm as u64, 8);
        i.dst(dst);
        self.push(i)
    }

    /// `MOV.i32 dst, a` (ISA.xml:2053-2060).
    pub fn mov_i32(&mut self, dst: impl Into<Dst>, a: impl Into<Src>) -> Placed<'_> {
        let mut i = Ins::new(0x091);
        i.src(a, 0);
        i.dst(dst);
        self.push(i)
    }

    /// `IADD.u32 dst, a, b`: wrapping a + b (ISA.xml:2512-2517, opcode2 0 at 19:16).
    pub fn iadd_u32(&mut self, dst: impl Into<Dst>, a: impl Into<Src>, b: impl Into<Src>) -> Placed<'_> {
        self.int2(0x0A0, 0x0, dst, a, b)
    }

    /// `ISUB.s32 dst, a, b`: wrapping a - b (ISA.xml:2629-2634, opcode2 1 at 19:16).
    pub fn isub_s32(&mut self, dst: impl Into<Dst>, a: impl Into<Src>, b: impl Into<Src>) -> Placed<'_> {
        self.int2(0x0A8, 0x1, dst, a, b)
    }

    /// `IMUL.i32 dst, a, b`: the low 32 bits of a * b (ISA.xml:2710-2715, opcode2 G#A at 19:16).
    pub fn imul_i32(&mut self, dst: impl Into<Dst>, a: impl Into<Src>, b: impl Into<Src>) -> Placed<'_> {
        self.int2(0x0A0, 0xA, dst, a, b)
    }

    /// Two-source integer group: widen fields (36, 26) and saturate (30) left 0.
    fn int2(&mut self, opcode: u64, opcode2: u64, dst: impl Into<Dst>, a: impl Into<Src>, b: impl Into<Src>) -> Placed<'_> {
        let mut i = Ins::new(opcode);
        i.bits(opcode2, 16);
        i.src(a, 0);
        i.src(b, 8);
        i.dst(dst);
        self.push(i)
    }

    /// `SHADDX.u64 dst, a, b.w0, shift:s`: a + (zero-extended low word of b << s), for addresses (ISA.xml:2686-2703).
    /// `b` is named as a pair, as Mesa does, but only its low register is read.
    pub fn shaddx_u64(&mut self, dst: Pair, a: impl Into<Src64>, b: Pair, shift: u8) -> Placed<'_> {
        // WHY: the shift is a 3-bit field at 22:20; 8 or more would spill into the source byte above it.
        assert!(shift < 8, "valhall: SHADDX shifts by 0 to 7");
        let mut i = Ins::new(0x1A3);
        i.bits(0x7, 16);
        i.bits(shift as u64, 20);
        i.src64(a, 0);
        i.src64(b, 8);
        // B's widen at 29:26 = 8, `w0` in `Swizzles (64-bit)` (ISA.xml:257-274).
        i.bits(8, 26);
        i.dst64(dst);
        self.push(i)
    }

    /// `LSHIFT_AND.i32 dst, a, shift.bN, b`: (a << byte N of shift) & b (ISA.xml:2849-2854, opcode2 G#100 under mask G#703 at 24).
    pub fn lshift_and_i32(&mut self, dst: impl Into<Dst>, a: impl Into<Src>, shift: impl Into<Src>, byte: Byte, b: impl Into<Src>) -> Placed<'_> {
        self.shift(1 << 32, dst, a, shift, byte, b)
    }

    /// `LSHIFT_OR.i32 dst, a, shift.bN, b`: (a << byte N of shift) | b (ISA.xml:2946-2951, opcode2 G#101).
    pub fn lshift_or_i32(&mut self, dst: impl Into<Dst>, a: impl Into<Src>, shift: impl Into<Src>, byte: Byte, b: impl Into<Src>) -> Placed<'_> {
        self.shift(1 << 32 | 1 << 24, dst, a, shift, byte, b)
    }

    /// `RSHIFT_OR.i32 dst, a, shift.bN, b`: (a >> byte N of shift, logical) | b (ISA.xml:2981-2986, opcode2 G#001).
    pub fn rshift_or_i32(&mut self, dst: impl Into<Dst>, a: impl Into<Src>, shift: impl Into<Src>, byte: Byte, b: impl Into<Src>) -> Placed<'_> {
        self.shift(1 << 24, dst, a, shift, byte, b)
    }

    /// The shift group: sources in ISA order A, shift, B (Mesa's IR swaps 1 and 2, valhall.h `va_swap_12`); the shift byte's lane at 29:26; not-B (35), not-result (30) and A's widen (39:36) left 0.
    fn shift(&mut self, opcode2: u64, dst: impl Into<Dst>, a: impl Into<Src>, shift: impl Into<Src>, byte: Byte, b: impl Into<Src>) -> Placed<'_> {
        let mut i = Ins::new(0x0B4);
        i.word |= opcode2;
        i.src(a, 0);
        i.src(shift, 8);
        i.src(b, 16);
        i.bits(byte as u64, 26);
        i.dst(dst);
        self.push(i)
    }

    /// `LSHIFT_XOR.i32 dst, a, shift.bN, b`: (a << byte N of shift) ^ b (ISA.xml:3043-3048, opcode2 G#102).
    pub fn lshift_xor_i32(&mut self, dst: impl Into<Dst>, a: impl Into<Src>, shift: impl Into<Src>, byte: Byte, b: impl Into<Src>) -> Placed<'_> {
        self.shift(1 << 32 | 2 << 24, dst, a, shift, byte, b)
    }

    /// `MUX.i32.mode dst, a, b, mask` (ISA.xml:3139-3154): [`Mux::Bit`] takes a's bits where the mask's are set and b's elsewhere, so a compare's all-ones/zero result selects whole words: `select(mask, a, b)`.
    pub fn mux_i32(&mut self, dst: impl Into<Dst>, mode: Mux, a: impl Into<Src>, b: impl Into<Src>, mask: impl Into<Src>) -> Placed<'_> {
        let mut i = Ins::new(0x0B8);
        i.src(a, 0);
        i.src(b, 8);
        i.src(mask, 16);
        i.bits(mode as u64, 32);
        i.dst(dst);
        self.push(i)
    }

    /// `ICMP_OR.u32.cond.result dst, a, b, or`: (a cond b) OR the boolean in `or`; tie `or` to [`Lut::ZERO`] for a plain compare (ISA.xml:3288-3293, group 3280-3312).
    pub fn icmp_or_u32(&mut self, dst: impl Into<Dst>, cond: Cond, result: CmpResult, a: impl Into<Src>, b: impl Into<Src>, or: impl Into<Src>) -> Placed<'_> {
        let mut i = Ins::new(0x0F0);
        i.src(a, 0);
        i.src(b, 8);
        i.src(or, 16);
        i.bits(result as u64, 30);
        i.bits(cond as u64, 32);
        i.dst(dst);
        self.push(i)
    }

    // --- Conversions -------------------------------------------------------

    /// `U32_TO_F32 dst, a`, round to nearest even (ISA.xml:1838-1843, opcode2 G#19 at 20:16).
    pub fn u32_to_f32(&mut self, dst: impl Into<Dst>, a: impl Into<Src>) -> Placed<'_> {
        let mut i = Ins::new(0x090);
        i.bits(0x19, 16);
        i.src(a, 0);
        i.dst(dst);
        self.push(i)
    }

    /// `S32_TO_F32 dst, a`, round to nearest even (ISA.xml:1823-1828, opcode2 9).
    pub fn s32_to_f32(&mut self, dst: impl Into<Dst>, a: impl Into<Src>) -> Placed<'_> {
        let mut i = Ins::new(0x090);
        i.bits(0x9, 16);
        i.src(a, 0);
        i.dst(dst);
        self.push(i)
    }

    /// `F32_TO_S32.round dst, a` (ISA.xml:1888-1893, opcode2 G#C; the round mode at 31:30, ISA.xml:387 and valhall.py `roundmode`).
    pub fn f32_to_s32(&mut self, dst: impl Into<Dst>, a: impl Into<Src>, round: Round) -> Placed<'_> {
        self.convert(0xC, dst, a, round)
    }

    /// `F32_TO_U32.round dst, a` (ISA.xml:1894-1899, opcode2 G#1C).
    pub fn f32_to_u32(&mut self, dst: impl Into<Dst>, a: impl Into<Src>, round: Round) -> Placed<'_> {
        self.convert(0x1C, dst, a, round)
    }

    /// `FROUND.f32.round dst, a`: a rounded to an integral value, as a float (ISA.xml:2034-2039, opcode2 G#D); [`Round::Rtn`] is floor.
    pub fn fround_f32(&mut self, dst: impl Into<Dst>, a: impl Into<Src>, round: Round) -> Placed<'_> {
        self.convert(0xD, dst, a, round)
    }

    fn convert(&mut self, opcode2: u64, dst: impl Into<Dst>, a: impl Into<Src>, round: Round) -> Placed<'_> {
        let mut i = Ins::new(0x090);
        i.bits(opcode2, 16);
        i.src(a, 0);
        i.bits(round as u64, 30);
        i.dst(dst);
        self.push(i)
    }

    /// `U16_TO_U32 dst, a.hN`: zero-extends one half (ISA.xml:1868-1873, opcode2 G#14 at 20:16, lane at 28).
    pub fn u16_to_u32(&mut self, dst: impl Into<Dst>, a: impl Into<Src>, half: Half) -> Placed<'_> {
        let mut i = Ins::new(0x090);
        i.bits(0x14, 16);
        i.src(a, 0);
        i.bits(half as u64, 28);
        i.dst(dst);
        self.push(i)
    }

    /// `MKVEC.v2i16 dst, a.hN, b.hM`: a's half | b's half << 16 (ISA.xml:2567-2575, opcode2 5 at 19:16, lanes at 28 and 26).
    pub fn mkvec_v2i16(&mut self, dst: impl Into<Dst>, a: impl Into<Src>, a_half: Half, b: impl Into<Src>, b_half: Half) -> Placed<'_> {
        let mut i = Ins::new(0x0A1);
        i.bits(0x5, 16);
        i.src(a, 0);
        i.src(b, 8);
        i.bits(a_half as u64, 28);
        i.bits(b_half as u64, 26);
        i.dst(dst);
        self.push(i)
    }

    // --- Float ALU ---------------------------------------------------------

    /// `FADD.f32 dst, a, b`, round to nearest even (ISA.xml:2361-2366, group 2360-2378); a [`Dst::H0`] destination narrows the sum to f16.
    pub fn fadd_f32(&mut self, dst: impl Into<Dst>, a: impl Into<Float>, b: impl Into<Float>) -> Placed<'_> {
        let mut i = Ins::new(0x0A4);
        i.float(a, 0);
        i.float(b, 1);
        i.dst(dst);
        self.push(i)
    }

    /// `FADD.v2f16 dst, a, b`, per lane (ISA.xml:2367-2372).
    pub fn fadd_v2f16(&mut self, dst: impl Into<Dst>, a: impl Into<Half2>, b: impl Into<Half2>) -> Placed<'_> {
        let mut i = Ins::new(0x0A5);
        i.half2(a, 0);
        i.half2(b, 1);
        i.dst(dst);
        self.push(i)
    }

    /// `FMA.f32 dst, a, b, c`: a * b + c with one rounding (ISA.xml:2830-2834, group 2829-2846); with `c` = `Lut::ZERO.neg()` it is Mesa's fmul.
    pub fn fma_f32(&mut self, dst: impl Into<Dst>, a: impl Into<Float>, b: impl Into<Float>, c: impl Into<Float>) -> Placed<'_> {
        let mut i = Ins::new(0x0B2);
        i.float(a, 0);
        i.float(b, 1);
        i.float(c, 2);
        i.dst(dst);
        self.push(i)
    }

    /// `FMA.v2f16 dst, a, b, c`, per lane (ISA.xml:2835-2839).
    pub fn fma_v2f16(&mut self, dst: impl Into<Dst>, a: impl Into<Half2>, b: impl Into<Half2>, c: impl Into<Half2>) -> Placed<'_> {
        let mut i = Ins::new(0x0B3);
        i.half2(a, 0);
        i.half2(b, 1);
        i.half2(c, 2);
        i.dst(dst);
        self.push(i)
    }

    /// `FRCP.f32 dst, a`: 1 / a (ISA.xml:2256-2261, source modifiers 2325).
    pub fn frcp_f32(&mut self, dst: impl Into<Dst>, a: impl Into<Float>) -> Placed<'_> {
        let mut i = Ins::new(0x09C);
        i.float(a, 0);
        i.dst(dst);
        self.push(i)
    }

    /// `FMIN.f32 dst, a, b` (ISA.xml:2382-2398, opcode G#A4 with opcode2 2 at 19:16).
    pub fn fmin_f32(&mut self, dst: impl Into<Dst>, a: impl Into<Float>, b: impl Into<Float>) -> Placed<'_> {
        let mut i = Ins::new(0x0A4);
        i.bits(0x2, 16);
        i.float(a, 0);
        i.float(b, 1);
        i.dst(dst);
        self.push(i)
    }

    /// `FMAX.f32 dst, a, b` (ISA.xml:2401-2417, opcode2 3).
    pub fn fmax_f32(&mut self, dst: impl Into<Dst>, a: impl Into<Float>, b: impl Into<Float>) -> Placed<'_> {
        let mut i = Ins::new(0x0A4);
        i.bits(0x3, 16);
        i.float(a, 0);
        i.float(b, 1);
        i.dst(dst);
        self.push(i)
    }

    /// `FRSQ.f32 dst, a`: 1 / sqrt(a) (ISA.xml:2268-2273, opcode2 2 under SFU G#9C).
    pub fn frsq_f32(&mut self, dst: impl Into<Dst>, a: impl Into<Float>) -> Placed<'_> {
        let mut i = Ins::new(0x09C);
        i.bits(0x2, 16);
        i.float(a, 0);
        i.dst(dst);
        self.push(i)
    }

    /// `FCMP_OR.f32.cond.result dst, a, b, or` (ISA.xml:3355-3360, group 3347-3372).
    pub fn fcmp_or_f32(&mut self, dst: impl Into<Dst>, cond: Cond, result: CmpResult, a: impl Into<Float>, b: impl Into<Float>, or: impl Into<Src>) -> Placed<'_> {
        let mut i = Ins::new(0x0F4);
        i.float(a, 0);
        i.float(b, 1);
        i.src(or, 16);
        i.bits(result as u64, 30);
        i.bits(cond as u64, 32);
        i.dst(dst);
        self.push(i)
    }

    // --- Branches ----------------------------------------------------------

    /// `BRANCHZ cond, target`: branches when `cond` is NONZERO (the ISA's default sense, despite the name; ISA.xml:804-827).
    /// A divergent branch needs `.reconverge()`.
    pub fn branchz(&mut self, cond: impl Into<Src>, target: Label) -> Placed<'_> {
        self.branch(cond, target, false)
    }

    /// `BRANCHZ.eq cond, target`: branches when `cond` is zero (bit 36).
    pub fn branchz_eq(&mut self, cond: impl Into<Src>, target: Label) -> Placed<'_> {
        self.branch(cond, target, true)
    }

    /// An unconditional jump: `BRANCHZ.eq` on the LUT zero, Mesa's lowering of a jump (va_lower_isel.c).
    pub fn jump(&mut self, target: Label) -> Placed<'_> {
        self.branch(Lut::ZERO, target, true)
    }

    fn branch(&mut self, cond: impl Into<Src>, target: Label, eq: bool) -> Placed<'_> {
        let mut i = Ins::new(0x01F);
        i.src(cond, 0);
        i.bits(eq as u64, 36);
        // No destination: Mesa writes the G#C0 placeholder there (va_pack.c, "Placeholder").
        i.bits(0xC0, DEST);
        let n = self.nbranches;
        self.branches[n] = (self.len as u32, target.0);
        self.nbranches = n + 1;
        self.push(i)
    }

    // --- Memory ------------------------------------------------------------

    /// `LOAD.i8.w0.unsigned.slotS @dst, [addr], offset`: one byte, zero-extended to 32 bits (ISA.xml:1454-1468; lane `w0` = 6, ISA.xml:300-309).
    pub fn load_i8_w0(&mut self, dst: Reg, addr: Pair, offset: i16, slot: Slot) -> Placed<'_> {
        self.load(0, 6, 1, dst, addr, offset, slot)
    }

    /// `LOAD.i16.w0.unsigned.slotS @dst, [addr], offset`: one half-word, zero-extended to 32 bits (ISA.xml:1470-1484; lane `w0` = 2, ISA.xml:311-320).
    pub fn load_i16_w0(&mut self, dst: Reg, addr: Pair, offset: i16, slot: Slot) -> Placed<'_> {
        self.load(1, 2, 1, dst, addr, offset, slot)
    }

    /// `LOAD.i32.unsigned.slotS @dst, [addr], offset` (ISA.xml:1502-1516; lane `w0` = 0).
    pub fn load_i32(&mut self, dst: Reg, addr: Pair, offset: i16, slot: Slot) -> Placed<'_> {
        self.load(3, 0, 1, dst, addr, offset, slot)
    }

    /// `LOAD.i64.unsigned.slotS @dst, [addr], offset` (ISA.xml:1534-1548; lane `identity` = 7, ISA.xml:354-363).
    pub fn load_i64(&mut self, dst: Pair, addr: Pair, offset: i16, slot: Slot) -> Placed<'_> {
        self.load(5, 7, 2, dst.lo(), addr, offset, slot)
    }

    /// The LOAD form (va_pack.c `va_pack_instr` + `va_pack_load`): size at 29:27, staging write register at 45:40 with flags 47:46, count at 35:33, lane at 38:36, unsigned bit 39 (Mesa always sets it), slot at 32:30, memory access 25:24 (none), signed byte offset 23:8, address pair 7:0.
    #[allow(clippy::too_many_arguments)]
    fn load(&mut self, size: u64, lane: u64, count: u64, dst: Reg, addr: Pair, offset: i16, slot: Slot) -> Placed<'_> {
        let mut i = Ins::new(0x060);
        i.bits(size, 27);
        i.bits(dst.0 as u64 | SR_WRITE << 6, DEST);
        i.bits(count, 33);
        i.bits(lane, 36);
        i.bits(1, 39);
        i.bits(slot as u64, 30);
        i.bits(offset as u16 as u64, 8);
        i.bits(addr.0 as u64, 0);
        self.push(i)
    }

    /// `STORE.i32.slotS @src, [addr], offset` (ISA.xml:1603-1608, group 1582-1638): staging read register at 45:40 with flags 47:46, count 1 at 35:33.
    pub fn store_i32(&mut self, src: Reg, addr: Pair, offset: i16, slot: Slot) -> Placed<'_> {
        let mut i = Ins::new(0x061);
        i.bits(3, 27);
        i.bits(src.0 as u64 | SR_READ << 6, DEST);
        i.bits(1, 33);
        i.bits(slot as u64, 30);
        i.bits(offset as u16 as u64, 8);
        i.bits(addr.0 as u64, 0);
        self.push(i)
    }
}
