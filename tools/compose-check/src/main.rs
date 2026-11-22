//! The display's compose kernel (payloads/display/src/gpu_kernel.rs) on the host, in valhall's emulator, against the same frame worked out the way the display's NEON path works it (main.rs `publish_app_rows` with `glass_front` and `bars_front`, neon.rs `present_app16`): every word of the owed rect equal, none outside it written.
//! The frame: a small screen, a front layer of every kind (nothing in front, the mask whole, an edge or a bar partly covering), two windows that do not overlap (one opaque, one with see-through pixels and a ring), black elsewhere, two phases' turns, a channel off.

#[path = "../../../payloads/display/src/gpu_layout.rs"]
#[allow(dead_code)]
mod gpu_layout;
#[path = "../../../payloads/display/src/gpu_kernel.rs"]
mod gpu_kernel;

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
    std::process::exit((wrong + outside != 0) as i32);
}
