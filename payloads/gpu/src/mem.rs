//! The memory the GPU and this process share (`gpu-mem`): a bump allocator over it, and the GPU's page tables built in it.
//!
//! The region is mapped for this process at its physical address (the kernel's identity map, VHE), so an allocation's address is both the CPU's pointer and the bus address the GPU and its page-table walker see. It is Normal non-cacheable on the CPU side: the GPU does not snoop the CPU's caches, so nothing written here needs cleaning.

/// A 4 KiB page.
pub const PAGE: usize = 1 << 12;

/// The region's base, and how much of it is handed out (from its start, in whole pages).
pub struct Mem {
    pub base: usize,
    used: usize,
}

impl Mem {
    pub fn new(base: usize) -> Mem {
        Mem { base, used: 0 }
    }

    /// `pages` zeroed pages, page aligned. Running past the region is a bug in this driver's budget (32 MiB for the firmware, its tables and one group); nothing checks it, since the first write past the end faults by itself (the memory above `gpu-mem` is not mapped for EL0).
    pub fn alloc(&mut self, pages: usize) -> usize {
        let a = self.base + self.used;
        self.used += pages * PAGE;
        for w in 0..pages * PAGE / 8 {
            unsafe { core::ptr::write_volatile((a + w * 8) as *mut u64, 0) };
        }
        a
    }

    pub fn used(&self) -> usize {
        self.used
    }
}

pub fn wr32(a: usize, v: u32) {
    unsafe { core::ptr::write_volatile(a as *mut u32, v) }
}
pub fn rd32(a: usize) -> u32 {
    unsafe { core::ptr::read_volatile(a as *const u32) }
}
pub fn wr64(a: usize, v: u64) {
    unsafe { core::ptr::write_volatile(a as *mut u64, v) }
}
pub fn rd64(a: usize) -> u64 {
    unsafe { core::ptr::read_volatile(a as *const u64) }
}

/// One address space's translation table: four levels of 512 entries over a 48-bit VA, 4 KiB pages, tables allocated as they are needed.
pub struct Table {
    pub root: usize,
}

impl Table {
    pub fn new(m: &mut Mem) -> Table {
        Table { root: m.alloc(1) }
    }

    /// Map `pages` pages from GPU VA `va` to physical `pa`, each leaf entry `pa | leaf` (`leaf` carries the format's valid/page bits and the attributes: GPU.md "MMU"); intermediate levels get `table` (the format's table-descriptor bits).
    pub fn map(&self, m: &mut Mem, va: u64, pa: usize, pages: usize, leaf: u64, table: u64) {
        for k in 0..pages {
            let v = va + (k * PAGE) as u64;
            let mut t = self.root;
            for level in 0..3 {
                let idx = ((v >> (39 - 9 * level)) & 511) as usize;
                let e = rd64(t + idx * 8);
                t = if e & 1 == 0 {
                    let n = m.alloc(1);
                    wr64(t + idx * 8, n as u64 | table);
                    n
                } else {
                    (e & 0x0000_FFFF_FFFF_F000) as usize
                };
            }
            let idx = ((v >> 12) & 511) as usize;
            wr64(t + idx * 8, (pa + k * PAGE) as u64 | leaf);
        }
    }
}
