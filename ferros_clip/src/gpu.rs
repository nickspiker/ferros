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

/// Where things lie in the GPU block: the kernel's words at 0, the frame at 1 MiB (the code below it is the kernel's slack before the first row), then the codes and the facts each on a MiB.
pub struct Layout {
    pub frame: usize,
    pub codes: usize,
    pub info: usize,
    /// The block's size the layout needs, as a power of two.
    pub log: u32,
}

impl Layout {
    pub fn of(height: usize, stride: usize) -> Layout {
        let mib = |n: usize| n.div_ceil(1 << 20) << 20;
        let frame = 1 << 20;
        // The kernel's slack after the last row: 64 bytes, a MiB's rounding gives at least that only when the frame is not a whole MiB, so one more 2^16 always.
        let codes = frame + mib(height * stride + (1 << 16));
        let info = codes + mib((height << BLOCKS_LOG2 << BODY_LOG2) * 4);
        let end = info + mib((height << BLOCKS_LOG2) * 4);
        Layout { frame, codes, info, log: end.next_power_of_two().trailing_zeros() }
    }
}

#[cfg(target_os = "ferros")]
pub use crate::ferros::GpuDevice as Device;
