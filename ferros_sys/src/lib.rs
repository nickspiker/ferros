//! The one place a ferros process touches the machine (KERNEL.md, process ABI): the `svc` syscalls, the generic timer and the process's arc base.
//!
//! Everything else in a driver or stack is plain no_std Rust. Today processes are native aarch64 capsules; when the one backend exists (Rust → WASM → toka → native with compiled masking), this crate becomes WASM imports and the code above it is recompiled, not rewritten (KERNEL.md "Build order").
//!
//! Memory passed to the kernel is named by arc offset (pointer − arc base): every buffer handed to a syscall must live in the process's own arc — its stack or its image — which is all a process has.

#![no_std]

use core::arch::asm;
use core::sync::atomic::{compiler_fence, Ordering};

/// A syscall failed: unknown or wrong token, out-of-range offset, no such object.
pub const ERR: u64 = u64::MAX;
/// A polling `recv` found no message waiting.
pub const NOTHING: u64 = u64::MAX - 1;

/// Syscall numbers (x8).
pub mod nr {
    pub const EXIT: u64 = 0;
    pub const CALL: u64 = 1;
    pub const RECV: u64 = 2;
    pub const REPLY: u64 = 3;
    pub const MMIO_READ: u64 = 4;
    pub const MMIO_WRITE: u64 = 5;
    pub const ENTROPY: u64 = 6;
    pub const DMA: u64 = 7;
    pub const INPUT: u64 = 8;
    pub const SLEEP: u64 = 9;
    pub const SPAWN: u64 = 10;
    pub const SET_PREF: u64 = 11;
    pub const FACTS: u64 = 12;
    pub const STORE: u64 = 13;
    pub const THREAD_SPAWN: u64 = 14;
    pub const THREAD_EXIT: u64 = 15;
    pub const WAIT: u64 = 16;
    pub const WAKE: u64 = 17;
    pub const IRQ_WAIT: u64 = 18;
    pub const IRQ_ACK: u64 = 19;
    pub const PIN: u64 = 20;
}
/// Core masks for `pin` (bit = core index on husky): the A715 workers, where processes run by default, and the X3, the kernel's core, for one real compute path.
pub const CORES_WORKERS: u64 = 0x00F0;
pub const CORES_BIG: u64 = 0x0100;

/// Where the kernel keeps this process's copy of the device preferences (SETTINGS.md): a 32-byte block in the grants page, rewritten by the kernel whenever a preference changes.
pub const PREFS: u64 = 0xFC0;
/// Preference 0: the number base, tatau's code.
pub const PREF_BASE: u64 = 0;
/// Preference 1: names under the map's circles (0 off, 1 on).
pub const PREF_LABELS: u64 = 1;
/// Preference 2: the screen, 0 on, 1 off (the power key toggles it).
pub const PREF_SCREEN_OFF: u64 = 2;
/// Preference 3: brightness in stops below the brightest white (0-8).
pub const PREF_BRIGHTNESS: u64 = 3;
/// Preference 4: the dither (0 spatial and temporal, 1 spatial only).
pub const PREF_DITHER: u64 = 4;

/// A preference byte as this process's copy holds it now.
pub fn pref(index: u64) -> u8 {
    unsafe { core::ptr::read_volatile((arc_base() + PREFS + index.min(31)) as *const u8) }
}

/// The device's number base right now (tatau's code: 10 arabic, 12 DMS, 16 hex, G#8C DLS). Read it at draw time, not once: the person can change it while the app runs.
pub fn device_base() -> u8 {
    unsafe { core::ptr::read_volatile((arc_base() + PREFS + PREF_BASE) as *const u8) }
}

/// A number formatter in the device base, right now: `tatau::count(&mut out, ferros_sys::numbers(Style::Glyphs), n, 1)`. Glyphs where the app draws with the Oxanium +glyphs face, words elsewhere.
pub fn numbers(style: tatau::Style) -> tatau::Fmt {
    tatau::Fmt::new(tatau::Base::from_code(device_base()).unwrap_or(tatau::Base::Dms), style)
}

/// Change a device preference, given the Settings capability: the kernel keeps it in the vault and every app sees it on its next frame. Returns 0, or ERR (no capability, or a value the preference does not take).
pub fn set_pref(cap: u64, index: u64, value: u8) -> u64 {
    svc(nr::SET_PREF, cap, index, value as u64, 0, 0)
}

/// The device's plain facts, for its dashboard (kernel SYS_FACTS); `None` where a fact is not known.
pub struct Facts(pub [u64; 16]);

/// Local wall time: (seconds since 1970 in local time — UTC plus the zone, `device.tz` — and the counter ticks into the current second), or None until the kernel has read the PMIC's clock. From the kernel's base (facts 8, 9) and this process's own counter.
pub fn local_time() -> Option<(i64, u64)> {
    // From the preferences block (bytes 16-31, the kernel's PREF_WALL): no syscall — the first version asked SYS_FACTS on every frame, and facts samples the battery gauge over I2C under the kernel lock (it hung the kernel, 2026-10-23).
    let at = arc_base() + PREFS + 16;
    let rd = |o: u64| unsafe { core::ptr::read_volatile((at + o) as *const u64) };
    let (base, tz) = (rd(0), rd(8) as i64);
    if base == 0 {
        return None;
    }
    let hz = freq();
    let t = base.wrapping_add(now());
    Some(((t / hz) as i64 + tz, t % hz))
}

impl Facts {
    fn get(&self, i: usize) -> Option<u64> {
        Some(self.0[i]).filter(|&v| v != u64::MAX)
    }
    /// From the press at "Ready?" to the shell starting, microseconds.
    pub fn boot_us(&self) -> Option<u64> {
        self.get(0)
    }
    /// The battery's state of charge, per cent.
    pub fn battery_pct(&self) -> Option<u64> {
        self.get(1)
    }
    /// The battery's temperature, hundredths of a degree C.
    pub fn battery_temp_cc(&self) -> Option<i64> {
        self.get(2).map(|v| v as i64)
    }
    /// The cell voltage, microvolts.
    pub fn cell_uv(&self) -> Option<u64> {
        self.get(3)
    }
    /// The battery current, microamps, positive = charging.
    pub fn battery_ua(&self) -> Option<i64> {
        self.get(4).map(|v| v as i64)
    }
    /// Whether a host has the USB link.
    pub fn usb_link(&self) -> Option<bool> {
        self.get(5).map(|v| v != 0)
    }
    /// Processes alive.
    pub fn processes(&self) -> Option<u64> {
        self.get(6)
    }
    /// Keys in the vault.
    pub fn vault_keys(&self) -> Option<u64> {
        self.get(7)
    }
}

/// Read `key` from the vault scope a Store capability names (the key is relative to the scope): Some(the value's whole length, of which at most `buf.len()` bytes were copied), or None if it is absent or the call was refused.
pub fn store_get(cap: u64, key: &[u8], buf: &mut [u8]) -> Option<usize> {
    let r = svc(nr::STORE, cap, (buf.len() as u64) << 8, off(key.as_ptr()), key.len() as u64, off(buf.as_mut_ptr()));
    if r == ERR || r == NOTHING { None } else { Some(r as usize) }
}

/// Write `value` under `key` in a Store capability's vault scope; true when the vault took it.
pub fn store_set(cap: u64, key: &[u8], value: &[u8]) -> bool {
    svc(nr::STORE, cap, 1 | (value.len() as u64) << 8, off(key.as_ptr()), key.len() as u64, off(value.as_ptr())) == 0
}

/// Read the facts now.
pub fn facts() -> Facts {
    let mut b = [0u8; 128];
    svc(nr::FACTS, off(b.as_ptr()), b.len() as u64, 0, 0, 0);
    Facts(core::array::from_fn(|i| u64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().unwrap())))
}

/// The raw trap: number in x8, arguments in x0..x4, result in x0. The kernel may write into any buffer named by the arguments, so nothing is assumed cached across it.
#[inline(always)]
pub fn svc(n: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
    let r: u64;
    compiler_fence(Ordering::SeqCst);
    unsafe { asm!("svc #0", inlateout("x0") a0 => r, in("x1") a1, in("x2") a2, in("x3") a3, in("x4") a4, in("x8") n, options(nostack)) };
    compiler_fence(Ordering::SeqCst);
    r
}

/// Apply the image's own relocations, for a payload linked `-pie` (see payloads/display/payload.ld + build.rs): native payloads run unrelocated at their arc base (KERNEL.md), so any absolute address the linker baked into data — a `dyn` vtable, a function-pointer table, a `&'static str` table — points into nowhere. With `-pie` the linker leaves those words as R_AARCH64_RELATIVE entries in `.rela.dyn` instead, and this adds the load address to each. Call it first in `_entry`, before anything that could touch such data; the loop itself uses only PC-relative addressing. No-op for an image linked without `.rela.dyn`.
///
/// # Safety
/// Must run exactly once, before any other code of the image, from the image this crate was linked into.
#[inline(never)]
pub unsafe fn relocate() {
    unsafe extern "C" {
        static __image_start: u8;
        static __rela_start: u8;
        static __rela_end: u8;
    }
    const R_AARCH64_RELATIVE: u64 = 1027;
    unsafe {
        let base = &raw const __image_start as u64;
        let mut p = &raw const __rela_start as *const u64;
        let end = &raw const __rela_end as *const u64;
        while p < end {
            let (offset, info, addend) = (p.read(), p.add(1).read(), p.add(2).read());
            if info & 0xFFFF_FFFF == R_AARCH64_RELATIVE {
                ((base + offset) as *mut u64).write(base.wrapping_add(addend));
            }
            p = p.add(3);
        }
        asm!("dsb ish", "isb", options(nostack, preserves_flags));
    }
}

/// The process's arc base (x28, set by the kernel at entry and never written by compiled code).
#[inline(always)]
pub fn arc_base() -> u64 {
    let b: u64;
    unsafe { asm!("mov {}, x28", out(reg) b, options(nomem, nostack, preserves_flags)) };
    b
}

/// The generic timer count (EL0 may read it: CNTHCTL_EL2.EL0VCTEN).
#[inline(always)]
pub fn now() -> u64 {
    let t: u64;
    unsafe { asm!("isb", "mrs {}, cntvct_el0", out(reg) t, options(nomem, nostack, preserves_flags)) };
    t
}

/// Timer ticks per second.
#[inline(always)]
pub fn freq() -> u64 {
    let f: u64;
    unsafe { asm!("mrs {}, cntfrq_el0", out(reg) f, options(nomem, nostack, preserves_flags)) };
    f
}

/// Busy-wait `us` microseconds on the timer.
pub fn delay_us(us: u64) {
    let (t0, f) = (now(), freq());
    while now().wrapping_sub(t0) < us * f / 1_000_000 {}
}

/// Microseconds since `t0` (a `now()` value).
pub fn us_since(t0: u64) -> u64 {
    now().wrapping_sub(t0) * 1_000_000 / freq()
}

/// Order this process's writes to device-visible memory (its DMA region) before what follows — typically a doorbell write telling the device to look (`dsb sy`).
#[inline(always)]
pub fn dma_barrier() {
    unsafe { asm!("dsb sy", options(nostack, preserves_flags)) };
}

/// Arc offset of something in this process's arc.
#[inline(always)]
pub fn off<T: ?Sized>(p: *const T) -> u64 {
    p as *const u8 as u64 - arc_base()
}

/// The token for grant `slot`, given the grants offset the kernel passed at entry (x0): a u32 count, then 32-byte tokens; slot 0 is the console.
#[inline(always)]
pub fn cap(grants: u64, slot: u64) -> u64 {
    grants + 4 + 32 * slot
}

/// Start the signed capsule stored in the vault under `key` as a new process called `name`, handing it copies of this process's own capabilities in grant `slots` (they become the child's slots 1.. in that order; slot 0 is always its console). The child is queued; this process carries on. Returns 0, or ERR (the reason is on the console). At most 24 slots.
pub fn spawn(grants: u64, name: &[u8], key: &[u8], slots: &[u64]) -> u64 {
    let mut req = [0u8; 2 + 64 + 64 + 1 + 24 * 32];
    let (nl, kl, n) = (name.len().min(64), key.len().min(64), slots.len().min(24));
    let mut at = 0;
    req[at] = nl as u8;
    req[at + 1..at + 1 + nl].copy_from_slice(&name[..nl]);
    at += 1 + nl;
    req[at] = kl as u8;
    req[at + 1..at + 1 + kl].copy_from_slice(&key[..kl]);
    at += 1 + kl;
    req[at] = n as u8;
    at += 1;
    for &slot in &slots[..n] {
        let tok = (arc_base() + cap(grants, slot)) as *const u8;
        for i in 0..32 {
            req[at + i] = unsafe { core::ptr::read_volatile(tok.add(i)) };
        }
        at += 32;
    }
    svc(nr::SPAWN, off(req.as_ptr()), at as u64, 0, 0, 0)
}

/// End this process; its arc and capabilities cease to exist.
pub fn exit(code: u64) -> ! {
    svc(nr::EXIT, code, 0, 0, 0, 0);
    loop {}
}

/// Call an endpoint (or the console): `msg` goes to the server, its reply lands in `reply`; returns the reply length (ERR on a bad token).
pub fn call(cap: u64, msg: &[u8], reply: &mut [u8]) -> u64 {
    svc(nr::CALL, cap, off(msg.as_ptr()), msg.len() as u64, off(reply.as_mut_ptr()), reply.len() as u64)
}

/// Receive on an endpoint: the message lands in `buf`, a single-use reply token in `token`; returns the message length. With `poll`, returns NOTHING at once when no message waits.
pub fn recv(cap: u64, buf: &mut [u8], token: &mut [u8; 32], poll: bool) -> u64 {
    svc(nr::RECV, cap, off(buf.as_mut_ptr()), buf.len() as u64, off(token.as_mut_ptr()), poll as u64)
}

/// A new thread of this process: `entry(arg)` on `stack` (its top is the thread's SP; give it 16 KiB or more), runnable at once while the caller carries on. Threads share the arc and every capability; they are not addressable (KERNEL.md "Threads, interrupts and cores"). Returns 0 or ERR.
pub fn thread_spawn(entry: extern "C" fn(u64) -> !, stack: &mut [u8], arg: u64) -> u64 {
    let top = unsafe { stack.as_mut_ptr().add(stack.len()) };
    svc(nr::THREAD_SPAWN, off(entry as usize as *const u8), off(top), arg, 0, 0)
}

/// This thread ends with `value`; the last thread of a process ends the process.
pub fn thread_exit(value: u64) -> ! {
    svc(nr::THREAD_EXIT, value, 0, 0, 0, 0);
    loop {
        core::hint::spin_loop();
    }
}

/// Block while `word` still holds `expected`, until another thread of this process calls `wake` on it (0), or the counter reaches `deadline` (2; 0 = none); 1 at once if the word had already changed. A futex with a timeout: the word is the state, this is the wait, and a server loop waits for a request or its next deadline in one call.
pub fn wait(word: &core::sync::atomic::AtomicU64, expected: u64, deadline: u64) -> u64 {
    svc(nr::WAIT, off(word as *const _ as *const u8), expected, deadline, 0, 0)
}

/// Make up to `count` threads waiting on `word` runnable; returns how many.
pub fn wake(word: &core::sync::atomic::AtomicU64, count: u64) -> u64 {
    svc(nr::WAKE, off(word as *const _ as *const u8), count, 0, 0, 0)
}

/// Block until the interrupt line the token grants fires (0), or the counter reaches `deadline` (2; 0 = none). A line that fired while no one waited answers at once. The line stays masked until `irq_ack`; clear the device's own pending bit before acking.
pub fn irq_wait(cap: u64, deadline: u64) -> u64 {
    svc(nr::IRQ_WAIT, cap, deadline, 0, 0, 0)
}

/// The device is handled: unmask its line.
pub fn irq_ack(cap: u64) -> u64 {
    svc(nr::IRQ_ACK, cap, 0, 0, 0, 0)
}

/// The cores this thread may run on from now (`CORES_*`; 0 = the workers). Threads it spawns later inherit it.
pub fn pin(mask: u64) -> u64 {
    svc(nr::PIN, mask, 0, 0, 0, 0)
}

/// Answer a call; the token stops existing.
pub fn reply(token: &[u8; 32], msg: &[u8]) -> u64 {
    svc(nr::REPLY, off(token.as_ptr()), off(msg.as_ptr()), msg.len() as u64, 0, 0)
}

/// Read a 32-bit register of a hardware grant (ERR if the offset is unaligned or outside it).
pub fn mmio_read(cap: u64, offset: u64) -> u64 {
    svc(nr::MMIO_READ, cap, offset, 0, 0, 0)
}

/// Write a 32-bit register of a hardware grant.
pub fn mmio_write(cap: u64, offset: u64, value: u32) -> u64 {
    svc(nr::MMIO_WRITE, cap, offset, value as u64, 0, 0)
}

/// Fill `buf` (≤ 4096 bytes) with randomness from the session's wairua; returns its length, or ERR without the entropy right or a wairua.
pub fn entropy(cap: u64, buf: &mut [u8]) -> u64 {
    svc(nr::ENTROPY, cap, off(buf.as_mut_ptr()), buf.len() as u64, 0, 0)
}

/// The physical base of a DMA region grant (non-cacheable memory the process reads and writes directly, its device's only DMA window).
pub fn dma(cap: u64) -> u64 {
    svc(nr::DMA, cap, 0, 0, 0, 0)
}

/// Copy spawn input into `buf`, starting at byte `off`; returns how many bytes (0 at the end). Large one-shot input (firmware) is streamed this way so the arc stays small.
pub fn input(start: u64, buf: &mut [u8]) -> usize {
    svc(nr::INPUT, start, off(buf.as_mut_ptr() as *const u8), buf.len() as u64, 0, 0) as usize
}

/// Sleep until the generic timer count reaches `deadline` (see `now`/`freq`): the process is parked, not scheduled, until then.
pub fn sleep_until(deadline: u64) {
    svc(nr::SLEEP, deadline, 0, 0, 0, 0);
}

/// Say something on the console (grant slot 0).
pub fn console(grants: u64, text: &[u8]) {
    let mut r = [0u8; 16];
    call(cap(grants, 0), text, &mut r);
}

/// What a payload's panic handler calls: exit with G#DEAD (the kernel prints `name: exit G#…DEAD`).
pub fn panic_exit() -> ! {
    exit(0xDEAD)
}
