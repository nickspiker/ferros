//! The kernel heap's window (docs/NO-CEILINGS.md 1, step 4): past the two fixed regions (main.rs `HEAP_SIZE`, cachemap `HEAP2`), a reach of addresses as large as the pool, EL2-only, into which frames of the pool are mapped at the top as the heap runs low, and from whose top the free part past a cushion goes back to the pool.
//! So one allocation can hold what the device holds (a 64 MiB value, a staged image, a call's message), and a large one made once is not held for the boot.
//!
//! The window is the first one the windows' buddy gives at `open` (process.rs `window_take`), its level-1 and level-2 tables made there whole (cachemap `tables_ready`), so a growth writes level-2 block entries and nothing else; each 2 MiB block is a frame of the pool mapped Normal write-back, the kernel's alone, never executable (cachemap `kheap_map`).
//! Its addresses are not physical: decision 8's check (docs/NO-CEILINGS.md "The kernel heap and DMA") is that no kernel heap buffer is ever handed to a device, the secure world or code running with the MMU off by its address.
//!
//! Who may grow it, and when:
//! - Every change to the window is under the kernel lock (its top, its blocks, the frames): growths and trims are serialized, and the frames pool is the kernel lock's as always (frames.rs).
//! - The allocator's own growth (`grow`, the hook `LockedTlsf::set_grow` calls with the heap's lock let go) takes the kernel lock itself, so only on a core holding no lock at all: never under a storage lock (the rank order, process.rs `Rank`: a core holding one never takes the kernel lock), and never under the kernel lock, where the allocation may sit in the middle of a frames count and its takes (process.rs `arc_resize`), which a growth taking frames would break.
//! - Under the kernel lock, code that takes something sized by someone else (a call's message, a stored value, a staged image) asks for room first at a point where no frames work is under way (`room`), then reserves fallibly and answers "kernel out of memory" when even that fails.
//! - Everything else (the kernel's own records under its lock or a storage lock) is served from the fixed regions and what the window holds, as before step 4.

use core::alloc::Layout;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};

use crate::frames::FRAME;

/// The window: its first address and its size (the pool's bytes rounded up to a power of two), 0 until `open`.
static BASE: AtomicUsize = AtomicUsize::new(0);
static REACH: AtomicUsize = AtomicUsize::new(0);
/// Bytes of the window mapped from its base. Written under the kernel lock.
static MAPPED: AtomicUsize = AtomicUsize::new(0);
/// The most it has held, and the growths refused (the pool empty, or a core that may not grow).
static PEAK: AtomicUsize = AtomicUsize::new(0);
static REFUSED: AtomicUsize = AtomicUsize::new(0);
/// Per core: inside a growth (a growth's own allocation, the frames' queue of `give`, never grows again).
static GROWING: [AtomicBool; 16] = [const { AtomicBool::new(false) }; 16];

/// Free bytes the window keeps at its top when it gives back (two frames): a burst of small allocations past the fixed regions does not grow and trim it frame by frame.
const CUSHION: usize = 2 * FRAME;

/// The window opened, from the service loop once the heap's second region has joined (main.rs `heap2_join`), so the window is the heap's last region and grows as one. Then the allocator's hook. Takes the kernel lock.
pub fn open() {
    if BASE.load(Relaxed) != 0 {
        return;
    }
    let reach = crate::frames::map_bytes().next_power_of_two();
    let opened = crate::process::with_lock(|| {
        let base = crate::process::window_take(reach)?;
        // The tables of the whole reach at once: one level-2 table a GiB (16 on the Pro, 64 KiB), so a growth never takes a frame for a table.
        if !crate::cachemap::tables_ready(base, base + reach) {
            crate::process::window_give(base, reach);
            return None;
        }
        Some(base)
    });
    match opened {
        Some(base) => {
            REACH.store(reach, Relaxed);
            BASE.store(base, Relaxed);
            crate::KERNEL_HEAP.set_grow(grow);
            crate::platform::pstore(b"ferros: the kernel heap's window is open\n");
        }
        None => crate::flashlog::line("kernel heap: no window (no addresses or no frame for its tables); it stays at its fixed regions"),
    }
}

/// The allocator's hook (`LockedTlsf::set_grow`): `bytes` more at the window's top, when this core may take the kernel lock now (above). The heap's lock is let go.
pub fn grow(bytes: usize) -> bool {
    if BASE.load(Relaxed) == 0 || crate::process::lock_mine() || crate::process::ranks_from(0) {
        REFUSED.fetch_add(1, Relaxed);
        return false;
    }
    crate::process::with_lock(|| grow_held(bytes))
}

/// Room for one allocation of `bytes` (at the heap's grain) in the kernel heap: true when it fits now or after the window grew. Under the kernel lock or holding no lock, at a point where no frames work is under way (a syscall's start, the service loop); never under a storage lock (false there).
pub fn room(bytes: usize) -> bool {
    let Ok(l) = Layout::from_size_align(bytes.max(1), 16) else { return false };
    if crate::KERNEL_HEAP.fits(l) {
        return true;
    }
    if BASE.load(Relaxed) == 0 || crate::process::ranks_from(0) {
        return false;
    }
    let Some(need) = ferros_heap::tlsf::Tlsf::grow_need(l) else { return false };
    crate::process::with_lock_held_or(|| grow_held(need)) && crate::KERNEL_HEAP.fits(l)
}

/// `bytes` more at the window's top: frames taken (not zeroed: the heap needs no zeroes, and a frame given back by the heap is wiped before its next owner, frames.rs `give`), mapped, and added to the heap. Under the kernel lock. False when the pool has none or the window is full.
fn grow_held(bytes: usize) -> bool {
    let c = crate::process::core();
    if GROWING[c].swap(true, Relaxed) {
        REFUSED.fetch_add(1, Relaxed);
        return false;
    }
    let base = BASE.load(Relaxed);
    let from = MAPPED.load(Relaxed);
    // A frame past what was asked: the old top's sentinel and the new block's header, and a cushion so the next small allocations past it do not grow again at once.
    let want = (bytes + FRAME).next_multiple_of(FRAME);
    let mut to = from;
    // No bound against the reach: the window maps frames of the pool, and the reach is the pool's bytes rounded up, so it can fill only as the pool empties, and the take refuses first.
    while to - from < want {
        let Some(f) = crate::frames::take_for_copy() else { break };
        crate::cachemap::kheap_map(base + to, Some(crate::frames::pa(f)));
        to += FRAME;
    }
    let ok = to > from;
    if ok {
        crate::cachemap::mapped();
        MAPPED.store(to, Relaxed);
        PEAK.fetch_max(to, Relaxed);
        // SAFETY: [base + from, base + to) is mapped above, the kernel's alone, and in no region of the heap yet; every growth starts at the window's mapped end.
        unsafe { crate::KERNEL_HEAP.extend(base + from, base + to) };
    }
    if to - from < want {
        REFUSED.fetch_add(1, Relaxed);
    }
    GROWING[c].store(false, Relaxed);
    ok
}

/// The window's free top past the cushion given back to the pool: from the service loop each pass (holding no lock). A cheap look first, the kernel lock only when there is something to give.
pub fn tend() {
    if MAPPED.load(Relaxed) <= CUSHION + FRAME {
        return;
    }
    if crate::KERNEL_HEAP.top_free_bytes() <= CUSHION + FRAME {
        return;
    }
    crate::process::with_lock(|| {
        let base = BASE.load(Relaxed);
        let Some((b, cut)) = crate::KERNEL_HEAP.trim_begin(CUSHION, FRAME) else { return };
        let end = base + MAPPED.load(Relaxed);
        // The block past `cut` is off the heap's lists: each frame unmapped, every core's TLB told, then back to the pool (frames.rs `give`: wiped before anyone else gets it).
        let mut va = cut;
        while va < end {
            let pa = crate::cachemap::kheap_pa(va);
            crate::cachemap::kheap_map(va, None);
            crate::cachemap::surface_unmapped(va, FRAME);
            crate::frames::give(crate::frames::of_pa(pa));
            va += FRAME;
        }
        MAPPED.store(cut - base, Relaxed);
        // SAFETY: `b` and `cut` from `trim_begin`, and nothing past `cut` is mapped any more.
        unsafe { crate::KERNEL_HEAP.trim_end(b, cut) };
    });
}

/// A refusal where the size was someone else's (docs/NO-CEILINGS.md 1: an answer, never a reset): "kernel out of memory" with what asked and how much, on the console and in the flash log.
pub fn refused(what: &str, bytes: usize) {
    // The pool's count is the kernel lock's (frames.rs); under a storage lock it cannot be taken, and the line goes without it.
    let free = if crate::process::ranks_from(0) { alloc::string::String::from("an unknown number of") } else { alloc::format!("A#{}", crate::process::with_lock_held_or(crate::frames::free) * FRAME) };
    let line = alloc::format!("kernel out of memory: {what} asked A#{bytes} bytes; the kernel heap's window holds A#{} KiB, the device has {free} of A#{} bytes free\n", MAPPED.load(Relaxed) >> 10, crate::frames::map_bytes());
    // The console is the kernel lock's: written directly when this core holds it, taken when it holds nothing; under a storage lock alone (the vault's read of a value) only the flash log hears it, whose own lock ranks above the vault's (process.rs `Rank`).
    if crate::process::lock_mine() || !crate::process::ranks_from(0) {
        crate::process::push_console_any(line.as_bytes());
    }
    if !crate::process::ranks_from(crate::process::LOG.rank()) {
        crate::flashlog::line(line.trim_end());
    }
}

/// Room for `bytes` (`room`) and a vector of that capacity, or the refusal said (`refused`) and None: for a buffer the kernel fills with someone else's bytes.
pub fn vec_for(what: &str, bytes: usize) -> Option<alloc::vec::Vec<u8>> {
    let mut v = alloc::vec::Vec::new();
    if room(bytes) && v.try_reserve_exact(bytes).is_ok() {
        return Some(v);
    }
    refused(what, bytes);
    None
}

/// `diag heap`'s window line.
pub fn report() -> alloc::string::String {
    let base = BASE.load(Relaxed);
    if base == 0 {
        return alloc::string::String::from("window: not open (the fixed regions alone)\n");
    }
    alloc::format!(
        "window: G#{base:X}, reach A#{} MiB, A#{} KiB mapped (peak A#{} KiB), growths refused A#{}\n",
        REACH.load(Relaxed) >> 20,
        MAPPED.load(Relaxed) >> 10,
        PEAK.load(Relaxed) >> 10,
        REFUSED.load(Relaxed)
    )
}
