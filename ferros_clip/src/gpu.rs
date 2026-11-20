//! The band coder's first half on the GPU (the 60/s work, 2026-11-19): the kernel ([`kernel`], src/gpu_kernel.rs), its words built on the host for each known geometry ([`PREBUILT`], build.rs), and on ferros the GPU service's run of it over a frame ([`Device`]).
//! The CPU's half is ferros_rawcodec `pack_band_strided` over [`Device::info`] and [`Device::codes`].

#[path = "gpu_kernel.rs"]
mod gpu_kernel;
pub use gpu_kernel::*;

/// A kernel compiled on the host for one geometry.
pub struct Prebuilt {
    pub width: usize,
    pub stride: usize,
    /// 64 registers a thread (the run's preload bit 15).
    pub regs64: bool,
    pub words: &'static [u64],
    /// The uniforms after `U_INFO`, in order.
    pub consts: &'static [u64],
}

include!(concat!(env!("OUT_DIR"), "/kernels.rs"));

/// The host-built kernel for a geometry, if one was built.
pub fn prebuilt(width: usize, stride: usize) -> Option<&'static Prebuilt> {
    PREBUILT.iter().find(|p| p.width == width && p.stride == stride)
}

/// Band k of a `height`-row frame packed from a run's facts and bodies (ferros_rawcodec `pack_band_bodies`, Bayer): `encode_band`'s bytes, A then B; returns (la, lb).
pub fn pack_band(info: &[u32], bodies: &[u32], k: usize, width: usize, height: usize, out: &mut [u8]) -> (usize, usize) {
    use ferros_rawcodec as rc;
    let at = (k * rc::BAND_ROWS) << BLOCKS_LOG2;
    rc::pack_band_bodies(&info[at..], 1 << BLOCKS_LOG2, &bodies[(at << BODY_LOG2) + BODY_AT..], BODY_LOG2, width, rc::band_rows(height, k), rc::Cfa::Bayer, out)
}

/// Sets of the frame, the bodies and the facts in the block: a frame is copied and run into one while the coders pack the one before from the other.
pub const SETS: usize = 2;

/// Where things lie in the GPU block: the kernel's words at 0, then each set's frame (the MiB before it is the kernel's slack before the first row), bodies and facts, each on a MiB.
pub struct Layout {
    pub frame: [usize; SETS],
    pub codes: [usize; SETS],
    pub info: [usize; SETS],
    /// The block's size the layout needs, as a power of two.
    pub log: u32,
}

impl Layout {
    pub fn of(height: usize, stride: usize) -> Layout {
        let mib = |n: usize| n.div_ceil(1 << 20) << 20;
        let (mut frame, mut codes, mut info) = ([0; SETS], [0; SETS], [0; SETS]);
        let mut at = 1 << 20;
        for s in 0..SETS {
            frame[s] = at;
            // The kernel's slack after the last row: 64 bytes, a MiB's rounding gives at least that only when the frame is not a whole MiB, so one more 2^16 always.
            codes[s] = frame[s] + mib(height * stride + (1 << 16));
            info[s] = codes[s] + mib((height << BLOCKS_LOG2 << BODY_LOG2) * 4);
            at = info[s] + mib((height << BLOCKS_LOG2) * 4);
        }
        Layout { frame, codes, info, log: at.next_power_of_two().trailing_zeros() }
    }
}

/// Whether `Recorder::start` codes on the GPU when the process holds one (on by default; a bench turns it off to compare).
static OFF: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// The recorder's GPU coding on (the default) or off for clips started from now.
pub fn enable(on: bool) {
    OFF.store(!on, core::sync::atomic::Ordering::Relaxed);
}

pub fn enabled() -> bool {
    !OFF.load(core::sync::atomic::Ordering::Relaxed)
}

#[cfg(target_os = "ferros")]
pub use crate::ferros::GpuDevice as Device;
#[cfg(not(target_os = "ferros"))]
pub use HostDevice as Device;

/// The GPU's work done on the host's CPU (ferros_rawcodec `split_codes`, each block's codes laid end to end as the kernel lays them): the recorder's GPU path tested on a host (tests/record.rs), the same facts and bodies the device's run leaves.
#[cfg(not(target_os = "ferros"))]
pub struct HostDevice {
    width: usize,
    height: usize,
    stride: usize,
    frame: [Vec<u8>; SETS],
    info: [Vec<u32>; SETS],
    bodies: [Vec<u32>; SETS],
}

#[cfg(not(target_os = "ferros"))]
impl HostDevice {
    pub fn new(width: usize, height: usize, stride: usize) -> HostDevice {
        let blocks = height << BLOCKS_LOG2;
        HostDevice { width, height, stride, frame: core::array::from_fn(|_| vec![0; height * stride]), info: core::array::from_fn(|_| vec![0; blocks]), bodies: core::array::from_fn(|_| vec![0; blocks << BODY_LOG2]) }
    }

    pub fn put(&mut self, set: usize, rows: &[u8]) {
        let n = rows.len().min(self.height * self.stride);
        self.frame[set][..n].copy_from_slice(&rows[..n]);
    }

    pub fn run(&mut self, set: usize, rows: &[u8]) -> Result<u32, String> {
        self.put(set, rows);
        self.go(set)
    }

    pub fn go(&mut self, set: usize) -> Result<u32, String> {
        use ferros_rawcodec as rc;
        let (w, h, stride) = (self.width, self.height, self.stride);
        let nb = rc::row_blocks(w);
        let mut work = Box::new(rc::Work::new());
        let (mut ci, mut cc) = (vec![0u32; rc::BAND_ROWS * nb], vec![0u16; rc::BAND_ROWS * w]);
        for k in 0..rc::bands(h) {
            let rows = rc::band_rows(h, k);
            rc::split_codes(&self.frame[set][rc::band_src(k, h, w, stride)], stride, w, rows, rc::Cfa::Bayer, &mut work, &mut ci, &mut cc);
            for y in 0..rows {
                for j in 0..nb {
                    let at = ((k * rc::BAND_ROWS + y) << BLOCKS_LOG2) + j;
                    let body = &mut self.bodies[set][(at << BODY_LOG2) + BODY_AT..(at + 1) << BODY_LOG2];
                    body.fill(0);
                    let mut pos = 0usize;
                    for &cd in &cc[y * w + j * rc::BLOCK..y * w + ((j + 1) * rc::BLOCK).min(w)] {
                        let l = (cd >> rc::SPLIT_LEN_SHIFT) as usize;
                        for s in (0..l).rev() {
                            body[pos >> 5] |= ((((cd & 0x3FF) as u32) >> s) & 1) << (31 - (pos & 31));
                            pos += 1;
                        }
                    }
                    self.info[set][at] = ci[y * nb + j] | (pos as u32) << rc::SPLIT_BITS_SHIFT;
                }
            }
        }
        Ok(0)
    }

    pub fn frame(&self, set: usize) -> &[u8] {
        &self.frame[set]
    }

    pub fn info(&self, set: usize) -> &[u32] {
        &self.info[set]
    }

    pub fn bodies(&self, set: usize) -> &[u32] {
        &self.bodies[set]
    }
}
