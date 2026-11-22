//! The display's compose kernel (payloads/display/src/gpu_kernel.rs) on the host, in valhall's emulator, against the same frame worked out the way the display's NEON path works it (main.rs `publish_app_rows` with `glass_front` and `bars_front`, neon.rs `present_app16`): every word of the owed rect equal, none outside it written.
//! The frame: a small screen, a front layer of every kind (nothing in front, the mask whole, an edge or a bar partly covering), two windows that do not overlap (one opaque, one with see-through pixels and a ring), black elsewhere, two phases' turns, a channel off.

#[path = "../../../payloads/display/src/gpu_layout.rs"]
#[allow(dead_code)]
mod gpu_layout;
#[path = "../../../payloads/display/src/gpu_kernel.rs"]
mod gpu_kernel;
#[path = "../../../payloads/display/src/gpu_layers.rs"]
mod gpu_layers;
#[path = "../../../payloads/compbench/src/noise.rs"]
#[allow(dead_code)]
mod noise;
#[path = "../../../payloads/compbench/src/layout.rs"]
#[allow(dead_code)]
mod layout;
#[path = "../../../payloads/compbench/src/tables.rs"]
#[allow(dead_code)]
mod tables;

use gpu_layout::{slot, PASS, WINDOWS};
use valhall::sim::{run, Memory};

const W: usize = 100;
const H: usize = 40;
const VA: u64 = 0x1000_0000;
const PHASE: usize = 0;
const FRONT: usize = 1 << 16;
const BLUE: usize = 2 << 16;
const SURF: usize = 3 << 16;
const SURF_BYTES: usize = W * H * 8;

/// neon.rs `BLUE` and `TURN`.
const BLUE_T: [[u8; 16]; 16] = [
    [78, 190, 245, 170, 143, 39, 192, 90, 172, 247, 163, 138, 7, 187, 133, 236],
    [3, 152, 47, 71, 210, 120, 234, 0, 213, 114, 20, 203, 83, 55, 208, 98],
    [221, 112, 135, 229, 17, 96, 159, 70, 140, 48, 97, 232, 176, 121, 23, 165],
    [41, 201, 26, 82, 174, 50, 200, 30, 184, 243, 155, 66, 33, 149, 240, 69],
    [88, 252, 158, 195, 129, 248, 108, 224, 80, 118, 15, 188, 91, 215, 110, 183],
    [14, 123, 60, 102, 5, 73, 147, 13, 169, 44, 207, 130, 251, 8, 51, 142],
    [168, 218, 34, 209, 238, 186, 38, 217, 134, 239, 100, 59, 160, 77, 199, 235],
    [99, 74, 179, 139, 53, 164, 122, 92, 64, 180, 4, 227, 31, 173, 119, 29],
    [154, 246, 11, 113, 84, 22, 233, 191, 25, 153, 115, 193, 141, 94, 220, 57],
    [136, 43, 194, 231, 151, 211, 105, 54, 253, 85, 219, 72, 49, 249, 12, 189],
    [212, 81, 124, 65, 35, 178, 2, 145, 204, 36, 131, 19, 205, 157, 76, 111],
    [228, 6, 167, 206, 95, 241, 128, 67, 166, 109, 185, 237, 101, 127, 181, 32],
    [150, 106, 250, 21, 146, 46, 216, 89, 244, 10, 63, 161, 45, 1, 242, 62],
    [177, 40, 137, 61, 196, 116, 18, 182, 42, 148, 230, 87, 214, 144, 198, 93],
    [226, 202, 86, 223, 175, 79, 156, 225, 107, 197, 125, 28, 171, 68, 117, 16],
    [56, 126, 27, 104, 9, 254, 58, 132, 24, 75, 52, 222, 103, 255, 37, 162],
];
const TURN: [u8; 4] = [0, 2, 3, 1];

fn rng(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

struct Win {
    rect: (usize, usize, usize, usize),
    ring: (usize, usize, usize, usize, usize),
    opaque: bool,
}

fn main() {
    let k = gpu_kernel::record();
    println!("the compose kernel: {} words, peak {} registers", k.words.len(), k.peak);
    let mut mem = Memory { base: VA, bytes: vec![0; SURF + 2 * SURF_BYTES] };
    let mut s = 0x9E37_79B9_7F4A_7C15u64;
    // The front: a quarter nothing in front, an eighth the mask whole (with and without its no-dither bit), the rest partly covering with what of the scene shows from 0 to 2^14.
    let mut front = vec![0u64; W * H];
    for f in front.iter_mut() {
        let r = rng(&mut s);
        let ch = |r: u64| (r & 0x3FFF) | (r >> 14 & 0x3FFF) << 16 | (r >> 28 & 0x3FFF) << 32;
        *f = match r >> 61 {
            0 | 1 => PASS,
            2 => ch(r) | (r >> 50 & 1) << 48,
            _ => {
                // What shows, and the channels so far no more than what the front covers (`under_pm`'s sum is at most one whole, so no channel passes its field).
                let t = (r >> 40) % 16385;
                let most = ((16384 - t) * 65535 >> 14) + 1;
                (0..3).fold(t << 49, |a, c| a | (rng(&mut s) % most) << (16 * c))
            }
        };
    }
    for (i, f) in front.iter().enumerate() {
        mem.bytes[FRONT + 8 * i..FRONT + 8 * i + 8].copy_from_slice(&f.to_le_bytes());
    }
    for y in 0..16 {
        mem.bytes[BLUE + 16 * y..BLUE + 16 * y + 16].copy_from_slice(&BLUE_T[y]);
    }
    // Two surfaces: baked pixels, some see-through (bit 63), some with the no-dither bit, channels up to the field's top.
    let mut surf = vec![vec![0u64; W * H]; 2];
    for (j, sf) in surf.iter_mut().enumerate() {
        for (i, p) in sf.iter_mut().enumerate() {
            let r = rng(&mut s);
            *p = (r & 0xFFFF_FFFF_FFFF) | (r >> 50 & 1) << 48 | if r >> 59 == 0 { 1 << 63 } else { 0 };
            mem.bytes[SURF + j * SURF_BYTES + 8 * i..SURF + j * SURF_BYTES + 8 * i + 8].copy_from_slice(&p.to_le_bytes());
        }
    }
    let wins = [Win { rect: (0, 0, 60, 25), ring: (5, 55, 3, 23, 7), opaque: false }, Win { rect: (60, 10, 100, 40), ring: (0, 0, 0, 0, 0), opaque: true }];
    let (x0, y0, x1, y1) = (3usize, 1usize, 97usize, 38usize);
    let mut wrong = 0;
    let mut outside = 0;
    for (phase, cm) in [(1usize, 0b111u32), (2, 0b101)] {
        let marker = 0xA5A5_A5A5u32;
        for i in 0..W * H {
            mem.bytes[PHASE + 4 * i..PHASE + 4 * i + 4].copy_from_slice(&marker.to_le_bytes());
        }
        let blocks = ((x1 - x0) as u32).div_ceil(16);
        let mut u = [0u64; 64];
        let on = |b: u32| if cm >> b & 1 == 1 { 0xFFu64 } else { 0 };
        u[slot::PHASE as usize] = VA + PHASE as u64;
        u[slot::FRONT as usize] = VA + FRONT as u64;
        u[slot::BLUE as usize] = VA + BLUE as u64;
        u[slot::RECT as usize] = (x0 as u64 | (y0 as u64) << 16) | (x1 as u64 | (y1 as u64) << 16) << 32;
        u[slot::GRID as usize] = blocks as u64 | ((1. / blocks as f32).to_bits() as u64) << 32;
        u[slot::SCREEN as usize] = W as u64 | (TURN[phase] as u64 | on(0) << 8 | on(1) << 16 | on(2) << 24) << 32;
        for (i, w) in wins.iter().enumerate() {
            let b = slot::WIN as usize + 4 * i;
            u[b] = VA + (SURF + i * SURF_BYTES) as u64;
            u[b + 1] = (w.rect.0 as u64 | (w.rect.1 as u64) << 16) | (w.rect.2 as u64 | (w.rect.3 as u64) << 16) << 32;
            u[b + 2] = (w.ring.0 as u64 | (w.ring.1 as u64) << 16) | (w.ring.2 as u64 | (w.ring.3 as u64) << 16) << 32;
            u[b + 3] = w.ring.4 as u64 | (w.opaque as u64) << 32;
        }
        assert!(wins.len() <= WINDOWS);
        let n = slot::RESERVED as usize + k.consts.len();
        u[slot::RESERVED as usize..n].copy_from_slice(&k.consts);
        let rows = (y1 - y0) as u32;
        for g in 0..rows * blocks * 16 {
            run(&k.words, &u[..n], [g, 0], &mut mem);
        }
        for y in 0..H {
            for x in 0..W {
                let i = y * W + x;
                let got = u32::from_le_bytes(mem.bytes[PHASE + 4 * i..PHASE + 4 * i + 4].try_into().unwrap());
                if !(x >= x0 && x < x1 && y >= y0 && y < y1) {
                    outside += (got != marker) as usize;
                    continue;
                }
                // The scene: main.rs `src_row`, the copy (opaque) or the select then `clear_mark`.
                let mut p = 0u64;
                for (j, w) in wins.iter().enumerate() {
                    if x >= w.rect.0 && x < w.rect.2 && y >= w.rect.1 && y < w.rect.3 {
                        let (rx0, rx1, ry0, ry1, o) = w.ring;
                        let sy = if x >= rx0 && x < rx1 && y >= ry0 && y < ry1 { let (r, h) = (y - ry0 + o, ry1 - ry0); ry0 + if r >= h { r - h } else { r } } else { y };
                        let v = surf[j][sy * W + x];
                        p = if w.opaque { v } else { v & ((v >> 63) ^ 1).wrapping_neg() };
                    }
                }
                // `glass_front` / `bars_front` from the front's (channels, what shows), then the present.
                let f = front[i];
                let t = f >> 49;
                let px = if t == 0x7FFF { p } else { (0..3).fold(f & 1 << 48, |a, c| a | (((f >> (16 * c)) & 0xFFFF) + (((p >> (16 * c)) & 0xFFFF) * t >> 14)) << (16 * c)) };
                let th = if px >> 48 & 1 == 1 { 0 } else { (BLUE_T[y & 15][x & 15] ^ TURN[phase]) as u64 };
                let code = |c: usize| (((((px >> (16 * c)) & 0xFFFF) + th).min(0xFFFF) >> 8) as u32) & on(c as u32) as u32;
                let want = code(2) << 24 | code(1) << 14 | code(0) << 4 | 3;
                if got != want {
                    if wrong < 4 {
                        println!("phase {phase} at {x},{y}: the kernel G#{got:08X}, the display G#{want:08X} (front G#{f:016X}, scene G#{p:016X})");
                    }
                    wrong += 1;
                }
            }
        }
    }
    println!("{wrong} words of the owed rects differ, {outside} written outside them");
    let (_, lfar, lout) = layered();
    std::process::exit((wrong + outside + lout != 0 || lfar > 1) as i32);
}

/// The layered kernel (gpu_layers.rs, step 4) in the emulator, a workgroup a half-row with its storage and barriers, against the same frame worked out on the CPU: the parallel noise by its scalar reference (compbench noise.rs `reference_row`), baked by compbench's scalar bake (tables.rs `bake_px`, the display's bake in the kernel's order), the pixel layers front to back in codes, the front layer and the present. → (words that differ, the most by in 8-bit codes, words written outside the owed rect).
fn layered() -> (usize, u32, usize) {
    use gpu_layout::lslot as ls;
    use valhall::sim::run_group;
    const LW: usize = 300;
    const LH: usize = 24;
    let k = gpu_layers::record();
    println!("the layered kernel: {} words, peak {} registers, workgroup 2^{}, {} B of storage", k.words.len(), k.peak, k.wg_log2, k.shared_bytes);
    let (front_at, blue_at, light_at, code_at, lay_at, phase_at) = (0usize, 1 << 20, 2 << 20, 3 << 20, 4 << 20, 6 << 20);
    let mut mem = Memory { base: VA, bytes: vec![0; 8 << 20] };
    let mut s = 0x2545_F491_4F6C_DD1Du64;
    let put64 = |m: &mut Memory, at: usize, v: u64| m.bytes[at..at + 8].copy_from_slice(&v.to_le_bytes());
    // The bake: compbench's tables for white two stops under the E peak (as its bench), the Pro's ODT.
    let odt = tables::odt_scaled(false);
    let kg = tables::gain(0.25);
    let light: Vec<u16> = (0..1 << 16).map(|c| tables::light_entry(kg, c, valhall::f32_to_f16)).collect();
    let code: Vec<f32> = (0..layout::CODE_STEPS + 2).map(|i| tables::code_entry(i, false)).collect();
    for (c, &l) in light.iter().enumerate() {
        mem.bytes[light_at + 2 * c..light_at + 2 * c + 2].copy_from_slice(&l.to_le_bytes());
    }
    for (i, &c) in code.iter().enumerate() {
        mem.bytes[code_at + 4 * i..code_at + 4 * i + 4].copy_from_slice(&c.to_bits().to_le_bytes());
    }
    for y in 0..16 {
        mem.bytes[blue_at + 16 * y..blue_at + 16 * y + 16].copy_from_slice(&BLUE_T[y]);
    }
    // The front: nothing in front but the first and last two columns (the mask, an edge), and a bar over the top three rows.
    let mut front = vec![PASS; LW * LH];
    for y in 0..LH {
        for x in 0..LW {
            let f = &mut front[y * LW + x];
            if x < 2 || x >= LW - 2 {
                *f = 0x1000 | 0x0800 << 16 | 1 << 48;
            } else if y < 3 && x > 40 && x < 90 {
                let t = (rng(&mut s) % 16385) as u64;
                let most = ((16384 - t) * 65535 >> 14) + 1;
                *f = (0..3).fold(t << 49, |a, c| a | (rng(&mut s) % most) << (16 * c));
            }
        }
    }
    for (i, &f) in front.iter().enumerate() {
        put64(&mut mem, front_at + 8 * i, f);
    }
    // Two layers: the chrome over the top 6 rows (opaque, partly covered and see-through pixels), the content under it with a ring and see-through gaps.
    let mut lay = vec![vec![0u64; LW * LH]; 2];
    for y in 0..LH {
        for x in 0..LW {
            let r = rng(&mut s);
            let lt = match r >> 62 {
                0 => 0,
                1 => (r >> 30) & 0x3FFF,
                _ => 0,
            };
            let see = r >> 60 == 0xF;
            // Premultiplied: a channel no more than what the pixel covers.
            let most = ((16384 - lt) * 65535 >> 14) + 1;
            let ch = (0..3).fold(0u64, |a, c| a | (rng(&mut s) % most) << (16 * c));
            lay[0][y * LW + x] = if see { 1 << 63 | ch } else { ch | lt << 49 };
            let r2 = rng(&mut s);
            lay[1][y * LW + x] = if r2 >> 61 == 0 { 1 << 63 } else { (r2 & 0xFFFF_FFFF_FFFF) | 1 << 49 };
        }
    }
    for (l, px) in lay.iter().enumerate() {
        for (i, &p) in px.iter().enumerate() {
            put64(&mut mem, lay_at + l * LW * LH * 8 + 8 * i, p);
        }
    }
    let layers = [((0usize, 0usize, LW, 6usize), (0usize, 0usize, 0usize, 0usize, 0usize)), ((0, 0, LW, LH), (10, 290, 2, 22, 5))];
    let walk = (0x0FFF_0FFF_1FFFu64, 0x3FFF_3FFF_3FFFu64, 0x1000_1800_2000u64);
    let (axis, scroll, texh, frame) = (LW / 2, 5u32, LH as u32, 0u32);
    let clip = (2usize, 1usize, LW - 2, LH - 1);
    let (x0, y0, x1, y1) = (1usize, 0usize, 297usize, 23usize);
    let marker = 0xA5A5_A5A5u32;
    let mut far = 0u32;
    let (mut wrong, mut outside) = (0usize, 0usize);
    for (phase, cm) in [(1usize, 0b111u32), (3, 0b011)] {
        for i in 0..LW * LH {
            mem.bytes[phase_at + 4 * i..phase_at + 4 * i + 4].copy_from_slice(&marker.to_le_bytes());
        }
        let on = |b: u32| if cm >> b & 1 == 1 { 0xFFu64 } else { 0 };
        let mut u = [0u64; 64];
        let pair = |a: u64, b: u64| a | b << 32;
        let xy = |x: usize, y: usize| (x | y << 16) as u64;
        u[ls::PHASE as usize] = VA + phase_at as u64;
        u[ls::FRONT as usize] = VA + front_at as u64;
        u[ls::BLUE as usize] = VA + blue_at as u64;
        u[ls::RECT as usize] = pair(xy(x0, y0), xy(x1, y1));
        u[ls::SCREEN as usize] = pair(LW as u64, TURN[phase] as u64 | on(0) << 8 | on(1) << 16 | on(2) << 24);
        let rows = (y1 - y0) as u32;
        // Both sides: right first; each reaches from the axis to the rect's far edge on its side.
        u[ls::SIDE as usize] = pair(0, rows as u64);
        u[ls::STEPS as usize] = pair((x1 - axis) as u64, (axis - x0) as u64);
        u[ls::AXIS as usize] = pair(axis as u64, scroll as u64);
        u[ls::TEX as usize] = pair(texh as u64, frame as u64);
        u[ls::MASK as usize] = walk.0;
        u[ls::SPECKLE as usize] = walk.1;
        u[ls::BASE as usize] = walk.2;
        u[ls::SHIMMER as usize] = 0;
        u[ls::CLIP as usize] = pair(xy(clip.0, clip.1), xy(clip.2, clip.3));
        u[ls::LIGHT as usize] = VA + light_at as u64;
        u[ls::CODE as usize] = VA + code_at as u64;
        for i in 0..5 {
            let w = |j: usize| if j < 9 { odt[j].to_bits() as u64 } else { 0 };
            u[ls::ODT as usize + i] = pair(w(2 * i), w(2 * i + 1));
        }
        for (l, &(r, ring)) in layers.iter().enumerate() {
            let b = ls::LAYER as usize + 4 * l;
            u[b] = VA + (lay_at + l * LW * LH * 8) as u64;
            u[b + 1] = pair(xy(r.0, r.1), xy(r.2, r.3));
            u[b + 2] = pair(xy(ring.0, ring.1), xy(ring.2, ring.3));
            u[b + 3] = ring.4 as u64;
        }
        let n = ls::RESERVED as usize + k.consts.len();
        u[ls::RESERVED as usize..n].copy_from_slice(&k.consts);
        for g in 0..2 * rows {
            run_group(&k.words, &u[..n], g << k.wg_log2, 0, 1 << k.wg_log2, k.shared_bytes as usize, &mut mem);
        }
        // The reference.
        let mut half = vec![0u64; LW];
        for y in 0..LH {
            let t = ((y as i64 - scroll as i64).rem_euclid(texh as i64)) as u32;
            let mut nzr = vec![0u64; LW];
            noise::reference_row(t, false, LW - axis, walk.0, walk.1, walk.2, frame, &mut half);
            nzr[axis..].copy_from_slice(&half[..LW - axis]);
            noise::reference_row(t, true, axis, walk.0, walk.1, walk.2, frame, &mut half);
            for j in 0..axis {
                nzr[axis - 1 - j] = half[j];
            }
            for x in 0..LW {
                let i = y * LW + x;
                let got = u32::from_le_bytes(mem.bytes[phase_at + 4 * i..phase_at + 4 * i + 4].try_into().unwrap());
                if !(x >= x0 && x < x1 && y >= y0 && y < y1) {
                    outside += (got != marker) as usize;
                    continue;
                }
                let (mut acc, mut clear) = ([0u64; 3], 16384u64);
                for (l, &(r, ring)) in layers.iter().enumerate() {
                    if !(x >= r.0 && x < r.2 && y >= r.1 && y < r.3) || clear == 0 {
                        continue;
                    }
                    let (rx0, rx1, ry0, ry1, o) = ring;
                    let sy = if x >= rx0 && x < rx1 && y >= ry0 && y < ry1 { let (rr, h) = (y - ry0 + o, ry1 - ry0); ry0 + if rr >= h { rr - h } else { rr } } else { y };
                    let v = lay[l][sy * LW + x];
                    let see = v >> 63 != 0;
                    let lt = if see { 16384 } else { v >> 49 & 0x3FFF };
                    for c in 0..3 {
                        let ch = if see { 0 } else { (v >> (16 * c)) & 0xFFFF };
                        acc[c] += ch * clear >> 14;
                    }
                    clear = clear * lt >> 14;
                }
                if x >= clip.0 && x < clip.2 && y >= clip.1 && y < clip.3 && clear != 0 {
                    let nb = tables::bake_px(nzr[x], &light, &code, &odt, valhall::f16_to_f32);
                    for c in 0..3 {
                        acc[c] += nb[c] as u64 * clear >> 14;
                    }
                }
                let f = front[i];
                let ft = f >> 49;
                let px = if ft == 0x7FFF { acc[0] | acc[1] << 16 | acc[2] << 32 } else { (0..3).fold(f & 1 << 48, |a, c| a | (((f >> (16 * c)) & 0xFFFF) + (acc[c] * ft >> 14)) << (16 * c)) };
                let th = if px >> 48 & 1 == 1 { 0 } else { (BLUE_T[y & 15][x & 15] ^ TURN[phase]) as u64 };
                let code8 = |c: usize| (((((px >> (16 * c)) & 0xFFFF) + th).min(0xFFFF) >> 8) as u32) & on(c as u32) as u32;
                let want = code8(2) << 24 | code8(1) << 14 | code8(0) << 4 | 3;
                if got != want {
                    let d = [4u32, 14, 24].iter().map(|&sh| ((got >> sh) & 0xFF).abs_diff((want >> sh) & 0xFF)).max().unwrap();
                    far = far.max(d);
                    if wrong < 3 && d > 1 {
                        println!("layered, phase {phase} at {x},{y}: the kernel G#{got:08X}, the reference G#{want:08X}");
                    }
                    wrong += 1;
                }
            }
        }
    }
    println!("layered: {wrong} words of the owed rects differ (the most by {far} 8-bit codes: the bake's f32 rounding), {outside} written outside them");
    (wrong, far, outside)
}
