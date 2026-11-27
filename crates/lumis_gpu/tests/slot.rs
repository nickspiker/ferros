//! The slot kernel (src/lib.rs `slot_kernel`) in valhall's emulator against lumis_core's CPU slot (`ui::draw::slot_cpu`): over a noisy background, for the camera picker's camera slot (pressed and not), Exit and a label slot, at the Pro's slot size and at half and twice it (the app scale): every channel of every pixel within a few codes, most exact.

use lumis_core::ui::draw::{slot_cpu, Channel, Slot};
use lumis_gpu::{slot, slot_kernel};
use valhall::sim::{run, Memory};

fn noise(n: usize) -> Vec<u8> {
    let mut s = 0x9E37_79B9_7F4A_7C15u64;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s as u8
        })
        .collect()
}

/// The largest difference of a channel, and how many differ at all, between the kernel's pixels and the CPU's.
fn compare(s: &Slot, stride: usize, rows: usize) -> (u8, usize) {
    let bg = noise(stride * rows * 3);
    let mut cpu = bg.clone();
    slot_cpu(s, &mut cpu, stride);
    // The GPU's memory: the pixels, a u32 each (R | G << 8 | B << 16).
    let base = 0x8000_0000u64;
    let mut mem = Memory { base, bytes: vec![0u8; stride * rows * 4] };
    for i in 0..stride * rows {
        mem.bytes[4 * i..4 * i + 3].copy_from_slice(&bg[3 * i..3 * i + 3]);
    }
    let k = slot_kernel();
    let w = |a: f32, b: f32| (a.to_bits() as u64) | (b.to_bits() as u64) << 32;
    let wu = |a: u32, b: u32| a as u64 | (b as u64) << 32;
    let (run_, rise) = ((s.x1 - s.x0) as u32, (s.y1 - s.y0) as u32);
    let mut uniforms = vec![0u64; slot::SLOTS as usize];
    uniforms[slot::OUT as usize] = base;
    uniforms[slot::ORIGIN as usize] = wu(s.x0 as u32, s.y0 as u32);
    uniforms[slot::SIZE as usize] = wu(run_, rise);
    uniforms[slot::STRIDE as usize] = wu(stride as u32, 0);
    uniforms[slot::SHAPE as usize] = w(s.big, s.glow);
    for c in 0..3 {
        uniforms[slot::CH[c] as usize] = w(s.ch[c].k, s.ch[c].f);
    }
    uniforms[slot::COLOUR_RG as usize] = w(s.ch[0].colour, s.ch[1].colour);
    uniforms[slot::COLOUR_B_FA as usize] = w(s.ch[2].colour, s.fa);
    uniforms.extend_from_slice(&k.consts);
    for id in 0..run_ * rise {
        run(&k.words, &uniforms, [id, 0], &mut mem);
    }
    let (mut most, mut differ) = (0u8, 0usize);
    for i in 0..stride * rows {
        for c in 0..3 {
            let d = mem.bytes[4 * i + c].abs_diff(cpu[3 * i + c]);
            most = most.max(d);
            differ += (d != 0) as usize;
        }
    }
    (most, differ)
}

fn slot_at(scale: f32, ch: [Channel; 3], fa: f32) -> (Slot, usize, usize) {
    // The Pro's picker slot: the panel's width less its margins, a tenth of its height between them, at the app scale.
    let (w, h) = ((1232. * scale) as i32 / 4, (274. * scale) as i32 / 4);
    let (x0, y0) = (3, 5);
    let s = Slot { x0, y0, x1: x0 + w, y1: y0 + h, big: h as f32 / 2., glow: h as f32 / 3., ch, fa };
    (s, (x0 + w + 7) as usize, (y0 + h + 4) as usize)
}

#[test]
fn slots_against_the_cpu() {
    let ch = |k: f32, f: f32, colour: f32| Channel { k, f, colour };
    let camera = [ch(10., 1.53, 0xC0 as f32), ch(8., 1.53, 0xC0 as f32), ch(5., 1.81, 0x48 as f32)];
    let exit = [ch(8., 1.65, 256.), ch(5., 1.8, 64.), ch(5., 1.8, 0.)];
    let teal = [ch(5., 1.7, 0.), ch(5., 1.7, 0xE0 as f32), ch(8., 1.55, 256.)];
    for (what, chs, fa) in [("camera", camera, 1.75), ("exit pressed", exit, 1.65), ("label", teal, 1.75)] {
        for scale in [0.5f32, 1., 2.] {
            let (s, stride, rows) = slot_at(scale, chs, fa);
            let (most, differ) = compare(&s, stride, rows);
            eprintln!("{what} at {scale}: {} x {}: a channel off by {most} at most, {differ} channels differ", s.x1 - s.x0, s.y1 - s.y0);
            assert!(most <= 2, "{what} at {scale}: off by {most}");
        }
    }
}
