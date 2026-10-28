#![no_std]
#![crate_type = "lib"]
// The ira and the wairua in CPU registers: the derivation (KEY_REGISTERS.md, "Off DRAM"). Safe Rust except the system-register access below; tools/keyreg-lift.sh compiles it, checks the output never touches the stack, and freezes it as keys.s, which the kernel links (ferros_kernel/src/keys.rs). This file is the source of truth; keys.s is the program.
//
// Each secret is held as two shares in registers EL0 cannot read, secret = share 0 XOR share 1:
//   ira:    share 0 = APIAKey, APIBKey (pointer-authentication keys, switched off by SCTLR_EL2)  share 1 = TPIDR_EL1, SP_EL1, ELR_EL12, FAR_EL12
//   wairua: share 0 = APDAKey, APDBKey                                                         share 1 = MAIR_EL12, APGAKey, TPIDR_EL2
// Under VHE (HCR_EL2.TGE=1, E2H=1) nothing runs at EL1, so the EL1 registers are never written by hardware. APGA's PACGA cannot be switched off, so it only ever holds a share, which is uniformly random alone.
//
// Each function joins the shares in its own registers, uses the secret, and returns; no secret is written to memory, only what the caller asked for (a derived key, a stream block, a sealed blob). The caller masks interrupts around the call and scrubs the caller-saved registers right after (keyreg_scrub in keys.s), so no trap frame captures a leftover.
//
// BLAKE3 is the reference compression (one block, single chunk): rows of four 32-bit words in NEON registers, the message gathered per round by table lookup.

use core::arch::aarch64::*;

macro_rules! mrs {
    ($r:literal) => {{
        let v: u64;
        // SAFETY: reading a system register this kernel owns at EL2; no memory, no side effects.
        unsafe { core::arch::asm!(concat!("mrs {}, ", $r), out(reg) v, options(nomem, nostack, preserves_flags)) };
        v
    }};
}
macro_rules! msr {
    ($r:literal, $v:expr) => {{
        let v: u64 = $v;
        // SAFETY: writing a register nothing else in ferros uses (KEY_REGISTERS.md): the pointer keys are switched off, the EL1 bank is dead under VHE.
        unsafe { core::arch::asm!(concat!("msr ", $r, ", {}"), in(reg) v, options(nomem, nostack, preserves_flags)) };
    }};
}

fn ira() -> [u64; 4] {
    [
        mrs!("S3_0_C2_C1_0") ^ mrs!("tpidr_el1"),
        mrs!("S3_0_C2_C1_1") ^ mrs!("sp_el1"),
        mrs!("S3_0_C2_C1_2") ^ mrs!("S3_5_C4_C0_1"),
        mrs!("S3_0_C2_C1_3") ^ mrs!("S3_5_C6_C0_0"),
    ]
}

fn wairua() -> [u64; 4] {
    [
        mrs!("S3_0_C2_C2_0") ^ mrs!("S3_5_C10_C2_0"),
        mrs!("S3_0_C2_C2_1") ^ mrs!("S3_0_C2_C3_0"),
        mrs!("S3_0_C2_C2_2") ^ mrs!("S3_0_C2_C3_1"),
        mrs!("S3_0_C2_C2_3") ^ mrs!("tpidr_el2"),
    ]
}

fn secret(slot: u32) -> [u64; 4] {
    if slot == 0 { ira() } else { wairua() }
}

const IV: [u64; 4] = [0xBB67AE85_6A09E667, 0xA54FF53A_3C6EF372, 0x9B05688C_510E527F, 0x5BE0CD19_1F83D9AB];
const CHUNK_START: u32 = 1;
const CHUNK_END: u32 = 2;
const ROOT: u32 = 8;
const KEYED_HASH: u32 = 16;
const DERIVE_KEY_MATERIAL: u32 = 64;
const PERMUTATION: [usize; 16] = [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8];

/// The table-lookup indices for each round's four message vectors (columns x/y, diagonals x/y), as byte offsets into the 64-byte message, two u64 halves each. Round r reads the message permuted r times, folded into the indices at compile time.
const SCHEDULE: [[u64; 2]; 28] = {
    let mut s = [0usize; 16];
    let mut i = 0;
    while i < 16 {
        s[i] = i;
        i += 1;
    }
    let mut out = [[0u64; 2]; 28];
    let mut r = 0;
    while r < 7 {
        let mut v = 0;
        while v < 4 {
            // v 0: m0 m2 m4 m6; 1: m1 m3 m5 m7; 2: m8 m10 m12 m14; 3: m9 m11 m13 m15.
            let base = (v >> 1) * 8 + (v & 1);
            let mut lane = 0;
            while lane < 4 {
                let w = s[base + lane * 2];
                let mut b = 0;
                while b < 4 {
                    let byte = (w * 4 + b) as u64;
                    let at = lane * 4 + b;
                    out[r * 4 + v][at / 8] |= byte << ((at % 8) * 8);
                    b += 1;
                }
                lane += 1;
            }
            v += 1;
        }
        let mut next = [0usize; 16];
        let mut k = 0;
        while k < 16 {
            next[k] = s[PERMUTATION[k]];
            k += 1;
        }
        s = next;
        r += 1;
    }
    out
};

// The helpers are macros, not functions: a #[target_feature] function cannot be forced inline, and one that is not inlined passes its rows through the stack (the first lift, refused).
macro_rules! row {
    ($lo:expr, $hi:expr) => {
        vcombine_u32(vcreate_u32($lo), vcreate_u32($hi))
    };
}

macro_rules! ror {
    ($x:expr, $n:literal, $m:literal) => {{
        let x = $x;
        vsliq_n_u32::<$m>(vshrq_n_u32::<$n>(x), x)
    }};
}

macro_rules! ror16 {
    ($x:expr) => {
        vreinterpretq_u32_u16(vrev32q_u16(vreinterpretq_u16_u32($x)))
    };
}

macro_rules! g {
    ($r:ident, $mx:expr, $my:expr) => {{
        $r[0] = vaddq_u32(vaddq_u32($r[0], $r[1]), $mx);
        $r[3] = ror16!(veorq_u32($r[3], $r[0]));
        $r[2] = vaddq_u32($r[2], $r[3]);
        $r[1] = ror!(veorq_u32($r[1], $r[2]), 12, 20);
        $r[0] = vaddq_u32(vaddq_u32($r[0], $r[1]), $my);
        $r[3] = ror!(veorq_u32($r[3], $r[0]), 8, 24);
        $r[2] = vaddq_u32($r[2], $r[3]);
        $r[1] = ror!(veorq_u32($r[1], $r[2]), 7, 25);
    }};
}

/// One BLAKE3 compression: the chaining value (two rows), the 64-byte message (four rows), the counter, the block length and the flags. The four state rows after the seven rounds, before the output XOR.
macro_rules! compress {
    ($cv:expr, $m:expr, $counter:expr, $len:expr, $flags:expr) => {{
        let cv: [uint32x4_t; 2] = $cv;
        let m: uint8x16x4_t = $m;
        let mut r = [cv[0], cv[1], row!(IV[0], IV[1]), row!($counter, (($flags as u64) << 32) | $len as u64)];
        let mut k = 0;
        while k < 7 {
            let pick = |v: usize| vreinterpretq_u32_u8(vqtbl4q_u8(m, vcombine_u8(vcreate_u8(SCHEDULE[k * 4 + v][0]), vcreate_u8(SCHEDULE[k * 4 + v][1]))));
            g!(r, pick(0), pick(1));
            r[1] = vextq_u32::<1>(r[1], r[1]);
            r[2] = vextq_u32::<2>(r[2], r[2]);
            r[3] = vextq_u32::<3>(r[3], r[3]);
            g!(r, pick(2), pick(3));
            r[1] = vextq_u32::<3>(r[1], r[1]);
            r[2] = vextq_u32::<2>(r[2], r[2]);
            r[3] = vextq_u32::<1>(r[3], r[3]);
            k += 1;
        }
        r
    }};
}

macro_rules! message {
    ($w:expr) => {{
        let w: [u64; 8] = $w;
        uint8x16x4_t(
            vreinterpretq_u8_u32(row!(w[0], w[1])),
            vreinterpretq_u8_u32(row!(w[2], w[3])),
            vreinterpretq_u8_u32(row!(w[4], w[5])),
            vreinterpretq_u8_u32(row!(w[6], w[7])),
        )
    }};
}

/// Outputs are 16-byte aligned: the kernel target is strict-align, and into a plain u64 array LLVM cannot store a vector row, so it bounced each row off the stack (the first two lifts, refused).
#[repr(C, align(16))]
pub struct Out4(pub [u64; 4]);
#[repr(C, align(16))]
pub struct Out8(pub [u64; 8]);

/// A row into two words of `out`.
macro_rules! put {
    ($out:ident, $at:literal, $x:expr) => {{
        let x = vreinterpretq_u64_u32($x);
        $out.0[$at] = vgetq_lane_u64::<0>(x);
        $out.0[$at + 1] = vgetq_lane_u64::<1>(x);
    }};
}

/// derive_key's second half: the secret in slot `slot` (0 ira, 1 wairua) as key material under `context_key` (blake3::hazmat::hash_derive_key_context of the context string, public). The derived key's two rows.
macro_rules! derive_rows {
    ($slot:expr, $context_key:expr) => {{
        let s = secret($slot);
        let c: &[u64; 4] = $context_key;
        let r = compress!([row!(c[0], c[1]), row!(c[2], c[3])], message!([s[0], s[1], s[2], s[3], 0, 0, 0, 0]), 0u64, 32u32, CHUNK_START | CHUNK_END | ROOT | DERIVE_KEY_MATERIAL);
        [veorq_u32(r[0], r[2]), veorq_u32(r[1], r[3])]
    }};
}

/// `blake3::derive_key(context, secret)` for the secret in `slot`, into `out`.
#[unsafe(no_mangle)]
#[target_feature(enable = "neon")]
pub extern "C" fn keyreg_derive(slot: u32, context_key: &[u64; 4], out: &mut Out4) {
    let k = derive_rows!(slot, context_key);
    put!(out, 0, k[0]);
    put!(out, 2, k[1]);
}

/// One 64-byte block, number `block`, of `blake3::Hasher::new_keyed(&derive_key(context, wairua))` over `msg` (`len` ≤ 64 bytes, zero-padded), read as an XOF: the derived key never leaves the registers. One block a call: with several, LLVM hoists the 28 gathered message vectors out of the loop and spills them (the second lift, refused).
#[unsafe(no_mangle)]
#[target_feature(enable = "neon")]
pub extern "C" fn keyreg_stream(context_key: &[u64; 4], msg: &[u64; 8], len: u32, block: u64, out: &mut Out8) {
    let k = derive_rows!(1, context_key);
    let r = compress!(k, message!(*msg), block, len, CHUNK_START | CHUNK_END | ROOT | KEYED_HASH);
    put!(out, 0, veorq_u32(r[0], r[2]));
    put!(out, 2, veorq_u32(r[1], r[3]));
    put!(out, 4, veorq_u32(r[2], k[0]));
    put!(out, 6, veorq_u32(r[3], k[1]));
}

/// Hold `a XOR b` in slot `slot` (0 ira, 1 wairua) as the shares `a XOR b XOR mask` and `mask`. The secret itself forms only in registers: a fresh value comes in as `a` with `b` zero (the caller then wipes `a`); an unsealed one as the sealed bytes and the keystream.
#[unsafe(no_mangle)]
pub extern "C" fn keyreg_set(slot: u32, mask: &[u64; 4], a: &[u64; 4], b: &[u64; 4]) {
    let s = [a[0] ^ b[0] ^ mask[0], a[1] ^ b[1] ^ mask[1], a[2] ^ b[2] ^ mask[2], a[3] ^ b[3] ^ mask[3]];
    if slot == 0 {
        msr!("S3_0_C2_C1_0", s[0]);
        msr!("S3_0_C2_C1_1", s[1]);
        msr!("S3_0_C2_C1_2", s[2]);
        msr!("S3_0_C2_C1_3", s[3]);
        msr!("tpidr_el1", mask[0]);
        msr!("sp_el1", mask[1]);
        msr!("S3_5_C4_C0_1", mask[2]);
        msr!("S3_5_C6_C0_0", mask[3]);
    } else {
        msr!("S3_0_C2_C2_0", s[0]);
        msr!("S3_0_C2_C2_1", s[1]);
        msr!("S3_0_C2_C2_2", s[2]);
        msr!("S3_0_C2_C2_3", s[3]);
        msr!("S3_5_C10_C2_0", mask[0]);
        msr!("S3_0_C2_C3_0", mask[1]);
        msr!("S3_0_C2_C3_1", mask[2]);
        msr!("tpidr_el2", mask[3]);
    }
}

/// A fresh split of the secret in `slot`: `mask` XORed into both shares, so the secret is unchanged and never forms. Unsealing takes masks from the seal itself (the firmware TRNG costs ~1.3 ms a call, 2026-10-28), then re-splits here with a mask from the wairua's stream, which the blob cannot predict.
#[unsafe(no_mangle)]
pub extern "C" fn keyreg_remask(slot: u32, mask: &[u64; 4]) {
    if slot == 0 {
        msr!("S3_0_C2_C1_0", mrs!("S3_0_C2_C1_0") ^ mask[0]);
        msr!("S3_0_C2_C1_1", mrs!("S3_0_C2_C1_1") ^ mask[1]);
        msr!("S3_0_C2_C1_2", mrs!("S3_0_C2_C1_2") ^ mask[2]);
        msr!("S3_0_C2_C1_3", mrs!("S3_0_C2_C1_3") ^ mask[3]);
        msr!("tpidr_el1", mrs!("tpidr_el1") ^ mask[0]);
        msr!("sp_el1", mrs!("sp_el1") ^ mask[1]);
        msr!("S3_5_C4_C0_1", mrs!("S3_5_C4_C0_1") ^ mask[2]);
        msr!("S3_5_C6_C0_0", mrs!("S3_5_C6_C0_0") ^ mask[3]);
    } else {
        msr!("S3_0_C2_C2_0", mrs!("S3_0_C2_C2_0") ^ mask[0]);
        msr!("S3_0_C2_C2_1", mrs!("S3_0_C2_C2_1") ^ mask[1]);
        msr!("S3_0_C2_C2_2", mrs!("S3_0_C2_C2_2") ^ mask[2]);
        msr!("S3_0_C2_C2_3", mrs!("S3_0_C2_C2_3") ^ mask[3]);
        msr!("S3_5_C10_C2_0", mrs!("S3_5_C10_C2_0") ^ mask[0]);
        msr!("S3_0_C2_C3_0", mrs!("S3_0_C2_C3_0") ^ mask[1]);
        msr!("S3_0_C2_C3_1", mrs!("S3_0_C2_C3_1") ^ mask[2]);
        msr!("tpidr_el2", mrs!("tpidr_el2") ^ mask[3]);
    }
}

/// Both secrets XOR `keystream`, into `out` (ira then wairua): the sealed form that crosses to another core or through deep sleep.
#[unsafe(no_mangle)]
pub extern "C" fn keyreg_seal(keystream: &[u64; 8], out: &mut [u64; 8]) {
    let (i, w) = (ira(), wairua());
    *out = [
        i[0] ^ keystream[0], i[1] ^ keystream[1], i[2] ^ keystream[2], i[3] ^ keystream[3],
        w[0] ^ keystream[4], w[1] ^ keystream[5], w[2] ^ keystream[6], w[3] ^ keystream[7],
    ];
}

/// Both slots to zero: shares and secret alike.
#[unsafe(no_mangle)]
pub extern "C" fn keyreg_clear() {
    keyreg_set(0, &[0; 4], &[0; 4], &[0; 4]);
    keyreg_set(1, &[0; 4], &[0; 4], &[0; 4]);
}
