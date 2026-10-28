//! Where 64 bytes (the ira and the wairua) could live off DRAM: a probe of the CPU's own registers that nothing in ferros uses (KEY_REGISTERS.md, "Off DRAM"). Development only: it writes test patterns, never a secret, and puts every register back as it found it.
//!
//! Two banks, so a secret can be held split (ira = bank 0 XOR bank 1, neither half alone being it):
//! - bank 0: the pointer-authentication keys APIA, APIB, APDA and APDB, 128 bits each = 64 bytes exactly. ferros signs no pointers, and with SCTLR_EL2's EnIA/EnIB/EnDA/EnDB clear every instruction that would use them leaves its value as it was, at EL2 and at EL0 (one SCTLR for the EL2&0 regime). APGA, the fifth key, has no such enable, so it is reported but never a home.
//! - bank 1: the EL1 registers, dead under VHE with no guest (HCR_EL2.TGE: nothing runs at EL1, and EL0's timer and thread registers are the EL2 ones). None is reachable from EL0.
//! - `diag keyreg dbg` adds the breakpoint and watchpoint value registers (TRESOR's trick, on ARM), kept apart because EL3 may trap debug register access.
//!
//! For each register: the bits that hold what is written (alternating and walking-bit patterns), the bits that come back unchanged across an SMC into EL3, and a second command (`diag keyreg check`) to read back what the first left, after the core has slept. Then the cost of fetching 64 bytes: from RAM, from bank 0, and split across both banks, in the core's own cycles. Run on the X3 (the kernel's core) and on the first A715 worker.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU32, Ordering};

/// A candidate register: its name, a reader and a writer.
pub struct Reg {
    pub name: &'static str,
    rd: fn() -> u64,
    wr: fn(u64),
}

macro_rules! reg {
    ($name:literal, $sr:literal) => {{
        fn rd() -> u64 {
            let v: u64;
            unsafe { core::arch::asm!(concat!("mrs {}, ", $sr), out(reg) v, options(nostack, nomem)) };
            v
        }
        fn wr(v: u64) {
            unsafe { core::arch::asm!(concat!("msr ", $sr, ", {}"), "isb", in(reg) v, options(nostack, nomem)) };
        }
        Reg { name: $name, rd, wr }
    }};
}

/// Bank 0: the four pointer-authentication keys a pointer instruction can switch off, low half then high half. Bank 0 ends at index 8; APGA follows, reported only.
const BANK0: usize = 8;
/// Bank 1 starts at index 10.
const BANK1: usize = 10;
/// The two EL1 timer compare values: written only while their timer is off (CTL bit 0), so no interrupt can fire from a pattern.
const CNTP: usize = BANK1 + 4;
const CNTV: usize = BANK1 + 5;

pub static REGS: [Reg; N_REGS] = [
    reg!("APIAKeyLo", "S3_0_C2_C1_0"),
    reg!("APIAKeyHi", "S3_0_C2_C1_1"),
    reg!("APIBKeyLo", "S3_0_C2_C1_2"),
    reg!("APIBKeyHi", "S3_0_C2_C1_3"),
    reg!("APDAKeyLo", "S3_0_C2_C2_0"),
    reg!("APDAKeyHi", "S3_0_C2_C2_1"),
    reg!("APDBKeyLo", "S3_0_C2_C2_2"),
    reg!("APDBKeyHi", "S3_0_C2_C2_3"),
    reg!("APGAKeyLo", "S3_0_C2_C3_0"),
    reg!("APGAKeyHi", "S3_0_C2_C3_1"),
    reg!("TPIDR_EL1", "tpidr_el1"),
    reg!("SP_EL1", "sp_el1"),
    reg!("ELR_EL12", "S3_5_C4_C0_1"),
    reg!("FAR_EL12", "S3_5_C6_C0_0"),
    reg!("CNTP_CVAL_EL02", "S3_5_C14_C2_2"),
    reg!("CNTV_CVAL_EL02", "S3_5_C14_C3_2"),
    reg!("PAR_EL1", "par_el1"),
    reg!("TPIDR_EL2", "tpidr_el2"),
    reg!("SPSR_EL12", "S3_5_C4_C0_0"),
    reg!("ESR_EL12", "S3_5_C5_C2_0"),
    reg!("CONTEXTIDR_EL12", "S3_5_C13_C0_1"),
    reg!("VBAR_EL12", "S3_5_C12_C0_0"),
    reg!("MAIR_EL12", "S3_5_C10_C2_0"),
    reg!("AMAIR_EL12", "S3_5_C10_C3_0"),
    reg!("AFSR0_EL12", "S3_5_C5_C1_0"),
    reg!("AFSR1_EL12", "S3_5_C5_C1_1"),
];
const N_REGS: usize = 26;

/// The breakpoint (6 on the X3 and A715) and watchpoint (4) value registers, `diag keyreg dbg` only. Each pair's control register is read first: one that is enabled is skipped.
pub static DBG: [(Reg, Reg); 10] = [
    (reg!("DBGBVR0", "S2_0_C0_C0_4"), reg!("DBGBCR0", "S2_0_C0_C0_5")),
    (reg!("DBGBVR1", "S2_0_C0_C1_4"), reg!("DBGBCR1", "S2_0_C0_C1_5")),
    (reg!("DBGBVR2", "S2_0_C0_C2_4"), reg!("DBGBCR2", "S2_0_C0_C2_5")),
    (reg!("DBGBVR3", "S2_0_C0_C3_4"), reg!("DBGBCR3", "S2_0_C0_C3_5")),
    (reg!("DBGBVR4", "S2_0_C0_C4_4"), reg!("DBGBCR4", "S2_0_C0_C4_5")),
    (reg!("DBGBVR5", "S2_0_C0_C5_4"), reg!("DBGBCR5", "S2_0_C0_C5_5")),
    (reg!("DBGWVR0", "S2_0_C0_C0_6"), reg!("DBGWCR0", "S2_0_C0_C0_7")),
    (reg!("DBGWVR1", "S2_0_C0_C1_6"), reg!("DBGWCR1", "S2_0_C0_C1_7")),
    (reg!("DBGWVR2", "S2_0_C0_C2_6"), reg!("DBGWCR2", "S2_0_C0_C2_7")),
    (reg!("DBGWVR3", "S2_0_C0_C3_6"), reg!("DBGWCR3", "S2_0_C0_C3_7")),
];

/// One core's findings.
#[derive(Clone, Copy)]
pub struct Found {
    pub midr: u64,
    /// SCTLR_EL2 and HCR_EL2 as the core runs (the pointer-key enables, bits 31/30/27/13; TGE, E2H, API, APK).
    pub sctlr: u64,
    pub hcr: u64,
    /// Per register: the bits that hold what was written; the bits unchanged across an SMC; and bit k of `skipped` set when register k was left alone (a live timer, an enabled debug pair).
    pub holds: [u64; N_REGS],
    pub smc: [u64; N_REGS],
    pub skipped: u32,
    /// Per register: cycles for 1<<12 reads (through a function pointer, so the PAC keys' figure is the floor).
    pub rd_cyc: [u64; N_REGS],
    pub dbg_holds: [u64; 10],
    pub dbg_skipped: u32,
    /// PACIA on a pointer with a test key in APIA: what came out (equal to the pointer = switched off). PACGA with a test key: the code it made (non-zero = the generic key computes).
    pub pacia_in: u64,
    pub pacia_out: u64,
    pub pacga: u64,
    /// Cycles for 1<<16 fetches of 64 bytes: from RAM (an L1-hot static), from bank 0 alone, and split (bank 0 XOR the fast full-width registers: TPIDR_EL1, SP_EL1, ELR_EL12, FAR_EL12, MAIR_EL12, APGA, TPIDR_EL2; the EL1 timer compares read ~30x slower, so they are left out).
    pub cyc_ram: u64,
    pub cyc_one: u64,
    pub cyc_split: u64,
    /// What `check` found: per register, the bits still holding the tag `run` left there.
    pub kept: [u64; N_REGS],
    pub kept_valid: bool,
}

const NONE: Found = Found { midr: 0, sctlr: 0, hcr: 0, holds: [0; N_REGS], smc: [0; N_REGS], skipped: 0, rd_cyc: [0; N_REGS], dbg_holds: [0; 10], dbg_skipped: 0, pacia_in: 0, pacia_out: 0, pacga: 0, cyc_ram: 0, cyc_one: 0, cyc_split: 0, kept: [0; N_REGS], kept_valid: false };

struct Slots(UnsafeCell<[Found; 2]>, UnsafeCell<[[u64; N_REGS]; 2]>);
// SAFETY: slot k is written only by the core serving request k, between ASK being set and DONE being set; the asker reads it only after DONE (Acquire). Slot 0 is the X3's, 1 the worker's.
unsafe impl Sync for Slots {}
/// The findings, and the values each register held before `run` tagged it (put back by `check`).
static SLOTS: Slots = Slots(UnsafeCell::new([NONE; 2]), UnsafeCell::new([[0; N_REGS]; 2]));

/// A request to worker 0: 1 run, 2 run with debug registers, 3 check. Cleared by the worker when it is done.
static ASK: AtomicU32 = AtomicU32::new(0);

/// The tag `run` leaves in register k on a core: distinct per register, per core, and per half, so a stale or swapped value shows.
fn tag(k: usize, core: usize) -> u64 {
    0xA5C3_0F96_5A3C_F069u64.rotate_left((k * 5 + core * 29) as u32) ^ (k as u64) << 56
}

/// Which bits of a register hold what is written.
fn holds(r: &Reg) -> u64 {
    let mut ok = !0u64;
    let mut try_one = |p: u64| {
        (r.wr)(p);
        ok &= !((r.rd)() ^ p);
    };
    for p in [0, !0, 0x5555_5555_5555_5555, 0xAAAA_AAAA_AAAA_AAAA] {
        try_one(p);
    }
    for b in 0..64 {
        try_one(1 << b);
        try_one(!(1u64 << b));
    }
    ok
}

/// PSCI_VERSION: a call into EL3 that changes nothing.
fn smc_noop() {
    unsafe { core::arch::asm!("smc #0", inout("x0") 0x8400_0000u64 => _, out("x1") _, out("x2") _, out("x3") _, options(nostack)) };
}

fn cycles() -> u64 {
    let c: u64;
    unsafe { core::arch::asm!("isb", "mrs {}, pmccntr_el0", out(reg) c, options(nostack, nomem)) };
    c
}

#[inline(always)]
fn fetch_one() -> u64 {
    let (a, b, c, d, e, f, g, h): (u64, u64, u64, u64, u64, u64, u64, u64);
    unsafe {
        core::arch::asm!(
            "mrs {a}, S3_0_C2_C1_0", "mrs {b}, S3_0_C2_C1_1", "mrs {c}, S3_0_C2_C1_2", "mrs {d}, S3_0_C2_C1_3",
            "mrs {e}, S3_0_C2_C2_0", "mrs {f}, S3_0_C2_C2_1", "mrs {g}, S3_0_C2_C2_2", "mrs {h}, S3_0_C2_C2_3",
            a = out(reg) a, b = out(reg) b, c = out(reg) c, d = out(reg) d, e = out(reg) e, f = out(reg) f, g = out(reg) g, h = out(reg) h,
            options(nostack, nomem),
        )
    };
    a ^ b ^ c ^ d ^ e ^ f ^ g ^ h
}

#[inline(always)]
fn fetch_split() -> u64 {
    let (a, b, c, d, e, f, g, h): (u64, u64, u64, u64, u64, u64, u64, u64);
    unsafe {
        core::arch::asm!(
            "mrs {a}, S3_0_C2_C1_0", "mrs {t}, tpidr_el1", "eor {a}, {a}, {t}",
            "mrs {b}, S3_0_C2_C1_1", "mrs {t}, sp_el1", "eor {b}, {b}, {t}",
            "mrs {c}, S3_0_C2_C1_2", "mrs {t}, S3_5_C4_C0_1", "eor {c}, {c}, {t}",
            "mrs {d}, S3_0_C2_C1_3", "mrs {t}, S3_5_C6_C0_0", "eor {d}, {d}, {t}",
            "mrs {e}, S3_0_C2_C2_0", "mrs {t}, S3_5_C10_C2_0", "eor {e}, {e}, {t}",
            "mrs {f}, S3_0_C2_C2_1", "mrs {t}, S3_0_C2_C3_0", "eor {f}, {f}, {t}",
            "mrs {g}, S3_0_C2_C2_2", "mrs {t}, S3_0_C2_C3_1", "eor {g}, {g}, {t}",
            "mrs {h}, S3_0_C2_C2_3", "mrs {t}, tpidr_el2", "eor {h}, {h}, {t}",
            a = out(reg) a, b = out(reg) b, c = out(reg) c, d = out(reg) d, e = out(reg) e, f = out(reg) f, g = out(reg) g, h = out(reg) h, t = out(reg) _,
            options(nostack, nomem),
        )
    };
    a ^ b ^ c ^ d ^ e ^ f ^ g ^ h
}

/// The probe, on this core, with IRQs masked throughout (nothing else runs here while the registers hold patterns). `core` is 0 for the X3, 1 for the worker; `dbg` adds the debug registers.
fn run_here(core: usize, dbg: bool) {
    let found = unsafe { &mut (*SLOTS.0.get())[core] };
    let saved = unsafe { &mut (*SLOTS.1.get())[core] };
    *found = NONE;
    unsafe {
        core::arch::asm!("msr daifset, #0xF", options(nostack));
        core::arch::asm!("mrs {}, midr_el1", "mrs {}, sctlr_el2", "mrs {}, hcr_el2", out(reg) found.midr, out(reg) found.sctlr, out(reg) found.hcr, options(nostack, nomem));
    }
    let ctl = |sr: usize| -> u64 {
        let v: u64;
        unsafe {
            if sr == CNTP {
                core::arch::asm!("mrs {}, S3_5_C14_C2_1", out(reg) v, options(nostack, nomem));
            } else {
                core::arch::asm!("mrs {}, S3_5_C14_C3_1", out(reg) v, options(nostack, nomem));
            }
        }
        v
    };
    for (k, r) in REGS.iter().enumerate() {
        if (k == CNTP || k == CNTV) && ctl(k) & 1 != 0 {
            found.skipped |= 1 << k;
            continue;
        }
        let was = (r.rd)();
        let t0 = cycles();
        let mut x = 0;
        for _ in 0..1 << 12 {
            x ^= (core::hint::black_box(r.rd))();
        }
        found.rd_cyc[k] = cycles() - t0;
        core::hint::black_box(x);
        found.holds[k] = holds(r);
        // Across an SMC: EL3 is entered and left; what it saves and restores (or does not) shows here.
        let t = tag(k, core);
        (r.wr)(t);
        smc_noop();
        found.smc[k] = !((r.rd)() ^ t);
        (r.wr)(was);
    }
    if dbg {
        for (k, (v, c)) in DBG.iter().enumerate() {
            if (c.rd)() & 1 != 0 {
                found.dbg_skipped |= 1 << k;
                continue;
            }
            let was = (v.rd)();
            found.dbg_holds[k] = holds(v);
            (v.wr)(was);
        }
    }
    // The pointer instructions with test keys in APIA and APGA: does the switched-off key change a pointer, and does the generic key compute?
    let (ia, ga) = ([(REGS[0].rd)(), (REGS[1].rd)()], [(REGS[8].rd)(), (REGS[9].rd)()]);
    (REGS[0].wr)(0x0123_4567_89AB_CDEF);
    (REGS[1].wr)(0xFEDC_BA98_7654_3210);
    (REGS[8].wr)(0x0F1E_2D3C_4B5A_6978);
    (REGS[9].wr)(0x8796_A5B4_C3D2_E1F0);
    found.pacia_in = 0x0000_7F00_1234_5670;
    let (mut p, g): (u64, u64);
    p = found.pacia_in;
    unsafe {
        core::arch::asm!(".inst 0xdac10020", inout("x0") p, in("x1") 0x42u64, options(nostack, nomem)); // pacia x0, x1
        core::arch::asm!(".inst 0x9ac23020", out("x0") g, in("x1") 0x0000_7F00_1234_5670u64, in("x2") 0x42u64, options(nostack, nomem)); // pacga x0, x1, x2
    }
    found.pacia_out = p;
    found.pacga = g;
    (REGS[0].wr)(ia[0]);
    (REGS[1].wr)(ia[1]);
    (REGS[8].wr)(ga[0]);
    (REGS[9].wr)(ga[1]);
    // The cost of 64 bytes. Bank 1's timer compares are only read here, so a live timer costs nothing.
    static RAM: [u64; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
    const N: usize = 1 << 16;
    let mut sink = 0u64;
    let t0 = cycles();
    for _ in 0..N {
        let mut x = 0;
        for w in &RAM {
            x ^= unsafe { core::ptr::read_volatile(w) };
        }
        sink ^= x;
    }
    let t1 = cycles();
    for _ in 0..N {
        sink ^= fetch_one();
    }
    let t2 = cycles();
    for _ in 0..N {
        sink ^= fetch_split();
    }
    let t3 = cycles();
    core::hint::black_box(sink);
    found.cyc_ram = t1 - t0;
    found.cyc_one = t2 - t1;
    found.cyc_split = t3 - t2;
    // Leave a tag in every register written above, for `check` to read after this core has slept; what was there goes into `saved`.
    for (k, r) in REGS.iter().enumerate() {
        if found.skipped & (1 << k) != 0 {
            continue;
        }
        saved[k] = (r.rd)();
        (r.wr)(tag(k, core));
    }
    unsafe { core::arch::asm!("msr daifclr, #0xF", options(nostack)) };
}

/// Read back the tags `run` left, and put the registers back.
fn check_here(core: usize) {
    let found = unsafe { &mut (*SLOTS.0.get())[core] };
    let saved = unsafe { &(*SLOTS.1.get())[core] };
    unsafe { core::arch::asm!("msr daifset, #0xF", options(nostack)) };
    for (k, r) in REGS.iter().enumerate() {
        if found.skipped & (1 << k) != 0 {
            continue;
        }
        found.kept[k] = !((r.rd)() ^ tag(k, core));
        (r.wr)(saved[k]);
    }
    found.kept_valid = true;
    unsafe { core::arch::asm!("msr daifclr, #0xF", options(nostack)) };
}

/// Worker 0's side: called from its idle loop on every pass (smp.rs). One load when nothing is asked.
pub fn serve(k: usize) {
    if k != 0 {
        return;
    }
    let a = ASK.load(Ordering::Acquire);
    if a == 0 {
        return;
    }
    match a {
        3 => check_here(1),
        _ => run_here(1, a == 2),
    }
    ASK.store(0, Ordering::Release);
}

/// Ask worker 0 and wait for it (a quarter second at most). False if it never answered (no workers: FERROS_SMP=0, or it is busy in a slice).
fn ask_worker(what: u32) -> bool {
    if crate::smp::UP.load(Ordering::Acquire) & 1 == 0 {
        return false;
    }
    ASK.store(what, Ordering::Release);
    crate::process::kick(0xFFFF);
    let f = crate::platform::cntfrq();
    let t0 = crate::platform::cntpct();
    while ASK.load(Ordering::Acquire) != 0 {
        if crate::platform::cntpct() - t0 > f / 4 {
            ASK.store(0, Ordering::Release);
            return false;
        }
        core::hint::spin_loop();
    }
    true
}

/// The PMU's information cells (pmu_alive G#1546_0000): INFORM0 at G#800 (Linux's c2 allow, exynos-cpupm.h), SYSIP_DAT0 at G#810 (the reboot mode), CPU_INFORM 0-8 at G#860-880 (each core's requested power mode for the firmware, flexpmu_cal_cpu_zuma.h). Read only: G#860-880 are live firmware state (a tag written to CPU4's cell lost a bit when the core was cycled, 2026-10-28) and the INFORM cells exist to survive resets, so none is a home for a secret (KEY_REGISTERS.md).
const PMU_INFORM: u64 = 0x1546_0800;
const PMU_INFORM_CELLS: usize = 64;

/// `diag keyreg ao`: read only. Every cell of the window through the EL3 monitor (a direct PMU read on husky hangs the next EL3 call), eight to a line; ERR where the monitor refused.
fn ao_survey(out: &mut alloc::vec::Vec<u8>) {
    for row in 0..PMU_INFORM_CELLS / 8 {
        let base = PMU_INFORM + (row * 32) as u64;
        out.extend_from_slice(alloc::format!("G#{:04X}:", base & 0xFFFF).as_bytes());
        for c in 0..8 {
            let v = crate::platform::priv_reg(base + c * 4, false, 0);
            if (v as i64) < 0 {
                out.extend_from_slice(b" ERR     ");
            } else {
                out.extend_from_slice(alloc::format!(" {:08X}", v as u32).as_bytes());
            }
        }
        out.push(b'\n');
    }
}

/// `diag keyreg [dbg]` and `diag keyreg check`: the probe on this core (the X3) and on worker 0, reported. `diag keyreg ao`: the always-on survey above.
pub fn diag(params: &[u8], out: &mut alloc::vec::Vec<u8>) {
    if params == b"ao" {
        ao_survey(out);
        return;
    }
    let check = params == b"check";
    let dbg = params == b"dbg";
    if check {
        check_here(0);
    } else {
        run_here(0, dbg);
    }
    let worker = ask_worker(if check { 3 } else if dbg { 2 } else { 1 });
    let slots = unsafe { &*SLOTS.0.get() };
    for (core, f) in slots.iter().enumerate() {
        if core == 1 && !worker {
            out.extend_from_slice(b"worker 0: no answer\n");
            continue;
        }
        let who = if core == 0 { "X3 (kernel)" } else { "A715 worker 0" };
        out.extend_from_slice(alloc::format!("{who}: MIDR G#{:X} SCTLR_EL2 G#{:X} HCR_EL2 G#{:X}\n", f.midr, f.sctlr, f.hcr).as_bytes());
        if check {
            if !f.kept_valid {
                out.extend_from_slice(b"  nothing to check: run `diag keyreg` first\n");
                continue;
            }
            for (k, r) in REGS.iter().enumerate() {
                if f.skipped & (1 << k) != 0 {
                    continue;
                }
                out.extend_from_slice(alloc::format!("  {:<16} kept G#{:016X}{}\n", r.name, f.kept[k], if f.kept[k] == !0 { "" } else { "  LOST BITS" }).as_bytes());
            }
            continue;
        }
        out.extend_from_slice(alloc::format!("  pointer keys: EnIA {} EnIB {} EnDA {} EnDB {}; HCR TGE {} E2H {} API {} APK {}\n", f.sctlr >> 31 & 1, f.sctlr >> 30 & 1, f.sctlr >> 27 & 1, f.sctlr >> 13 & 1, f.hcr >> 27 & 1, f.hcr >> 34 & 1, f.hcr >> 41 & 1, f.hcr >> 40 & 1).as_bytes());
        out.extend_from_slice(alloc::format!("  PACIA G#{:X} -> G#{:X} ({})  PACGA -> G#{:X} ({})\n", f.pacia_in, f.pacia_out, if f.pacia_in == f.pacia_out { "off" } else { "SIGNS" }, f.pacga, if f.pacga == 0 { "zero" } else { "computes" }).as_bytes());
        for (k, r) in REGS.iter().enumerate() {
            let bank = if k < BANK0 { "0" } else if k < BANK1 { "-" } else { "1" };
            if f.skipped & (1 << k) != 0 {
                out.extend_from_slice(alloc::format!("  {bank} {:<16} skipped (timer on)\n", r.name).as_bytes());
                continue;
            }
            out.extend_from_slice(alloc::format!("  {bank} {:<16} holds G#{:016X} smc G#{:016X} read A#{:>6}{}\n", r.name, f.holds[k], f.smc[k], f.rd_cyc[k], if f.holds[k] == !0 && f.smc[k] == !0 { "  full" } else { "" }).as_bytes());
        }
        if dbg {
            for (k, (v, _)) in DBG.iter().enumerate() {
                if f.dbg_skipped & (1 << k) != 0 {
                    out.extend_from_slice(alloc::format!("    {:<16} skipped (enabled)\n", v.name).as_bytes());
                } else {
                    out.extend_from_slice(alloc::format!("    {:<16} holds G#{:016X}\n", v.name, f.dbg_holds[k]).as_bytes());
                }
            }
        }
        out.extend_from_slice(alloc::format!("  cycles for G#10000 fetches of 64 bytes: RAM A#{}  bank 0 A#{}  split A#{}\n", f.cyc_ram, f.cyc_one, f.cyc_split).as_bytes());
    }
}
