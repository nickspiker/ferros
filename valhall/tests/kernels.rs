//! Every kernel already run on the device, rebuilt through the builder and compared word for word with the verified words.
//! Golden words: payloads/gpu/src/main.rs (the compute shader, `MANDEL`, `PRESENT`, the load bench) and Harbor's ferros-compute-spec.md, ferros-mandel-spec.md, ferros-present-spec.md section 3.

use valhall::*;

/// Asserts word by word, so a failure names the instruction.
fn same(built: &[u64], gold: &[u64]) {
    for (n, (b, g)) in built.iter().zip(gold).enumerate() {
        assert_eq!(b, g, "instruction {n}: built G#{b:016X}, verified G#{g:016X}");
    }
    assert_eq!(built.len(), gold.len(), "instruction count");
}

#[test]
fn compute_shader() {
    const GOLD: [u64; 3] = [0x0110_C0C0_FFEE_003C, 0x01A3_C200_2027_3C80, 0x7861_4002_1800_0002];
    let mut k = Kernel::<4>::new();
    k.iadd_imm_i32(r(0), r(60), 0xC0FF_EE00);
    k.shaddx_u64(pair(2), u(0), pair(60), 2);
    k.store_i32(r(0), pair(2), 0, Slot::S0).end();
    same(k.finish(), &GOLD);
}

#[test]
fn mandelbrot() {
    const GOLD: [u64; 26] = [
        0x00B4_CD00_11C0_823C, 0x00B4_CC01_11C0_820D, 0x00A8_CC00_0001_0C3C, 0x0090_CC00_0019_000C, 0x0090_CD00_0019_000D, 0x0091_CA00_0000_0086, 0x00B2_C600_0084_0A0C, 0x00B2_C700_0085_0A0D,
        0x0110_C440_8000_00C0, 0x0110_C000_0000_00C0, 0x0110_C100_0000_00C0, 0x5110_C500_0000_00C0, 0x00B2_C804_00C0_0101, 0x00B2_C904_00C0_0505, 0x00A4_CA00_0000_0908, 0x00F0_CB03_80C0_8300,
        0x00F4_CC05_80C0_040A, 0x00B4_CB01_100C_C00B, 0x00A4_CA00_0000_0101, 0x00B2_C500_0007_050A, 0x00A4_CA10_0000_0908, 0x00A4_C100_0000_060A, 0x00A8_C000_0001_0B00, 0x501F_C007_FFFF_F40B,
        0x01A3_C200_2027_3C80, 0x7861_4002_1800_0002,
    ];
    let mut k = Kernel::<32>::new();
    // Preheader: px, py, cx, cy; n = zx = zy = 0; 4.0 in r4.
    k.rshift_or_i32(r(13), r(60), u(1).w0(), Byte::B0, Lut::ZERO);
    k.lshift_or_i32(r(12), r(13), u(1).w0(), Byte::B0, Lut::ZERO);
    k.isub_s32(r(12), r(60), r(12));
    k.u32_to_f32(r(12), r(12));
    k.u32_to_f32(r(13), r(13));
    k.mov_i32(r(10), u(3).w0());
    k.fma_f32(r(6), r(12), r(10), u(2).w0());
    k.fma_f32(r(7), r(13), r(10), u(2).w1());
    k.iadd_imm_i32(r(4), Lut::ZERO, 4f32.to_bits());
    k.iadd_imm_i32(r(0), Lut::ZERO, 0);
    k.iadd_imm_i32(r(1), Lut::ZERO, 0);
    k.iadd_imm_i32(r(5), Lut::ZERO, 0).reconverge();
    // Loop.
    let top = k.here();
    k.fma_f32(r(8), r(1), r(1), Lut::ZERO.neg());
    k.fma_f32(r(9), r(5), r(5), Lut::ZERO.neg());
    k.fadd_f32(r(10), r(8), r(9));
    k.icmp_or_u32(r(11), Cond::Ne, CmpResult::M1, r(0), u(1).w1(), Lut::ZERO);
    k.fcmp_or_f32(r(12), Cond::Le, CmpResult::M1, r(10), r(4), Lut::ZERO);
    k.lshift_and_i32(r(11), r(11), Lut::ZERO, Byte::B0, r(12));
    k.fadd_f32(r(10), r(1), r(1));
    k.fma_f32(r(5), r(10), r(5), r(7));
    k.fadd_f32(r(10), r(8), r(9).neg());
    k.fadd_f32(r(1), r(10), r(6));
    k.isub_s32(r(0), r(0), r(11));
    k.branchz(r(11), top).reconverge();
    // Exit.
    k.shaddx_u64(pair(2), u(0), pair(60), 2);
    k.store_i32(r(0), pair(2), 0, Slot::S0).end();
    same(k.finish(), &GOLD);
}

#[test]
fn present_stage() {
    const GOLD: [u64; 92] = [
        0x00A0_C000_000A_883D, 0x00A0_C000_0000_3C00, 0x01A3_C200_2037_0080, 0x0060_84F4_2800_0002, 0x00B4_C601_108A_C03C, 0x00B4_C701_148B_C93D, 0x00B4_C601_1107_C006, 0x01A3_C800_2007_0686,
        0x1860_81E2_4000_0008, 0x0090_C600_0014_0004, 0x0090_C700_1014_0004, 0x0090_C800_0014_0005, 0x0090_C600_0019_0006, 0x00B2_C204_00C0_8C06, 0x00B2_C304_00C0_0202, 0x00A4_C400_0000_8E03,
        0x00B2_C400_008F_0403, 0x00B2_C400_0090_0403, 0x00B2_C500_0093_9203, 0x00B2_C500_0091_0503, 0x00B2_C500_0090_0503, 0x00B2_C404_00C0_0402, 0x009C_C300_0000_0005, 0x00B2_C204_00C0_0304,
        0x00B2_C440_0004_0502, 0x00B2_C200_0002_0304, 0x00B2_C204_00C0_0202, 0x00B2_C204_00C0_8D02, 0x00A4_4C10_0000_C002, 0x0090_C700_0019_0007, 0x00B2_C204_00C0_8C07, 0x00B2_C304_00C0_0202,
        0x00A4_C400_0000_8E03, 0x00B2_C400_008F_0403, 0x00B2_C400_0090_0403, 0x00B2_C500_0093_9203, 0x00B2_C500_0091_0503, 0x00B2_C500_0090_0503, 0x00B2_C404_00C0_0402, 0x009C_C300_0000_0005,
        0x00B2_C204_00C0_0304, 0x00B2_C440_0004_0502, 0x00B2_C200_0002_0304, 0x00B2_C204_00C0_0202, 0x00B2_C204_00C0_8D02, 0x00A4_4D10_0000_C002, 0x0090_C800_0019_0008, 0x00B2_C204_00C0_8C08,
        0x00B2_C304_00C0_0202, 0x00A4_C400_0000_8E03, 0x00B2_C400_008F_0403, 0x00B2_C400_0090_0403, 0x00B2_C500_0093_9203, 0x00B2_C500_0091_0503, 0x00B2_C500_0090_0503, 0x00B2_C404_00C0_0402,
        0x009C_C300_0000_0005, 0x00B2_C204_00C0_0304, 0x00B2_C440_0004_0502, 0x00B2_C200_0002_0304, 0x00B2_C204_00C0_0202, 0x00B2_C204_00C0_8D02, 0x00A4_4E10_0000_C002, 0x00A1_C900_0005_0E0C,
        0x00A5_CA10_2000_0D09, 0x00B3_CB04_2AC0_9409, 0x00B3_CB00_0A0B_950A, 0x00B3_CB00_3A0B_960A, 0x00B3_CC04_0AC0_970D, 0x00B3_CC00_0A0C_980A, 0x00B3_CC00_3A0C_990A, 0x0090_C600_0014_000B,
        0x0090_C800_0014_000C, 0x0090_CA00_1014_000B, 0x01A3_C200_2017_0684, 0x01A3_C400_2017_0884, 0x01A3_CC00_2017_0A84, 0x0060_86A2_0800_0002, 0x0060_88A2_4800_0004, 0x3860_8AA2_8800_000C,
        0x00A0_C600_0000_0106, 0x00A0_C800_0000_0108, 0x00A0_CA00_0000_010A, 0x00B4_C600_19C0_C906, 0x00B4_C800_19C0_C908, 0x00B4_CA00_19C0_C90A, 0x00B4_CA01_1908_CA0A, 0x00B4_CA01_1906_CA0A,
        0x00B4_CA01_11C0_C90A, 0x0110_CA00_0000_030A, 0x01A3_C200_2027_0082, 0x7861_4A02_1800_0002,
    ];
    let mut k = Kernel::<128>::new();
    // 0-11: t = gy * W + gx; the canvas pixel; the threshold; the channels zero-extended.
    k.imul_i32(r(0), r(61), u(4).w0());
    k.iadd_u32(r(0), r(0), r(60));
    k.shaddx_u64(pair(2), u(0), pair(0), 3);
    k.load_i64(pair(4), pair(2), 0, Slot::S0);
    k.lshift_and_i32(r(6), r(60), Lut::ZERO, Byte::B0, u(5).w0());
    k.lshift_and_i32(r(7), r(61), Lut::BYTES_4_7, Byte::B1, u(5).w1());
    k.lshift_or_i32(r(6), r(6), Lut::ZERO, Byte::B0, r(7));
    k.shaddx_u64(pair(8), u(3), pair(6), 0);
    // The wait acts after this load, so instruction 9 on sees both loads.
    k.load_i8_w0(r(1), pair(8), 0, Slot::S1).wait(Slot::S0).wait(Slot::S1);
    k.u16_to_u32(r(6), r(4), Half::H0);
    k.u16_to_u32(r(7), r(4), Half::H1);
    k.u16_to_u32(r(8), r(5), Half::H0);
    // 12-62: the light of each channel, the same 17 instructions with a different source and destination half.
    for (v, light) in [(r(6), r(12)), (r(7), r(13)), (r(8), r(14))] {
        let zero = Lut::ZERO.neg();
        k.u32_to_f32(v, v);
        k.fma_f32(r(2), v, u(6).w0(), zero);
        k.fma_f32(r(3), r(2), r(2), zero);
        k.fadd_f32(r(4), r(3), u(7).w0());
        k.fma_f32(r(4), r(3), r(4), u(7).w1());
        k.fma_f32(r(4), r(3), r(4), u(8).w0());
        k.fma_f32(r(5), r(3), u(9).w0(), u(9).w1());
        k.fma_f32(r(5), r(3), r(5), u(8).w1());
        k.fma_f32(r(5), r(3), r(5), u(8).w0());
        k.fma_f32(r(4), r(2), r(4), zero);
        k.frcp_f32(r(3), r(5));
        k.fma_f32(r(2), r(4), r(3), zero);
        k.fma_f32(r(4), r(2).neg(), r(5), r(4));
        k.fma_f32(r(2), r(4), r(3), r(2));
        k.fma_f32(r(2), r(2), r(2), zero);
        k.fma_f32(r(2), r(2), u(6).w1(), zero);
        k.fadd_f32(Dst::H0(light), r(2), zero);
    }
    // 63-70: the ODT in f16.
    k.mkvec_v2i16(r(9), r(12), Half::H0, r(14), Half::H0);
    k.fadd_v2f16(r(10), r(9), r(13).h00().neg());
    k.fma_v2f16(r(11), r(9), u(10).w0(), Lut::ZERO.neg());
    k.fma_v2f16(r(11), r(10).h00(), u(10).w1(), r(11));
    k.fma_v2f16(r(11), r(10).h11(), u(11).w0(), r(11));
    k.fma_v2f16(r(12), r(13).h00(), u(11).w1(), Lut::ZERO.neg());
    k.fma_v2f16(r(12), r(10).h00(), u(12).w0(), r(12));
    k.fma_v2f16(r(12), r(10).h11(), u(12).w1(), r(12));
    // 71-79: the code lookups.
    k.u16_to_u32(r(6), r(11), Half::H0);
    k.u16_to_u32(r(8), r(12), Half::H0);
    k.u16_to_u32(r(10), r(11), Half::H1);
    k.shaddx_u64(pair(2), u(2), pair(6), 1);
    k.shaddx_u64(pair(4), u(2), pair(8), 1);
    k.shaddx_u64(pair(12), u(2), pair(10), 1);
    k.load_i16_w0(r(6), pair(2), 0, Slot::S0);
    k.load_i16_w0(r(8), pair(4), 0, Slot::S1);
    k.load_i16_w0(r(10), pair(12), 0, Slot::S2).wait(Slot::S0).wait(Slot::S1).wait(Slot::S2);
    // 80-89: dither and pack.
    k.iadd_u32(r(6), r(6), r(1));
    k.iadd_u32(r(8), r(8), r(1));
    k.iadd_u32(r(10), r(10), r(1));
    k.rshift_or_i32(r(6), r(6), Lut::BYTES_4_7, Byte::B2, Lut::ZERO);
    k.rshift_or_i32(r(8), r(8), Lut::BYTES_4_7, Byte::B2, Lut::ZERO);
    k.rshift_or_i32(r(10), r(10), Lut::BYTES_4_7, Byte::B2, Lut::ZERO);
    k.lshift_or_i32(r(10), r(10), Lut::BYTES_8_11, Byte::B2, r(8));
    k.lshift_or_i32(r(10), r(10), Lut::BYTES_8_11, Byte::B2, r(6));
    k.lshift_or_i32(r(10), r(10), Lut::BYTES_4_7, Byte::B0, Lut::ZERO);
    k.iadd_imm_i32(r(10), r(10), 3);
    // 90-91: out[t] = word.
    k.shaddx_u64(pair(2), u(1), pair(0), 2);
    k.store_i32(r(10), pair(2), 0, Slot::S0).end();
    same(k.finish(), &GOLD);
}

/// The load, store, convert and move forms measured one at a time on the device (payloads/gpu bench and coherency tests).
#[test]
fn load_store_forms() {
    let mut k = Kernel::<8>::new();
    k.load_i32(r(4), pair(2), 0, Slot::S0);
    k.load_i64(pair(4), pair(2), 0, Slot::S0);
    k.load_i64(pair(4), pair(2), 0, Slot::S0).wait(Slot::S0);
    k.store_i32(r(4), pair(2), 0, Slot::S0).end();
    k.u32_to_f32(r(6), r(4));
    k.mov_i32(r(6), r(4));
    same(k.finish(), &[0x0060_8482_1800_0002, 0x0060_84F4_2800_0002, 0x0860_84F4_2800_0002, 0x7861_4402_1800_0002, 0x0090_C600_0019_0004, 0x0091_C600_0000_0004]);
}

/// Forward branches and jumps against Mesa's own vectors (assembler-cases.txt:97, test-packing.cpp:179-184).
#[test]
fn branches_forward_and_back() {
    let mut k = Kernel::<16>::new();
    let skip = k.label();
    k.branchz(r(0), skip).reconverge();
    for _ in 0..3 {
        k.mov_i32(r(1), r(1));
    }
    k.bind(skip);
    let back = k.here();
    for _ in 0..7 {
        k.mov_i32(r(1), r(1));
    }
    k.jump(back);
    let w = k.finish();
    assert_eq!(w[0], 0x501F_C000_0000_0300);
    assert_eq!(w[11], 0x001F_C017_FFFF_F8C0);
}

#[test]
#[should_panic(expected = "one 64-bit uniform slot")]
fn fau_two_slots_refused() {
    let mut k = Kernel::<4>::new();
    k.fma_f32(r(6), r(12), u(3).w0(), u(2).w0());
}

#[test]
#[should_panic(expected = "two distinct 32-bit")]
fn fau_three_words_refused() {
    let mut k = Kernel::<4>::new();
    k.fma_f32(r(6), u(2).w0(), u(2).w1(), Lut::F32_ONE);
}

#[test]
fn fau_same_word_twice_is_one_word() {
    let mut k = Kernel::<4>::new();
    k.fma_f32(r(6), u(2).w0(), u(2).w0(), u(2).w1());
}

#[test]
#[should_panic(expected = "already carries a flow")]
fn flow_end_over_wait_refused() {
    let mut k = Kernel::<4>::new();
    k.load_i32(r(4), pair(2), 0, Slot::S0).wait(Slot::S0).end();
}

#[test]
fn uniform_page() {
    let mut k = Kernel::<4>::new();
    // u37.w1: slot 37 = page 1 (bits 58:57), byte G#80 | 5 << 1 | 1 = G#8B.
    k.mov_i32(r(0), u(37).w1());
    assert_eq!(k.finish(), &[0x0291_C000_0000_008B]);
}

/// The forms the f16 path, the clamps and the blocks emit, against Mesa's assembler test vectors (assembler-cases.txt; not yet run on the device): an f32 operand widened from either f16 half, a v2f16 add with broadcast halves, an f32 add with a destination clamp and a negated absolute operand, and a NOP carrying a flow.
#[test]
fn mesa_vectors_f16_clamp_nop() {
    let mut k = Kernel::<8>::new();
    // 01 00 00 08 00 c0 a4 00: FADD.f32 r0, r1, r0.h1
    k.fadd_f32(r(0), r(1), Float::from(r(0)).hi16());
    // 01 00 00 04 00 c0 a4 00: FADD.f32 r0, r1, r0.h0
    k.fadd_f32(r(0), r(1), Float::from(r(0)).lo16());
    // 01 00 00 0c 00 c0 a5 00: FADD.v2f16 r0, r1.h00, r0.h11
    k.fadd_v2f16(r(0), r(1).h00(), r(0).h11());
    // 01 02 00 00 32 c0 a4 00: FADD.f32.clamp_m1_1 r0, r1, r2.neg.abs
    k.fadd_f32(r(0), r(1), Operand::abs(r(2)).neg()).clamp(Clamp::MinusOneOne);
    // 00 00 00 00 00 c0 00 78: NOP.end
    k.nop().end();
    same(k.finish(), &[0x00A4_C000_0800_0001, 0x00A4_C000_0400_0001, 0x00A5_C000_0C00_0001, 0x00A4_C032_0000_0201, 0x7800_C000_0000_0000]);
}
