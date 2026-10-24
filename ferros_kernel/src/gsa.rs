//! GSA, Google's security processor, through its non-secure mailbox (zuma-gsa.dtsi: G#1649_0000; protocol from soc-gs `drivers/soc/google/gsa/gsa_mbox.c`). Sixteen shared registers: a request is [cmd, argc, args...], the doorbell is bit 0 of INTGR1, the answer [cmd | 1<<31, err, argc, args...] is flagged in bit 0 of INTSR0. Polled, no interrupt.
//!
//! Only read-only commands are wired: the echo test and the AoC's state. GSA also holds keys (KDN), the secure-JTAG controls and firmware loading; none of those is reachable from here.

use core::ptr::{read_volatile, write_volatile};

pub const MBOX: usize = 0x1649_0000;
const INTCR0: usize = 0x24;
const INTMR0: usize = 0x28;
const INTSR0: usize = 0x2C;
const INTGR1: usize = 0x40;
const SR: usize = 0x80;

pub const CMD_ECHO: u32 = 34;
pub const CMD_AOC: u32 = 91;
/// `enum gsa_aoc_cmd` (gsa_aoc.h): 0 get state; the answer is `enum gsa_aoc_state`, 0 inactive (no firmware, in reset), 1 loaded (in reset), 2 running.
pub const AOC_GET_STATE: u32 = 0;

fn rd(o: usize) -> u32 {
    unsafe { read_volatile((MBOX + o) as *const u32) }
}
fn wr(o: usize, v: u32) {
    unsafe { write_volatile((MBOX + o) as *mut u32, v) }
}

/// One exchange: Ok((response cmd, err, args)) or Err("timeout") after `ms`.
pub fn call(cmd: u32, args: &[u32], ms: u64) -> Result<(u32, u32, alloc::vec::Vec<u32>), &'static str> {
    if args.len() + 2 > 16 {
        return Err("too many args");
    }
    wr(SR, cmd);
    wr(SR + 4, args.len() as u32);
    for (i, &a) in args.iter().enumerate() {
        wr(SR + 8 + 4 * i, a);
    }
    wr(INTCR0, 1);
    wr(INTMR0, rd(INTMR0) & !1);
    unsafe { core::arch::asm!("dsb sy") };
    wr(INTGR1, 1);
    let f = crate::platform::cntfrq();
    let t0 = crate::platform::cntpct();
    while rd(INTSR0) & 1 == 0 {
        if crate::platform::cntpct() - t0 > f / 1000 * ms {
            wr(INTMR0, rd(INTMR0) | 1);
            return Err("timeout");
        }
    }
    let (c, e, n) = (rd(SR), rd(SR + 4), rd(SR + 8).min(13));
    let out = (0..n as usize).map(|i| rd(SR + 12 + 4 * i)).collect();
    wr(INTCR0, 1);
    wr(INTMR0, rd(INTMR0) | 1);
    Ok((c, e, out))
}
