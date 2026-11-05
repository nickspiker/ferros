//! The CSF firmware image (`gpu.fw` in the vault: Arm's mali_csffw-r52p0.bin from the device's own vendor image), read whole into `gpu-mem` so its sections can be copied from it to where the MCU sees them.

use crate::mem::{Mem, PAGE};

/// The image in memory: its address and length.
pub struct Image {
    pub at: usize,
    pub len: usize,
}

impl Image {
    pub fn bytes(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.at as *const u8, self.len) }
    }
    pub fn u32_at(&self, off: usize) -> u32 {
        let b = self.bytes();
        u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
    }
}

/// The image from the vault scope `store` into freshly allocated pages, 4 KiB a read. None when the vault has no `gpu.fw`.
/// Each read lands in a buffer in the arc and is copied on from there: a syscall's buffer is an offset into the arc, and `gpu-mem` lies outside it (a read straight into it was taken as an offset and overwrote this process's own code, 2026-11-04).
pub fn load(store: u64, m: &mut Mem) -> Option<Image> {
    let mut head = [0u8; 4];
    let len = ferros_sys::store_get_at(store, b"fw", 0, &mut head)?;
    let at = m.alloc(len.div_ceil(PAGE));
    let mut buf = [0u8; PAGE];
    let mut off = 0;
    while off < len {
        let n = (len - off).min(PAGE);
        ferros_sys::store_get_at(store, b"fw", off, &mut buf[..n])?;
        unsafe { core::ptr::copy_nonoverlapping(buf.as_ptr(), (at + off) as *mut u8, n) };
        off += n;
    }
    Some(Image { at, len })
}
