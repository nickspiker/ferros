//! Probe whether ABL's UFS descriptor carveout (UTRLBA G#F8C42000) is CPU-readable, and dump the UFS S2MPU.
//!
//! The pbl-style reuse test showed UTRLBA=G#F8C42000 (a high region) but reads of its UTRD DW4/DW5 came back 0.
//! This isolates: is that high region CPU-accessible at all (reads nonzero = readable; all-zero = protected carveout the UFS master reaches but the CPU can't)? And what does the HSI2 UFS S2MPU (G#131F0000) actually hold — is it truly disabled, or is there a whitelist that only admits the carveout?

#![no_std]
#![no_main]

struct NoAlloc;
unsafe impl core::alloc::GlobalAlloc for NoAlloc {
    unsafe fn alloc(&self, _: core::alloc::Layout) -> *mut u8 { core::ptr::null_mut() }
    unsafe fn dealloc(&self, _: *mut u8, _: core::alloc::Layout) {}
}
#[global_allocator]
static NO_ALLOC: NoAlloc = NoAlloc;

struct Out { ptr: *mut u8, cap: usize, n: usize }
impl Out {
    fn put(&mut self, b: u8) { if self.n < self.cap { unsafe { *self.ptr.add(self.n) = b; } self.n += 1; } }
    fn s(&mut self, s: &[u8]) { for &b in s { self.put(b); } }
    fn hex(&mut self, v: u32) { let h = b"0123456789ABCDEF"; for i in (0..8).rev() { self.put(h[((v >> (i * 4)) & 0xF) as usize]); } }
    fn line(&mut self, l: &[u8], v: u32) { self.s(l); self.put(b'='); self.hex(v); self.put(b'\n'); }
}
fn rd(a: usize) -> u32 { unsafe { core::ptr::read_volatile(a as *const u32) } }

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(_i: *const u8, _il: usize, out: *mut u8, cap: usize) -> u64 {
    unsafe extern "C" { static mut __bss_start: u8; static mut __bss_end: u8; }
    unsafe { let mut p = &raw mut __bss_start as *mut u8; let e = &raw mut __bss_end as *mut u8; while p < e { core::ptr::write_volatile(p, 0); p = p.add(1); } }
    let mut o = Out { ptr: out, cap, n: 0 };

    // UFS transfer-list base ABL left running.
    let utrlba = rd(0x1320_0050);
    o.line(b"UTRLBA", utrlba);
    o.line(b"UTRLBAU", rd(0x1320_0054));
    o.line(b"UTRLRSR", rd(0x1320_0060));

    // Is ABL's descriptor region CPU-readable? Dump the first 8 words of the UTRD.
    let base = utrlba as usize;
    for i in 0..8u32 {
        let mut lbl = *b"DW0 ";
        lbl[2] = b'0' + i as u8;
        o.line(&lbl[..3], rd(base + (i as usize) * 4));
    }

    // Compare against a known-CPU-readable low DRAM address (our own region ~2GB).
    o.line(b"LOW_80000000", rd(0x8000_0000));
    o.line(b"LOW_80100000", rd(0x8010_0000));

    // V9 S2MPU bypass check for BOTH masters. PROT_EN_PER_VID (0x50) = current per-VID protection bitmap; 0 = bypassed. USB (HSI0) works, UFS (HSI2) doesn't — compare.
    // HSI0 (USB, works):
    o.line(b"USB_PROTEN", rd(0x1107_0000 + 0x50));
    // HSI2 (UFS, fails):
    o.line(b"UFS_PROTEN", rd(0x131F_0000 + 0x50));
    // Re-write the CLR bypass to UFS S2MPU and re-read — does our write stick (0) or is protection held on (nonzero = pKVM trap / locked)?
    unsafe { core::ptr::write_volatile((0x131F_0000 + 0x54) as *mut u32, 0xFF); core::arch::asm!("dsb sy"); }
    o.line(b"UFS_PROTEN2", rd(0x131F_0000 + 0x50));

    // UFS Protector (UFSP, G#132A0000): is UFS DMA marked SECURE? If so, our non-secure descriptors are unreachable by the secure-marked DMA. RSECURITY 0x10 / WSECURITY 0x110 hold NSSMU(bit14)+AXPROT bits; region0 SBEGIN/END/LUN/CTRL at 0x200. AxPROT[1]=1 = non-secure.
    o.s(b"-- UFSP 132A0000 --\n");
    o.line(b"UFSPRCTRL", rd(0x132A_0000 + 0x000));
    o.line(b"UFSPRSECUR", rd(0x132A_0000 + 0x010));
    o.line(b"UFSPWCTRL", rd(0x132A_0000 + 0x100));
    o.line(b"UFSPWSECUR", rd(0x132A_0000 + 0x110));
    // THE candidate fix: write path is SECURE (WSECUR=0), so controller write-backs (OCS/response/data) can't reach our non-secure buffer. Mirror the read path's non-secure config to the write path and check it sticks (vs secure-locked).
    unsafe { core::ptr::write_volatile((0x132A_0000 + 0x110) as *mut u32, 0xFFE2_6492); core::arch::asm!("dsb sy"); }
    o.line(b"UFSPWSECUR_AFTER", rd(0x132A_0000 + 0x110));

    o.n as u64
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
