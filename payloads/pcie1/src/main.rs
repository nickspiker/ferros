//! The WiFi PCIe driver, as a process: brings up PCIe channel 1 to the BCM4398 following WIFI.md, touching hardware only through the hardware grants it was spawned with (ferros_kernel process.rs, syscalls 4/5), and reports every step through its console.
//!
//! Spawn (grant order matters — slots 1..12):
//!   ferros-bridge spawn pcie1.bin <stage> --name pcie1 --hw pcie1-elbi --hw pcie1-soc --hw pcie1-pcs --hw pcie1-udbg --hw pcie1-phy --hw pcie1-dbi --hw pcie1-cfg --hw pcie1-mem --hw gpio-hsi2 --hw gpio-far-alive --hw cmu-hsi2 --hw pmu
//! Stage (input byte, hex): 00 read-only survey (CMU gates, PMU PHY control, GPIO, ELBI/LTSSM, DBI ID) · 01 + PHY bring-up (WIFI.md §5-8) · 02 + link training (§9) · 03 + config space and the chip id (§10-11) · 04 + dump the chip's NCI enumeration ROM (read-only).
//! Each stage only goes further than the last, so a hang (bus stall → watchdog reset) points at the stage that caused it.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use ferros_sys::svc;

/// The heap the link's detail document is built in (ferros_detail): a few hundred bytes a build, rebuilt only when the link changes.
const HEAP_SIZE: usize = 1 << 16;
#[repr(C, align(64))]
struct Heap([u8; HEAP_SIZE]);
static mut HEAP: Heap = Heap([0; HEAP_SIZE]);
#[global_allocator]
static ALLOC: ferros_heap::LockedHeap = ferros_heap::LockedHeap::empty();

const ELBI: u64 = 1;
const SOC: u64 = 2;
const PCS: u64 = 3;
const UDBG: u64 = 4;
const PHY: u64 = 5;
const DBI: u64 = 6;
const CFG: u64 = 7;
const MEM: u64 = 8;
const GPIO: u64 = 9;
const CMU: u64 = 11;
const PMU: u64 = 12;
/// The entropy right (`--entropy`, after the hardware grants).
const ENTROPY: u64 = 13;
/// The DMA region (`--dma pcie1-dma`, after the entropy right).
const DMA: u64 = 14;
/// The vault scope `wifi.` (grant slot after the `wlan` endpoint; init grants it at boot): the firmware (`fw`), the NVRAM text (`nvram`), the CLM blob (`clm`) and the credential (`cred`), for a start with no input files.
const STORE: u64 = 16;
/// The ira-derived key right (`--ira`, after the vault scope; init grants it at boot): the key the per-network MAC is hashed under.
const IRA: u64 = 17;
/// Bluetooth's power switch (`--power bt`, after the ira key; init grants it at boot): the chip's Bluetooth core is turned off before WL_REG_ON is cycled, as Android's REG_ON coordinator resets Bluetooth before a WiFi chip reset (wl_android.c, wl2wbrc_req_bt_reset). Cycling WL_REG_ON under a powered Bluetooth core wedged the chip and the device with it (2026-10-31: two watchdog resets, each within a minute of a WiFi restart). The Bluetooth driver hears its chip go silent and brings it up again once WiFi is up. A start without the grant (the bridge's) skips it.
const BT_POWER: u64 = 18;
/// PCIe channel 1's interrupt (kernel IRQS `pcie1`, SPI 439): the slot after the last, which the kernel adds for a process granted the controller (process.rs `build_from`), so a spawn with 19 or fewer slots has none.
const IRQ: u64 = 19;
/// Wait on the chip's interrupt, never on the clock (owner, 2026-11-10: "No polling! No timers unless PROVEN necessary!"; 2026-11-13, the 8a dark at 782-859 mW with this driver waking ~1,300 times a second). Channel 1's MSIs reach the GIC on SPI 439 thru ELBI PCIE_IRQ2 bit 17 (pcie-exynos-rc.c); the chip raises one for its completions once its mailbox mask is set at the PCIe core's G#C34 (brcmfmac pcie.c, the register set of PCIe core rev 64 and up: the BCM4398's is rev 130, the BCM4383's 77). `FERROS_WIFI_IRQ=0` builds keep the millisecond's sleep; otherwise a chip that raises no MSI is found at serve start (`irq_selftest`) and served with the sleep, said on the console.
const WIFI_IRQ: bool = match option_env!("FERROS_WIFI_IRQ") {
    Some(v) => !(v.len() == 1 && v.as_bytes()[0] == b'0'),
    None => true,
};
/// The interrupt's doorbell: counted up by `irq_thread` at each interrupt it takes. The serving loop's idle wait, and an ioctl's wait for its completion, are on this word: ended by a count that moved, a call (`wait_world_or_call`), or a preference changing (the kernel wakes every word waiter then, process.rs `write_pref_all`).
static IRQ_RUNG: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// 1 while `irq_thread` waits on the line; 0 once it took an interrupt and left the line masked, until the serving loop has cleared the latches and read its rings (`irq_arm`). One interrupt an idle spell, however many completions come while the loop is busy (Linux's NAPI does the same): under load the loop reads the rings each pass anyway.
static IRQ_ARMED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The interrupt proven and in use (`irq_selftest` passed): `Msgbuf::ioctl` waits on it then, else it spins its millisecond as during bring-up.
static IRQ_ON: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// `irq_thread`'s stack: it only waits and counts.
static mut IRQ_STACK: [u8; 1 << 13] = [0; 1 << 13];

/// The interrupt's thread: armed, it waits on the line with no deadline; the line's interrupt counts `IRQ_RUNG` up and wakes the serving loop, and the thread disarms itself, leaving the line masked (the kernel masks it until `irq_ack`). The loop clears the chip's and the controller's latches, acks the line and re-arms it (`irq_arm`) only when it is about to wait, after reading its rings. `cap` is the line's token. A refused wait (no such grant) counts `IRQ_RUNG` to `u64::MAX`: the loop falls back to sleeping.
extern "C" fn irq_thread(cap: u64) -> ! {
    use core::sync::atomic::Ordering::{Acquire, Release};
    loop {
        while IRQ_ARMED.load(Acquire) == 0 {
            ferros_sys::wait_world(&IRQ_ARMED, 0, 0);
        }
        let r = ferros_sys::irq_wait_world(cap, 0);
        IRQ_ARMED.store(0, Release);
        if r == ERR {
            IRQ_RUNG.store(u64::MAX, Release);
        } else {
            IRQ_RUNG.fetch_add(1, Release);
        }
        ferros_sys::wake(&IRQ_RUNG, 1);
    }
}

/// The line re-armed for the next idle spell: the latches cleared (`irq_clear`), the line acked (unmasked), `irq_thread` woken to wait on it. The caller reads its rings again after this and before it waits: a completion before the clear is in them, one after raises a new interrupt.
fn irq_arm(d: &mut Drv) {
    irq_clear(d);
    ferros_sys::irq_ack(d.cap(IRQ));
    IRQ_ARMED.store(1, core::sync::atomic::Ordering::Release);
    ferros_sys::wake(&IRQ_ARMED, 1);
}
const ERR: u64 = u64::MAX;
/// BAR1's offset inside the pcie1-mem window (stage 6 places it above BAR0's 64 KiB), and its size: the 4 MiB slice of the dongle backplane that cfg G#84 selects.
const BAR1: u64 = 0x40_0000;
const BAR1_SIZE: u32 = 0x40_0000;
/// akita's BCM4383 (an AI backplane): its Cortex-R4 core and wrapper, and its RAM base, as its enumeration ROM gave them (docs/BOARDS.md, 2026-11-09; dhd_pcie.c CR4_4383_RAM_BASE G#6E_0000).
const CR4_CORE: u32 = 0x1803_0000;
const CR4_WRAP: u32 = 0x1813_0000;
const RAM_BASE_4383: u32 = 0x6E_0000;

struct Drv {
    grants: u64,
    base: u64,
    freq: u64,
    log: [u8; 3000],
    /// The console call's 16-byte reply.
    reply: [u8; 16],
    n: usize,
    failed: bool,
    /// The backplane window BAR1 shows now (cfg G#84), so a run of accesses in one window costs one config write.
    win: u32,
}

impl Drv {
    fn cap(&self, slot: u64) -> u64 {
        self.grants + 0x204 + 32 * slot
    }
    fn rd(&mut self, g: u64, off: u64) -> u32 {
        let v = svc(4, self.cap(g), off, 0, 0, 0);
        if v == ERR {
            self.failed = true;
            self.put(b"!rd ");
            self.hex(g as u32);
            self.put(b"+");
            self.hex(off as u32);
            self.put(b"\n");
        }
        v as u32
    }
    fn wr(&mut self, g: u64, off: u64, v: u32) {
        if svc(5, self.cap(g), off, v as u64, 0, 0) == ERR {
            self.failed = true;
            self.put(b"!wr ");
            self.hex(g as u32);
            self.put(b"+");
            self.hex(off as u32);
            self.put(b"\n");
        }
    }
    fn set(&mut self, g: u64, off: u64, bits: u32) {
        let v = self.rd(g, off);
        self.wr(g, off, v | bits);
    }
    fn clr(&mut self, g: u64, off: u64, bits: u32) {
        let v = self.rd(g, off);
        self.wr(g, off, v & !bits);
    }
    fn now(&self) -> u64 {
        ferros_sys::now()
    }
    fn us(&self, n: u64) {
        let t0 = self.now();
        while self.now().wrapping_sub(t0) < n * self.freq / 1_000_000 {}
    }
    /// Poll until (reg & mask) == want or `max_us` passes; returns the elapsed microseconds, or None on timeout (logged, not fatal: WIFI.md §13.10).
    fn poll(&mut self, g: u64, off: u64, mask: u32, want: u32, max_us: u64) -> Option<u64> {
        let t0 = self.now();
        loop {
            if self.rd(g, off) & mask == want {
                return Some(self.now().wrapping_sub(t0) * 1_000_000 / self.freq);
            }
            if self.now().wrapping_sub(t0) > max_us * self.freq / 1_000_000 {
                return None;
            }
            self.us(10);
        }
    }
    fn put(&mut self, s: &[u8]) {
        for &c in s {
            if self.n < self.log.len() {
                self.log[self.n] = c;
                self.n += 1;
            }
        }
    }
    fn hex(&mut self, v: u32) {
        let h = b"0123456789ABCDEF";
        self.put(b"G#");
        for i in (0..8).rev() {
            let c = h[((v >> (i * 4)) & 15) as usize];
            self.put(&[c]);
        }
    }
    fn dec(&mut self, mut v: u32) {
        let mut b = [0u8; 10];
        let mut i = b.len();
        loop {
            i -= 1;
            b[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.put(b"A#");
        let digits = b;
        self.put(&digits[i..]);
    }
    fn kv(&mut self, k: &[u8], v: u32) {
        self.put(k);
        self.put(b"=");
        self.hex(v);
        self.put(b"\n");
    }
    fn show(&mut self, k: &[u8], g: u64, off: u64) {
        let v = self.rd(g, off);
        self.kv(k, v);
    }
    /// One word of dongle memory (backplane address) through BAR1, moving the window first (dhdpcie_bus_membytes with bar1 switching).
    /// The backplane power request of a multi-backplane chip (the BCM4383, siutils.c si_srpwr_request over PCIe: the PCIe core's power control at BAR0 + G#2000 + G#1E8, the ARM/backplane domain's request bit 9 and its ready bit 17, up to 15 ms, PMU_MAX_TRANSITION_DLY). dhd holds it around every TCM access; this driver holds it from the halt on. Some(µs to ready) or None.
    fn srpwr(&mut self, on: bool) -> Option<u64> {
        let v = self.rd(MEM, 0x21E8);
        self.wr(MEM, 0x21E8, if on { v | 1 << 9 } else { v & !(1 << 9) });
        if on { self.poll(MEM, 0x21E8, 1 << 17, 1 << 17, 15_000) } else { Some(0) }
    }
    /// The Cortex-R4's reset through its AI wrapper (aiutils.c _ai_core_reset, the CR4 path of dhd_pcie.c __dhdpcie_bus_download_state): `halt` leaves it reset-released with CPUHALT set (ioctrl G#21), else running (ioctrl G#01). The wrapper (the EROM read on akita: master wrapper G#1813_0000, its only one) through BAR0's second window, its registers at +G#1000: ioctrl +G#408, resetstatus +G#804, resetctrl +G#800. False if a reset status never settled.
    fn cr4_reset(&mut self, halt: bool) -> bool {
        const IOCTRL: u64 = 0x1408;
        const RESETCTRL: u64 = 0x1800;
        const RESETSTATUS: u64 = 0x1804;
        let bits = if halt { 0x20 } else { 0 };
        self.wr(CFG, 0x70, CR4_WRAP);
        let mut ok = self.poll(MEM, RESETSTATUS, 0xFFFF_FFFF, 0, 300).is_some();
        self.wr(MEM, RESETCTRL, 1);
        self.us(10);
        ok &= self.poll(MEM, RESETSTATUS, 0xFFFF_FFFF, 0, 300).is_some();
        // Clock on and forced (FGC) with the core's bits, then out of reset, then the force off.
        self.wr(MEM, IOCTRL, bits | 0x3);
        let _ = self.rd(MEM, IOCTRL);
        ok &= self.poll(MEM, RESETSTATUS, 0xFFFF_FFFF, 0, 300).is_some();
        for _ in 0..10 {
            if self.rd(MEM, RESETCTRL) == 0 {
                break;
            }
            self.wr(MEM, RESETCTRL, 0);
            ok &= self.poll(MEM, RESETSTATUS, 0xFFFF_FFFF, 0, 300).is_some();
        }
        self.wr(MEM, IOCTRL, bits | 0x1);
        let _ = self.rd(MEM, IOCTRL);
        self.us(1);
        ok
    }
    /// The CR4's RAM in bytes (siutils_host.c si_tcm_size): corecap at core + 4, its banks [3:0] + [7:4]; each bank's info by writing its index to core + G#40 and reading core + G#44, size (bits 7:0 + 1) × (1 KiB with bit 9, else 8 KiB). The core must be up (halted is fine).
    fn cr4_ramsize(&mut self) -> u32 {
        self.wr(CFG, 0x80, CR4_CORE);
        let cap = self.rd(MEM, 0x4);
        let banks = (cap & 0xF) + ((cap >> 4) & 0xF);
        let mut size = 0u32;
        for i in 0..banks {
            self.wr(MEM, 0x40, i);
            let bi = self.rd(MEM, 0x44);
            size += ((bi & 0xFF) + 1) * if bi & (1 << 9) != 0 { 1024 } else { 8192 };
        }
        self.wr(CFG, 0x80, 0x1800_0000);
        size
    }
    fn window(&mut self, addr: u32) {
        let w = addr & !(BAR1_SIZE - 1);
        if w != self.win {
            self.wr(CFG, 0x84, w);
            self.win = w;
        }
    }
    fn tcm_rd(&mut self, addr: u32) -> u32 {
        self.window(addr);
        self.rd(MEM, BAR1 + (addr & (BAR1_SIZE - 1)) as u64)
    }
    fn tcm_wr(&mut self, addr: u32, v: u32) {
        self.window(addr);
        self.wr(MEM, BAR1 + (addr & (BAR1_SIZE - 1)) as u64, v);
    }
    /// Write bytes to dongle memory as little-endian words, the last one zero-padded; returns how many words read back different.
    fn tcm_write_verify(&mut self, addr: u32, bytes: &[u8]) -> u32 {
        let words = bytes.len().div_ceil(4);
        let word = |i: usize| {
            let mut w = [0u8; 4];
            for (j, b) in w.iter_mut().enumerate() {
                *b = bytes.get(i * 4 + j).copied().unwrap_or(0);
            }
            u32::from_le_bytes(w)
        };
        for i in 0..words {
            self.tcm_wr(addr + 4 * i as u32, word(i));
        }
        let mut bad = 0;
        for i in 0..words {
            if self.tcm_rd(addr + 4 * i as u32) != word(i) {
                bad += 1;
            }
        }
        bad
    }
    /// Printable bytes of a NUL-terminated string in the DMA region (at most `len`, 512).
    fn text(&mut self, a: u32, len: u32) {
        for k in 0..len.min(512) {
            let c = dbr(a + k);
            if c == 0 {
                break;
            }
            if (0x20..0x7F).contains(&c) {
                self.put(&[c]);
            }
        }
        self.put(b"\n");
    }
    fn flush(&mut self) {
        if self.n > 0 {
            // The reply into the driver's own memory: G#A00 was in the grants page, read-only since the sandbox work, and the kernel's write of the reply there faulted under its lock and reset the 8a at every run (2026-11-09).
            let r = core::ptr::addr_of_mut!(self.reply) as u64 - self.base;
            svc(1, self.grants + 0x204, self.log.as_ptr() as u64 - self.base, self.n as u64, r, 16);
            self.n = 0;
        }
    }
    fn pmu_rmw(&mut self, off: u64, mask: u32, val: u32) {
        let v = self.rd(PMU, off);
        self.wr(PMU, off, (v & !mask) | (val & mask));
    }
}

// The entry with ferros_rt's image header (offset 8: "ferrosRT", then .bss + 16, three unused words, a heap of 0), as ahau, the modem driver and hive carry it: the kernel sizes the arc with the zeroed tail, so the vault files' buffers in .bss are in the arc.
core::arch::global_asm!(
    r#"
    .section .text.entry, "ax"
    .globl _entry
    .type _entry, %function
_entry:
    b {main}
    .4byte 0
    .ascii "ferrosRT"
    .quad __ferros_tail
    .quad 0
    .quad 0
    .quad 0
    .quad 0
    .size _entry, . - _entry
"#,
    main = sym pcie1_main
);

extern "C" fn pcie1_main(grants: u64, input: u64, input_len: u64) -> ! {
    // First: the image's own relocations (payload.ld, build.rs: linked -pie).
    unsafe { ferros_sys::relocate() };
    let (base, freq) = (ferros_sys::arc_base(), ferros_sys::freq());
    unsafe { ALLOC.init((&raw mut HEAP) as usize, HEAP_SIZE) };
    // Spawned by init at boot there is no input (a spawn from the vault carries none): with the vault scope granted, the whole bring-up from the vault's files; from the bridge, the stage byte it was given.
    // The grant count at 0x200 of the grants page (the handle table fills the first 512 bytes since 2026-11-03; read at 0 it was 0, and the driver thought it had no vault scope).
    let has_store = unsafe { core::ptr::read_volatile((base + grants + 0x200) as *const u32) } as u64 > STORE;
    let stage = if input_len > 0 { unsafe { core::ptr::read_volatile((base + input) as *const u8) } } else if has_store { 14 } else { 0 };
    let mut d = Drv { grants, base, freq, log: [0; 3000], reply: [0; 16], n: 0, failed: false, win: u32::MAX };
    let rest: &mut [u8] = if input_len > 1 { unsafe { core::slice::from_raw_parts_mut((base + input + 1) as *mut u8, input_len as usize - 1) } } else { &mut [] };
    run(&mut d, stage, rest);
    d.flush();
    ferros_sys::exit(d.failed as u64)
}

/// The WiFi switch turned off (stage G#F0, kernel service.rs `radio_down`, 2026-11-13): the chip powered down, as Linux's exynos_pcie_rc_poweroff and dhd's wifi_platform_set_power off leave it, where a killed driver left it powered and its link up with no host (the 8a dark with the radios switched off drew 968 mW, more than with them on). Bluetooth's core off first (the REG_ON rule: never move WL_REG_ON under a powered Bluetooth core; its driver brings it back by itself, Bluetooth running with WL_REG_ON low), PERST low, the PHY's power-down (stage 1's A3-A8), the PMU's PHY isolation, WL_REG_ON low. Never the controller's ELBI or DBI: a fresh process does not know the link's state, and a PHY already isolated (a cold boot, an `off` before this one) is left as it is. A chip at rest with no link loses nothing by the order: no PME_Turn_Off, as the chip's power goes with WL_REG_ON.
fn off(d: &mut Drv) {
    ferros_sys::power(d.cap(BT_POWER), false);
    d.clr(GPIO, 0x4, 1 << 0);
    let iso = d.rd(PMU, 0x3EC8);
    if iso & 1 == 1 {
        d.clr(PHY, 0x204, 0xC);
        for (off, v) in [(0x1044u64, 0x2A), (0x1048, 0xAA), (0x104C, 0xA8), (0x1050, 0x80), (0x185C, 0x0A)] {
            d.wr(PHY, off, v);
        }
        d.us(1);
        d.wr(PHY, 0x208, 0xFF);
        d.us(1);
        d.wr(PHY, 0x580, 0x0A);
        d.wr(PHY, 0x928, 0xAA);
        d.wr(PHY, 0x00C, 0x0A);
        d.us(50);
        d.clr(UDBG, 0xC700, 1);
        d.us(10);
        d.pmu_rmw(0x3EC8, 1, 0);
        d.put(b"off: PERST low, PHY powered down and isolated\n");
    } else {
        d.kv(b"off: PERST low, PHY already isolated, pmu", iso);
    }
    d.clr(GPIO, 0x4, 1 << 4);
    let g = d.rd(GPIO, 0x4);
    d.kv(b"off: WL_REG_ON low, gph3.dat", g);
}

fn run(d: &mut Drv, stage: u8, files: &mut [u8]) {
    if stage == 0xF0 {
        return off(d);
    }
    // ---- stage 0: survey (WIFI.md §2, §3, §9 status registers) ----
    d.put(b"pcie1 stage ");
    d.hex(stage as u32);
    d.put(b"\n");
    for &(name, off) in &[
        (&b"cmu.phy_refclk"[..], 0x201Cu64),
        (b"cmu.dbi_aclk", 0x2088),
        (b"cmu.mstr_aclk", 0x208C),
        (b"cmu.slv_aclk", 0x2090),
        (b"cmu.subctrl_apb", 0x2094),
        (b"cmu.phy_udbg_apb", 0x2098),
        (b"cmu.uasc_dbi_aclk", 0x20EC),
        (b"cmu.uasc_slv_aclk", 0x20F4),
        (b"cmu.qch_apb", 0x3058),
        (b"cmu.qch_axi", 0x3060),
        (b"cmu.pcie_user_mux", 0x0620),
    ] {
        let v = d.rd(CMU, off);
        d.kv(name, v);
    }
    let pmu = d.rd(PMU, 0x3EC8);
    d.kv(b"pmu.phy_ch1", pmu);
    let gcon = d.rd(GPIO, 0x0);
    let gdat = d.rd(GPIO, 0x4);
    d.kv(b"gph3.con", gcon);
    d.kv(b"gph3.dat", gdat);
    let lt = d.rd(ELBI, 0x2C8);
    d.kv(b"elbi.ltssm", lt);
    let id = d.rd(DBI, 0x0);
    d.kv(b"dbi.id", id);
    d.flush();
    if stage < 1 {
        return;
    }

    // ---- stage 1: PHY into a known state, WL_REG_ON, PHY calibration (WIFI.md §5-8) ----
    for off in [0x10u64, 0x14, 0x18] {
        d.wr(ELBI, off, 0); // A1
    }
    d.pmu_rmw(0x3EC8, 1, 1); // A2: PHY isolation release
    d.clr(PHY, 0x204, 0xC); // A3
    for (off, v) in [(0x1044u64, 0x2A), (0x1048, 0xAA), (0x104C, 0xA8), (0x1050, 0x80), (0x185C, 0x0A)] {
        d.wr(PHY, off, v); // A4
    }
    d.us(1);
    d.wr(PHY, 0x208, 0xFF); // A5
    d.us(1);
    d.wr(PHY, 0x580, 0x0A); // A6
    d.wr(PHY, 0x928, 0xAA);
    d.wr(PHY, 0x00C, 0x0A); // A7
    d.us(50);
    d.clr(UDBG, 0xC700, 1); // A8
    d.us(10);
    // B: pins. PERST (gph3-0) output low, WL_REG_ON (gph3-4) output low then high, CLKREQ (gph3-1) function 2 pull-up, dev_wake (gph3-5) input pull-down.
    let mut con = d.rd(GPIO, 0x0);
    con = (con & !0xF) | 0x1;
    con = (con & !(0xF << 4)) | (0x2 << 4);
    con = (con & !(0xF << 16)) | (0x1 << 16);
    con &= !(0xF << 20);
    ferros_sys::power(d.cap(BT_POWER), false);
    d.clr(GPIO, 0x4, (1 << 0) | (1 << 4)); // B1: both low before switching to outputs
    d.wr(GPIO, 0x0, con);
    let pud = d.rd(GPIO, 0x8);
    d.wr(GPIO, 0x8, (pud & !((0xF << 4) | (0xF << 16) | (0xF << 20))) | (3 << 4) | (1 << 20));
    let drv = d.rd(GPIO, 0xC);
    d.wr(GPIO, 0xC, (drv & !(0xF | (0xF << 4) | (0xF << 16))) | 3 | (3 << 4));
    d.us(20_000); // WL_REG_ON low ≥ 20 ms (§4)
    d.set(GPIO, 0x4, 1 << 4); // B2: WL_REG_ON high
    d.us(200_000); // B3
    d.wr(SOC, 0x4000, 0x15); // B5
    // C: PHY power-down clear.
    d.set(UDBG, 0xC700, 1);
    d.us(100);
    d.wr(PHY, 0x00C, 0);
    d.wr(PHY, 0x928, 0x55);
    d.wr(PHY, 0x580, 0x02);
    d.wr(PHY, 0x208, 0);
    for off in [0x1044u64, 0x1048, 0x104C, 0x1050, 0x185C] {
        d.wr(PHY, off, 0);
    }
    d.us(10);
    // D: zuma PHY configuration, ch1.
    d.wr(PHY, 0x32C, 0x3); // D1
    d.set(UDBG, 0xC700, 1 << 0); // D2
    d.clr(UDBG, 0xC700, 1 << 1); // D3
    match d.poll(UDBG, 0xC734, 1 << 2, 1 << 2, 100_000) {
        Some(t) => d.kv(b"ext_pll_lock_us", t as u32),
        None => d.put(b"ext_pll_lock TIMEOUT\n"),
    }
    d.clr(SOC, 0x4004, 1 << 2); // D5
    d.set(SOC, 0x4004, 0x30); // D6
    d.wr(ELBI, 0x080, 0x4); // D7
    d.wr(ELBI, 0x3A4, 0xF); // D8
    d.wr(ELBI, 0x3A4, 0xD);
    d.us(10);
    d.wr(ELBI, 0x3A4, 0xF); // D9
    d.us(10);
    for v in [1u32, 0] {
        d.wr(ELBI, 0x1404, v); // D10
        d.wr(ELBI, 0x1408, v);
        d.wr(ELBI, 0x1400, v);
    }
    d.wr(ELBI, 0x3D8, 1); // D11
    const PHYCFG: &[(u64, u32)] = &[
        // D12
        (0x0000, 0x88), (0x001C, 0x66), (0x01F4, 0x00), (0x0514, 0x59), (0x051C, 0x11), (0x062C, 0x0E), (0x0644, 0x22), (0x0688, 0x03), (0x06D4, 0x28), (0x0788, 0x64), (0x078C, 0x64), (0x0790, 0x50), (0x0794, 0x50), (0x0944, 0x05), (0x0948, 0x05), (0x094C, 0x05), (0x0950, 0x05),
        // D13-D16
        (0x0590, 0x02), (0x07F8, 0xB0), (0x0730, 0x08), (0x0344, 0xC0), (0x0040, 0x04), (0x0204, 0x03), (0x02D4, 0x1F), (0x0358, 0x10), (0x0018, 0x01),
        // D17-D18
        (0x0514, 0x5B), (0x0608, 0x0C), (0x060C, 0x0F), (0x0610, 0x0F), (0x0614, 0x0F), (0x0618, 0x0F), (0x0510, 0x80), (0x0688, 0x03), (0x0644, 0x23), (0x0624, 0x11), (0x0630, 0x0F), (0x06D0, 0x53),
        // D19 (lane)
        (0x1140, 0x04), (0x1144, 0x04), (0x1148, 0x04), (0x114C, 0x02), (0x1150, 0), (0x1154, 0), (0x1158, 0), (0x115C, 0), (0x12CC, 0x1C), (0x12DC, 0x6C), (0x130C, 0x29), (0x13B4, 0x2F), (0x1A64, 0x05), (0x1A68, 0x05), (0x1A84, 0x05), (0x1A88, 0x05), (0x1A98, 0), (0x1A9C, 0), (0x1AA8, 0x07), (0x1AB8, 0), (0x1ABC, 0), (0x1AF8, 0x90), (0x1B34, 0x03), (0x1BB0, 0x03), (0x1BB4, 0x03), (0x1BC0, 0x06), (0x1BC4, 0x06), (0x1BE8, 0x01), (0x1BF8, 0x04), (0x1C98, 0), (0x1CA4, 0x81),
        // D20 (BCM)
        (0x1B20, 0x02), (0x1340, 0x23), (0x002C, 0x3C),
    ];
    for &(off, v) in PHYCFG {
        d.wr(PHY, off, v);
    }
    d.set(PHY, 0x1350, 1); // D21
    const PCSCFG: &[(u64, u32)] = &[(0x190, 0x100B_0604), (0x154, 0x0007_00D5), (0x100, 0x1640_0000), (0x104, 0x0860_0000), (0x114, 0x1850_0000), (0x124, 0x6070_0000), (0x174, 0x7), (0x178, 0x100), (0x17C, 0x700), (0x110, 0x0160_0202)];
    for &(off, v) in PCSCFG {
        d.wr(PCS, off, v); // D22-D24
    }
    d.wr(ELBI, 0x1404, 1); // D25
    d.us(10);
    d.wr(ELBI, 0x1408, 1); // D26
    d.wr(ELBI, 0x1400, 1);
    for &(name, off, bit) in &[(&b"pll_lock_us"[..], 0xA80u64, 0u32), (b"cdr_lock_us", 0x15C0, 4), (b"ofs_cal_us", 0x140C, 7)] {
        match d.poll(PHY, off, 1 << bit, 1 << bit, 100_000) {
            Some(t) => d.kv(name, t as u32),
            None => {
                d.put(name);
                d.put(b" TIMEOUT\n");
            }
        }
    }
    d.wr(PHY, 0x32C, 0); // D30
    d.wr(ELBI, 0x1078, 1); // D31
    d.flush();
    if stage < 2 {
        return;
    }

    // ---- stage 2: RC setup, PERST, LTSSM (WIFI.md §9), Gen1 forced (no Gen3 insistence) ----
    d.pmu_rmw(0x3EC8, 0x400, 0x400); // E1
    d.wr(UDBG, 0xC800, 0x421); // E2
    d.wr(PHY, 0x32C, 0); // E3
    d.wr(DBI, 0x890, 0x1_2000); // E4
    d.set(GPIO, 0x4, 1 << 0); // E5: PERST# high
    d.us(16_000); // E6
    d.set(ELBI, 0x3BC, 0x11); // E7
    d.wr(ELBI, 0x3A0, 0x2); // E8
    d.wr(ELBI, 0x074, 1); // E9
    d.clr(ELBI, 0x3A8, 0xFFF); // E10
    d.wr(ELBI, 0x474, 1); // E11
    d.wr(ELBI, 0x1078, 1); // E12
    // E13 dw_pcie_setup_rc, with link speed capped at Gen1.
    d.set(DBI, 0x8BC, 1);
    let v = d.rd(DBI, 0xA0);
    d.wr(DBI, 0xA0, (v & !0xF) | 1);
    let v = d.rd(DBI, 0x7C);
    d.wr(DBI, 0x7C, (v & !0xF) | 1);
    let v = d.rd(DBI, 0x710);
    d.wr(DBI, 0x710, (v & !((1 << 7) | (0x3F << 16))) | (1 << 5) | (1 << 16));
    let v = d.rd(DBI, 0x80C);
    d.wr(DBI, 0x80C, (v & !(0x1F << 8)) | (1 << 8));
    d.wr(DBI, 0x10, 4);
    d.wr(DBI, 0x14, 0);
    let v = d.rd(DBI, 0x3C);
    d.wr(DBI, 0x3C, (v & !(0xFF << 8)) | (1 << 8));
    let v = d.rd(DBI, 0x18);
    d.wr(DBI, 0x18, (v & 0xFF00_0000) | 0x00FF_0100);
    let v = d.rd(DBI, 0x4);
    d.wr(DBI, 0x4, (v & !0xFFFF) | 0x107);
    d.wr(DBI, 0x10, 0);
    let v = d.rd(DBI, 0x8);
    d.wr(DBI, 0x8, (v & 0xFFFF) | (0x0604 << 16)); // class code (16-bit at G#0A)
    d.clr(DBI, 0x8BC, 1);
    // E14 exynos_pcie_setup_rc (Gen1 cap kept).
    d.wr(DBI, 0x8BC, 1);
    d.wr(DBI, 0x0, 0xECED_144D);
    let v = d.rd(DBI, 0x7C);
    d.wr(DBI, 0x7C, (v & !((7 << 15) | 0xF)) | 0x3_8000 | 1);
    d.wr(DBI, 0xB40, 0x1A);
    d.wr(DBI, 0xB44, 0xEA);
    d.wr(DBI, 0x44, 0);
    let v = d.rd(DBI, 0x98);
    d.wr(DBI, 0x98, (v & !0xF) | 5);
    // E15 history buffer.
    d.clr(ELBI, 0xC00, 1 << 2);
    d.wr(ELBI, 0xC04, 0xFFFF_FFFF);
    d.wr(ELBI, 0xC08, 0xFFFF_FFFF);
    d.set(ELBI, 0xC00, 3);
    let id = d.rd(DBI, 0x0);
    d.kv(b"dbi.id", id);
    d.wr(ELBI, 0x054, 1); // E17: LTSSM enable
    let t0 = d.now();
    let mut last = 0xFFu32;
    let mut l0 = false;
    for _ in 0..6000 {
        let s = d.rd(ELBI, 0x2C8) & 0x3F;
        if s != last {
            d.put(b"ltssm ");
            d.hex(s);
            d.put(b"\n");
            last = s;
        }
        if s == 0x11 {
            l0 = true;
            break;
        }
        d.us(10);
    }
    let dt = (d.now().wrapping_sub(t0) * 1_000_000 / d.freq) as u32;
    d.kv(b"ltssm_us", dt);
    let ls = d.rd(DBI, 0x80);
    d.kv(b"dbi.lnkctl_sta", ls);
    d.put(if l0 { b"LINK UP (L0)\n" } else { b"no L0\n" });
    d.flush();
    if !l0 || stage < 3 {
        return;
    }

    // ---- stage 3: iATU, config space, BAR0, chip id (WIFI.md §10-11) ----
    let atu = 0x30_0000u64;
    for (off, v) in [(0x08u64, 0x60FF_E000u32), (0x0C, 0), (0x10, 0x60FF_FFFF), (0x14, 0x0100_0000), (0x18, 0), (0x00, 0x4), (0x04, 0x8000_0000)] {
        d.wr(DBI, atu + off, v);
    }
    for (off, v) in [(0x200u64, 0u32), (0x208, 0x6000_0000), (0x20C, 0), (0x210, 0x60FE_FFFF), (0x214, 0x6000_0000), (0x218, 0), (0x204, 0x8000_0000)] {
        d.wr(DBI, atu + off, v);
    }
    d.wr(ELBI, 0x074, 1);
    let vid = d.rd(CFG, 0x0);
    d.wr(ELBI, 0x074, 0);
    d.kv(b"ep.vendor_device", vid);
    if vid & 0xFFFF != 0x14E4 {
        d.put(b"not Broadcom\n");
        return;
    }
    // BAR0 at the bottom of the window (64-bit), the bridge window around it, memory + bus master on.
    d.wr(CFG, 0x10, 0x6000_0000);
    d.wr(CFG, 0x14, 0);
    d.wr(DBI, 0x20, 0x60F0_6000);
    d.set(CFG, 0x04, 0x6);
    d.wr(CFG, 0x80, 0x1800_0000); // PCI_BAR0_WIN = SI_ENUM_BASE: chipcommon
    let chip = d.rd(MEM, 0x0);
    d.kv(b"chipcommon.chipid", chip);
    d.kv(b"chip", chip & 0xFFFF);
    d.kv(b"rev", (chip >> 16) & 0xF);
    d.flush();
    if stage < 4 {
        return;
    }

    // ---- stage 4: dump the NCI enumeration ROM (read-only) — the chip describes its own cores there (nciutils.c nci_init: ChipCommon EromPtrOffset G#FC → EROM2 base; EROM1/OOBR sit below it). The host parses the dump before any core reset is attempted. ----
    let erom2 = d.rd(MEM, 0xFC);
    d.kv(b"cc.eromptr", erom2);
    let ai = chip >> 28 == 1;
    // The backplane by the chip id's top nibble (siutils.c si_doattach): 6 NCI (the BCM4398, its EROM and OOBR below the pointer), 1 AI (the BCM4383 on akita, 2026-11-09). On an AI chip the region the NCI arithmetic derives is not decoded: reading it stalled the bus and reset the 8a. There the ROM is at the pointer itself (aiutils.c ai_scan; in the OOBR, NCI-format, when its low three bits are set, aiutils.c ai_erom_in_oobr), dumped only as stage G#41, its whole G#E00 bytes; stage 4 stops at the pointer.
    if ai {
        d.kv(b"backplane_ai", 1);
        d.kv(b"erom_in_oobr", (erom2 & 7 != 0) as u32);
        d.flush();
        if stage == 0x41 && erom2 & 7 == 0 {
            d.wr(CFG, 0x80, erom2 & !0xFFF);
            let off = (erom2 & 0xFFF) as u64;
            for i in 0..(0xE00u64 / 4) {
                if i % 8 == 0 {
                    d.flush();
                    d.hex((i * 4) as u32);
                    d.put(b":");
                }
                let v = d.rd(MEM, off + i * 4);
                d.put(b" ");
                d.hex(v);
                if i % 8 == 7 {
                    d.put(b"\n");
                }
            }
            d.flush();
            d.wr(CFG, 0x80, 0x1800_0000);
        }
        if stage < 5 || stage == 0x41 {
            return;
        }
    }
    let erom1 = erom2 & !0xFFF;
    let oobr = erom2 & !0x1FFF;
    d.kv(b"erom1_base", erom1);
    d.kv(b"oobr_base", oobr);
    d.flush();
    let nci_roms = [(&b"oobr"[..], oobr), (&b"erom"[..], erom1)];
    for &(name, base) in if ai { &nci_roms[..0] } else { &nci_roms[..] } {
        d.wr(CFG, 0x80, base);
        d.put(b"dump ");
        d.put(name);
        d.put(b" ");
        d.hex(base);
        d.put(b"\n");
        // Measure first: time the first reads (a window the chip doesn't decode costs a PCIe completion timeout per read and starves everything else); dump further only if they come back fast.
        let t0 = d.now();
        let probe = d.rd(MEM, 0);
        let us = (d.now().wrapping_sub(t0) * 1_000_000 / d.freq) as u32;
        d.kv(b"first_read", probe);
        d.kv(b"first_read_us", us);
        d.flush();
        let words = if us > 100 || probe == 0xFFFF_FFFF { 0u64 } else { 512 };
        for i in 0..words {
            if i % 8 == 0 {
                d.flush();
                d.hex((i * 4) as u32);
                d.put(b":");
            }
            let v = d.rd(MEM, i * 4);
            d.put(b" ");
            d.hex(v);
            if i % 8 == 7 {
                d.put(b"\n");
            }
        }
        d.flush();
    }
    d.wr(CFG, 0x80, 0x1800_0000);
    if stage < 5 {
        return;
    }

    // ---- stage 5: measure before the firmware download touches anything (read-only on the chip). BAR sizes; the DAR security status (pcie_core.h: secure boot means a signature file is required); the CA7's NCI state: core info and DMP control in the OOBR (nci_core_cflags: OOBR + core offset + 8, bit 5 CPUHALT), reset control in its AMNI wrapper (nci_iscoreup: wrapper + G#140 bit 0 = in reset), the wrapper reached through BAR0's second 4 KiB window (cfg G#70). ----
    d.clr(CFG, 0x04, 0x2);
    for (name, off, restore) in [(&b"bar0"[..], 0x10u64, 0x6000_0000u32), (b"bar1", 0x18, 0)] {
        d.wr(CFG, off, 0xFFFF_FFFF);
        let raw = d.rd(CFG, off);
        d.wr(CFG, off, restore);
        d.put(name);
        d.kv(b".raw", raw);
        d.put(name);
        d.kv(b".size", (!(raw & !0xF)).wrapping_add(1));
    }
    d.set(CFG, 0x04, 0x2);
    d.show(b"cfg.bar1win", CFG, 0x84);
    d.show(b"cfg.bar0win2", CFG, 0x70);
    d.wr(CFG, 0x80, 0x1800_1000);
    d.show(b"pcie.dar_security_status", MEM, 0xA74);
    if ai {
        // The CR4's state through its wrapper, read before anything changes it (WL_REG_ON has just released the chip): ioctrl (G#21 = halted, clocked), resetctrl (0 = out of reset); then the backplane power request, which the CR4 and the TCM need on a multi-backplane chip, and how long it took to come ready.
        d.wr(CFG, 0x70, CR4_WRAP);
        d.show(b"cr4.ioctrl", MEM, 0x1408);
        d.show(b"cr4.resetctrl", MEM, 0x1800);
        d.show(b"cr4.resetstatus", MEM, 0x1804);
        d.show(b"pcie.pwr_ctl", MEM, 0x21E8);
        d.flush();
        let r = d.srpwr(true);
        d.kv(b"srpwr.ready_us", r.map(|u| u as u32).unwrap_or(u32::MAX));
        d.show(b"pcie.pwr_ctl", MEM, 0x21E8);
        d.flush();
    } else {
    d.wr(CFG, 0x80, oobr);
    d.show(b"ca7.coreinfo", MEM, 0x240);
    d.show(b"ca7.dmpctrl", MEM, 0x248);
    d.show(b"ca7.dmp+c", MEM, 0x24C);
    d.wr(CFG, 0x80, 0x1800_0000);
    d.flush();
    d.wr(CFG, 0x70, 0x1850_4000);
    let t0 = d.now();
    let id = d.rd(MEM, 0x1100);
    d.kv(b"ca7.amni.first_read_us", (d.now().wrapping_sub(t0) * 1_000_000 / d.freq) as u32);
    d.kv(b"ca7.amni.idm_device_id", id);
    d.flush();
    if id != 0xFFFF_FFFF {
        d.show(b"ca7.amni.reset_control", MEM, 0x1140);
        d.show(b"ca7.amni.reset_status", MEM, 0x1144);
    }
    d.flush();
    // The module's CIS tuples in OTP (dhd_custom_cis.c: 4398 range G#1801_115C..G#1801_11B3; tag G#80 start, G#81 vendor → the VID that names the module NVRAM, vid_naming_table_4398). Read-only, first read timed like the ROM dump.
    d.wr(CFG, 0x80, 0x1801_1000);
    let t0 = d.now();
    let first = d.rd(MEM, 0x15C);
    d.kv(b"otp.cis.first_read_us", (d.now().wrapping_sub(t0) * 1_000_000 / d.freq) as u32);
    d.flush();
    if first != 0xFFFF_FFFF {
        d.put(b"otp.cis:");
        for off in (0x15Cu64..0x1B4).step_by(4) {
            let v = d.rd(MEM, off);
            d.put(b" ");
            d.hex(v);
        }
        d.put(b"\n");
    }
    d.wr(CFG, 0x80, 0x1800_0000);
    }
    d.flush();
    if stage < 6 {
        return;
    }

    // ---- stage 6: the dongle's RAM through BAR1, ARM still halted. RAM size the way si_sysmem_size counts it (sysmem rev 14 ≥ 12: coreinfo ROM banks [31:24], SRAM banks [23:16]; per bank, bankidx G#10 ← index, bankinfo G#40 size = 8 KiB × ([6:0] + 1)); the RAM base is the sysmem's second slave port (G#2A_0000, dhd_pcie.c NCI default). BAR1 (4 MiB) sits above BAR0 (64 KiB) at G#6040_0000; cfg G#84 picks which 4 MiB of the backplane it shows (dhdpcie_setbar1win). Then a pattern round trip in both windows, and the shared-area word at the top of RAM zeroed (dhdpcie_init_shared_addr). ----
    // An AI chip (akita's BCM4383): its CR4 halted through the wrapper (dhd resets it into halt whatever its state, the CR4 path of __dhdpcie_bus_download_state), the power request held from stage 5, and the RAM sized from the CR4's banks (si_tcm_size); its RAM based at G#6E_0000.
    let ramsize = if ai {
        let ok = d.cr4_reset(true);
        d.kv(b"cr4.halt_ok", ok as u32);
        d.wr(CFG, 0x70, CR4_WRAP);
        d.show(b"cr4.ioctrl", MEM, 0x1408);
        d.show(b"cr4.resetctrl", MEM, 0x1800);
        d.flush();
        if !ok {
            d.failed = true;
            return;
        }
        let s = d.cr4_ramsize();
        d.wr(CFG, 0x80, CR4_CORE);
        d.show(b"cr4.corecap", MEM, 0x4);
        d.wr(CFG, 0x80, 0x1800_0000);
        s
    } else {
        d.wr(CFG, 0x80, 0x1808_8000);
        let ci = d.rd(MEM, 0x0);
        d.kv(b"sysmem.coreinfo", ci);
        let (nrb, nb) = ((ci >> 24) & 0xFF, (ci >> 16) & 0xFF);
        let mut ramsize = 0u32;
        for i in nrb..nrb + nb {
            d.wr(MEM, 0x10, i);
            let bi = d.rd(MEM, 0x40);
            ramsize += 8192 * ((bi & 0x7F) + 1);
        }
        d.wr(CFG, 0x80, 0x1800_0000);
        ramsize
    };
    d.kv(b"ramsize", ramsize);
    let ram_base: u32 = if ai { RAM_BASE_4383 } else { 0x2A_0000 };
    d.wr(CFG, 0x18, 0x6040_0000);
    d.wr(CFG, 0x1C, 0);
    d.flush();
    let t0 = d.now();
    let w0 = d.tcm_rd(ram_base);
    d.kv(b"bar1.first_read_us", (d.now().wrapping_sub(t0) * 1_000_000 / d.freq) as u32);
    d.kv(b"ram[base]", w0);
    d.flush();
    if w0 == 0xFFFF_FFFF {
        d.put(b"BAR1 does not decode\n");
        return;
    }
    let top = ram_base + ramsize - 4;
    let v = d.tcm_rd(top);
    d.kv(b"ram[top]", v);
    for (addr, pat) in [(ram_base, 0xA5C3_3C5Au32), (top - 4, 0x5A3C_C3A5)] {
        d.tcm_wr(addr, pat);
        let back = d.tcm_rd(addr);
        d.put(if back == pat { b"roundtrip ok " } else { b"roundtrip BAD " });
        d.hex(addr);
        d.put(b" ");
        d.hex(back);
        d.put(b"\n");
    }
    d.tcm_wr(top, 0);
    let v = d.tcm_rd(top);
    d.kv(b"ram[top] zeroed", v);
    d.show(b"cfg.bar1win", CFG, 0x84);
    d.flush();
    if stage < 7 {
        return;
    }

    // ---- stage 7: download and start the firmware (dhd_pcie.c _dhdpcie_download_firmware with the CA7 path of __dhdpcie_bus_download_state). Input after the stage byte: [u32 LE len][firmware .bin][u32 LE len][NVRAM text]. ----
    // The files: the bridge's --input-file inputs, or, with none, the vault scope `wifi.` (init's boot start): the firmware streamed from the vault into the chip's RAM a piece at a time, the small files into scratch at 1 MiB into the DMA region (its rings and buffers end below 576 KiB).
    let mut from_store = false;
    let (fw, nv, clm, cred): (Fw, &mut [u8], &[u8], &[u8]) = match split_files(files) {
        Some((fw, nv, clm, cred)) => (Fw::Mem(fw), nv, clm, cred),
        None => match from_vault(d) {
            Some(v) => {
                from_store = true;
                v
            }
            None => {
                d.put(b"stage 7 wants --input-file firmware --input-file nvram [--input-file clm_blob [--input-file wifi_credential]], or the vault's wifi.fw / wifi.nvram / wifi.clm / wifi.cred\n");
                d.failed = true;
                d.flush();
                return;
            }
        },
    };
    d.kv(b"fw.len", fw.len() as u32);
    d.kv(b"nvram.raw_len", nv.len() as u32);
    d.kv(b"clm.len", clm.len() as u32);
    d.flush();
    if fw.len() < 4 || fw.len() as u32 > ramsize {
        d.put(b"firmware does not fit\n");
        d.failed = true;
        return;
    }
    // MSI before the firmware runs: it signals boot-complete with an MSI, and with none programmed the write goes to address 0 and the firmware traps (stage 08's first console). The RC's DesignWare MSI receiver (DBI port logic: MSI_ADDR_LO/HI G#820/G#824, vector enable G#828, mask G#82C, status G#830) terminates writes to MSI_ADDR inside the controller; the EP's MSI capability points at the same address. MSI_TARGET is in the boot ROM region, never DRAM, so a write that ever escaped interception would land nowhere.
    const MSI_TARGET: u32 = 0xF000;
    d.wr(DBI, 0x820, MSI_TARGET);
    d.wr(DBI, 0x824, 0);
    d.wr(DBI, 0x828, 0xFFFF_FFFF);
    d.wr(DBI, 0x82C, 0);
    d.wr(DBI, 0x830, 0xFFFF_FFFF);
    let msi = find_cap(d, 0x05);
    d.kv(b"ep.msi_cap", msi.unwrap_or(0) as u32);
    if let Some(c) = msi {
        let ctl = d.rd(CFG, c) >> 16;
        let is64 = ctl & 0x80 != 0;
        d.wr(CFG, c + 4, MSI_TARGET);
        if is64 {
            d.wr(CFG, c + 8, 0);
        }
        let data_off = if is64 { c + 0xC } else { c + 8 };
        let dv = d.rd(CFG, data_off & !3);
        d.wr(CFG, data_off & !3, dv & 0xFFFF_0000);
        // Message control: enable, one vector (multiple-message enable [6:4] = 0).
        let w = d.rd(CFG, c);
        d.wr(CFG, c, (w & !(0x71 << 16)) | (1 << 16));
        d.show(b"ep.msi_ctl", CFG, c);
    }
    d.flush();
    // Enter: the CA7 must be out of reset and halted (stage 5 found it so after WL_REG_ON; dhd resets it into that state only when it isn't, which this driver doesn't do yet).
    // An AI chip: its CR4 halted in stage 6, the wrapper reading ioctrl G#21 (halted, clocked) and resetctrl 0.
    let (dmp, in_reset) = if ai {
        d.wr(CFG, 0x70, CR4_WRAP);
        (d.rd(MEM, 0x1408), d.rd(MEM, 0x1800) & 1)
    } else {
        d.wr(CFG, 0x80, oobr);
        let dmp = d.rd(MEM, 0x248);
        d.wr(CFG, 0x80, 0x1800_0000);
        d.wr(CFG, 0x70, 0x1850_4000);
        (dmp, d.rd(MEM, 0x1140) & 1)
    };
    if dmp & 0x20 == 0 || in_reset != 0 {
        d.kv(b"ca7 not halted+up, dmpctrl", dmp);
        d.failed = true;
        return;
    }
    let ramtop = ram_base + ramsize;
    d.tcm_wr(ramtop - 4, 0);
    // The image at the RAM base; its first word is the reset instruction, written to address 0 last.
    let t0 = d.now();
    let (resetinstr, bad) = match fw {
        Fw::Mem(fw) => (u32::from_le_bytes([fw[0], fw[1], fw[2], fw[3]]), d.tcm_write_verify(ram_base, fw)),
        Fw::Vault(len) => {
            let mut chunk = [0u8; 4096];
            let (mut off, mut bad, mut first) = (0usize, 0u32, 0u32);
            while off < len {
                let n = (len - off).min(chunk.len());
                if store_get_at(d, b"fw", off, &mut chunk[..n]).is_none() {
                    bad += 1;
                    break;
                }
                if off == 0 {
                    first = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                }
                bad += d.tcm_write_verify(ram_base + off as u32, &chunk[..n]);
                off += n;
            }
            (first, bad)
        }
    };
    d.kv(b"fw.write_verify_ms", ((d.now().wrapping_sub(t0) * 1000) / d.freq) as u32);
    d.kv(b"fw.bad_words", bad);
    d.flush();
    if bad != 0 {
        d.failed = true;
        return;
    }
    // NVRAM: text → NUL-separated vars (process_nvram_vars), zero-padded to 4 bytes, plus the terminating NUL dhd counts (varsz = len + 1, then ROUNDUP 4) — at the top of RAM below the length token.
    let n = process_nvram_vars(nv);
    let vlen = n.div_ceil(4) * 4;
    let varsize = (vlen + 1).div_ceil(4) * 4;
    let varaddr = ramtop - 4 - varsize as u32;
    let bad = d.tcm_write_verify(varaddr, &nv[..vlen.min(nv.len())]);
    for a in ((varaddr + vlen as u32)..(ramtop - 4)).step_by(4) {
        d.tcm_wr(a, 0);
    }
    let w = (varsize / 4) as u32;
    let token = (!w << 16) | (w & 0xFFFF);
    d.tcm_wr(ramtop - 4, token);
    d.kv(b"nvram.vars_len", n as u32);
    d.kv(b"nvram.addr", varaddr);
    d.kv(b"nvram.bad_words", bad);
    d.kv(b"nvram.token", token);
    // Random seed for the firmware's RNG (dhdpcie_wrt_rnd → a reverse TLV under the vars: 128 bytes, then length (no checksum on this TLV, the firmware crashes on one), then type G#FEEDC0DE). From the session's wairua through the entropy right — never from the host.
    let mut seed = [0u8; 128];
    let off = seed.as_mut_ptr() as u64 - d.base;
    if svc(6, d.cap(ENTROPY), off, 128, 0, 0) != 128 {
        d.put(b"no entropy right or no wairua: refusing to start the firmware without a seed\n");
        d.failed = true;
        return;
    }
    let rnd = varaddr - 8 - 128;
    let bad = d.tcm_write_verify(rnd, &seed);
    d.tcm_wr(rnd + 128, 128);
    d.tcm_wr(rnd + 132, 0xFEED_C0DE);
    d.kv(b"rnd.addr", rnd);
    d.kv(b"rnd.bad_words", bad);
    // Exit: reset instruction at 0, then clear CPUHALT in the CA7's DMP control ("for ARM CA7 it is enough if we clear bit5 in IO DMP ctrl") and wait for it to read back.
    d.tcm_wr(0, resetinstr);
    d.kv(b"resetinstr", resetinstr);
    let v = d.tcm_rd(0);
    d.kv(b"ram[0]", v);
    d.flush();
    if ai {
        // The CR4 (dhd_pcie.c __dhdpcie_bus_download_state, its CR4 branch): the reset instruction must read back from address 0, the DAR power control's bit 9 set (buscorerev 77 ≥ 68: the ARM cold-boot workaround, BAR0 + G#2A0C), then a full wrapper reset with no CPUHALT, not a halt-bit clear.
        if v != resetinstr {
            d.put(b"the reset instruction did not read back from address 0\n");
            d.failed = true;
            d.flush();
            return;
        }
        let p = d.rd(MEM, 0x2A0C);
        d.wr(MEM, 0x2A0C, p | 1 << 9);
        let ok = d.cr4_reset(false);
        d.kv(b"cr4.release_ok", ok as u32);
        d.wr(CFG, 0x70, CR4_WRAP);
        d.show(b"cr4.ioctrl", MEM, 0x1408);
    } else {
        d.wr(CFG, 0x80, oobr);
        let v = d.rd(MEM, 0x248) & !0x20;
        d.wr(MEM, 0x248, v);
        let rel = d.poll(MEM, 0x248, 0xFFFF_FFFF, v, 5000);
        d.kv(b"ca7.released_us", rel.map(|u| u as u32).unwrap_or(u32::MAX));
        d.wr(CFG, 0x80, 0x1800_0000);
    }
    d.flush();
    // The firmware boots and replaces the length token in the last RAM word with the address of its shared area (dhdpcie_readshared). Watch it for up to 4 s.
    let t0 = d.now();
    let mut last = token;
    let shared = loop {
        let v = d.tcm_rd(ramtop - 4);
        if v != last {
            d.kv(b"ramtop changed", v);
            d.kv(b"  at_ms", ((d.now().wrapping_sub(t0) * 1000) / d.freq) as u32);
            last = v;
        }
        if v != token && v != 0 && v >= ram_base && v < ramtop {
            break Some(v);
        }
        if d.now().wrapping_sub(t0) > 4 * d.freq {
            break None;
        }
        d.us(1000);
    };
    d.flush();
    let Some(sh) = shared else {
        d.kv(b"no shared area; ramtop word", last);
        if ai {
            d.wr(CFG, 0x70, CR4_WRAP);
            d.show(b"cr4.ioctrl", MEM, 0x1408);
        } else {
            d.wr(CFG, 0x80, oobr);
            d.show(b"ca7.dmpctrl", MEM, 0x248);
            d.wr(CFG, 0x80, 0x1800_0000);
        }
        d.flush();
        return;
    };
    d.kv(b"FIRMWARE UP, shared", sh);
    d.show(b"rc.msi_status", DBI, 0x830);
    d.put(b"shared:");
    for i in 0..24 {
        let v = d.tcm_rd(sh + 4 * i);
        d.put(b" ");
        d.hex(v);
    }
    d.put(b"\n");
    d.flush();
    if stage < 8 {
        return;
    }

    // ---- stage 8: what the running firmware says about itself. The settled shared area (bcmpcie.h pciedev_shared_t: flags, trap_addr, assert_exp/file, assert_line, console_addr at +G#14, fwid, ..., rings_info_ptr at +G#30) once its pointers are filled in; trap_addr non-zero means the firmware trapped. Then its console: hnd_cons_t at console_addr, log ring at +8 (buf, buf_size, idx) — the firmware's own boot log, printed as text. ----
    // Timeline first (stage 8's first run saw the shared flags and fwid go to zero within 2 s, console never set): every 5 ms for 3 s, log any change in shared flags / console_addr, the ramtop word and the CA7's DMP control. Not backplane G#0: once booted the firmware disables that region, and a host read there through BAR1 is an AXI slave error the firmware traps on (seen: "AXI Slave Err on s_pcie ... addr 0", then a FIQ trap).
    let t0 = d.now();
    let mut prev = [u32::MAX; 4];
    while d.now().wrapping_sub(t0) < 3 * d.freq {
        let mut cur = [d.tcm_rd(sh), d.tcm_rd(sh + 0x14), d.tcm_rd(ramtop - 4), 0];
        if ai {
            d.wr(CFG, 0x70, CR4_WRAP);
            cur[3] = d.rd(MEM, 0x1408);
        } else {
            d.wr(CFG, 0x80, oobr);
            cur[3] = d.rd(MEM, 0x248);
            d.wr(CFG, 0x80, 0x1800_0000);
        }
        if cur != prev {
            d.hex(((d.now().wrapping_sub(t0) * 1_000_000) / d.freq) as u32);
            d.put(b"us flags/console/ramtop/dmp:");
            for v in cur {
                d.put(b" ");
                d.hex(v);
            }
            d.put(b"\n");
            prev = cur;
            if d.n > 2400 {
                d.flush();
            }
        }
        d.us(5000);
    }
    // The first ramtop value is an early, temporary one: around 430 ms in, the firmware publishes the real shared area there (dhd never sees the first — it reads after the firmware-boot interrupt, dhdpcie_wait_readshared_area_addr).
    let sh = d.tcm_rd(ramtop - 4);
    d.kv(b"shared (settled)", sh);
    d.show(b"rc.msi_status", DBI, 0x830);
    if sh >= ram_base && sh < ramtop {
        // bcmpcie.h: PCIE_SHARED_ASSERT G#200, PCIE_SHARED_TRAP G#400.
        let f = d.tcm_rd(sh);
        d.put(if f & 0x600 != 0 { b"FIRMWARE TRAPPED/ASSERTED\n" } else { b"firmware running, no trap\n" });
    }
    if sh < ram_base || sh >= ramtop {
        d.flush();
        return;
    }
    for (k, off) in [(&b"sh.flags"[..], 0u32), (b"sh.trap_addr", 4), (b"sh.assert_line", 0x10), (b"sh.console_addr", 0x14), (b"sh.fwid", 0x1C), (b"sh.h2d_mb", 0x28), (b"sh.d2h_mb", 0x2C), (b"sh.rings_info", 0x30)] {
        let v = d.tcm_rd(sh + off);
        d.kv(k, v);
    }
    d.flush();
    let cons = d.tcm_rd(sh + 0x14);
    if cons < ram_base || cons >= ramtop {
        return;
    }
    let (buf, size, idx) = (d.tcm_rd(cons + 8), d.tcm_rd(cons + 12), d.tcm_rd(cons + 16));
    d.kv(b"cons.buf", buf);
    d.kv(b"cons.size", size);
    d.kv(b"cons.idx", idx);
    d.flush();
    if buf < ram_base || size == 0 || size > 0x1_0000 || buf + size > ramtop || idx > size {
        return;
    }
    // Unwrapped start (idx is where the next byte goes): oldest first when the ring has wrapped, i.e. the byte after idx is non-zero.
    let wrapped = d.tcm_rd(buf + (idx & !3) + 4) != 0 && idx + 8 < size;
    let (start, len) = if wrapped { (idx, size) } else { (0, idx) };
    console_dump(d, buf, size, start, len);
    if stage < 9 {
        return;
    }

    // ---- stage 9: the msgbuf rings and the first ioctl (dhd_msgbuf.c dhd_prot_init order; WIFI.md §14). Read/write indices stay in TCM (no DMA'd index arrays), so every completion item is checked by epoch and XOR (D2H_SYNC_XORCSUM). Everything the chip may DMA lives in the pcie1-dma region, inside the S2MPU fence. ----
    let dma = svc(7, d.cap(DMA), 0, 0, 0, 0);
    if dma == ERR {
        d.put(b"no DMA region (--dma pcie1-dma)\n");
        d.failed = true;
        return;
    }
    let dma = dma as u32;
    d.kv(b"dma.base", dma);
    // DMA region layout: the five common rings, the 8-byte scratch, the ioctl request buffer, 8 ioctl response buffers (8 KiB), 8 event buffers (2 KiB).
    const RINGS: [(u32, u16, u16); 5] = [(0x0_0000, 64, 40), (0x0_1000, 512, 32), (0x0_5000, 64, 24), (0x0_6000, 1024, 24), (0x0_C000, 512, 40)];
    const SCRATCH: u32 = 0x1_1000;
    const IOCTL_REQ: u32 = 0x1_3000;
    const IOCTL_RESP: u32 = 0x2_0000;
    const EVENT_BUF: u32 = 0x3_0000;
    for off in (0..0x4_0000u32).step_by(4) {
        dw(dma + off, 0);
    }
    let ri = d.tcm_rd(sh + 48);
    let ringmem = d.tcm_rd(ri);
    let (h2d_w, h2d_r, d2h_w, d2h_r) = (d.tcm_rd(ri + 4), d.tcm_rd(ri + 8), d.tcm_rd(ri + 12), d.tcm_rd(ri + 16));
    d.kv(b"ri.ringmem", ringmem);
    d.kv(b"ri.h2d_w", h2d_w);
    d.kv(b"ri.d2h_w", d2h_w);
    let v = d.tcm_rd(ri + 52);
    d.kv(b"ri.max_flowrings|max_subq", v);
    let v = d.tcm_rd(ri + 56);
    d.kv(b"ri.max_cplrings|max_vdevs", v);
    // host_cap: the IPC revision both speak (the lower of the host's 9 and the firmware's, flags[7:0]: the BCM4398 reports 9, akita's BCM4383 7; dhd_msgbuf.c dhd_set_host_cap), HOSTRDY only if the firmware supports it (flags bit 28, PCIE_SHARED_HOSTRDY_SUPPORT), deep sleep off (NO_OOB_DW without INBAND_DW), no trap on a PCIe Unsupported Request.
    let fl = d.tcm_rd(sh);
    let ipc = (fl & 0xFF).min(9);
    let hostrdy = if fl & (1 << 28) != 0 { 0x400 } else { 0 };
    d.kv(b"host_cap.ipc", ipc);
    d.tcm_wr(sh + 84, 0x0080_1000 | hostrdy | ipc);
    d.tcm_wr(sh + 112, 0);
    for (i, &(off, items, len)) in RINGS.iter().enumerate() {
        let m = ringmem + 16 * i as u32;
        let id = d.tcm_rd(m);
        d.put(b"ring ");
        d.hex(i as u32);
        d.kv(b" idx|type", id);
        d.tcm_wr(m + 4, items as u32 | (len as u32) << 16);
        d.tcm_wr(m + 8, dma + off);
        d.tcm_wr(m + 12, 0);
    }
    for i in 0..2u32 {
        d.tcm_wr(h2d_w + 4 * i, 0);
        d.tcm_wr(h2d_r + 4 * i, 0);
    }
    for i in 0..3u32 {
        d.tcm_wr(d2h_w + 4 * i, 0);
        d.tcm_wr(d2h_r + 4 * i, 0);
    }
    d.tcm_wr(sh + 56, dma + SCRATCH);
    d.tcm_wr(sh + 60, 0);
    d.tcm_wr(sh + 52, 8);
    d.flush();
    // Hostready: H2D doorbell 1 (PCIe core registers at BAR0 + G#2000; DB0 G#140, DB1 G#144).
    ferros_sys::dma_barrier();
    d.wr(MEM, 0x2144, 0x1234_5678);
    let cons_before = d.tcm_rd(cons + 16);
    let t0 = d.now();
    let rx_off = d.tcm_rd(sh + 36);
    let mut m = Msgbuf { dma, h2d_w, d2h_r, w: 0, seq: 254, rd: 0, dseq: 254, trans: 0, silent: 0, verbose: stage < 11, t0, rx_off, scan_done: false, seen: [[0; 6]; 96], nseen: 0, want: [0; 32], want_len: 0, target: None, band: 0, linked: false, keyed: false, join_failed: false, rxp_w: 0, rxp_seq: 254, rx_rd: 0, rx_dseq: 254, rx_frames: 0, flow_status: None, tx_w: 0, tx_rd: 0, tx_dseq: 254, tx_done: 0, tx_status: 0, mac: [0; 6], xid: 0, offer: None, acked: false, nak: false, lease: [[0; 4]; 3], lease_s: 0, serving: false, rxq: [(0, 0, 0); RX_BUFS as usize], rxq_t: [0; RX_BUFS as usize], rxq_head: 0, rxq_len: 0, nets: [Net { name: [0; 32], len: 0, rssi: 0, flags: 0, seen_in: 0 }; 32], nnets: 0, scan_gen: 0, scan_ver: 1, quiet: false, outcome: 0, rx_up: false, personal: true, retry_s: 1 << 6 };
    // Control submit ring: 8 ioctl response buffers and 8 event buffers first (the firmware DMAs ioctl results into a posted response buffer, named by its request id in the completion).
    for b in 0..8u32 {
        m.post(d, 0x0B, 1 + b, [8192, 0, dma + IOCTL_RESP + 0x2000 * b, 0, 0, 0, 0, 0]);
    }
    for b in 0..8u32 {
        m.post(d, 0x0D, 0x101 + b, [2048, 0, dma + EVENT_BUF + 0x800 * b, 0, 0, 0, 0, 0]);
    }
    m.ring(d);
    match m.get_var(d, b"ver", dma + IOCTL_REQ) {
        Some((0, b, len)) => {
            d.put(b"ver: ");
            d.text(dma + IOCTL_RESP + 0x2000 * b, len);
        }
        r => d.kv(b"ver failed, status", r.map(|r| r.0).unwrap_or(u32::MAX)),
    }
    d.flush();
    if stage >= 10 {
        // ---- stage 10: a second query the normal way, then the WiFi chip's S2MPU VID, measured: an ioctl whose request buffer sits outside the fence (G#8008_0000: the kernel image, inside the window the first fence allowed). The firmware's DMA read of it is denied (DECERR) and the S2MPU latches the fault with the VID, which `bridge diag` reports (S2MPU_FAULT_INFO bits 26:24). A denied read can't touch memory; the firmware may trap. ----
        match m.get_var(d, b"cur_etheraddr", dma + IOCTL_REQ) {
            Some((0, b, _)) => {
                // The address the chip reports before the join sets its own (its OTP's, else the calibration file's `macaddr`): shown so it can be found in the access point's lists.
                let h = b"0123456789abcdef";
                d.put(b"MAC ");
                for k in 0..6u32 {
                    let v = dbr(m.resp(b) + k);
                    d.put(&[h[(v >> 4) as usize], h[(v & 15) as usize]]);
                    if k < 5 {
                        d.put(b":");
                    }
                }
                d.put(b"\n");
            }
            r => d.kv(b"cur_etheraddr failed, status", r.map(|r| r.0).unwrap_or(u32::MAX)),
        }
    }
    if stage == 10 {
        d.put(b"probe: ioctl request buffer at G#80080000 (the kernel image, outside the fence)\n");
        d.flush();
        let r = m.get_var(d, b"ver", 0x8008_0000);
        d.kv(b"probe result status", r.map(|r| r.0).unwrap_or(u32::MAX));
        let f = d.tcm_rd(sh);
        d.kv(b"sh.flags (G#400 = trapped)", f);
    }
    if stage >= 11 {
        if from_store && stage >= 12 {
            // The boot path: the vault's credential, or one the network page hands in, joined (`connect`); the page served meanwhile.
            connect(d, &mut m, clm);
        } else {
            // A staged run: the credential document given as an input file (`ferros-bridge wifi-cred`), if any.
            let c = credential(cred);
            if let Some((ssid, _)) = &c {
                m.set_want(ssid);
            }
            // The band knob as it stands now: the join takes it (docs/RADIO-CONTROLS.md).
            m.band = ferros_sys::controls(ferros_sys::control::WIFI)[ferros_sys::control::WIFI_BAND as usize];
            scan(d, &mut m, clm);
            if stage >= 12 {
                match &c {
                    Some((ssid, pass)) => join(d, &mut m, ssid, pass),
                    None => {
                        d.put(b"stage 12 wants a credential document as its fourth input file (ferros-bridge wifi-cred)\n");
                        d.flush();
                    }
                }
            }
        }
        if stage == 13 && m.keyed {
            dhcp(d, &mut m);
        }
        if stage >= 14 && m.keyed {
            serve(d, &mut m, sh);
        }
    }
    let f = d.tcm_rd(sh);
    d.kv(b"sh.flags", f);
    let idx = d.tcm_rd(cons + 16);
    console_dump(d, buf, size, cons_before, (idx + size - cons_before) % size);
}

/// The control path of msgbuf (dhd_msgbuf.c): H2D control submit (ring 0, 64 × 40 B) and D2H control complete (ring 2, 64 × 24 B) in the DMA region, indices in TCM: this side's (H2D write, D2H read) written there, the chip's D2H write indices never read, each completion known by its own epoch (`item_new`).
struct Msgbuf {
    dma: u32,
    h2d_w: u32,
    d2h_r: u32,
    /// H2D control submit write index and sequence (epoch = seq % 253, seq from 254; flags = phase G#80 on the first lap).
    w: u32,
    seq: u32,
    /// D2H control complete read index and expected sequence.
    rd: u32,
    dseq: u32,
    trans: u32,
    /// Ioctls in a row the firmware never completed (`ioctl`): `SILENT_MAX` of them while serving ends the driver (`serve`).
    silent: u32,
    /// Print every completion item (bring-up stages) or only what matters (the scan).
    verbose: bool,
    t0: u64,
    /// pcie_shared dma_rxoffset: where a packet starts inside a posted buffer.
    rx_off: u32,
    scan_done: bool,
    seen: [[u8; 6]; 96],
    nseen: usize,
    /// The network to join (stage 12): its SSID from the credential file, and the best matching BSS the scan found (WPA2-PSK, not 6 GHz).
    want: [u8; 32],
    want_len: usize,
    target: Option<Target>,
    /// The band knob as it stood before the scan (ferros_sys::control::WIFI_BAND: 0 or unset any, 1 5 GHz only, 2 2.4 GHz only), for the join's choice.
    band: u64,
    /// Join progress from events: link up (16), keyed (46 with status 6 or 10, reason 0), or a failure.
    linked: bool,
    keyed: bool,
    join_failed: bool,
    /// H2D rxpost (ring 1, 512 × 32 B at +G#1000, index slot 1) write index and sequence; D2H rx complete (ring 4, 512 × 40 B at +G#C000, index slot 2) read index and sequence; frames seen.
    rxp_w: u32,
    rxp_seq: u32,
    rx_rd: u32,
    rx_dseq: u32,
    rx_frames: u32,
    /// Transmit (stage 13): flowring create status (MSG_TYPE_FLOW_RING_CREATE_CMPLT G#04), flowring write index and packets, D2H tx complete (ring 3, 1024 × 24 B at +G#6000, index slot 1) read index, sequence, count and last status.
    flow_status: Option<u32>,
    tx_w: u32,
    tx_rd: u32,
    tx_dseq: u32,
    tx_done: u32,
    tx_status: u32,
    mac: [u8; 6],
    /// DHCP: transaction id, the offer (address, server), the ack and what it carried (subnet, router, DNS, lease seconds).
    xid: u32,
    offer: Option<([u8; 4], [u8; 4])>,
    acked: bool,
    nak: bool,
    lease: [[u8; 4]; 3],
    lease_s: u32,
    /// Server mode (stage 14): received frames wait in their rx buffers until a client takes one (buffer index, frame offset, length); the buffer is re-posted only then — backpressure without a copy.
    serving: bool,
    rxq: [(u8, u16, u16); RX_BUFS as usize],
    /// When each queued frame's completion was taken (the counter), for the measure of how long frames wait for the IP stack (`Path`).
    rxq_t: [u64; RX_BUFS as usize],
    rxq_head: usize,
    rxq_len: usize,
    /// The networks the last scan found, one entry per name (its strongest BSS), for the network page (SCAN, `serve_unjoined`).
    nets: [Net; 32],
    nnets: usize,
    /// Counts scans: an entry not seen in the latest is dropped when it ends (`end_scan`), so the page keeps the last list while a new scan runs.
    scan_gen: u8,
    /// The escan request layout (`scan_ver`), read once the radio is up.
    scan_ver: u32,
    /// After the first scan, BSSes are not printed again.
    quiet: bool,
    /// The last join's outcome for STATUS: 0 none yet, 1 no WPA2-PSK BSS of that name in range, 2 refused (a wrong passphrase among the reasons), 3 timed out, 4 joined.
    outcome: u8,
    /// The 64 receive buffers posted (the first join).
    rx_up: bool,
    /// The credential is the person's (`wifi.cred`; a staged run's input file counts as one): the MAC is this network's, from the ira. A device's (`wifi.net`, joined before anyone attested) joins under a random one each time: no stable identifier on the air while locked (AUTH.md, rule 2).
    personal: bool,
    /// Seconds before a kept credential that did not join is tried again (`serve_unjoined`): 64, doubling to 1024, back to 64 on a join.
    retry_s: u64,
}

/// A network as the page shows it: its name, the strongest signal seen (dBm), and flags: bit 0 joinable here (WPA2-PSK outside 6 GHz), bit 1 open, bit 2 other security (WEP, WPA, SAE only, enterprise), bits 3, 5 and 6 seen on 5, 2.4 and 6 GHz.
#[derive(Clone, Copy)]
struct Net {
    name: [u8; 32],
    len: u8,
    rssi: i16,
    flags: u8,
    /// The scan it was last seen in (`scan_gen`).
    seen_in: u8,
}

#[derive(Clone, Copy)]
struct Target {
    bssid: [u8; 6],
    chanspec: u32,
    rssi: i16,
    /// The AP's RSN group cipher suite (OUI + type) and whether it can do management frame protection (RSN capabilities bit 7).
    group: [u8; 4],
    mfpc: bool,
    /// Control channel (bss_info +88) and whether it is 5 GHz.
    ctl: u8,
    five: bool,
}

impl Msgbuf {
    /// Write one control submit item (cmn_msg_hdr + 32 bytes of body); `ring` publishes.
    fn post(&mut self, _d: &mut Drv, ty: u32, req: u32, body: [u32; 8]) {
        let phase = if (self.w / 64) % 2 == 0 { 0x80 } else { 0 };
        let a = self.dma + 40 * (self.w % 64);
        dw(a, ty | phase << 16 | (self.seq % 253) << 24);
        dw(a + 4, req);
        for (k, v) in body.iter().enumerate() {
            dw(a + 8 + 4 * k as u32, *v);
        }
        self.seq += 1;
        self.w += 1;
    }
    /// Publish posted items: the TCM write index, then doorbell 0 (BAR0 + G#2140).
    fn ring(&mut self, d: &mut Drv) {
        ferros_sys::dma_barrier();
        d.tcm_wr(self.h2d_w, self.w % 64);
        d.wr(MEM, 0x2140, 0xFF00_0000 | (self.w % 64));
    }
    /// Post receive buffer `i` (2 KiB at RX_BASE + i × G#800) on the rxpost ring: host_rxbuf_post_t — header (type G#11, request id = G#1000 + i), u16 metadata length 0, u16 data length, rsvd, metadata address 0, data address.
    fn post_rx(&mut self, i: u32) {
        let phase = if (self.rxp_w / 512) % 2 == 0 { 0x80 } else { 0 };
        let a = self.dma + 0x1000 + 32 * (self.rxp_w % 512);
        let buf = self.dma + RX_BASE + 0x800 * i;
        let words = [0x11 | phase << 16 | (self.rxp_seq % 253) << 24, 0x1000 + i, 2048 << 16, 0, 0, 0, buf, 0];
        for (k, v) in words.iter().enumerate() {
            dw(a + 4 * k as u32, *v);
        }
        self.rxp_seq += 1;
        self.rxp_w += 1;
    }
    /// Publish posted rx buffers: rxpost write index (H2D slot 1), doorbell 0 for ring 1.
    fn ring_rx(&mut self, d: &mut Drv) {
        ferros_sys::dma_barrier();
        d.tcm_wr(self.h2d_w + 4, self.rxp_w % 512);
        d.wr(MEM, 0x2140, 0xFF00_0000 | 1 << 16 | (self.rxp_w % 512));
    }
    /// Consume rx completions (host_rxbuf_cmpl_t, type G#12: request id = buffer, data_len +14, data_offset +16): log length and EtherType, re-post the buffer. The read index goes back to the chip once for the whole run of completions, not once each (each is a register write thru a syscall).
    fn pump_rx(&mut self, d: &mut Drv) {
        let rd0 = self.rx_rd;
        loop {
            let a = self.dma + 0xC000 + 40 * self.rx_rd;
            let words: [u32; 10] = core::array::from_fn(|k| dr(a + 4 * k as u32));
            if !item_new(&words, self.rx_dseq) {
                break;
            }
            self.rx_dseq += 1;
            self.rx_rd = (self.rx_rd + 1) % 512;
            self.rx_one(d, words);
        }
        if self.rx_rd != rd0 {
            d.tcm_wr(self.d2h_r + 8, self.rx_rd);
        }
    }
    /// One rx completion (its ten words), taken.
    fn rx_one(&mut self, d: &mut Drv, words: [u32; 10]) {
        let i = words[1].wrapping_sub(0x1000);
        let len = words[3] >> 16;
        // dhd: the frame starts at data_offset if non-zero, otherwise at pcie_shared dma_rxoffset (8 here), measured from the posted buffer; it is a plain Ethernet frame.
        let off = if words[4] & 0xFFFF != 0 { words[4] & 0xFFFF } else { self.rx_off };
        self.rx_frames += 1;
        if i < RX_BUFS {
            let p = self.dma + RX_BASE + 0x800 * i + off;
            if self.serving {
                // Hold the buffer for a client (re-posted when taken). The completion flags don't separate Ethernet from 802.11 frames here (an EAPOL frame came flagged G#02 too), so everything goes to the client, which ignores what it can't parse. data_len counts the frame from the offset on: subtracting the offset clipped 8 bytes off every frame (DHCP options and router advertisement prefixes lost).
                if self.rxq_len < self.rxq.len() && len > 0 {
                    let at = (self.rxq_head + self.rxq_len) % self.rxq.len();
                    self.rxq[at] = (i as u8, off as u16, len.min(0x800 - off) as u16);
                    self.rxq_t[at] = ferros_sys::now();
                    self.rxq_len += 1;
                } else {
                    self.post_rx(i);
                    self.ring_rx(d);
                }
                return;
            }
            if be16(p + 12) == 0x0800 {
                self.ipv4(d, p + 14, len.saturating_sub(off).saturating_sub(14));
            }
            d.put(b"rx frame len ");
            d.dec(len);
            d.put(b" ethertype ");
            d.hex(be16(p + 12));
            d.put(b"\n");
            if false {
                d.put(b"  cmpl:");
                for v in words {
                    d.put(b" ");
                    d.hex(v);
                }
                d.put(b"\n  buf:");
                let b0 = self.dma + RX_BASE + 0x800 * i;
                for k in 0..8 {
                    d.put(b" ");
                    d.hex(dr(b0 + 4 * k));
                }
                d.put(b"\n");
            }
            self.post_rx(i);
            self.ring_rx(d);
        }
    }
    /// An IPv4 packet: if it is UDP to port 68 carrying a BOOTP reply with our transaction id, take the DHCP message (options after the 240-byte BOOTP header + magic cookie): 53 type (2 OFFER, 5 ACK, 6 NAK), 54 server, 1 subnet, 3 router, 6 DNS, 51 lease.
    fn ipv4(&mut self, d: &mut Drv, ip: u32, len: u32) {
        let ihl = (dbr(ip) & 0xF) as u32 * 4;
        if dbr(ip) >> 4 != 4 || dbr(ip + 9) != 17 || len < ihl + 8 + 240 {
            return;
        }
        let udp = ip + ihl;
        if be16(udp + 2) != 68 {
            return;
        }
        let b = udp + 8;
        if dbr(b) != 2 || be32(b + 4) != self.xid {
            return;
        }
        let yiaddr: [u8; 4] = core::array::from_fn(|k| dbr(b + 16 + k as u32));
        let (mut ty, mut server) = (0u8, [0u8; 4]);
        let mut o = b + 240;
        // Bounded by the IP header's total length (the rx completion's length doesn't cover the whole packet the way it was read here: the options walk stopped early).
        let end = ip + (be16(ip + 2)).min(len.max(be16(ip + 2)));
        while o < end {
            let code = dbr(o);
            if code == 255 {
                break;
            }
            if code == 0 {
                o += 1;
                continue;
            }
            let l = dbr(o + 1) as u32;
            let v4: [u8; 4] = core::array::from_fn(|k| dbr(o + 2 + k as u32));
            match code {
                53 => ty = dbr(o + 2),
                54 => server = v4,
                1 => self.lease[0] = v4,
                3 => self.lease[1] = v4,
                6 => self.lease[2] = v4,
                51 => self.lease_s = u32::from_be_bytes(v4),
                _ => {}
            }
            o += 2 + l;
        }
        // One put per arm: a `match` yielding string slices compiles to a table of absolute pointers in .rodata, which a payload loaded at its arc base (no relocation) can't follow (seen: data abort in pump_rx, FAR G#…9927).
        d.put(b"dhcp ");
        if ty == 2 {
            d.put(b"OFFER");
        } else if ty == 5 {
            d.put(b"ACK");
        } else if ty == 6 {
            d.put(b"NAK");
        } else {
            d.put(b"other");
        }
        d.put(b" ");
        ip_put(d, yiaddr);
        d.put(b" from ");
        ip_put(d, server);
        d.put(b"\n");
        match ty {
            2 if self.offer.is_none() => self.offer = Some((yiaddr, server)),
            5 => self.acked = true,
            6 => self.nak = true,
            _ => {}
        }
    }
    /// Consume tx completions (host_txbuf_cmpl_t, type G#10, 24 B: request id = packet, status & G#F 0 = sent).
    fn pump_tx(&mut self, d: &mut Drv) {
        let rd0 = self.tx_rd;
        loop {
            let a = self.dma + 0x6000 + 24 * self.tx_rd;
            let words: [u32; 6] = core::array::from_fn(|k| dr(a + 4 * k as u32));
            if !item_new(&words, self.tx_dseq) {
                break;
            }
            self.tx_dseq += 1;
            self.tx_rd = (self.tx_rd + 1) % 1024;
            self.tx_done += 1;
            self.tx_status = words[2] & 0xF;
        }
        // The read index back once for the run, as `pump_rx` does.
        if self.tx_rd != rd0 {
            d.tcm_wr(self.d2h_r + 4, self.tx_rd);
        }
    }
    /// Send one Ethernet frame now: `tx_post`, then `tx_ring`. False: refused (`tx_post`).
    fn send(&mut self, d: &mut Drv, frame: &[u8]) -> bool {
        let ok = self.tx_post(d, frame);
        if ok {
            self.tx_ring(d);
        }
        ok
    }
    /// The oldest queued frame into `out` (its length returned), its buffer posted to the chip again, unpublished until `ring_rx` (once for a batch). The caller has checked a frame is queued and that `out` holds it.
    fn rxq_take(&mut self, out: &mut [u8], path: &mut Path) -> usize {
        let t0 = ferros_sys::now();
        let (i, off, l) = self.rxq[self.rxq_head];
        let waited = t0.wrapping_sub(self.rxq_t[self.rxq_head]);
        self.rxq_head = (self.rxq_head + 1) % self.rxq.len();
        self.rxq_len -= 1;
        let l = l as usize;
        dma_get(self.dma + RX_BASE + 0x800 * i as u32 + off as u32, &mut out[..l]);
        self.post_rx(i as u32);
        path.to_ip += 1;
        path.wait_t += waited;
        // WHY the max: the high water, for the measure.
        path.wait_most = path.wait_most.max(waited);
        path.copy_t += ferros_sys::now().wrapping_sub(t0);
        l
    }
    /// Publish the frames `tx_post` wrote: the flowring's TCM write index, then doorbell 0 for ring 5. Once for a batch: each is a register write thru a syscall.
    fn tx_ring(&mut self, d: &mut Drv) {
        ferros_sys::dma_barrier();
        d.tcm_wr(self.h2d_w + 8, self.tx_w % 512);
        d.wr(MEM, 0x2140, 0xFF00_0000 | 5 << 16 | (self.tx_w % 512));
    }
    /// One Ethernet frame onto flowring 2 (msgbuf ring 5, H2D index slot 2), unpublished until `tx_ring`: copied a word at a time into a free tx buffer (`TX_BUFS` of 2 KiB from `TX_BASE`), then host_txbuf_post_t v1 (48 B): header (type G#0F, request id), the 14-byte Ethernet header, flags G#01 (802.3), seg_cnt 1, data address = the payload after the header, data_len = length − 14.
    /// A buffer is free once the chip has completed the frame it held (`tx_done`, from the tx completions): with every buffer still in flight the completions are read once more, and if none came the frame is refused (false), as a full transmit ring drops (TCP sends again). Until 2026-11-11 one frame went per request and eight buffers were reused in turn unchecked; a batch from the IP stack (`serve` op 8) would overwrite buffers the chip had not fetched yet.
    /// WHY the length test: the frame comes from another process; its Ethernet header must be whole and it must fit a buffer.
    fn tx_post(&mut self, d: &mut Drv, frame: &[u8]) -> bool {
        if frame.len() <= 14 || frame.len() > 0x800 {
            return false;
        }
        if self.tx_w.wrapping_sub(self.tx_done) >= TX_BUFS {
            self.pump_tx(d);
            if self.tx_w.wrapping_sub(self.tx_done) >= TX_BUFS {
                return false;
            }
        }
        let k = self.tx_w % TX_BUFS;
        let buf = self.dma + TX_BASE + 0x800 * k;
        dma_put(buf, frame);
        let phase = if (self.tx_w / 512) % 2 == 0 { 0x80 } else { 0 };
        let a = self.dma + 0x6_0000 + 48 * (self.tx_w % 512);
        let mut item = [0u8; 48];
        item[0] = 0x0F;
        item[2] = phase as u8;
        item[4..8].copy_from_slice(&(0x2000 + self.tx_w).to_le_bytes());
        item[8..22].copy_from_slice(&frame[..14]);
        item[22] = 0x01;
        item[23] = 1;
        item[32..36].copy_from_slice(&(buf + 14).to_le_bytes());
        item[42..44].copy_from_slice(&((frame.len() - 14) as u16).to_le_bytes());
        for j in 0..12 {
            dw(a + 4 * j, u32::from_le_bytes(item[4 * j as usize..4 * j as usize + 4].try_into().unwrap()));
        }
        self.tx_w += 1;
        true
    }
    /// Consume every valid D2H control completion: events are handled (and their buffers re-posted) here; an ioctl completion is returned as (status, response buffer index, length), its buffer re-posted.
    fn pump(&mut self, d: &mut Drv) -> Option<(u32, u32, u32)> {
        loop {
            let a = self.dma + 0x5000 + 24 * self.rd;
            let words: [u32; 6] = core::array::from_fn(|k| dr(a + 4 * k as u32));
            if !item_new(&words, self.dseq) {
                break;
            }
            self.dseq += 1;
            self.rd = (self.rd + 1) % 64;
            d.tcm_wr(self.d2h_r, self.rd);
            if self.verbose {
                d.hex(((d.now().wrapping_sub(self.t0) * 1_000_000) / d.freq) as u32);
                d.put(b"us d2h ctrl:");
                for v in words {
                    d.put(b" ");
                    d.hex(v);
                }
                d.put(b"\n");
            }
            match words[0] & 0xFF {
                0x0C => {
                    let b = words[1].wrapping_sub(1);
                    if b < 8 {
                        self.post(d, 0x0B, 1 + b, [8192, 0, self.dma + 0x2_0000 + 0x2000 * b, 0, 0, 0, 0, 0]);
                        self.ring(d);
                    }
                    return Some((words[2] & 0xFFFF, b, words[3] & 0xFFFF));
                }
                // MSG_TYPE_FLOW_RING_CREATE_CMPLT: status +8, flow ring id +10.
                0x04 => self.flow_status = Some(words[2] & 0xFFFF),
                // MSG_TYPE_WL_EVENT: request id = the event buffer, event_data_len at +12; the packet starts rx_dataoffset into the buffer.
                0x0E => {
                    let b = words[1].wrapping_sub(0x101);
                    if b < 8 {
                        let at = self.dma + 0x3_0000 + 0x800 * b;
                        self.event(d, at + self.rx_off, words[3] & 0xFFFF);
                        self.post(d, 0x0D, 0x101 + b, [2048, 0, at, 0, 0, 0, 0, 0]);
                        self.ring(d);
                    }
                }
                _ => {}
            }
        }
        None
    }
    /// A firmware event (bcm_event_t: Ethernet header, ether type G#886C, BRCM header, then wl_event_msg_t with big-endian fields: type +28, status +32, datalen +44, data at +72).
    fn event(&mut self, d: &mut Drv, p: u32, len: u32) {
        if len < 72 || be16(p + 12) != 0x886C {
            return;
        }
        let (ty, status, dlen) = (be32(p + 28), be32(p + 32), be32(p + 44));
        if ty != 69 {
            // Join progress (stage 12): SET_SSID 0, AUTH 3, DEAUTH 5/6, ASSOC 7, DISASSOC 11/12, LINK 16 (flags bit 0 = up), PSK_SUP 46 (status 6 KEYED / 10 WAIT_G1 with reason 0 = authorized), AUTHORIZED 136.
            let (reason, flags) = (be32(p + 36), be16(p + 26));
            d.put(b"event ");
            d.dec(ty);
            d.put(b" status ");
            d.dec(status);
            d.put(b" reason ");
            d.dec(reason);
            d.put(b" flags ");
            d.hex(flags);
            d.put(b"\n");
            match ty {
                16 => self.linked = flags & 1 != 0,
                46 if (status == 6 || status == 10) && reason == 0 => self.keyed = true,
                // WLC_E_PSK_SUP (46): a failed handshake unless the reason is 0, 21 (PTK update) or 23 (GTK update: the BCM4383's driver ignores it too, wl_cfg80211.c).
                46 if status < 10 && reason != 0 && reason != 21 && reason != 23 => self.join_failed = true,
                0 | 3 | 7 if status != 0 => self.join_failed = true,
                5 | 6 | 11 | 12 => self.join_failed = true,
                _ => {}
            }
            return;
        }
        // WLC_E_ESCAN_RESULT: status 8 = a partial result carrying one BSS, 0 = the scan finished, anything else = it ended otherwise.
        if status != 8 {
            self.scan_done = true;
            // Networks the scan just ended did not see are gone.
            let g = self.scan_gen;
            let mut k = 0;
            for j in 0..self.nnets {
                if self.nets[j].seen_in == g {
                    self.nets[k] = self.nets[j];
                    k += 1;
                }
            }
            self.nnets = k;
            d.kv(b"escan finished, status", status);
            return;
        }
        let r = p + 72;
        if dlen < 12 || dr32u(r + 10) & 0xFFFF == 0 {
            return;
        }
        self.bss(d, r + 12);
    }
    /// One wl_bss_info (the version-independent prefix): BSSID +8, capability +16, SSID +18/+19, chanspec +72, RSSI +78, ctl_ch +88, IEs at ie_offset +116 / ie_length +120. Printed once per BSSID.
    fn bss(&mut self, d: &mut Drv, bi: u32) {
        let bssid: [u8; 6] = core::array::from_fn(|k| dbr(bi + 8 + k as u32));
        if self.seen[..self.nseen].contains(&bssid) {
            return;
        }
        if self.nseen < self.seen.len() {
            self.seen[self.nseen] = bssid;
            self.nseen += 1;
        }
        let cs = dr32u(bi + 72) & 0xFFFF;
        let band: &[u8] = if cs & 0xC000 == 0 {
            b"2G"
        } else if cs & 0xC000 == 0x4000 {
            b"6G"
        } else {
            b"5G"
        };
        let ch = if cs & 0x3800 == 0x1000 { cs & 0xFF } else { dbr(bi + 88) as u32 };
        let rssi = (dr32u(bi + 78 - 2) >> 16) as u16 as i16;
        let (ie_off, ie_len) = (dr32u(bi + 116) & 0xFFFF, dr32u(bi + 120));
        let mut sec: &[u8] = if dr32u(bi + 16) & 0x10 != 0 { b"WEP" } else { b"open" };
        let mut psk = false;
        let mut q = bi + ie_off;
        let end = q + ie_len.min(2048);
        while q + 2 <= end {
            let (id, l) = (dbr(q), dbr(q + 1) as u32);
            if id == 48 {
                sec = b"RSN";
                psk = rsn_psk(q + 2, l).is_some();
                self.consider(bi, q + 2, l, cs);
            } else if id == 221 && l >= 4 && sec != b"RSN" && dbr(q + 2) == 0x00 && dbr(q + 3) == 0x50 && dbr(q + 4) == 0xF2 && dbr(q + 5) == 1 {
                sec = b"WPA";
            }
            q += 2 + l;
        }
        self.note_net(bi, cs, rssi, if psk && band != b"6G" { 1 } else if sec == b"open" { 2 } else { 4 });
        if self.quiet {
            return;
        }
        d.put(band);
        d.put(b" ch ");
        d.dec(ch);
        d.put(b" rssi -");
        d.dec((-(rssi as i32)).max(0) as u32);
        d.put(b" ");
        d.put(sec);
        d.put(b" \"");
        let n = (dbr(bi + 18) as u32).min(32);
        for k in 0..n {
            let c = dbr(bi + 19 + k);
            let c = if (0x20..0x7F).contains(&c) { c } else { b'?' };
            d.put(&[c]);
        }
        d.put(b"\"\n");
        if d.n > 2400 {
            d.flush();
        }
    }
    /// A BSS whose SSID is the one to join and whose RSN IE (version 2, group 4, pairwise count + list, AKM count + list, capabilities) offers PSK (00-0F-AC:2), outside 6 GHz (which needs WPA3-SAE): keep the strongest.
    fn consider(&mut self, bi: u32, rsn: u32, l: u32, cs: u32) {
        let n = dbr(bi + 18) as usize;
        if self.want_len == 0 || n != self.want_len || cs & 0xC000 == 0x4000 || l < 8 {
            return;
        }
        if (0..n).any(|k| dbr(bi + 19 + k as u32) != self.want[k]) {
            return;
        }
        let group: [u8; 4] = core::array::from_fn(|k| dbr(rsn + 2 + k as u32));
        let Some(mfpc) = rsn_psk(rsn, l) else { return };
        let rssi = (dr32u(bi + 76) >> 16) as u16 as i16;
        let five = cs & 0xC000 == 0xC000;
        // The band knob (ferros_sys::control::WIFI_BAND): 1 keeps only 5 GHz, 2 only 2.4 GHz; anything else (0, unset, a value this driver does not know) is any.
        if (self.band == 1 && !five) || (self.band == 2 && five) {
            return;
        }
        // 5 GHz whenever the network has it, then signal; 2.4 GHz only where there is no 5 GHz. No 2.4 GHz join has held under ferros: each associated, then lost the link waiting for the first beacon (WLC_E_LINK reason 8, NO_FIRST_BCN_RX) — the first on 2026-10-15, three more on 2026-11-04 at −36 dBm, chosen over 5 GHz at −55 by the 15 dB preference this replaces. 2.4 GHz shares the antenna with Bluetooth, which nothing here coordinates with WiFi yet, and which was advertising the pairing beacon each time.
        if self.target.map(|t| (t.five, t.rssi) >= (five, rssi)).unwrap_or(false) {
            return;
        }
        let bssid: [u8; 6] = core::array::from_fn(|k| dbr(bi + 8 + k as u32));
        self.target = Some(Target { bssid, chanspec: cs, rssi, group, mfpc, ctl: dbr(bi + 88), five });
    }
    /// The network to join: its name, which the scan matches (`consider`) and the MAC is keyed on (`set_mac`).
    fn set_want(&mut self, name: &[u8]) {
        self.want = [0; 32];
        self.want[..name.len()].copy_from_slice(name);
        self.want_len = name.len();
    }
    /// SCAN's reply: [scanning 0/1, count, then per network strongest first: name length, name, signal (dBm, signed byte), flags (`Net`, bit 4: the network being joined or kept)].
    fn scan_reply(&self, scanning: bool, out: &mut [u8]) -> usize {
        let mut order = [0u8; 32];
        for (k, o) in order.iter_mut().enumerate() {
            *o = k as u8;
        }
        let order = &mut order[..self.nnets];
        order.sort_unstable_by(|a, b| self.nets[*b as usize].rssi.cmp(&self.nets[*a as usize].rssi));
        out[0] = scanning as u8;
        out[1] = self.nnets as u8;
        let mut at = 2;
        for &k in order.iter() {
            let e = &self.nets[k as usize];
            let name = &e.name[..e.len as usize];
            let known = if self.want_len > 0 && name == &self.want[..self.want_len] { 1 << 4 } else { 0 };
            out[at] = e.len;
            out[at + 1..at + 1 + name.len()].copy_from_slice(name);
            at += 1 + name.len();
            out[at] = e.rssi as i8 as u8;
            out[at + 1] = e.flags | known;
            at += 2;
        }
        at
    }
    /// STATUS's reply: [state: 0 waiting for a network, 1 scanning, 2 joined; the last join's outcome (`outcome`); the name length; the name being joined or kept].
    fn status_reply(&self, state: u8, out: &mut [u8]) -> usize {
        out[0] = state;
        out[1] = self.outcome;
        out[2] = self.want_len as u8;
        out[3..3 + self.want_len].copy_from_slice(&self.want[..self.want_len]);
        3 + self.want_len
    }
    /// One BSS into the network list: a name seen before keeps its strongest signal and gathers the flags; a hidden network (no name) is not listed.
    fn note_net(&mut self, bi: u32, cs: u32, rssi: i16, flags: u8) {
        // The SSID length is the firmware's byte from the air (0..=255 as a field); the name buffer is 32 bytes (802.11's limit), so a longer one would index past it: bounded, as `bss` bounds its print.
        let n = (dbr(bi + 18) as usize).min(32);
        if n == 0 {
            return;
        }
        let name: [u8; 32] = core::array::from_fn(|k| if k < n { dbr(bi + 19 + k as u32) } else { 0 });
        // The band it was seen on (chanspec bits 14-15: 0 2.4 GHz, 1 6 GHz, 3 5 GHz): bit 5, bit 6 and bit 3 of the flags.
        let five = match cs & 0xC000 {
            0 => 1 << 5,
            0x4000 => 1 << 6,
            _ => 1 << 3,
        };
        let g = self.scan_gen;
        if let Some(e) = self.nets[..self.nnets].iter_mut().find(|e| e.len as usize == n && e.name == name) {
            // First sight in this scan: the entry starts over; then the strongest BSS of the name.
            if e.seen_in != g {
                (e.rssi, e.flags, e.seen_in) = (rssi, 0, g);
            }
            e.rssi = e.rssi.max(rssi);
            e.flags |= flags | five;
        } else if self.nnets < self.nets.len() {
            self.nets[self.nnets] = Net { name, len: n as u8, rssi, flags: flags | five, seen_in: g };
            self.nnets += 1;
        }
    }
    /// One ioctl: `in_len` bytes at the request buffer go to the firmware, up to `out_len` come back in a posted response buffer. Waits for the completion (3 s), handling events meanwhile.
    fn ioctl(&mut self, d: &mut Drv, cmd: u32, req_buf: u32, in_len: u32, out_len: u32) -> Option<(u32, u32, u32)> {
        self.trans += 1;
        self.post(d, 0x09, 0xFFFE, [cmd, self.trans | in_len << 16, out_len, 0, req_buf, 0, 0, 0]);
        self.ring(d);
        let ts = d.now();
        while d.now().wrapping_sub(ts) < 3 * d.freq {
            let seen = IRQ_RUNG.load(core::sync::atomic::Ordering::Acquire);
            if let Some(r) = self.pump(d) {
                self.silent = 0;
                return Some(r);
            }
            if IRQ_ON.load(core::sync::atomic::Ordering::Relaxed) {
                // With the interrupt (serving): the completion's MSI ends the wait, the ioctl's 3 s the deadline (the firmware's own answer time, dhd's IOCTL_RESP_TIMEOUT order). The line re-armed first if it is not, and the ring read again before any wait.
                if IRQ_ARMED.load(core::sync::atomic::Ordering::Acquire) == 0 {
                    irq_arm(d);
                    continue;
                }
                ferros_sys::wait(&IRQ_RUNG, seen, ts + 3 * d.freq);
            } else {
                d.us(1000);
            }
        }
        d.put(b"no ioctl completion\n");
        self.silent += 1;
        None
    }
    /// WLC_GET_VAR (262): the iovar name and its NUL (then `arg`) at the request buffer; dhd passes the whole buffer length both ways. The value comes back at offset 0 of the response buffer.
    fn get_var(&mut self, d: &mut Drv, name: &[u8], req_buf: u32) -> Option<(u32, u32, u32)> {
        self.get_var_arg(d, name, &[], req_buf)
    }
    fn get_var_arg(&mut self, d: &mut Drv, name: &[u8], arg: &[u8], req_buf: u32) -> Option<(u32, u32, u32)> {
        if req_buf >= self.dma && req_buf < self.dma + 0x20_0000 {
            for (k, c) in name.iter().chain(&[0u8]).chain(arg).enumerate() {
                db(req_buf + k as u32, *c);
            }
        }
        self.ioctl(d, 262, req_buf, 256, 256)
    }
    /// WLC_SET_VAR (263): name, NUL, value, no padding (bcm_mkiovar); the length is exactly that.
    fn set_var(&mut self, d: &mut Drv, name: &[u8], value: &[u8]) -> u32 {
        let req = self.dma + 0x1_3000;
        let mut k = 0;
        for c in name.iter().chain(&[0u8]).chain(value) {
            db(req + k, *c);
            k += 1;
        }
        self.ioctl(d, 263, req, k, k).map(|r| r.0).unwrap_or(u32::MAX)
    }
    fn set_int(&mut self, d: &mut Drv, name: &[u8], v: u32) -> u32 {
        self.set_var(d, name, &v.to_le_bytes())
    }
    /// A plain ioctl carrying one int (WLC_SET_INFRA, WLC_SET_PM, ...).
    fn ioctl_int(&mut self, d: &mut Drv, cmd: u32, v: u32) -> u32 {
        let req = self.dma + 0x1_3000;
        for (k, b) in v.to_le_bytes().iter().enumerate() {
            db(req + k as u32, *b);
        }
        self.ioctl(d, cmd, req, 4, 4).map(|r| r.0).unwrap_or(u32::MAX)
    }
    /// The response buffer holding an ioctl's answer.
    fn resp(&self, b: u32) -> u32 {
        self.dma + 0x2_0000 + 0x2000 * b
    }
}

/// The link's measures for the corners' WiFi circle (ferros_sys::measure), from the firmware's own answers (bcm4398 wlioctl_defs.h, bcmwifi_channels.h): RSSI (WLC_GET_RSSI 127, dBm) and the noise floor (WLC_GET_PHY_NOISE 135, dBm) and their difference; the chanspec iovar's centre channel, band and bandwidth as a frequency; the rate (WLC_GET_RATE 12, in 500 kbit/s); loss as the transmit failures over the last two seconds' attempts (WLC_GET_PKTCNTS 137, get_pktcnt_t: rx good, rx bad, tx good, tx bad, cumulative). A slot the firmware did not answer is emptied, never filled with a guess.
fn publish_measures(m: &mut Msgbuf, d: &mut Drv, pkts: &mut [u32; 4]) {
    use ferros_sys::{measure as k, measure_word as w, status, MEASURE_NONE};
    let req = m.dma + 0x1_3000;
    let ask = |m: &mut Msgbuf, d: &mut Drv, cmd: u32, len: u32| -> Option<u32> {
        for i in 0..len {
            db(req + i, 0);
        }
        match m.ioctl(d, cmd, req, len, len) {
            Some((0, b, _)) => Some(m.resp(b)),
            _ => None,
        }
    };
    // Connected only while the join holds: `keyed` is set once by the handshake and cleared only by the next join, so on its own it said connected after the access point let the device go, and the firmware answered the measures with its last values (the 8a, 2026-11-10: "why did it say the wifi was connected").
    let up = m.keyed && m.linked;
    status(0, w(k::STATE, if up { 2 } else { 1 }));
    if !up {
        for s in 1..8 {
            status(s, MEASURE_NONE);
        }
        for t in 0..WIFI_TRACKS {
            ferros_sys::status_track(t, MEASURE_NONE);
        }
        return;
    }
    let rssi = ask(m, d, 127, 12).map(|a| dr32u(a) as i32);
    let noise = ask(m, d, 135, 4).map(|a| dr32u(a) as i32);
    status(1, rssi.map_or(MEASURE_NONE, |v| w(k::RSSI, v as i64 * 1000)));
    status(2, noise.map_or(MEASURE_NONE, |v| w(k::NOISE, v as i64 * 1000)));
    status(3, match (rssi, noise) {
        (Some(r), Some(n)) => w(k::SNR, (r - n) as i64 * 1000),
        _ => MEASURE_NONE,
    });
    // The chanspec: the channel (the centre one for a wide channel) in bits 0-7, the bandwidth in 11-13, the band in 14-15; the centre frequency is the band's base plus 5 MHz a channel (2.4 GHz: 2407, channel 14 at 2484; 5 GHz: 5000; 6 GHz: 5950).
    let cs = match m.get_var(d, b"chanspec", req) {
        Some((0, b, _)) => Some(dr32u(m.resp(b))),
        _ => None,
    };
    let (freq, bw) = match cs {
        Some(cs) => {
            let ch = (cs & 0xFF) as i64;
            let mhz = match cs & 0xC000 {
                0x0000 if ch == 14 => Some(2484),
                0x0000 => Some(2407 + 5 * ch),
                0xC000 => Some(5000 + 5 * ch),
                0x4000 => Some(5950 + 5 * ch),
                _ => None,
            };
            let bw = match cs & 0x3800 {
                0x1000 => Some(20),
                0x1800 => Some(40),
                0x2000 => Some(80),
                0x2800 => Some(160),
                0x0000 => Some(320),
                _ => None,
            };
            (mhz, bw)
        }
        None => (None, None),
    };
    status(4, freq.map_or(MEASURE_NONE, |v| w(k::FREQ, v * 1000)));
    status(5, bw.map_or(MEASURE_NONE, |v: i64| w(k::BANDWIDTH, v * 1000)));
    status(6, ask(m, d, 12, 4).map_or(MEASURE_NONE, |a| w(k::RATE_DOWN, dr32u(a) as i64 * 500_000)));
    let now = ask(m, d, 137, 20).map(|a| [dr32u(a), dr32u(a + 4), dr32u(a + 8), dr32u(a + 12)]);
    status(7, match now {
        Some(c) => {
            // Counters wrap at 2^32: the difference wraps with them.
            let (good, bad) = (c[2].wrapping_sub(pkts[2]) as i64, c[3].wrapping_sub(pkts[3]) as i64);
            *pkts = c;
            // No packets sent in the window: loss is undefined there, so no measure (and no divide by zero).
            if good + bad == 0 { MEASURE_NONE } else { w(k::LOSS, bad * 1_000_000 / (good + bad)) }
        }
        None => MEASURE_NONE,
    });
    // The tracks (docs/RADIO-CONTROLS.md), what the eight slots have no room for: the channel, the transmit power (`qtxpower`: quarter dBm, bit 31 the override flag; brcmfmac's reading), the power-save mode the firmware holds (WLC_GET_PM 85), and the packet counters (WLC_GET_PKTCNTS, cumulative since the chip came up). A track the firmware did not answer is emptied.
    let track = |t: usize, v: Option<u64>| {
        ferros_sys::status_track(t, v.unwrap_or(MEASURE_NONE));
    };
    track(0, cs.map(|cs| w(k::CHANNEL, (cs & 0xFF) as i64)));
    let q = match m.get_var(d, b"qtxpower", req) {
        Some((0, b, _)) => Some((dr32u(m.resp(b)) & 0x7FFF_FFFF) as i64),
        _ => None,
    };
    track(1, q.map(|q| w(k::TX_POWER, q * 250)));
    track(2, ask(m, d, 85, 4).map(|a| w(k::POWER_SAVE, dr32u(a) as i64)));
    track(3, now.map(|c| w(k::PACKETS_DOWN, c[0] as i64)));
    track(4, now.map(|c| w(k::ERRORS_DOWN, c[1] as i64)));
    track(5, now.map(|c| w(k::PACKETS_UP, c[2] as i64)));
    track(6, now.map(|c| w(k::ERRORS_UP, c[3] as i64)));
}

/// The WiFi link's tracks this driver fills (`publish_measures`), emptied while there is no link.
const WIFI_TRACKS: usize = 7;

/// The link's detail for its settings page (ferros_detail::WIFI): the network's name from the credential (its SSID octets as `nw`, any language), the access point joined, this device's address on it, and what DHCP gave when this driver ran it (in service the `ip` process runs DHCP, and its lease is not here).
fn publish_detail(m: &Msgbuf) {
    use ferros_detail::publish;
    use ferros_detail::vsf::VsfType;
    let mut f = alloc::vec::Vec::new();
    let ssid = &m.want[..m.want_len];
    if let Some(s) = ferros_detail::vsf::Ssid::new(ssid).filter(|s| !s.is_empty()) {
        f.push(("ssid", VsfType::nw(s)));
    }
    if let Some(t) = m.target {
        f.push(("bssid", VsfType::nm(t.bssid)));
    }
    if m.mac != [0; 6] {
        f.push(("mac", VsfType::nm(m.mac)));
    }
    if let (true, Some((ip, _))) = (m.acked, m.offer) {
        f.push(("ipv4", VsfType::ni(ip)));
        f.push(("router", VsfType::ni(m.lease[1])));
        f.push(("dns", VsfType::ni(m.lease[2])));
        f.push(("lease", VsfType::u(m.lease_s as usize, false)));
    }
    publish(ferros_sys::control::WIFI, f);
}

/// The knobs as the control block holds them (ferros_sys::control): power save applied at once (WLC_SET_PM 86: 0 off, 1 PM1, 2 PM2); 3 or unset follows the screen, PM2 while it is off (the radio sleeps after a quiet spell, as Android runs it) and off while it is on, set at the join and at every screen change (2026-11-10: the screen-off draw had grown to 1.07 W from 0.58, the WiFi link the largest part left); a value past 3 is not this driver's and is left alone. The band is the join's (`consider`), read before the scan.
fn apply_controls(m: &mut Msgbuf, d: &mut Drv, c: &[u64; ferros_sys::control::WORDS], dark: bool) {
    let pm = match c[ferros_sys::control::WIFI_POWER_SAVE as usize] {
        ferros_sys::control::UNSET | 3 => if dark { 2 } else { 0 },
        v => v,
    };
    if pm <= 2 {
        let r = m.ioctl_int(d, 86, pm as u32);
        d.kv(b"power save set", pm as u32);
        d.kv(b"WLC_SET_PM", r);
        d.flush();
    }
}

fn be16(a: u32) -> u32 {
    (dbr(a) as u32) << 8 | dbr(a + 1) as u32
}
fn be32(a: u32) -> u32 {
    be16(a) << 16 | be16(a + 2)
}
/// A little-endian u32 at any byte address.
fn dr32u(a: u32) -> u32 {
    u32::from_le_bytes([dbr(a), dbr(a + 1), dbr(a + 2), dbr(a + 3)])
}

/// The DMA region through its handle (SANDBOX.md, "Handles"): every access `base + (offset & mask)`, the same instructions native and masked. The driver keeps the region's physical addresses (the chip is told them); the handle takes the offset from the region's base.
type DmaH = ferros_sys::Handle<DMA>;
fn doff(a: u32) -> u64 {
    a as u64 - DmaH::pair().0
}
fn dw(a: u32, v: u32) {
    DmaH::write32(doff(a), v)
}
fn dr(a: u32) -> u32 {
    DmaH::read32(doff(a))
}
fn db(a: u32, v: u8) {
    DmaH::write8(doff(a), v)
}
fn dbr(a: u32) -> u8 {
    DmaH::read8(doff(a))
}
/// Whether a D2H completion item (its words, read from the ring in the DMA region) is the next one, `dseq` the sequence expected there: a message type (zeroed ring memory has none), the epoch dhd's sync mode writes (seq % 253, the firmware's D2H_SYNC_SEQNUM), and a zero XOR over the item (its XORCSUM: none of the item is half written).
/// The rings are read this way alone, never thru the chip's write index (2026-11-11). That index lives in the chip's memory behind PCIe: each read was a syscall whose register access held the kernel's one lock while the link woke from L1.2, and every pass read three (control, rx and tx completions): `diag sched` at idle, a pass took A#460 to A#550 µs; `flame` at idle (2 s), the driver's register reads were A#550 ms of it, the lock held all the while, and spinning on the lock was A#17 % of all five cores' time. The item says for itself whether it is new: one left from the ring's last lap carries an epoch 6 (rx, 512 items), 12 (tx, 1024) or 64 (control, 64) away from the expected one, never equal; a zeroed one has no type.
fn item_new(words: &[u32], dseq: u32) -> bool {
    words[0] & 0xFF != 0 && words[0] >> 24 == dseq % 253 && words.iter().fold(0, |x, v| x ^ v) == 0
}
/// `out.len()` bytes of the DMA region from `a`, read a 64-bit word at a time. The region is Normal non-cacheable: every load is its own trip to DRAM, and a frame read a byte at a time cost about half a microsecond a byte (the scheduler trace, 2026-11-11: the driver held a request A#808 µs for a full frame, A#200 µs for a short one). Eight bytes a load is an eighth of the trips.
fn dma_get(a: u32, out: &mut [u8]) {
    let mut w = a & !7;
    let mut skip = (a - w) as usize;
    let mut o = 0;
    while o < out.len() {
        let b = DmaH::read64(doff(w)).to_le_bytes();
        // WHY the min: the first word may start before `a` and the last may run past the end; these are the bytes of this word that are the frame's.
        let k = (8 - skip).min(out.len() - o);
        out[o..o + k].copy_from_slice(&b[skip..skip + k]);
        o += k;
        skip = 0;
        w += 8;
    }
}
/// `data` into the DMA region at `a` (8-byte aligned), a 64-bit word at a time, the last word padded with zeros (a tx buffer is 2 KiB, the frame at most that).
fn dma_put(a: u32, data: &[u8]) {
    for (k, c) in data.chunks(8).enumerate() {
        let mut b = [0u8; 8];
        b[..c.len()].copy_from_slice(c);
        DmaH::write64(doff(a + 8 * k as u32), u64::from_le_bytes(b));
    }
}

/// The firmware's console ring (hnd_cons log) from `start`, `len` bytes, printable text only.
fn console_dump(d: &mut Drv, buf: u32, size: u32, start: u32, len: u32) {
    d.put(b"---- firmware console\n");
    let mut i = 0;
    while i < len {
        let pos = (start + i) % size;
        let w = d.tcm_rd(buf + (pos & !3));
        let c = (w >> (8 * (pos & 3))) as u8;
        if c == b'\n' || (0x20..0x7F).contains(&c) {
            d.put(&[c]);
        }
        if d.n > 2400 && c == b'\n' {
            d.flush();
        }
        i += 1;
    }
    d.put(b"\n---- end\n");
    d.flush();
}

/// Walk the endpoint's capability list (cfg G#34) for `id`; returns the capability's config offset.
fn find_cap(d: &mut Drv, id: u32) -> Option<u64> {
    find_cap_in(d, CFG, id)
}

/// The same walk in `space`: CFG the endpoint's configuration, DBI the root complex's own.
fn find_cap_in(d: &mut Drv, space: u64, id: u32) -> Option<u64> {
    let mut p = (d.rd(space, 0x34) & 0xFC) as u64;
    for _ in 0..48 {
        if p == 0 {
            return None;
        }
        let h = d.rd(space, p);
        if h & 0xFF == id {
            return Some(p);
        }
        p = ((h >> 8) & 0xFC) as u64;
    }
    None
}

/// The extended capability `id` in `space` (from G#100: id in bits 15:0, the next one's offset in bits 31:20), or None.
fn find_ext_cap(d: &mut Drv, space: u64, id: u32) -> Option<u64> {
    let mut p = 0x100u64;
    for _ in 0..48 {
        let h = d.rd(space, p);
        if h & 0xFFFF == id {
            return Some(p);
        }
        p = (h >> 20) as u64 & 0xFFC;
        if p < 0x100 {
            return None;
        }
    }
    None
}

/// ASPM L1 with its substates on the WiFi link, as Linux's exynos_pcie_rc_set_l1ss does for EP_BCM_WIFI (pcie-exynos-rc.c 4423-4503; dhd enables it once in dhd_bus_start, after the firmware is up and the first ioctl answered: earlier gave link-down): PCI-PM on the root complex then the endpoint (T_POWER_ON 200 us, LTR on, the LTR L1.2 threshold 160 x 1024 ns, common-mode restore, the four enables), the endpoint's LTR maxima (about 3 ms, snoop and no-snoop), then ASPM L1 with common clock on the root complex and the endpoint (CLKREQ enabled on the endpoint only). The link's idle state while the screen is off was the largest SoC-side item left (2026-11-10: L24S_HSI and L3S_PCIE1 ~75 mW over the 2026-10-25 figure). Returns false when a capability is missing (nothing then written).
fn enable_l1ss(d: &mut Drv) -> bool {
    let (Some(rc_pcie), Some(ep_pcie)) = (find_cap_in(d, DBI, 0x10), find_cap_in(d, CFG, 0x10)) else {
        d.put(b"l1ss: no PCIe capability\n");
        return false;
    };
    let (Some(rc_l1ss), Some(ep_l1ss), Some(ep_ltr)) = (find_ext_cap(d, DBI, 0x1E), find_ext_cap(d, CFG, 0x1E), find_ext_cap(d, CFG, 0x18)) else {
        d.put(b"l1ss: no L1 substates or LTR capability\n");
        return false;
    };
    d.kv(b"l1ss rc pcie", rc_pcie as u32);
    d.kv(b"l1ss rc l1ss", rc_l1ss as u32);
    d.kv(b"l1ss ep pcie", ep_pcie as u32);
    d.kv(b"l1ss ep l1ss", ep_l1ss as u32);
    d.kv(b"l1ss ep ltr", ep_ltr as u32);
    // PCI-PM, the root complex.
    d.wr(DBI, rc_l1ss + 0xC, 0xA1);
    d.wr(DBI, 0xB44, 0xEA);
    d.set(DBI, rc_pcie + 0x28, 0x400);
    d.wr(DBI, rc_l1ss + 0x8, 0x40A0_2A0F);
    // PCI-PM, the endpoint.
    d.wr(CFG, ep_l1ss + 0xC, 0xA1);
    d.wr(CFG, ep_ltr + 0x4, 0x1003_1003);
    d.set(CFG, ep_pcie + 0x28, 0x400);
    d.wr(CFG, ep_l1ss + 0x8, 0x40A0_0A0F);
    // ASPM L1: the root complex, then the endpoint.
    let v = d.rd(DBI, rc_pcie + 0x10);
    d.wr(DBI, rc_pcie + 0x10, (v & !0x3) | 0x40 | 0x2);
    let v = d.rd(CFG, ep_pcie + 0x10);
    d.wr(CFG, ep_pcie + 0x10, (v & !0x3) | 0x100 | 0x40 | 0x2);
    let back = [d.rd(DBI, rc_l1ss + 0x8), d.rd(CFG, ep_l1ss + 0x8), d.rd(DBI, rc_pcie + 0x10), d.rd(CFG, ep_pcie + 0x10)];
    d.kv(b"l1ss rc ctl1", back[0]);
    d.kv(b"l1ss ep ctl1", back[1]);
    d.kv(b"l1ss rc linkctl", back[2]);
    d.kv(b"l1ss ep linkctl", back[3]);
    d.flush();
    true
}

/// A#192.168.1.1 — one base prefix for the dotted quad.
/// The `wlan` endpoint (grant slot after the DMA region): Ethernet frames for other processes. Requests (first byte): 1 SEND frame → [0] sent / [1] refused (too short, or every tx buffer in flight); 2 RECV → the oldest received frame, or nothing (length 0); 3 INFO → our MAC (6 bytes) + link up (1); 8 EXCHANGE, frames both ways in one call (below). Between requests the driver keeps its rings moving; `recv` polls (flag 1) so it never blocks.
/// 8 EXCHANGE: [8, most (u32 BE), then frames to send, each a u16 BE length and its bytes] → [flags, refused (u16 BE), then received frames, each a u16 BE length and its bytes]: every frame of the request sent (refused counts those that were not: too short, too long, or no tx buffer free), then as many received frames, oldest first, as fit in `most` bytes of reply (header included; the kernel cuts a reply at the caller's buffer, so the caller says how much it takes). Flags bit 0: more frames are waiting. A request cut short mid-frame ends there. A driver without op 8 answers with nothing (length 0), which the caller reads as "one frame a call" (ops 1 and 2).
/// Why (2026-11-11, Mahere's map cells at about 0.2 MiB/s a connection): one frame a call cost a pass of this loop each (then about 270 µs of chip register reads thru syscalls, the PCIe link waking from L1 for each; the rings are read from host memory now, `item_new`) plus the call's own scheduling, and with every worker core busy each call waited for a 10 ms slice to end (`diag sched`: ip → pcie1 waited A#9740 µs on average, A#76 calls in A#877 ms). A call carries a batch now: the waits and the passes are paid once for many frames.
const WLAN: u64 = 15;
/// Receive buffers posted to the chip, 2 KiB each from RX_BASE in the DMA region (whose upper 1.5 MiB nothing else uses). 64 served the BCM4398; with 64 posted akita's BCM4383 delivered a third of its unicast and no ARP at all, so nothing on the LAN could be resolved and DNS never left (2026-11-09: Android's dhd posts 2040). With 256: every echo answered, ARP resolved, fgtw.org reached. A power of two under the rxpost ring's 512 slots; the rx queue's index fits a byte.
const RX_BUFS: u32 = 1 << 8;
const RX_BASE: u32 = 0x8_0000;
/// Transmit buffers, 2 KiB each from TX_BASE, above the receive buffers in the DMA region's upper MiB (256 KiB; nothing else is there since the vault's files moved into the arc, `vault_file`).
/// A batch from the IP stack is mostly its ACKs, one every second segment (RFC 5681): up to 44 for a reply of 88 full frames, and a 4096-byte request holds up to 66 frames of the 60-byte minimum. 32 buffers (2026-11-11 to 2026-11-12, at G#7_0000) could refuse a batch's last ACKs while the chip had not completed its first, and a batch's last ACK refused leaves its sender waiting for its timeout; 128 hold two full requests before any completion. Measured on the 8a with 128: a 421 KB fetch and a 17 MB one alone none refused; three font packs (9 to 17 MB) fetched at once 225 of 12864 refused, no segment resent; 256 (512 KiB, to G#18_0000) would fit, not tried yet.
const TX_BUFS: u32 = 1 << 7;
/// Ioctls in a row with no completion that end serving (`serve`): four, 12 s of waiting (the measures ask five every two seconds, each waiting 3 s), where one late completion is a slow firmware and four in a row a stopped one.
const SILENT_MAX: u32 = 1 << 2;
const TX_BASE: u32 = 0x10_0000;
/// The largest request: 4096 bytes, all a call carries (the kernel's SYS_CALL reads at most that).
const REQ_MAX: usize = 1 << 12;
/// The largest reply to op 8: 128 KiB, about 88 full frames, a third of the posted buffers (the kernel carries a reply of up to 256 KiB).
const REPLY_MAX: usize = 1 << 17;

/// The receive path's measures while serving, over the 32 s the link states cover (owner, 2026-11-11: measure the path before fixing it). Counters only, said on one line with the link states when frames moved; counter ticks where a time is kept.
#[derive(Default)]
struct Path {
    /// Passes of the serving loop, and the ones that ended in a sleep (no request).
    passes: u32,
    sleeps: u32,
    /// Ticks spent reading the completion rings each pass (`pump`, `pump_rx`, `pump_tx`; from host memory since 2026-11-11, `item_new`, before then thru three chip register reads a pass).
    ring_t: u64,
    /// Requests that moved frames: exchanges (op 8), and single frames either way (ops 1 and 2).
    exchanges: u32,
    singles: u32,
    /// Frames to the IP stack, the most in one exchange, and the ticks spent copying them out of the DMA region and re-posting their buffers.
    to_ip: u32,
    to_ip_most: u32,
    copy_t: u64,
    /// How long frames waited between their completion being taken and the IP stack taking them, in all and the longest, in ticks; the most frames waiting at once.
    wait_t: u64,
    wait_most: u64,
    queued_most: u32,
    /// Frames from the IP stack sent, and refused (malformed, or every tx buffer in flight).
    from_ip: u32,
    refused: u32,
    /// The chip's interrupt (`WIFI_IRQ`): waits ended by the line, the passes after one that found new frames, and passes after any other wake (the backstop, a call) that found new frames: frames no interrupt announced. And the doorbells answered (`wlan` op 9).
    irq_wakes: u32,
    irq_rx: u32,
    other_rx: u32,
    rings: u32,
    /// How the idle waits ended besides the interrupt: a call (an application's request), the deadline (the measures' 2 s while the screen is on), anything else (a preference changed).
    by_call: u32,
    by_deadline: u32,
    by_other: u32,
    /// The line re-armed (`irq_arm`): once an idle spell at most.
    arms: u32,
}

impl Path {
    /// One line: `rx path, last 32 s: A#<n> passes (A#<n> slept), rings read in A#<us> us a pass; A#<n> exchanges, A#<n> single frames; to ip A#<n> frames (most A#<n> an exchange, copied in A#<ns> ns each, waited A#<us> us on average, A#<us> us most, A#<n> most queued); from ip A#<n> frames, A#<n> refused`. Said only when frames moved, so after a request, so after a pass: `passes` is not zero.
    fn say(&self, d: &mut Drv, ms: u64) {
        let f = d.freq;
        d.put(b"rx path, last A#");
        d.dec(ms as u32);
        d.put(b" ms: ");
        d.dec(self.passes);
        d.put(b" passes (");
        d.dec(self.sleeps);
        d.put(b" slept), rings read in ");
        d.dec((self.ring_t * 1_000_000 / f / self.passes as u64) as u32);
        d.put(b" us a pass; ");
        d.dec(self.exchanges);
        d.put(b" exchanges, ");
        d.dec(self.singles);
        d.put(b" single frames; to ip ");
        d.dec(self.to_ip);
        d.put(b" frames");
        // WHY the test: the averages are per frame to the IP stack, and a window may have moved frames the other way only.
        if self.to_ip > 0 {
            let n = self.to_ip as u64;
            d.put(b" (most ");
            d.dec(self.to_ip_most);
            d.put(b" an exchange, copied in ");
            d.dec((self.copy_t * 1_000_000_000 / f / n) as u32);
            d.put(b" ns each, waited ");
            d.dec((self.wait_t * 1_000_000 / f / n) as u32);
            d.put(b" us on average, ");
            d.dec((self.wait_most * 1_000_000 / f) as u32);
            d.put(b" us most, ");
            d.dec(self.queued_most);
            d.put(b" most queued)");
        }
        d.put(b"; from ip ");
        d.dec(self.from_ip);
        d.put(b" frames, ");
        d.dec(self.refused);
        d.put(b" refused; doorbell ");
        d.dec(self.rings);
        d.put(b" rings");
        if WIFI_IRQ {
            d.put(b"; irq ");
            d.dec(self.irq_wakes);
            d.put(b" wakes, ");
            d.dec(self.irq_rx);
            d.put(b" with new frames; new frames with no irq ");
            d.dec(self.other_rx);
            d.put(b"; waits ended by a call ");
            d.dec(self.by_call);
            d.put(b", the deadline ");
            d.dec(self.by_deadline);
            d.put(b", else ");
            d.dec(self.by_other);
            d.put(b"; armed ");
            d.dec(self.arms);
        }
        d.put(b"\n");
    }
}

/// The interrupt's latches cleared, before the rings are read (`WIFI_IRQ`): the root complex's ELBI PCIE_IRQ2 (write back what it holds, as the Exynos RC driver's handler does), its MSI status (DBI G#830, write-1-to-clear, as the modem driver does on channel 0), then the chip's mailbox interrupt (BRCMF_PCIE_64_PCIE2REG_MAILBOXINT G#C30, write back, as brcmfmac's handler does). In that order, and the rings read after: a completion before the chip's latch is cleared is in the rings this pass reads, and one after it raises a new MSI into latches already clear, so the line rises again and the next wait ends at once.
fn irq_clear(d: &mut Drv) {
    let s = d.rd(ELBI, 0x8);
    d.wr(ELBI, 0x8, s);
    let m = d.rd(DBI, 0x830);
    if m != 0 {
        d.wr(DBI, 0x830, m);
    }
    let c = d.rd(MEM, 0x2C30);
    if c != 0 {
        d.wr(MEM, 0x2C30, c);
    }
}

/// The interrupt proven before the loop relies on it: `irq_thread` started and armed, then one ioctl (`allmulti` 1: all multicast in, as IPv6 neighbour discovery lives on 33:33:… groups) waited for by the clock as during bring-up; its completion must have raised an interrupt within 2^4 of the counter's ~1 ms after it was seen (the MSI is posted after the completion's write). True: the loop waits on the line. False: no thread, no interrupt from this chip (a mailbox mask not where brcmfmac puts it, an MSI not routed), said on the console, and the loop sleeps a millisecond between passes as before.
fn irq_selftest(d: &mut Drv, m: &mut Msgbuf) -> bool {
    use core::sync::atomic::Ordering::Acquire;
    // SAFETY: the stack is this thread's alone, started once (`serve` runs once a process: a lost link ends the driver).
    let stack = unsafe { &mut *(&raw mut IRQ_STACK) };
    if ferros_sys::thread_spawn(irq_thread, stack, d.cap(IRQ)) == ERR {
        d.put(b"irq: no thread: sleeping a millisecond between passes\n");
        d.flush();
        let r = m.set_int(d, b"allmulti", 1);
        d.kv(b"allmulti", r);
        return false;
    }
    irq_arm(d);
    let rung0 = IRQ_RUNG.load(Acquire);
    let t0 = d.now();
    let r = m.set_int(d, b"allmulti", 1);
    d.kv(b"allmulti", r);
    let t1 = d.now();
    while IRQ_RUNG.load(Acquire) == rung0 && d.now().wrapping_sub(t1) < (d.freq >> 10) << 4 {
        core::hint::spin_loop();
    }
    let rung = IRQ_RUNG.load(Acquire);
    let ok = rung != rung0 && rung != u64::MAX;
    d.kv(b"irq: self-test ioctl answered in us", (t1.wrapping_sub(t0) * 1_000_000 / d.freq) as u32);
    d.put(if ok { b"irq: the chip's completion raised SPI 439: waiting on the line, no timer\n".as_slice() } else { b"irq: the chip's completion raised no interrupt: sleeping a millisecond between passes\n" });
    d.flush();
    ok
}

fn serve(d: &mut Drv, m: &mut Msgbuf, sh: u32) {
    if !flowring(d, m) {
        return;
    }
    // Checksum offload: the firmware fills transmit checksums only with extended (56-byte) tx items, offered by pcie_shared flags2 bit G#40_0000 (TXPOST_EXT); receive completions flag G#800 when it verified a checksum.
    let f2 = d.tcm_rd(sh + 80);
    d.kv(b"shared flags2", f2);
    d.put(if f2 & 0x0040_0000 != 0 { b"tx checksum offload offered (TXPOST_EXT)\n" } else { b"no tx checksum offload (no TXPOST_EXT)\n" });
    m.serving = true;
    m.verbose = false;
    d.put(b"serving wlan\n");
    d.flush();
    enable_l1ss(d);
    // The chip's interrupt (the line granted): the root complex's MSI receiver onto the controller's line, the chip's completions onto MSI, the interrupt's thread started, then proven by an ioctl's completion (`irq_selftest`).
    let irq = WIFI_IRQ && unsafe { core::ptr::read_volatile((d.base + d.grants + 0x200) as *const u32) } as u64 > IRQ;
    if irq {
        irq_clear(d);
        // ELBI PCIE_IRQ2_EN bit 17 (IRQ_MSI_CTRL_EN_RISING_EDG): an MSI raises SPI 439 (pcie-exynos-rc.c exynos_pcie_rc_msi_init, channel 1 having no separate MSI lines). The A1 step zeroed every enable; the MSI receiver itself (DBI G#820-G#82C) is set since the firmware's download.
        d.set(ELBI, 0x18, 1 << 17);
        // The chip's D2H doorbells onto its host interrupt: the mailbox mask (brcmfmac pcie.c brcmf_pcie_intr_enable, BRCMF_PCIE_64_PCIE2REG_MAILBOXMASK G#C34 = BRCMF_PCIE_64_MB_INT_D2H_DB G#FFFF), the PCIe core at BAR0 + G#2000.
        d.wr(MEM, 0x2C34, 0xFFFF);
        let (en, mm) = (d.rd(ELBI, 0x18), d.rd(MEM, 0x2C34));
        d.kv(b"irq: elbi irq2_en", en);
        d.kv(b"irq: chip mailbox mask", mm);
        d.flush();
    } else if WIFI_IRQ {
        d.put(b"irq: no interrupt grant (fewer than 20 slots): sleeping a millisecond between passes\n");
    }
    let mut irq = if irq {
        irq_selftest(d, m)
    } else {
        // All multicast in: IPv6 neighbour discovery lives on 33:33:… groups.
        let r = m.set_int(d, b"allmulti", 1);
        d.kv(b"allmulti", r);
        false
    };
    IRQ_ON.store(irq, core::sync::atomic::Ordering::Relaxed);
    // The screen's state as last looked at (the preference block, a memory read): with it on, the corners' measures every 2 s; off, none.
    let mut dark_now = ferros_sys::pref(ferros_sys::PREF_SCREEN_OFF) == 1;
    // The doorbell (`wlan` op 9): the reply token of a call held until frames wait.
    let mut ring: Option<[u8; 32]> = None;
    // How the last wait ended: 0 not waited (a request was served), 1 the interrupt, 2 a call, 3 the deadline, 4 anything else (a preference changed, the kernel waking every word waiter).
    let mut woke = 0u8;
    // The link's power state as the PHY reports it (PCS G#188 bits 2:0: 0 L0, 2 L1, 5 L1.1, 6 L1.2; read without waking the link), sampled each pass and said every 32 s.
    let mut lstates = [0u32; 8];
    let mut lstates_at = d.now();
    // The receive path's measures over the same 32 s, said with the link states when frames moved.
    let mut path = Path::default();
    let mut msg = [0u8; REQ_MAX];
    // The reply: up to `REPLY_MAX` bytes of frames, a static (not the 64 KiB stack).
    static mut OUT: [u8; REPLY_MAX] = [0; REPLY_MAX];
    // SAFETY: this loop is the only user of OUT, and the driver has one thread.
    let out = unsafe { &mut *(&raw mut OUT) };
    let mut tok = [0u8; 32];
    // The corners' WiFi circle (SYS_STATUS): the link's own measures every two seconds, each in its unit, published only where the firmware answers (owner: many dimensions, never bars). Into the scan's request area: no scan runs while serving.
    let mut status_at = 0u64;
    let mut pkts = [0u32; 4];
    // The link's names and addresses for its settings page (ferros_detail, behind the Settings capability): they hold while this join does.
    publish_detail(m);
    // The link this join made, watched while serving (`pump` reads the firmware's events): a join's own failures are behind it.
    let was_linked = m.linked;
    m.join_failed = false;
    // The control block (docs/RADIO-CONTROLS.md) with the screen's state: a generation moved since the last look is a knob set, a screen change moves an unset power save. None: not looked at yet, so the first look applies what is kept. The screen is a memory read, looked at every pass; the block is a syscall, looked at after a wait no interrupt or call ended (a preference or a knob changed: the kernel wakes every word waiter then) and at most every quarter second while passes run back to back. No timer brings a look: with the line idle the loop sleeps until something happens.
    let mut control_at = 0u64;
    let mut control_gen: Option<u64> = None;
    let mut dark_was = !dark_now;
    loop {
        // The process watchdog's pet, once a pass (ferros_sys::pet; measuring, `diag pets`).
        ferros_sys::pet();
        // The interrupt's count before the rings are read: an interrupt taken while this pass runs ends its wait at once.
        let seen = IRQ_RUNG.load(core::sync::atomic::Ordering::Acquire);
        dark_now = ferros_sys::pref(ferros_sys::PREF_SCREEN_OFF) == 1;
        if dark_now != dark_was || woke == 4 || d.now().wrapping_sub(control_at) >= d.freq / 4 {
            control_at = d.now();
            let c = ferros_sys::controls(ferros_sys::control::WIFI);
            if control_gen != Some(c[0]) || dark_now != dark_was {
                // The screen back on: the corners' measures at once, not 2 s later.
                if !dark_now && dark_was {
                    status_at = 0;
                }
                control_gen = Some(c[0]);
                dark_was = dark_now;
                apply_controls(m, d, &c, dark_now);
            }
        }
        // The corners' measures only while someone can see them: none with the screen off, where they were 2 s wakes and five ioctls for nobody.
        if !dark_now && d.now().wrapping_sub(status_at) >= 2 * d.freq {
            status_at = d.now();
            publish_measures(m, d, &mut pkts);
        }
        let ls = d.rd(PCS, 0x188) & 7;
        lstates[ls as usize] += 1;
        if d.now().wrapping_sub(lstates_at) >= 32 * d.freq {
            // The span since the last line: 32 s or more, as no timer brings a pass.
            let ms = d.now().wrapping_sub(lstates_at) * 1000 / d.freq;
            lstates_at = d.now();
            d.put(b"link states, passes in the last A#");
            d.dec(ms as u32);
            d.put(b" ms (PCS G#188 2:0 = 0 L0, 2 L1, 5 L1.1, 6 L1.2):");
            for (k, n) in lstates.iter().enumerate() {
                if *n > 0 {
                    d.put(b" ");
                    d.put(&[b'0' + k as u8]);
                    d.put(b"=");
                    d.hex(*n);
                }
            }
            d.put(b"\n");
            if path.to_ip + path.from_ip > 0 || irq {
                path.say(d, ms);
            }
            d.flush();
            lstates = [0; 8];
            path = Path::default();
        }
        path.passes += 1;
        let t_rings = d.now();
        let _ = m.pump(d);
        // The link lost while serving (the firmware's LINK down, or a deauthentication or disassociation from the access point): until 2026-11-10 the loop served on unlinked, the IP stack heard "not linked" every 4 s and the device was off the network until someone restarted this driver (the 8a, overnight). Now it ends, saying why, and init's keeper starts it again (at most every 64 s), which brings the radio up and joins afresh: the path every boot takes.
        if was_linked && (!m.linked || m.join_failed) {
            d.put(if m.linked { b"the access point dropped this device while serving: ending; init's keeper starts this driver again\n".as_slice() } else { b"the link went down while serving: ending; init's keeper starts this driver again\n" });
            d.flush();
            m.serving = false;
            d.failed = true;
            return;
        }
        // The firmware silent (2026-11-12, the owner's Pro: hundreds of "no ioctl completion" over minutes, no transmit completions, every frame refused, each measure's ioctl waiting out its 3 s so one exchange with ip took A#21 s, and Mahere's tiles never loaded): it ends the same way, the firmware's shared flags and console said after it (`run`), and init's keeper starts it again, the chip brought up afresh.
        if m.silent >= SILENT_MAX {
            d.put(b"the firmware completes no ioctl while serving: ending; init's keeper starts this driver again\n");
            d.flush();
            m.serving = false;
            d.failed = true;
            return;
        }
        let rx0 = m.rxq_len;
        m.pump_rx(d);
        m.pump_tx(d);
        path.ring_t += d.now().wrapping_sub(t_rings);
        // New frames after a wake: announced by the interrupt, or found without one.
        if m.rxq_len > rx0 {
            path.irq_rx += (woke == 1) as u32;
            path.other_rx += (woke >= 2) as u32;
        }
        woke = 0;
        // WHY the max: the high water, for the measure.
        path.queued_most = path.queued_most.max(m.rxq_len as u32);
        // Frames wait: the held doorbell answered (`wlan` op 9).
        if m.rxq_len > 0 {
            if let Some(t) = ring.take() {
                out[0] = 1;
                svc(3, t.as_ptr() as u64 - d.base, out.as_ptr() as u64 - d.base, 1, 0, 0);
                path.rings += 1;
            }
        }
        let n = svc(2, d.cap(WLAN), msg.as_mut_ptr() as u64 - d.base, msg.len() as u64, tok.as_mut_ptr() as u64 - d.base, 1);
        if n == u64::MAX - 1 || n == ERR {
            // No request: a millisecond's sleep, not a spinning core (the driver runs from boot since init starts it, 2026-10-26), frames waiting or not. A call ends the sleep at once (the kernel wakes a polling server asleep between polls), so waiting frames lose nothing by it. Until 2026-11-11 the loop spun while frames waited, and with every worker core busy it spun thru whole 10 ms slices of poll misses while the IP stack, the one process that would take them, waited for a core (`diag sched`).
            if irq {
                use core::sync::atomic::Ordering::Acquire;
                // With the interrupt (2026-11-13): the line re-armed first if the last interrupt left it masked, and the rings read once more before any wait (`irq_arm`).
                if IRQ_ARMED.load(Acquire) == 0 {
                    path.arms += 1;
                    irq_arm(d);
                    continue;
                }
                path.sleeps += 1;
                // Then a wait on nothing but what brings work: the interrupt's count moving (1, or 0 when it moved during the wait), a call (3), a preference changing (0, the count unmoved). The one deadline is the corners' measures while the screen is on; none in the dark.
                let until = if dark_now { 0 } else { status_at + 2 * d.freq };
                let r = ferros_sys::wait_world_or_call(&IRQ_RUNG, seen, until);
                let rung = IRQ_RUNG.load(Acquire);
                woke = match r {
                    _ if rung == u64::MAX => {
                        // The thread's wait refused: not an interrupt grant after all, the sleep from here on.
                        irq = false;
                        IRQ_ON.store(false, core::sync::atomic::Ordering::Relaxed);
                        d.put(b"irq: the wait was refused: sleeping a millisecond between passes\n");
                        4
                    }
                    0 | 1 if rung != seen => 1,
                    3 => 2,
                    2 => 3,
                    _ => 4,
                };
                path.irq_wakes += (woke == 1) as u32;
                path.by_call += (woke == 2) as u32;
                path.by_deadline += (woke == 3) as u32;
                path.by_other += (woke == 4) as u32;
            } else {
                path.sleeps += 1;
                ferros_sys::sleep_until(ferros_sys::now() + d.freq / 1000);
            }
            continue;
        }
        let msg = unsafe { core::ptr::read_volatile(&msg) };
        let n = n as usize;
        // The doorbell asked while no frame waits: its token kept, answered once frames come (above). An older one still kept is let go with 0 (its asker asks again), so one asker never holds two.
        if msg.first() == Some(&9) && m.rxq_len == 0 {
            if let Some(t) = ring.replace(tok) {
                out[0] = 0;
                svc(3, t.as_ptr() as u64 - d.base, out.as_ptr() as u64 - d.base, 1, 0, 0);
            }
            continue;
        }
        let len = match msg.first() {
            Some(1) if n > 14 => {
                path.singles += 1;
                let ok = m.send(d, &msg[1..n]);
                path.from_ip += ok as u32;
                path.refused += !ok as u32;
                out[0] = !ok as u8;
                1
            }
            Some(2) if m.rxq_len > 0 => {
                path.singles += 1;
                let l = m.rxq_take(out, &mut path);
                m.ring_rx(d);
                l
            }
            Some(8) if n >= 5 => {
                path.exchanges += 1;
                // WHY the min: `most` is the caller's word; the reply holds no more than this buffer either.
                let most = (u32::from_be_bytes([msg[1], msg[2], msg[3], msg[4]]) as usize).min(out.len());
                // The frames to send, each a u16 BE length and its bytes; one that runs past the request ends it (refused).
                let (mut at, mut sent, mut refused) = (5, 0u32, 0u32);
                while at + 2 <= n {
                    let l = u16::from_be_bytes([msg[at], msg[at + 1]]) as usize;
                    at += 2;
                    if at + l > n {
                        refused += 1;
                        break;
                    }
                    if m.tx_post(d, &msg[at..at + l]) {
                        sent += 1;
                    } else {
                        refused += 1;
                    }
                    at += l;
                }
                if sent > 0 {
                    m.tx_ring(d);
                }
                path.from_ip += sent;
                path.refused += refused;
                // The frames received, oldest first, as many as fit.
                let mut o = 3;
                let mut k = 0u32;
                while m.rxq_len > 0 && o + 2 + m.rxq[m.rxq_head].2 as usize <= most {
                    let l = m.rxq_take(&mut out[o + 2..], &mut path);
                    out[o..o + 2].copy_from_slice(&(l as u16).to_be_bytes());
                    o += 2 + l;
                    k += 1;
                }
                if k > 0 {
                    m.ring_rx(d);
                }
                // WHY the max: the high water, for the measure.
                path.to_ip_most = path.to_ip_most.max(k);
                out[0] = (m.rxq_len > 0) as u8;
                // A request of 4096 bytes holds fewer than 2^11 frames: the count fits.
                out[1..3].copy_from_slice(&(refused as u16).to_be_bytes());
                o
            }
            Some(3) => {
                out[..6].copy_from_slice(&m.mac);
                out[6] = m.linked as u8;
                7
            }
            // The network page while joined: the list from before the join (no scan runs while serving), JOIN refused with 3 (joined already; another network is a later change), and the status.
            Some(4) => m.scan_reply(false, out),
            // WATCH while joined: the list from before the join, and the last (no scan runs while serving).
            Some(7) => watch_reply(m, false, false, out),
            Some(5) => {
                out[0] = 3;
                1
            }
            Some(6) => m.status_reply(2, out),
            Some(1) => {
                out[0] = 1;
                1
            }
            // The doorbell with frames already waiting: rung at once.
            Some(9) => {
                path.rings += 1;
                out[0] = 1;
                1
            }
            _ => 0,
        };
        svc(3, tok.as_ptr() as u64 - d.base, out.as_ptr() as u64 - d.base, len as u64, 0, 0);
    }
}

fn ip_put(d: &mut Drv, a: [u8; 4]) {
    d.put(b"A#");
    for (k, b) in a.iter().enumerate() {
        if k > 0 {
            d.put(b".");
        }
        let mut v = *b as u32;
        let mut digits = [0u8; 3];
        let mut n = 0;
        loop {
            digits[2 - n] = b'0' + (v % 10) as u8;
            n += 1;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        d.put(&digits[3 - n..]);
    }
}

/// Stage 13: one transmit flowring, then DHCP — DISCOVER (broadcast), OFFER, REQUEST, ACK — from the EL0 driver. Flowring 2 (dhd's first flow id) is created on the control ring with its DMA ring (512 × 48 B at +G#6_0000), DA broadcast and SA our MAC, as dhd does for a STA's first packet; a STA has one flowring per priority, whatever the destination.
fn dhcp(d: &mut Drv, m: &mut Msgbuf) {
    if !flowring(d, m) {
        return;
    }
    dhcp_run(d, m);
}

/// Create transmit flowring 2 (see `dhcp`); true when the firmware answered status 0.
fn flowring(d: &mut Drv, m: &mut Msgbuf) -> bool {
    let req = m.dma + 0x1_3000;
    if let Some((0, b, _)) = m.get_var(d, b"cur_etheraddr", req) {
        m.mac = core::array::from_fn(|k| dbr(m.resp(b) + k as u32));
    }
    d.tcm_wr(m.h2d_w + 8, 0);
    let mut c = [0u8; 32];
    c[0..6].copy_from_slice(&[0xFF; 6]);
    c[6..12].copy_from_slice(&m.mac);
    c[14..16].copy_from_slice(&2u16.to_le_bytes());
    c[20..22].copy_from_slice(&512u16.to_le_bytes());
    c[22..24].copy_from_slice(&48u16.to_le_bytes());
    c[24..28].copy_from_slice(&(m.dma + 0x6_0000).to_le_bytes());
    let body: [u32; 8] = core::array::from_fn(|k| u32::from_le_bytes(c[4 * k..4 * k + 4].try_into().unwrap()));
    m.flow_status = None;
    m.post(d, 0x03, 0, body);
    m.ring(d);
    let ts = d.now();
    while m.flow_status.is_none() && d.now().wrapping_sub(ts) < 2 * d.freq {
        let _ = m.pump(d);
        d.us(1000);
    }
    d.kv(b"flowring create status", m.flow_status.unwrap_or(u32::MAX));
    d.flush();
    m.flow_status == Some(0)
}

fn dhcp_run(d: &mut Drv, m: &mut Msgbuf) {
    m.xid = 0xF3A0_0000 | (d.now() as u32 & 0xFFFF);
    for attempt in 0..3u32 {
        let frame = dhcp_frame(&m.mac, m.xid, 1, None);
        m.send(d, &frame[..342]);
        d.kv(b"DISCOVER sent, attempt", attempt);
        d.flush();
        let ts = d.now();
        while m.offer.is_none() && d.now().wrapping_sub(ts) < 3 * d.freq {
            let _ = m.pump(d);
            m.pump_rx(d);
            m.pump_tx(d);
            d.us(1000);
        }
        if m.offer.is_some() {
            break;
        }
    }
    d.kv(b"tx completions", m.tx_done);
    d.kv(b"last tx status", m.tx_status);
    let Some((ip, server)) = m.offer else {
        d.put(b"no DHCP offer\n");
        d.flush();
        return;
    };
    let frame = dhcp_frame(&m.mac, m.xid, 3, Some((ip, server)));
    m.send(d, &frame[..342]);
    d.flush();
    let ts = d.now();
    while !m.acked && !m.nak && d.now().wrapping_sub(ts) < 4 * d.freq {
        let _ = m.pump(d);
        m.pump_rx(d);
        m.pump_tx(d);
        d.us(1000);
    }
    if m.acked {
        d.put(b"LEASED ");
        ip_put(d, ip);
        d.put(b" mask ");
        ip_put(d, m.lease[0]);
        d.put(b" router ");
        ip_put(d, m.lease[1]);
        d.put(b" dns ");
        ip_put(d, m.lease[2]);
        d.put(b" lease ");
        d.dec(m.lease_s);
        d.put(b" s\n");
        d.kv(b"rx frames", m.rx_frames);
    } else {
        d.put(if m.nak { b"DHCP NAK\n" } else { b"no DHCP ACK\n" });
    }
    d.flush();
}

/// A broadcast DHCP message from 0.0.0.0:68 to 255.255.255.255:67 (342 bytes): Ethernet, IPv4 (header checksum, UDP checksum 0 as IPv4 allows), BOOTP (op 1, broadcast flag so replies come broadcast, our MAC as chaddr), the magic cookie, options 53 (1 DISCOVER / 3 REQUEST), 50 + 54 on a REQUEST, 55 (subnet, router, DNS, lease), end.
fn dhcp_frame(mac: &[u8; 6], xid: u32, ty: u8, req: Option<([u8; 4], [u8; 4])>) -> [u8; 342] {
    let mut f = [0u8; 342];
    f[0..6].copy_from_slice(&[0xFF; 6]);
    f[6..12].copy_from_slice(mac);
    f[12..14].copy_from_slice(&[0x08, 0x00]);
    let ip = 14;
    let ip_len = (342 - 14) as u16;
    f[ip..ip + 12].copy_from_slice(&[0x45, 0, (ip_len >> 8) as u8, ip_len as u8, 0, 0, 0, 0, 64, 17, 0, 0]);
    f[ip + 16..ip + 20].copy_from_slice(&[255; 4]);
    let mut sum = 0u32;
    for k in (0..20).step_by(2) {
        sum += u16::from_be_bytes([f[ip + k], f[ip + k + 1]]) as u32;
    }
    while sum > 0xFFFF {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    f[ip + 10..ip + 12].copy_from_slice(&(!(sum as u16)).to_be_bytes());
    let udp = ip + 20;
    let udp_len = ip_len - 20;
    f[udp..udp + 8].copy_from_slice(&[0, 68, 0, 67, (udp_len >> 8) as u8, udp_len as u8, 0, 0]);
    let b = udp + 8;
    f[b..b + 4].copy_from_slice(&[1, 1, 6, 0]);
    f[b + 4..b + 8].copy_from_slice(&xid.to_be_bytes());
    f[b + 10] = 0x80;
    f[b + 28..b + 34].copy_from_slice(mac);
    f[b + 236..b + 240].copy_from_slice(&[0x63, 0x82, 0x53, 0x63]);
    let mut o = b + 240;
    for chunk in [&[53u8, 1, ty][..]] {
        f[o..o + chunk.len()].copy_from_slice(chunk);
        o += chunk.len();
    }
    if let Some((ip_req, server)) = req {
        f[o..o + 2].copy_from_slice(&[50, 4]);
        f[o + 2..o + 6].copy_from_slice(&ip_req);
        f[o + 6..o + 8].copy_from_slice(&[54, 4]);
        f[o + 8..o + 12].copy_from_slice(&server);
        o += 12;
    }
    f[o..o + 6].copy_from_slice(&[55, 4, 1, 3, 6, 51]);
    f[o + 6] = 255;
    f
}

/// [u32 LE len][bytes] twice (firmware, NVRAM), then optionally the CLM blob and the WiFi credential (empty if absent).
fn split_files(b: &mut [u8]) -> Option<(&[u8], &mut [u8], &[u8], &[u8])> {
    let take = |b: &[u8]| -> Option<usize> { Some(u32::from_le_bytes(b.get(..4)?.try_into().ok()?) as usize) };
    let a = take(b)?;
    if b.len() < 4 + a + 4 {
        return None;
    }
    let (first, rest) = b[4..].split_at_mut(a);
    let n = take(rest)?;
    if rest.len() < 4 + n {
        return None;
    }
    let (second, rest) = rest[4..].split_at_mut(n);
    let (third, rest): (&[u8], &[u8]) = match take(rest) {
        Some(c) if rest.len() >= 4 + c => (&rest[4..4 + c], &rest[4 + c..]),
        _ => (&[], &[]),
    };
    let fourth: &[u8] = match take(rest) {
        Some(c) if rest.len() >= 4 + c => &rest[4..4 + c],
        _ => &[],
    };
    Some((first, second, third, fourth))
}

/// Where the firmware comes from: the process input (the bridge), or the vault, streamed (its length).
enum Fw<'a> {
    Mem(&'a [u8]),
    Vault(usize),
}
impl Fw<'_> {
    fn len(&self) -> usize {
        match self {
            Fw::Mem(f) => f.len(),
            Fw::Vault(n) => *n,
        }
    }
}

/// Part of a value in the vault scope `wifi.` (SYS_STORE, piecewise: bit 1, the offset in bits 32-63): `buf.len()` bytes from `off`. Returns the value's whole length.
fn store_get_at(d: &Drv, key: &[u8], off: usize, buf: &mut [u8]) -> Option<usize> {
    let r = svc(13, d.cap(STORE), 2 | (buf.len() as u64) << 8 | (off as u64) << 32, key.as_ptr() as u64 - d.base, key.len() as u64, buf.as_mut_ptr() as u64 - d.base);
    (r != ERR && r != ERR - 1).then_some(r as usize)
}

/// The files from the vault scope `wifi.`: the firmware's length (it is streamed later), and the NVRAM text, CLM blob and credential read whole into scratch in the DMA region (1 MiB in, 64 KiB each at most).
fn from_vault(d: &Drv) -> Option<(Fw<'static>, &'static mut [u8], &'static [u8], &'static [u8])> {
    let dma = svc(7, d.cap(DMA), 0, 0, 0, 0);
    if dma == ERR {
        return None;
    }
    let mut head = [0u8; 4];
    let fw_len = store_get_at(d, b"fw", 0, &mut head)?;
    let nv = vault_file(d, b"nvram", 0x10_0000)?;
    let clm = vault_file(d, b"clm", 0x11_0000).map(|b| &*b).unwrap_or(&[]);
    // The credential is read by `connect` (`stored_cred`): the person's or the device's, as a document.
    Some((Fw::Vault(fw_len), nv, clm, &[]))
}

/// One small file of the vault scope `wifi.`, read whole into scratch `at` bytes into the DMA region (64 KiB at most). None when it is absent, too large, or locked (the credential is the person's, AUTH.md).
fn vault_file(d: &Drv, key: &[u8], at: u64) -> Option<&'static mut [u8]> {
    // Into the arc (FILES), one 64 KiB buffer per file by its old place in the DMA region (nvram at 1 MiB, clm 1 MiB + 64 KiB, cred 1 MiB + 128 KiB): nothing reads them but this driver, so they need not be where the chip can see them, and a masked process reads only its own arc and its handles.
    let slot = ((at - 0x10_0000) >> 16) as usize;
    let mut h = [0u8; 4];
    let len = store_get_at(d, key, 0, &mut h)?;
    if len > 0x1_0000 {
        return None;
    }
        let buf = unsafe { core::slice::from_raw_parts_mut((&raw mut FILES as *mut u8).add(slot << 16), len) };
        // The vault copies into this process's own memory (an arc offset): through a piece on the stack, then into the scratch (the first try handed the DMA region's address and the process ended without a word, 2026-10-26).
        let mut piece = [0u8; 4096];
        let mut off = 0;
        while off < len {
            let n = (len - off).min(piece.len());
            store_get_at(d, key, off, &mut piece[..n])?;
            buf[off..off + n].copy_from_slice(&piece[..n]);
            off += n;
        }
    Some(buf)
}

/// The network credential (ferros_detail::network, a VSF document): its name and passphrase, or None if it is not one. Never printed.
fn credential(c: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    ferros_detail::network::parse(c)
}

/// The credential the vault keeps: the person's (`wifi.cred`, readable once someone attests, AUTH.md) before the device's (`wifi.net`, a network joined before anyone attested). A value that is not a credential document (the two-line text file it replaced, 2026-11-09) is dropped, never parsed.
fn stored_cred(d: &mut Drv) -> Option<(Vec<u8>, Vec<u8>, &'static [u8])> {
    for key in [&b"cred"[..], b"net"] {
        let Some(doc) = vault_file(d, key, 0x12_0000) else { continue };
        match credential(doc) {
            Some((n, p)) => return Some((n, p, key)),
            None => {
                let gone = ferros_sys::store_delete(d.cap(STORE), key);
                d.put(b"wifi.");
                d.put(key);
                d.put(if gone == Some(true) { b" dropped: not a credential document\n" } else { b" is not a credential document and could not be dropped\n" });
                d.flush();
            }
        }
    }
    None
}

/// Eagle time now, in oscillations (the vault process's `eagle`): 0 until the kernel has read the PMIC's clock.
fn eagle() -> i64 {
    // The Eagle epoch (1969-07-20 20:17:40 UTC) in the wall clock's seconds.
    const EPOCH_UNIX: i128 = -14_182_940;
    let Some((secs, ticks, hz)) = ferros_sys::utc_time() else { return 0 };
    let ops = ferros_detail::vsf::OSCILLATIONS_PER_SECOND as i128;
    ((secs as i128 - EPOCH_UNIX) * ops + ticks as i128 * ops / hz as i128) as i64
}

/// The boot path's join (init's pcie1): the radio up, then a credential — the vault's (`stored_cred`), or one the network page hands in (JOIN, `serve_unjoined`) — scanned for and joined. A join that fails goes back to serving the page; a credential handed in that failed is deleted again (a mistyped passphrase is not kept), and the vault's own is retried when a scan sees its network.
fn connect(d: &mut Drv, m: &mut Msgbuf, clm: &[u8]) {
    let mut next = stored_cred(d).map(|(n, p, k)| (n, p, k, false));
    if let Some((n, _, k, _)) = &next {
        m.set_want(n);
        m.personal = *k == b"cred";
    }
    m.band = ferros_sys::controls(ferros_sys::control::WIFI)[ferros_sys::control::WIFI_BAND as usize];
    radio_up(d, m, clm);
    // radio_up set this credential's MAC; any other needs the radio brought down first (`rejoin_as`).
    let mut fresh = true;
    let mut skip: Option<(Vec<u8>, Vec<u8>, &'static [u8])> = None;
    loop {
        let (name, pass, key, handed) = match next.take() {
            Some(c) => c,
            None => {
                fresh = false;
                serve_unjoined(d, m, &skip)
            }
        };
        if fresh {
            m.target = None;
            escan_start(d, m);
            escan_wait(d, m);
        } else {
            rejoin_as(d, m, &name, key == b"cred");
        }
        fresh = false;
        join(d, m, &name, &pass);
        if m.keyed {
            m.retry_s = 1 << 6;
            return;
        }
        // Not joined: the firmware let go of any half-made association (WLC_DISASSOC 52), and the join's state cleared for the next.
        let req = m.dma + 0x1_3000;
        for k in 0..12 {
            db(req + k, 0);
        }
        let _ = m.ioctl(d, 52, req, 12, 12);
        m.linked = false;
        m.join_failed = false;
        skip = if handed {
            let _ = ferros_sys::store_delete(d.cap(STORE), key);
            d.put(b"the credential handed in did not join: deleted\n");
            d.flush();
            None
        } else {
            Some((name, pass, key))
        };
    }
}


/// The network page's list as last answered to a WATCH (op 7), counted: moved on each time the list or its scanning flag changes, kept across a join that failed so a page waiting from before it is answered at once after.
static PAGE_GEN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// WATCH's reply: [count u32 LE][1 more will come, 0 this is the last (joined: no scan runs while serving)][SCAN's reply (`scan_reply`)].
fn watch_reply(m: &Msgbuf, scanning: bool, live: bool, out: &mut [u8]) -> usize {
    out[..4].copy_from_slice(&PAGE_GEN.load(core::sync::atomic::Ordering::Relaxed).to_le_bytes());
    out[4] = live as u8;
    5 + m.scan_reply(scanning, &mut out[5..])
}

/// The list's count moved on when it differs from the one last answered (`last`).
fn watch_count(m: &Msgbuf, scanning: bool, last: &mut Vec<u8>) {
    let mut b = [0u8; 2048];
    let n = m.scan_reply(scanning, &mut b);
    if b[..n] != last[..] {
        *last = b[..n].to_vec();
        PAGE_GEN.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
}

/// Every held WATCH answered with the list as it stands: when it changed, and before the page loop leaves for a join (a held call nobody answers keeps its caller waiting for good; it calls again, and that call waits in the endpoint while the join runs).
fn watch_answer(d: &Drv, m: &Msgbuf, scanning: bool, held: &mut Vec<[u8; 32]>) {
    let mut out = [0u8; 2048];
    let len = watch_reply(m, scanning, true, &mut out);
    for tok in core::mem::take(held) {
        svc(3, tok.as_ptr() as u64 - d.base, out.as_ptr() as u64 - d.base, len as u64, 0, 0);
    }
}
/// Before any join (`connect`), the network page's requests on `wlan`, beside the vault looked at four times a second: 3 the MAC (not linked), 4 SCAN ([4, 1] starts a scan unless one runs, [4] reads the list; the reply is the list so far, the last scan's until this one ends, `scan_reply`), 5 JOIN ([5, name length, name, passphrase]: kept in the vault, then joined), 6 STATUS (`status_reply`), 7 WATCH (the page's held call: the list when it changes, `watch_reply`). The radio sends nothing until asked: no scan runs unless a page watches (one after another while it does) or a kept credential is waiting for its network. Returns a credential to join and, for one handed in, the vault key it was kept under. `skip`: the vault's credential that last failed; it is tried again once a scan sees its network.
fn serve_unjoined(d: &mut Drv, m: &mut Msgbuf, skip: &Option<(Vec<u8>, Vec<u8>, &'static [u8])>) -> (Vec<u8>, Vec<u8>, &'static [u8], bool) {
    d.put(b"no network to join: serving the network page\n");
    d.flush();
    // The corners' WiFi circle: up, no link.
    ferros_sys::status(0, ferros_sys::measure_word(ferros_sys::measure::STATE, 1));
    // The last join's outcome for the network page (measure JOIN), when one failed; nothing before any.
    ferros_sys::status(1, if (1..=3).contains(&m.outcome) { ferros_sys::measure_word(ferros_sys::measure::JOIN, m.outcome as i64) } else { ferros_sys::MEASURE_NONE });
    let mut msg = [0u8; 256];
    let mut out = [0u8; 2048];
    let mut tok = [0u8; 32];
    let mut vault_at = 0u64;
    let mut scanning = false;
    // WATCH calls held until the list changes, and the list as last answered.
    let mut held: Vec<[u8; 32]> = Vec::new();
    let mut held_gen = 0u32;
    let mut last: Vec<u8> = Vec::new();
    // A kept credential that did not join is looked for again after `m.retry_s`, which doubles at each try that finds its network (`connect`).
    let mut retry_at = d.now() + m.retry_s * d.freq;
    loop {
        ferros_sys::pet();
        let _ = m.pump(d);
        if scanning && m.scan_done {
            scanning = false;
            if let Some((name, pass, key)) = skip {
                if m.nets[..m.nnets].iter().any(|e| e.flags & 1 != 0 && &e.name[..e.len as usize] == &name[..]) {
                    // The next wait twice this one, up to 1024 s (the modem's policy): this router throttles repeated joins (2026-10-15: association succeeds, the WPA2 handshake fails, for about ten minutes), and a try every 64 s kept the throttle on.
                    if m.retry_s < 1 << 10 {
                        m.retry_s <<= 1;
                    }
                    watch_answer(d, m, scanning, &mut held);
                    return (name.clone(), pass.clone(), key, false);
                }
            }
        }
        if skip.is_some() && !scanning && d.now() >= retry_at {
            retry_at = d.now() + m.retry_s * d.freq;
            escan_start(d, m);
            scanning = true;
        }
        // The page's held WATCH calls answered once the list moved (a network found, a scan begun or ended).
        if !held.is_empty() {
            watch_count(m, scanning, &mut last);
            if PAGE_GEN.load(core::sync::atomic::Ordering::Relaxed) != held_gen {
                watch_answer(d, m, scanning, &mut held);
            }
        }
        if d.now().wrapping_sub(vault_at) >= d.freq / 4 {
            vault_at = d.now();
            if let Some(c) = stored_cred(d) {
                if skip.as_ref() != Some(&c) {
                    d.put(b"network credential readable: joining\n");
                    d.flush();
                    watch_answer(d, m, scanning, &mut held);
                    return (c.0, c.1, c.2, false);
                }
            }
        }
        let n = svc(2, d.cap(WLAN), msg.as_mut_ptr() as u64 - d.base, msg.len() as u64, tok.as_mut_ptr() as u64 - d.base, 1);
        if n == u64::MAX - 1 || n == ERR {
            if scanning {
                // A scan running: its results come as the firmware's events, read every millisecond (the interrupt is set up only for serving).
                ferros_sys::sleep_until(ferros_sys::now() + d.freq / 1000);
            } else {
                // No scan: nothing comes from the chip unasked, so the wait is for a call (the page's WATCH, a join), the next look at the vault for the credential (a quarter second: nothing rings when a person attests and it becomes readable), or a kept credential's retry (2026-11-13: every millisecond before, the driver's whole time on a device nobody had attested).
                // WHY the min: the earlier of two deadlines, not a bound on a value.
                let until = (vault_at + d.freq / 4).min(if skip.is_some() { retry_at } else { u64::MAX });
                ferros_sys::wait_world_or_call(&IRQ_RUNG, IRQ_RUNG.load(core::sync::atomic::Ordering::Acquire), until);
            }
            continue;
        }
        let msg = unsafe { core::ptr::read_volatile(&msg) };
        let n = n as usize;
        let mut handed = None;
        // 7 WATCH [count u32 LE]: the page's held call (owner, 2026-11-10: no scan button, the list filling as networks are found). Answered at once when the list differs from the caller's count (u32::MAX at the page's opening, never a count given), else held until it changes; a WATCH with nothing new to say starts the next scan, so scans run back to back while a page waits and stop when it closes (it calls no more).
        if msg.first() == Some(&7) && n >= 5 {
            let theirs = u32::from_le_bytes([msg[1], msg[2], msg[3], msg[4]]);
            watch_count(m, scanning, &mut last);
            let now_gen = PAGE_GEN.load(core::sync::atomic::Ordering::Relaxed);
            // WHY the cap: callers are other processes, each held call keeps 32 bytes until the list moves; the page holds one, and past 4 a caller is answered at once with what there is.
            if theirs != now_gen || held.len() >= 4 {
                let len = watch_reply(m, scanning, true, &mut out);
                svc(3, tok.as_ptr() as u64 - d.base, out.as_ptr() as u64 - d.base, len as u64, 0, 0);
            } else {
                if !scanning {
                    escan_start(d, m);
                    scanning = true;
                }
                held.push(tok);
                held_gen = now_gen;
            }
            continue;
        }
        let len = match msg.first() {
            Some(3) => {
                out[..6].copy_from_slice(&m.mac);
                out[6] = 0;
                7
            }
            // [4, 1] starts a scan (unless one runs), [4] or [4, 0] only reads the list (the page watches instead, op 7).
            Some(4) => {
                if n >= 2 && msg[1] == 1 && !scanning {
                    escan_start(d, m);
                    scanning = true;
                }
                m.scan_reply(scanning, &mut out)
            }
            Some(5) if n >= 2 && 2 + msg[1] as usize <= n => {
                let (name, pass) = (&msg[2..2 + msg[1] as usize], &msg[2 + msg[1] as usize..n]);
                // Kept before it is tried, so the join survives a restart; deleted again if it does not join (`connect`). The person's when someone has attested, else the device's.
                match ferros_detail::network::build(name, pass, eagle()) {
                    None => {
                        out[0] = 1;
                    }
                    Some(doc) => {
                        let cap = d.cap(STORE);
                        let key: Option<&'static [u8]> = [&b"cred"[..], b"net"].into_iter().find(|k| ferros_sys::store_set_at(cap, k, 0, &doc, false));
                        match key {
                            Some(k) => {
                                out[0] = 0;
                                out[1] = (k == b"net") as u8;
                                handed = Some((name.to_vec(), pass.to_vec(), k));
                            }
                            None => out[0] = 2,
                        }
                    }
                }
                if handed.is_some() { 2 } else { 1 }
            }
            Some(6) => m.status_reply(if scanning { 1 } else { 0 }, &mut out),
            _ => 0,
        };
        // The join under way, for the page (measure JOIN 0), published before the reply: the page reads the facts, not this driver, until the join ends, and must never see an older outcome as this one's.
        if handed.is_some() {
            ferros_sys::status(1, ferros_sys::measure_word(ferros_sys::measure::JOIN, 0));
        }
        svc(3, tok.as_ptr() as u64 - d.base, out.as_ptr() as u64 - d.base, len as u64, 0, 0);
        if let Some((name, pass, key)) = handed {
            d.put(b"a network handed in by the page: joining\n");
            d.flush();
            watch_answer(d, m, scanning, &mut held);
            return (name, pass, key, true);
        }
    }
}

/// The radio brought down and up again as this network's MAC (`set_mac` needs it down: WLC_DOWN 3, then WLC_UP 2), then a scan for its BSSes.
fn rejoin_as(d: &mut Drv, m: &mut Msgbuf, name: &[u8], personal: bool) {
    m.set_want(name);
    m.personal = personal;
    m.target = None;
    m.band = ferros_sys::controls(ferros_sys::control::WIFI)[ferros_sys::control::WIFI_BAND as usize];
    let req = m.dma + 0x1_3000;
    for k in 0..4 {
        db(req + k, 0);
    }
    let r = m.ioctl(d, 3, req, 4, 4).map(|r| r.0).unwrap_or(u32::MAX);
    d.kv(b"WLC_DOWN", r);
    set_mac(d, m);
    let r = m.ioctl(d, 2, req, 4, 4).map(|r| r.0).unwrap_or(u32::MAX);
    d.kv(b"WLC_UP", r);
    escan_start(d, m);
    escan_wait(d, m);
}

/// Stage 12: join the best scanned BSS of the credential's network with WPA2-PSK, the firmware doing the 4-way handshake (dhd's wl_cfg80211_connect order: sup_wpa, STA defaults, infra, security cleared, wpa_auth provisional → auth → wsec → mfp → wpa_auth final, the passphrase as WLC_SET_WSEC_PMK, our RSN IE as "wpaie", then "join" with the BSSID and chanspec). Done when the firmware's supplicant reports keyed.
fn join(d: &mut Drv, m: &mut Msgbuf, ssid: &[u8], pass: &[u8]) {
    let Some(t) = m.target else {
        m.outcome = 1;
        d.put(b"no WPA2-PSK BSS of that SSID in the scan (outside 6 GHz)\n");
        d.flush();
        return;
    };
    d.put(if t.five { b"joining: 5G ch " } else { b"joining: 2G ch " });
    d.dec(t.ctl as u32);
    d.put(b" rssi -");
    d.dec((-(t.rssi as i32)).max(0) as u32);
    d.put(if t.mfpc { b" (AP can do MFP)\n" } else { b"\n" });
    // Receive buffers first: the firmware runs split RX (SPLITRX_MODE 4), so data frames — the AP's EAPOL handshake messages among them — land in host buffers from the rxpost ring; with none posted the handshake never starts (seen: associated, no PSK_SUP, beacon loss after 15 s). Posted once: a later join (another network, `connect`) finds them there.
    if !m.rx_up {
        for i in 0..RX_BUFS {
            m.post_rx(i);
        }
        m.ring_rx(d);
        m.rx_up = true;
    }
    let r = m.set_int(d, b"sup_wpa", 1);
    d.kv(b"sup_wpa", r);
    let r = m.set_int(d, b"roam_off", 1);
    d.kv(b"roam_off", r);
    let r = m.set_int(d, b"buf_key_b4_m4", 1);
    d.kv(b"buf_key_b4_m4", r);
    let r = m.ioctl_int(d, 86, 0);
    d.kv(b"PM off", r);
    let r = m.ioctl_int(d, 20, 1);
    d.kv(b"infra", r);
    for name in [&b"wsec"[..], b"auth", b"wpa_auth"] {
        let _ = m.set_int(d, name, 0);
    }
    let r = [m.set_int(d, b"wpa_auth", 0xC0), m.set_int(d, b"auth", 0), m.set_int(d, b"wsec", 4), m.set_int(d, b"mfp", t.mfpc as u32), m.set_int(d, b"wpa_auth", 0x80)];
    d.put(b"security wpa_auth/auth/wsec/mfp/wpa_auth:");
    for v in r {
        d.put(b" ");
        d.hex(v);
    }
    d.put(b"\n");
    // wsec_pmk_t: u16 key_len, u16 flags (1 = passphrase), u8 key[256], u16 opt_len — 263 bytes as dhd sends it.
    let req = m.dma + 0x1_3000;
    for k in 0..263 {
        db(req + k, 0);
    }
    for (k, b) in (pass.len() as u16).to_le_bytes().iter().chain(&1u16.to_le_bytes()).chain(pass).enumerate() {
        db(req + k as u32, *b);
    }
    let r = m.ioctl(d, 268, req, 263, 263).map(|r| r.0).unwrap_or(u32::MAX);
    d.kv(b"WSEC_PMK", r);
    for k in 0..263 {
        db(req + k, 0);
    }
    // Our RSN IE: version 1, the AP's group cipher, pairwise CCMP, AKM PSK, capabilities (MFPC if we set mfp 1).
    let mut ie = [0u8; 22];
    ie[..4].copy_from_slice(&[48, 20, 1, 0]);
    ie[4..8].copy_from_slice(&t.group);
    ie[8..20].copy_from_slice(&[1, 0, 0x00, 0x0F, 0xAC, 4, 1, 0, 0x00, 0x0F, 0xAC, 2]);
    ie[20] = if t.mfpc { 0x80 } else { 0 };
    let r = m.set_var(d, b"wpaie", &ie);
    d.kv(b"wpaie", r);
    // join: v0 (wl_extjoin_params_t) unless join_ver says otherwise; ssid (u32 len + 32), scan params (scan_type u8, pad, nprobes, active, passive, home = -1), then the assoc params: BSSID, bssid_cnt 0, one chanspec.
    // A firmware without `join_ver` (akita's BCM4383, 2026-11-09: the query failed and v0 was refused) has its layout told by its `wlc_ver` (wl_cfg80211.c wl_get_join_iovar_ver: v1 from wlc_ver_major 17, or 16.1 and later, or 14.2 and 14.4; v0 before).
    let jv = match m.get_var(d, b"join_ver", req) {
        Some((0, b, _)) => dr32u(m.resp(b) + 4) & 0xFFFF,
        other => {
            d.kv(b"join_ver status", other.map(|o| o.0).unwrap_or(u32::MAX));
            match m.get_var(d, b"wlc_ver", req) {
                Some((0, b, _)) => {
                    let (maj, min) = (dr32u(m.resp(b) + 12) & 0xFFFF, dr32u(m.resp(b) + 12) >> 16);
                    d.kv(b"wlc_ver major", maj);
                    d.kv(b"wlc_ver minor", min);
                    (maj >= 17 || (maj == 16 && min >= 1) || (maj == 14 && (min == 2 || min == 4))) as u32
                }
                _ => 0,
            }
        }
    };
    d.kv(b"join_ver", jv);
    let mut j = [0u8; 88];
    // v3 (akita's BCM4383, wlioctl.h wl_extjoin_params_v3_t): the assoc params gain an MLD address and pad after the BSSID count, so chanspec_num moves to 80 and the list to 84; dhd writes the version into the header (0) and into the assoc params (60) for it (wl_cfg80211.c, WL_JOIN_VERSION_MAJOR_3).
    let (o_ssid, o_scan, o_bssid, o_cnt, o_num, o_list) = match jv {
        0 => (0usize, 36usize, 56usize, 62usize, 64usize, 68usize),
        1 => (4, 40, 64, 70, 72, 76),
        2 => (4, 40, 64, 70, 76, 80),
        _ => (4, 40, 64, 70, 80, 84),
    };
    if jv >= 3 {
        j[0..2].copy_from_slice(&(jv as u16).to_le_bytes());
        j[60..62].copy_from_slice(&(jv as u16).to_le_bytes());
    }
    j[o_ssid..o_ssid + 4].copy_from_slice(&(ssid.len() as u32).to_le_bytes());
    j[o_ssid + 4..o_ssid + 4 + ssid.len()].copy_from_slice(ssid);
    for k in 0..4 {
        let at = o_scan + 4 + 4 * k;
        j[at..at + 4].copy_from_slice(&(-1i32).to_le_bytes());
    }
    j[o_bssid..o_bssid + 6].copy_from_slice(&t.bssid);
    j[o_cnt..o_cnt + 2].copy_from_slice(&0u16.to_le_bytes());
    j[o_num..o_num + 4].copy_from_slice(&1u32.to_le_bytes());
    j[o_list..o_list + 2].copy_from_slice(&(t.chanspec as u16).to_le_bytes());
    m.linked = false;
    m.keyed = false;
    m.join_failed = false;
    let r = m.set_var(d, b"join", &j[..o_list + 2]);
    d.kv(b"join", r);
    d.flush();
    let ts = d.now();
    while !m.keyed && !m.join_failed && d.now().wrapping_sub(ts) < 15 * d.freq {
        let _ = m.pump(d);
        m.pump_rx(d);
        if d.n > 2000 {
            d.flush();
        }
        d.us(2000);
    }
    d.kv(b"join_ms", ((d.now().wrapping_sub(ts) * 1000) / d.freq) as u32);
    d.kv(b"rx frames", m.rx_frames);
    m.outcome = if m.keyed { 4 } else if m.join_failed { 2 } else { 3 };
    if m.keyed {
        d.put(b"ASSOCIATED AND KEYED (WPA2-PSK, firmware supplicant)\n");
        // scb_val_t: i32 val, u8 ea[6], pad — WLC_GET_RSSI (127).
        for k in 0..12 {
            db(req + k, 0);
        }
        if let Some((0, b, _)) = m.ioctl(d, 127, req, 12, 12) {
            let v = dr32u(m.resp(b)) as i32;
            d.put(b"link rssi -");
            d.dec((-v).max(0) as u32);
            d.put(b"\n");
        }
    } else {
        d.put(if m.join_failed { b"join failed (see events)\n" } else { b"join timed out\n" });
    }
    d.flush();
}

/// A blob into the firmware through `clmload` or `txcapload` (dhd_common.c _dhd_apply_default_clm / dhd_download_blob): wl_dload_data_t per 1408-byte chunk, u16 flag (handler version 1 << 12 | DL_BEGIN 2 on the first | DL_END 4 on the last), u16 type 2 (DL_TYPE_CLM, for both), u32 len, u32 crc 0, the data, the chunk padded to 8. The first failing status, or 0.
fn dload(d: &mut Drv, m: &mut Msgbuf, iovar: &[u8], blob: &[u8]) -> u32 {
    let mut off = 0;
    let mut status = 0;
    while off < blob.len() && status == 0 {
        let n = (blob.len() - off).min(1408);
        let mut flag = 1u16 << 12;
        if off == 0 {
            flag |= 2;
        }
        if off + n == blob.len() {
            flag |= 4;
        }
        let mut chunk = [0u8; 1424];
        chunk[0..2].copy_from_slice(&flag.to_le_bytes());
        chunk[2..4].copy_from_slice(&2u16.to_le_bytes());
        chunk[4..8].copy_from_slice(&(n as u32).to_le_bytes());
        chunk[12..12 + n].copy_from_slice(&blob[off..off + n]);
        let total = (12 + n).div_ceil(8) * 8;
        status = m.set_var(d, iovar, &chunk[..total]);
        off += n;
    }
    status
}

/// Stage 11: a scan, the way dhd's up path and wl_run_escan do it. The regulatory blob first, loaded whatever `country` reads (`dload`), then the txcap blob, the escan-result event (69) enabled in `event_msgs_ext`, WLC_UP (2), `scan_ver` to pick the request layout, then `escan` over every channel; results arrive as events, one BSS each, until a final status.
fn scan(d: &mut Drv, m: &mut Msgbuf, clm: &[u8]) {
    radio_up(d, m, clm);
    escan_start(d, m);
    escan_wait(d, m);
}

/// The radio up for scanning and joining (stage 11's first half, `scan`): the regulatory and transmit-power blobs, the events, this network's MAC (`set_mac`), `bus:host_access`, WLC_UP, and the escan layout.
fn radio_up(d: &mut Drv, m: &mut Msgbuf, clm: &[u8]) {
    let req = m.dma + 0x1_3000;
    // The regulatory blob, whatever `country` reads (dhd_common.c dhd_apply_default_clm loads it every time: a firmware with its own table, as akita's BCM4383 a3, would otherwise run its built-in table, not the SKU's), then the transmit-power caps after it (dhd_apply_default_txcap, the same chunks with `txcapload`; dhd only logs its failure). The txcap blob from the vault's `wifi.txcap`, when there is one.
    let country = |d: &mut Drv, m: &mut Msgbuf, when: &[u8]| {
        if let Some((0, b, _)) = m.get_var(d, b"country", req) {
            let cc: [u8; 4] = core::array::from_fn(|k| dbr(m.resp(b) + k as u32));
            d.put(b"country ");
            d.put(when);
            d.put(b": ");
            d.put(&cc[..2]);
            d.put(b"\n");
        }
    };
    country(d, m, b"before CLM");
    if clm.is_empty() {
        d.put(b"no CLM blob given (--input-file clm_blob or the vault's wifi.clm): the firmware's own table stands\n");
    } else {
        let status = dload(d, m, b"clmload", clm);
        d.kv(b"clmload status", status);
        country(d, m, b"after CLM");
    }
    if let Some(txcap) = vault_file(d, b"txcap", 0x13_0000) {
        let status = dload(d, m, b"txcapload", txcap);
        d.kv(b"txcapload status", status);
    }
    // event_msgs_ext: GET the mask (ver 1, command 0, len 27, maxgetsize 27), set bit 69, SET it back (command 3).
    let mut mask = [0u8; 27];
    if let Some((0, b, _)) = m.get_var_arg(d, b"event_msgs_ext", &[1, 0, 27, 27], req) {
        for (k, v) in mask.iter_mut().enumerate() {
            *v = dbr(m.resp(b) + 4 + k as u32);
        }
    }
    for e in [69u32, 0, 3, 5, 6, 7, 11, 12, 16, 46, 136] {
        mask[(e >> 3) as usize] |= 1 << (e & 7);
    }
    let mut ev = [0u8; 31];
    ev[..4].copy_from_slice(&[1, 3, 27, 0]);
    ev[4..].copy_from_slice(&mask);
    let r = m.set_var(d, b"event_msgs_ext", &ev);
    d.kv(b"event_msgs_ext set", r);
    for (k, v) in 0u32.to_le_bytes().iter().enumerate() {
        db(req + k as u32, *v);
    }
    set_mac(d, m);
    // `bus:host_access` = 1 just before WLC_UP, as both Google drivers send it (dhd_cfg80211.c, WLAN_ACCEL_BOOT, built into the BCM4398's and the BCM4383's): the host may reach the dongle's memory. Its status logged; a firmware without it answers an error and nothing else changes.
    let r = m.set_var(d, b"bus:host_access", &1u32.to_le_bytes());
    d.kv(b"bus:host_access", r);
    let r = m.ioctl(d, 2, req, 4, 4).map(|r| r.0).unwrap_or(u32::MAX);
    d.kv(b"WLC_UP", r);
    m.scan_ver = match m.get_var(d, b"scan_ver", req) {
        Some((0, b, _)) => dr32u(m.resp(b) + 4) & 0xFFFF,
        _ => 1,
    };
    d.kv(b"scan_ver", m.scan_ver);
}

/// The MAC to join with, set while the radio is down (`radio_up`, before WLC_UP; `rejoin_as` brings the radio down for another network).
fn set_mac(d: &mut Drv, m: &mut Msgbuf) {
    // Since 2026-11-09 the address is the person's and this device's together (owner: "We mix the ira in with our identity. can't have same address devices on network"): the key is the kernel's person-bound ira key (`ira_person_key`), which exists only while the person's identity is unlocked, so two people on one device, or one person on two devices, never share an address. Before:
    // The MAC to join with, set while the radio is still down: one address per network (owner, 2026-10-31) — the first six bytes of BLAKE3 keyed by this device's ira-derived key over the network's SSID, made a locally administered unicast address (bit 1 of the first byte set, bit 0 clear). The same on a network every time, different on every other, and not linkable between them without the ira. Keyed on the SSID, not the access point's MAC: the SSID is known before the radio comes up, which is when the address must be set, and one network answers from a different MAC on each band and each mesh node. Without the ira right, or with no SSID to join, a random address from the entropy right. Never the chip's own: the calibration file's Broadcom placeholder (00:90:4c:50:10:01, the same on every unit), which pangolin refused from the evening of 2026-10-29 on.
    let mut mac = [0u8; 6];
    let ssid = if m.personal { &m.want[..m.want_len] } else { &[][..] };
    let keyed = match ferros_sys::ira_person_key(d.cap(IRA), "wifi mac per network v2") {
        Some(key) if !ssid.is_empty() => {
            mac.copy_from_slice(&blake3::keyed_hash(&key, ssid).as_bytes()[..6]);
            true
        }
        _ => false,
    };
    if keyed || svc(6, d.cap(ENTROPY), mac.as_mut_ptr() as u64 - d.base, 6, 0, 0) == 6 {
        mac[0] = (mac[0] & 0xFC) | 2;
        let r = m.set_var(d, b"cur_etheraddr", &mac);
        d.kv(if keyed { b"cur_etheraddr set (this network's, the person's and this device's)" } else { b"cur_etheraddr set (random: no network yet, a network joined before anyone attested, or nobody's identity unlocked)" }, r);
        let h = b"0123456789abcdef";
        d.put(b"joining as MAC ");
        for (k, v) in mac.iter().enumerate() {
            d.put(&[h[(v >> 4) as usize], h[(v & 15) as usize]]);
            if k < 5 {
                d.put(b":");
            }
        }
        d.put(b"\n");
    }
}

/// A scan started (`escan`), its results arriving as events (`event`, `bss`): the network list starts over.
fn escan_start(d: &mut Drv, m: &mut Msgbuf) {
    let ver = m.scan_ver;
    m.nseen = 0;
    m.scan_gen = m.scan_gen.wrapping_add(1);
    // escan: u32 version, u16 action 1 (start), u16 sync id, then the scan params — v1 without the version/length prefix; v2/v3 72 bytes, v4 76 (scan_type_ext). Broadcast SSID, BSSID ff…ff, any BSS type, active, firmware-default times, every channel.
    let mut p = [0u8; 84];
    let (hdr_ver, plen): (u32, usize) = match ver {
        2 | 3 => (ver, 72),
        4 => (4, 76),
        _ => (1, 64),
    };
    p[0..4].copy_from_slice(&hdr_ver.to_le_bytes());
    p[4..6].copy_from_slice(&1u16.to_le_bytes());
    p[6..8].copy_from_slice(&0x4F31u16.to_le_bytes());
    let s = 8;
    let o = if hdr_ver == 1 { 0 } else { 4 };
    if hdr_ver != 1 {
        p[s..s + 2].copy_from_slice(&(hdr_ver as u16).to_le_bytes());
        p[s + 2..s + 4].copy_from_slice(&(plen as u16).to_le_bytes());
    }
    p[s + o + 36..s + o + 42].copy_from_slice(&[0xFF; 6]);
    p[s + o + 42] = 2;
    let t = s + if hdr_ver == 1 { 44 } else if hdr_ver == 4 { 56 } else { 52 };
    for k in 0..4 {
        p[t + 4 * k..t + 4 * k + 4].copy_from_slice(&(-1i32).to_le_bytes());
    }
    m.scan_done = false;
    let r = m.set_var(d, b"escan", &p[..8 + plen]);
    d.kv(b"escan", r);
    d.flush();
}

/// The scan waited out (12 s at most), events handled meanwhile; BSSes are printed for the first scan only.
fn escan_wait(d: &mut Drv, m: &mut Msgbuf) {
    let ts = d.now();
    while !m.scan_done && d.now().wrapping_sub(ts) < 12 * d.freq {
        let _ = m.pump(d);
        d.us(2000);
    }
    d.kv(b"BSSes", m.nseen as u32);
    d.kv(b"scan_ms", ((d.now().wrapping_sub(ts) * 1000) / d.freq) as u32);
    d.flush();
    m.quiet = true;
}

/// An RSN IE's body at `rsn`, `l` bytes (version 2, group 4, pairwise count + list, AKM count + list, capabilities): Some(whether the AP can do management frame protection) when it offers PSK (00-0F-AC:2), None otherwise.
fn rsn_psk(rsn: u32, l: u32) -> Option<bool> {
    if l < 8 {
        return None;
    }
    let np = (dbr(rsn + 6) as u32) | (dbr(rsn + 7) as u32) << 8;
    let a = rsn + 8 + 4 * np;
    if a + 2 > rsn + l {
        return None;
    }
    let na = (dbr(a) as u32) | (dbr(a + 1) as u32) << 8;
    let psk = (0..na).any(|k| {
        let s = a + 2 + 4 * k;
        dbr(s) == 0x00 && dbr(s + 1) == 0x0F && dbr(s + 2) == 0xAC && dbr(s + 3) == 2
    });
    let caps = a + 2 + 4 * na;
    psk.then_some(caps + 1 < rsn + l && dbr(caps) & 0x80 != 0)
}

/// bcmutils.c process_nvram_vars, in place: drop '\r', '#' comments to end of line and empty lines; each line ends in a NUL. Returns the length used (the rest is zeroed).
fn process_nvram_vars(v: &mut [u8]) -> usize {
    let (mut dp, mut find_newline, mut column) = (0usize, false, 0usize);
    for n in 0..v.len() {
        let c = v[n];
        if c == b'\r' || (find_newline && c != b'\n') {
            continue;
        }
        find_newline = false;
        if c == b'#' {
            find_newline = true;
            continue;
        }
        if c == b'\n' {
            if column == 0 {
                continue;
            }
            v[dp] = 0;
            dp += 1;
            column = 0;
            continue;
        }
        v[dp] = c;
        dp += 1;
        column += 1;
    }
    for b in &mut v[dp..] {
        *b = 0;
    }
    dp
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}

/// The vault files (nvram, clm, cred), 64 KiB each, in the arc (`vault_file`).
static mut FILES: [[u8; 0x1_0000]; 4] = [[0; 0x1_0000]; 4];
