//! The front end's frame geometry, rewritten for a mode other than the one its job was captured at (docs/CAMERA.md "Modes, the bench and the slit"), and the slim per-frame program.
//!
//! The captured program and the CPU's setup writes carry the geometry in the open (docs/oracle/CAMERA.md "The sensor is ours"): the line stride (G#1400 for 4080 RAW10 samples on a 256-byte boundary) at loch0's image path +G#554 and PD path +G#2154; the frame size as (height << 16) | width at the image path +G#474, the PD path +G#2074 (its height a quarter), the CSI link's three channels (+G#44, +G#54, +G#64), and about a dozen processing blocks; the height halved and quartered, the width halved and quartered, here and there.
//! October's tools/camera-pdma-image.py `--height` rewrote every height-derived half anywhere in the program; `Scope::All` does the same for heights and widths, `Scope::Core` only the image and PD paths and the link, which is all a frame's way into memory goes thru (the processing blocks write statistics, not the image).
//! A half that holds one of the old sizes for another reason is rewritten too: the risk the October tool took, said on the console with the count.

/// A frame's shape as the front end sees it: width and height in samples, a line's bytes, the PD channel's height, and the image's MIPI data type (G#2B RAW10, G#2C RAW12).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geom {
    pub w: u16,
    pub h: u16,
    pub stride: u32,
    pub pd_h: u16,
    pub dt: u8,
}

pub const DT_RAW10: u8 = 0x2B;
pub const DT_RAW12: u8 = 0x2C;

impl Geom {
    /// A mode's geometry: a line of `w` samples at `bits` each on a 256-byte boundary (the captured strides: the main's 4080 RAW10 G#1400; the ultrawide's 4000 at G#13C0 is on 64, so a camera's own captured stride wins where the width is its captured one, `with_stride`), the PD channel a quarter of the rows (the main's 768 of 3072).
    pub const fn of(w: u16, h: u16, bits: u16) -> Geom {
        let line = w as u32 * bits as u32 / 8;
        Geom { w, h, stride: (line + 255) & !255, pd_h: h / 4, dt: if bits == 12 { DT_RAW12 } else { DT_RAW10 } }
    }
    pub const fn with_stride(mut self, stride: u32) -> Geom {
        self.stride = stride;
        self
    }
}

/// How far the rewrite reaches.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scope {
    /// loch0's image and PD paths (G#1C32_0000 to G#1C33_0000) and the CSI links' channel words: the frame's way into memory.
    Core,
    /// Every register the program and the CPU write (the October tool's reach).
    All,
}

/// The CSI links' channel words: data type and size for channels 0 to 2, at +G#40/+G#44, +G#50/+G#54, +G#60/+G#64 of each link (G#1C04_0000 link 0, G#1C05_0000 link 1, G#1C06_0000 link 2, ...).
fn link_word(reg: u32) -> Option<u32> {
    let o = reg & 0xFFFF;
    ((0x1C04_0000..0x1C0C_0000).contains(&reg) && matches!(o, 0x40 | 0x44 | 0x50 | 0x54 | 0x60 | 0x64)).then_some(o)
}

/// The image path's and PD path's stride registers (loch0, the main's job: G#1400 at both).
pub const STRIDE_REGS: [u32; 2] = [0x1C32_0554, 0x1C32_2154];

/// One register value rewritten from `from` to `to`, or as it was. Address registers never reach here (the job's `relocate` owns them).
pub fn value(reg: u32, v: u32, from: &Geom, to: &Geom, scope: Scope) -> u32 {
    if STRIDE_REGS.contains(&reg) && v == from.stride {
        return to.stride;
    }
    let link = link_word(reg);
    // A channel's data type (bits 7 to 2 of +G#40 and +G#50; channel 2 is embedded data, G#12, and keeps its type).
    if matches!(link, Some(0x40 | 0x50)) && (v >> 2) & 0x3F == from.dt as u32 {
        return v & !(0x3F << 2) | (to.dt as u32) << 2;
    }
    let core = (0x1C32_0000..0x1C33_0000).contains(&reg) || matches!(link, Some(0x44 | 0x54 | 0x64));
    if scope == Scope::Core && !core {
        return v;
    }
    // The embedded-data channel's size: its four lines stay, its width follows.
    let m = |half: u32| -> u32 {
        let h = half as u16;
        let r = if h == from.h {
            to.h
        } else if h == from.h / 2 {
            to.h / 2
        } else if h == from.pd_h {
            to.pd_h
        } else if h == from.w {
            to.w
        } else if h == from.w / 2 {
            to.w / 2
        } else if h == from.w / 4 && scope == Scope::All {
            to.w / 4
        } else {
            h
        };
        r as u32
    };
    m(v >> 16) << 16 | m(v & 0xFFFF)
}

fn w32(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// Every write a program (a job's `list_piece`) makes: (the value's word index, its register), in order; op 2 (register, value) pairs and op 1 bursts, as `job::each_write` reads them.
pub fn writes(list: &[u8], f: &mut dyn FnMut(usize, u32)) {
    let words = list.len() / 4;
    let mut i = 0;
    while i < words {
        let h = w32(list, i * 4);
        if h == 0 {
            break;
        }
        let (cnt, op) = ((h >> 14) as usize, h & 0x3FFF);
        if i + 1 + cnt > words {
            break;
        }
        match op {
            2 => {
                let mut k = i + 1;
                while k + 1 <= i + cnt {
                    f(k + 1, crate::job::ISPFE.wrapping_add(w32(list, k * 4)));
                    k += 2;
                }
            }
            1 if cnt > 0 => {
                let start = crate::job::ISPFE.wrapping_add(w32(list, (i + 1) * 4));
                for j in 0..cnt - 1 {
                    f(i + 2 + j, start.wrapping_add(4 * j as u32));
                }
            }
            _ => {}
        }
        i += 1 + cnt;
    }
}

/// A program's geometry rewritten in place: how many words changed.
pub fn rewrite(list: &mut [u8], from: &Geom, to: &Geom, scope: Scope) -> u32 {
    let mut changes = [(0usize, 0u32); 1 << 7];
    let mut n = 0;
    {
        let snap: &[u8] = list;
        writes(snap, &mut |i, reg| {
            if crate::job::ADDR_REGS.contains(&reg) || crate::job::ADDR_REGS.contains(&reg.wrapping_sub(4)) {
                return;
            }
            let v = w32(snap, i * 4);
            let nv = value(reg, v, from, to, scope);
            // WHY the bound: the main's program has 27 geometry words; a table this size holds every one, and one past it is left as captured and counted below as not rewritten.
            if nv != v && n < changes.len() {
                changes[n] = (i, nv);
                n += 1;
            }
        });
    }
    for &(i, v) in &changes[..n] {
        list[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    n as u32
}

/// The processing blocks' switches, what a windowed mode may turn off so loch0's frame goes to memory alone (docs/CAMERA.md "The slit's second blocker"): (register, the value written instead, the bit of the `detach` mask that asks it).
/// Read off the slit deaths of 2026-11-17 night (`ack_say`): with a window, the bayer front-end context 3 (FC ctx3, G#1C33_7300) reports FC_ERROR (bit 7) and the LMP's bayer context 3 (G#1C35_00E8) FE_LMP_ERROR (bit 3), never their EOF, then loch0 SHORT_VB_FATAL; with whole frames neither errs. Both blocks are set up by the program for 4080 x 3072 (FC ctx3's size at G#1C33_7904, its statistics windows past G#1C33_7500, the LMP's thousands of words at G#1C35_4000), and geometry everywhere (`Scope::All`) did not save the slit.
/// The values are Linux's close's (kernel camera_close.rs): G#1C33_7028 and G#1C33_B028 to 0 (the contexts' enables, 1 in the program), the crossbar words G#1C33_0C08 and G#1C33_0D08 to 2 (G#7C and 2 at the open, CPU 'before'), and G#1C34_0000 to 0 (1 in the program; the block at G#1C34 sized for the PD rows, G#1C34_0014 = 768 x 4080).
pub const DETACH: [(u32, u32, u8); 5] = [(0x1C33_7028, 0, 1 << 0), (0x1C33_B028, 0, 1 << 1), (0x1C33_0C08, 2, 1 << 2), (0x1C33_0D08, 2, 1 << 2), (0x1C34_0000, 0, 1 << 3)];

/// An open's processing-block switches off (`DETACH`) and whether it runs the slim program: `over` the driver's override byte (0 the defaults; with G#80 its low four bits the switches exactly and `asked` the program), `windowed` a frame not of the captured shape (a slit, an x window, or a 1:1 frame), `asked` the open's own slim flag.
/// The defaults, as measured on the Pro: a windowed mode detached (FC ctx3 off: windowed, it erred and loch0 stalled on SHORT_VB_FATAL) and slim (detached, the full program missed a slit's blanking, LATE_CONFIG; detached and slim, the main's 261120 slits at 1230 a second, none lost, 2026-11-18); whole frames as asked.
/// On every camera since 2026-11-18 (docs/CAMERA.md "Slits on every camera"): under Lumis the ultrawide's, the front's and the tele's slits took two frames and stopped (the full program, FC ctx3 on), and each streams detached and slim. FC ctx3 is in every camera's job (the front's 8.42 MP stalls on it even whole, `Alt::detach`).
/// A 1:1 frame takes the same defaults: the statistics pipeline is laid out for the binned frame as it is for the windows it erred on, and detached and slim is the shape whole binned frames ran clean in (the line sweep, 100 a second; the main's 1:1 ran so, 2026-11-18); an override (`over`) keeps it as asked for the A/B.
pub fn shape(over: u8, windowed: bool, asked: bool) -> (u8, bool) {
    match (over & 0x80 != 0, windowed) {
        (true, _) => (over & 0xF, asked),
        (false, true) => (1, true),
        (false, false) => (0, asked),
    }
}

/// A write's value with the `mask`'s switches off (`DETACH`), or as it was.
pub fn detached(reg: u32, v: u32, mask: u8) -> u32 {
    for &(r, off, bit) in DETACH.iter() {
        if r == reg && mask & bit != 0 {
            return off;
        }
    }
    v
}

/// The program's writes with the `mask`'s switches off (`DETACH`): how many words changed.
pub fn detach(list: &mut [u8], mask: u8) -> u32 {
    let mut changes = [(0usize, 0u32); 1 << 3];
    let mut n = 0;
    {
        let snap: &[u8] = list;
        writes(snap, &mut |i, reg| {
            let v = w32(snap, i * 4);
            let nv = detached(reg, v, mask);
            // WHY the bound: `DETACH` names five registers and the main's program writes each at most once; one past the table is left as captured.
            if nv != v && n < changes.len() {
                changes[n] = (i, nv);
                n += 1;
            }
        });
    }
    for &(i, v) in &changes[..n] {
        list[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    n as u32
}

/// The slim per-frame program's reach: level 1 the image and PD paths (G#1C32: their DMA setup, addresses and requests, the frame's whole way into memory), level 2 also the processing blocks' request and go words the program ends with (G#1C33_7xxx, G#1C33_Bxxx, G#1C34_00xx).
pub fn slim_reg(reg: u32, level: u8) -> bool {
    let l1 = (0x1C32_0000..0x1C33_0000).contains(&reg);
    let l2 = (0x1C33_7000..0x1C33_7100).contains(&reg) || (0x1C33_B000..0x1C33_B100).contains(&reg) || (0x1C34_0000..0x1C34_0100).contains(&reg);
    l1 || (level >= 2 && l2)
}

/// A slim copy of a program: only its writes `slim_reg` keeps, in order, as one op-2 record and the terminating zero, into `out`; its length in bytes. The full program re-runs every processing block's setup and its twelve LUT loads each frame (6620 bytes, 1447 writes and 3048 loaded words for the main); at thousands of frames a second that is the engine's time, the slim one re-arms only the frame's way into memory.
/// None when `out` is too small.
pub fn slim(list: &[u8], level: u8, out: &mut [u8]) -> Option<usize> {
    let mut n = 0usize;
    let mut fits = true;
    {
        writes(list, &mut |i, reg| {
            if !slim_reg(reg, level) {
                return;
            }
            let at = 4 + n * 8;
            if at + 8 + 4 > out.len() {
                fits = false;
                return;
            }
            out[at..at + 4].copy_from_slice(&reg.wrapping_sub(crate::job::ISPFE).to_le_bytes());
            out[at + 4..at + 8].copy_from_slice(&list[i * 4..i * 4 + 4]);
            n += 1;
        });
    }
    if !fits {
        return None;
    }
    out[..4].copy_from_slice(&(((2 * n as u32) << 14) | 2).to_le_bytes());
    let end = 4 + n * 8;
    out[end..end + 4].copy_from_slice(&0u32.to_le_bytes());
    Some(end + 4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job;

    const MAIN: Geom = Geom { w: 4080, h: 3072, stride: 0x1400, pd_h: 768, dt: DT_RAW10 };

    #[test]
    fn values() {
        let slit = Geom::of(4080, 4, 10).with_stride(0x1400);
        assert_eq!(slit.pd_h, 1);
        assert_eq!(value(0x1C32_0474, 0x0C00_0FF0, &MAIN, &slit, Scope::Core), 0x0004_0FF0);
        assert_eq!(value(0x1C32_2074, 0x0300_0FF0, &MAIN, &slit, Scope::Core), 0x0001_0FF0);
        assert_eq!(value(0x1C05_0044, 0x0C00_0FF0, &MAIN, &slit, Scope::Core), 0x0004_0FF0);
        // The embedded-data channel keeps its four lines.
        assert_eq!(value(0x1C05_0064, 0x0004_0FF0, &MAIN, &slit, Scope::Core), 0x0004_0FF0);
        // A processing block: left in Core, rewritten in All.
        assert_eq!(value(0x1C35_4774, 0x0C00_0FF0, &MAIN, &slit, Scope::Core), 0x0C00_0FF0);
        assert_eq!(value(0x1C35_4774, 0x0C00_0FF0, &MAIN, &slit, Scope::All), 0x0004_0FF0);
        let full = Geom::of(8160, 6144, 10);
        assert_eq!(full.stride, 0x2800);
        assert_eq!(value(0x1C32_0554, 0x1400, &MAIN, &full, Scope::Core), 0x2800);
        assert_eq!(value(0x1C32_0474, 0x0C00_0FF0, &MAIN, &full, Scope::Core), 0x1800_1FE0);
        assert_eq!(value(0x1C35_4E3C, 0x07F8_0004, &MAIN, &full, Scope::All), 0x0FF0_0004);
        let r12 = Geom::of(4080, 3072, 12);
        assert_eq!(r12.stride, 0x1800);
        assert_eq!(value(0x1C05_0040, 0x30AC, &MAIN, &r12, Scope::Core), 0x30B0);
        assert_eq!(value(0x1C05_0050, 0x130AC, &MAIN, &r12, Scope::Core), 0x130B0);
        // The embedded channel's type (G#12) is not the image's.
        assert_eq!(value(0x1C05_0060, 0xB048, &MAIN, &r12, Scope::Core), 0xB048);
    }

    /// The main's job (target/cam/cam.pdma, local): the slit's rewrite touches the image and PD paths' sizes and strides only where they differ, and the slim program keeps the image body's address once.
    #[test]
    fn the_local_job() {
        let Ok(mut jb) = std::fs::read("/mnt/Harbor/Code/ferros/target/cam/cam.pdma") else { return };
        let (at, len) = job::list_piece(&jb).unwrap();
        let orig = jb[at..at + len].to_vec();
        let slit = Geom::of(4080, 4, 10).with_stride(0x1400);
        let core = rewrite(&mut jb[at..at + len], &MAIN, &slit, Scope::Core);
        let mut all_list = orig.clone();
        let all = rewrite(&mut all_list, &MAIN, &slit, Scope::All);
        println!("a slit of 4: A#{core} words in Core, A#{all} in All");
        // The program writes loch0's sizes only thru the CPU (G#1C32_0474 is a CPU 'after' write), so Core leaves the program alone for a slit: its strides do not move.
        assert_eq!(core, 0);
        assert!(all >= 19, "the October tool rewrote 19 words for a 64-row slit");
        let full = Geom::of(8160, 6144, 10);
        let mut l = orig.clone();
        let n = rewrite(&mut l, &MAIN, &full, Scope::Core);
        assert_eq!(n, 2, "the two strides");
        let mut out = [0u8; 1 << 10];
        for level in [1u8, 2] {
            let k = slim(&orig, level, &mut out).unwrap();
            let (lo, hi) = job::image_words(&out[..k]).unwrap();
            assert_eq!(w32(&out, lo * 4), job::OLD_SLICES as u32 + 2 * job::OLD_SLICE as u32);
            assert_eq!(hi, lo + 2);
            println!("slim level {level}: {k} bytes of the program's {len}");
        }
    }

    /// The CPU's 'after' writes (cam.frame's 'CPR2'): the image path's size and the link's three channels are there, and Core rewrites them.
    #[test]
    fn the_local_cpu_writes() {
        let Ok(b) = std::fs::read("/mnt/Harbor/Code/ferros/target/cam/cam.frame") else { return };
        let (_, prog) = crate::inputs::split_bundle(&b).unwrap();
        let p = crate::inputs::prog(prog).unwrap();
        let slit = Geom::of(4080, 4, 10).with_stride(0x1400);
        let mut changed = vec![];
        for (a, v) in p.before.iter().chain(p.after.iter()) {
            let nv = value(a as u32, v, &MAIN, &slit, Scope::Core);
            if nv != v {
                changed.push((a, v, nv));
            }
        }
        for c in &changed {
            println!("G#{:X}: G#{:X} -> G#{:X}", c.0, c.1, c.2);
        }
        assert!(changed.iter().any(|c| c.0 == 0x1C32_0474) && changed.iter().any(|c| c.0 == 0x1C05_0044));
    }

    /// The processing blocks' switches (`DETACH`): each bit changes only its own words, in the main's program (G#1C33_7028, G#1C33_B028, G#1C34_0000, once each) and its CPU 'before' writes (the crossbar's G#1C33_0C08 from G#7C; G#1C33_0D08 is 2 already); none of loch0's.
    #[test]
    fn windowed_defaults() {
        // Defaults: any camera windowed detached and slim; whole frames as asked.
        assert_eq!(shape(0, true, false), (1, true));
        assert_eq!(shape(0, false, false), (0, false));
        assert_eq!(shape(0, false, true), (0, true));
        // The override: G#80 nothing switched, the program as asked; G#8F every switch.
        assert_eq!(shape(0x80, true, false), (0, false));
        assert_eq!(shape(0x8F, false, true), (0xF, true));
    }

    #[test]
    fn detach_switches() {
        assert_eq!(detached(0x1C33_7028, 1, 1), 0);
        assert_eq!(detached(0x1C33_7028, 1, 2), 1);
        assert_eq!(detached(0x1C33_0C08, 0x7C, 4), 2);
        assert_eq!(detached(0x1C32_0474, 0x0C00_0FF0, 0xFF), 0x0C00_0FF0);
        let Ok(jb) = std::fs::read("/mnt/Harbor/Code/ferros/target/cam/cam.pdma") else { return };
        let (at, len) = job::list_piece(&jb).unwrap();
        let orig = jb[at..at + len].to_vec();
        for (mask, want) in [(1u8, 1u32), (2, 1), (4, 0), (8, 1), (0xF, 3)] {
            let mut l = orig.clone();
            assert_eq!(detach(&mut l, mask), want, "mask {mask}");
        }
        let Ok(b) = std::fs::read("/mnt/Harbor/Code/ferros/target/cam/cam.frame") else { return };
        let (_, prog) = crate::inputs::split_bundle(&b).unwrap();
        let p = crate::inputs::prog(prog).unwrap();
        let n = p.before.iter().chain(p.after.iter()).filter(|&(a, v)| detached(a as u32, v, 4) != v).count();
        assert_eq!(n, 1);
    }
}
