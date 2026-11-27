//! Threads: the kernel's THREAD_SPAWN on a stack from the heap, each with its own TLS block in TPIDR_EL0 (the kernel carries the register per thread since 2026-10-27), a join word for the futex, and a registry so a detached thread's memory comes back.
//!
//! A thread's last act is in assembly with no stack access: it settles its state word (1 exited-joinable, or 3 exited-detached), wakes a joiner, and traps out — so whoever frees the stack (the joiner, or the next spawn's reaper) can never free it under a still-running thread.
//!
//! Under every stack lies a guard page the kernel holds kernel-only (SYS_ARC `ARC_GUARD`, 2026-11-14): a stack run past its end faults there, its thread named, where until then it wrote on over whatever the heap kept below (Photon's alias thread signed on a 256 KiB stack with ~370 KiB of frames, and a thread died later on a return address nobody wrote). The thread hands the page back in its last act, before its state settles.

use core::alloc::Layout;
use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::lock::Lock;
use crate::sys::{self, ERR, nr, off};

const RUNNING: u64 = 0;
const EXITED: u64 = 1;
const DETACHED: u64 = 2;
const DEAD: u64 = 3;

/// The 64 KiB stack the kernel gives the entry thread at the arc's top (process.rs STACK); the heap stops below it and the guard page under it (layout.rs).
pub const ENTRY_STACK: usize = crate::layout::ENTRY_STACK;

/// What std gives a thread that names no stack size (the fork's `DEFAULT_MIN_STACK_SIZE`): 1 MiB (owner, 2026-11-14: "1MB"), since a thread's stack is fully committed on ferros (Photon's 79 threads: ~79 MiB at 1 MiB against ~158 MiB at Linux's 2 MiB) and the guard page under every stack makes an overflow fault at once. It was 256 KiB until 2026-11-14, and Photon's alias signing alone took ~370 KiB (fgtw's `SigningBundle::eggs` 242 KiB of frame, fn-dsa's key decode 116 KiB under it).
pub const DEFAULT_STACK: usize = 1 << 20;

/// The guard page under each stack: one page, what LLVM's inline stack probes step by, so no frame can step over it.
pub const GUARD: usize = 1 << 12;

/// SYS_ARC's guard ops (kernel process.rs `ARC_GUARD`, `ARC_UNGUARD`) and the reason a kernel from before refuses them (`ARC_WHY_OP`).
const ARC_GUARD: u64 = 4;
const ARC_UNGUARD: u64 = 5;
const ARC_WHY_OP: u64 = 6;

/// Set once the kernel has said it has no guard op (a kernel from before 2026-11-14): stacks go unguarded from then on, and it is not asked again (each refusal is a console line).
static NO_GUARDS: AtomicBool = AtomicBool::new(false);

pub struct Block {
    state: AtomicU64,
    next: *mut Block,
    stack: *mut u8,
    stack_layout: Layout,
    tls: *mut u8,
    tls_layout: Layout,
    entry: extern "C" fn(u64),
    arg: u64,
    /// The guard page's arc offset (the stack allocation's first page), 0 when the stack runs unguarded.
    guard: u64,
}

/// Make the page at `page` (page-aligned, inside the heap) kernel-only: its arc offset when the kernel took it, 0 when it did not (a kernel from before, which is then not asked again).
pub fn guard(page: *mut u8) -> u64 {
    if NO_GUARDS.load(Ordering::Relaxed) {
        return 0;
    }
    let at = off(page);
    let (r, why) = sys::svc2(nr::ARC, ARC_GUARD, at);
    if r == ERR {
        if why == ARC_WHY_OP {
            NO_GUARDS.store(true, Ordering::Relaxed);
        }
        return 0;
    }
    at
}

/// Hand a guard page back (its arc offset from `guard`; 0 is none).
fn unguard(at: u64) {
    if at != 0 {
        sys::svc2(nr::ARC, ARC_UNGUARD, at);
    }
}

/// Name this thread to the kernel (SYS_THREAD_NAME): a fault's console line carries the name (its first 16 bytes). A kernel from before answers ERR, which changes nothing.
pub fn set_name(name: &[u8]) {
    sys::svc(nr::THREAD_NAME, off(name.as_ptr()), name.len() as u64, 0, 0, 0);
}

/// A spawned thread: join it, or drop it to detach.
pub struct Handle(*mut Block);
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

struct Head(*mut Block);
unsafe impl Send for Head {}

static REGISTRY: Lock<Head> = Lock::new(Head(core::ptr::null_mut()));

/// Start `entry(arg)` on a fresh stack of at least `stack_bytes`. Err when the heap or the kernel refuses.
pub fn spawn(entry: extern "C" fn(u64), stack_bytes: usize, arg: u64) -> Result<Handle, ()> {
    reap();
    // The stack's pages, then the guard under them: the allocation page-aligned, the guard its first page.
    let stack_layout = Layout::from_size_align(((stack_bytes.max(16 * 1024) + 4095) & !4095) + GUARD, GUARD).map_err(|_| ())?;
    let stack = unsafe { crate::alloc::alloc(stack_layout) };
    if stack.is_null() {
        return Err(());
    }
    let guarded = guard(stack);
    let Some((tls, tls_layout)) = tls::new_block() else {
        unguard(guarded);
        unsafe { crate::alloc::dealloc(stack, stack_layout) };
        return Err(());
    };
    let block_layout = Layout::new::<Block>();
    let block = unsafe { crate::alloc::alloc(block_layout) } as *mut Block;
    if block.is_null() {
        unguard(guarded);
        unsafe {
            crate::alloc::dealloc(stack, stack_layout);
            crate::alloc::dealloc(tls, tls_layout);
        }
        return Err(());
    }
    unsafe { block.write(Block { state: AtomicU64::new(RUNNING), next: core::ptr::null_mut(), stack, stack_layout, tls, tls_layout, entry, arg, guard: guarded }) };
    {
        let mut head = REGISTRY.lock();
        unsafe { (*block).next = head.0 };
        head.0 = block;
    }
    let top = unsafe { stack.add(stack_layout.size()) };
    if sys::svc(nr::THREAD_SPAWN, off(trampoline as *const u8), off(top), off(block as *const u8), 0, 0) == ERR {
        unlink(block);
        unguard(guarded);
        unsafe { free(block) };
        return Err(());
    }
    Ok(Handle(block))
}

extern "C" fn trampoline(block_off: u64) -> ! {
    let block = (sys::arc_base() + block_off) as *mut Block;
    // The thread pointer first: nothing may touch a thread-local before it.
    unsafe { tls::install((*block).tls) };
    let (entry, arg) = unsafe { ((*block).entry, (*block).arg) };
    entry(arg);
    finish(block)
}

/// The end of a thread, with no stack access after the state settles: 0 → 1 (joinable, wake the joiner) or 2 → 3 (detached, the reaper's).
/// The guard page goes back first (SYS_ARC `ARC_UNGUARD`, x7 its offset, 0 none), so whoever frees the stack once the state settles finds every page of it the process's again; the trap keeps x5-x7.
fn finish(block: *mut Block) -> ! {
    let state = unsafe { &raw const (*block).state } as *const u8;
    let guard = unsafe { (*block).guard };
    unsafe {
        asm!(
            "cbz x7, 5f",
            "mov x0, #{unguard}",
            "mov x1, x7",
            "mov x8, #{arc}",
            "svc #0",
            "5:",
            "2:",
            "ldaxr x2, [x5]",
            "mov x3, #1",
            "cbz x2, 3f",
            "mov x3, #3",
            "3:",
            "stlxr w4, x3, [x5]",
            "cbnz w4, 2b",
            "mov x0, x6",
            "mov x1, #-1",
            "mov x8, #17",
            "svc #0",
            "mov x0, #0",
            "mov x8, #15",
            "svc #0",
            "4: b 4b",
            in("x5") state,
            in("x6") off(state),
            in("x7") guard,
            unguard = const ARC_UNGUARD,
            arc = const nr::ARC,
            options(noreturn, nostack)
        )
    }
}

/// Wait for the thread to end, then take its memory back.
pub fn join(h: Handle) {
    let block = h.0;
    core::mem::forget(h);
    let state = unsafe { &(*block).state };
    loop {
        let s = state.load(Ordering::Acquire);
        if s == EXITED {
            break;
        }
        sys::svc(nr::WAIT, off(state), s, 0, 0, 0);
    }
    unlink(block);
    unsafe { free(block) };
}

impl Drop for Handle {
    /// Detach: the thread's memory returns through the reaper once it ends — or now, if it already has.
    fn drop(&mut self) {
        let state = unsafe { &(*self.0).state };
        if state.compare_exchange(RUNNING, DETACHED, Ordering::AcqRel, Ordering::Acquire) == Err(EXITED) {
            unlink(self.0);
            unsafe { free(self.0) };
        }
    }
}

fn unlink(block: *mut Block) {
    let mut head = REGISTRY.lock();
    let mut p: *mut *mut Block = &raw mut head.0;
    unsafe {
        while !(*p).is_null() {
            if *p == block {
                *p = (**p).next;
                return;
            }
            p = &raw mut (**p).next;
        }
    }
}

/// Free the memory of every detached thread that has ended.
fn reap() {
    let mut dead: [*mut Block; 16] = [core::ptr::null_mut(); 16];
    let mut n = 0;
    {
        let mut head = REGISTRY.lock();
        let mut p: *mut *mut Block = &raw mut head.0;
        unsafe {
            while !(*p).is_null() && n < dead.len() {
                if (**p).state.load(Ordering::Acquire) == DEAD {
                    dead[n] = *p;
                    n += 1;
                    *p = (**p).next;
                } else {
                    p = &raw mut (**p).next;
                }
            }
        }
    }
    for &b in &dead[..n] {
        unsafe { free(b) };
    }
}

unsafe fn free(block: *mut Block) {
    unsafe {
        let b = block.read();
        crate::alloc::dealloc(b.stack, b.stack_layout);
        if b.tls_layout.size() != 0 {
            crate::alloc::dealloc(b.tls, b.tls_layout);
        }
        crate::alloc::dealloc(block as *mut u8, Layout::new::<Block>());
    }
}

/// Give the core up to anyone runnable.
pub fn yield_now() {
    sys::svc(nr::SLEEP, 0, 0, 0, 0, 0);
}

/// End this thread (the last thread ends the process).
pub fn exit(value: u64) -> ! {
    sys::svc(nr::THREAD_EXIT, value, 0, 0, 0, 0);
    loop {
        core::hint::spin_loop();
    }
}

/// Pin this process's threads to a core mask (ferros_sys::pin); the kernel's four A715 workers are G#F0.
pub fn pin(mask: u64) -> u64 {
    sys::svc(nr::PIN, mask, 0, 0, 0, 0)
}

/// Thread-local storage: one block per thread, laid out as lld lays out TPOFF for AArch64 (variant I): the thread pointer, 16 bytes of TCB, then the TLS image at `16 + ((p_vaddr − 16) & (p_align − 1))` — the image's `.tdata` copied in, `.tbss` zero.
pub mod tls {
    use core::alloc::Layout;
    use core::arch::asm;

    unsafe extern "C" {
        static __tdata_start: u8;
    }

    /// Where the image lands from the thread pointer.
    pub fn image_offset() -> usize {
        let h = crate::start::header();
        let align = h.tls_align.max(1);
        let vma = (&raw const __tdata_start as usize).wrapping_sub(crate::sys::image_base());
        16 + (vma.wrapping_sub(16) & (align - 1))
    }

    /// A new block with the TLS image in place, zero elsewhere. None when the heap is out.
    pub fn new_block() -> Option<(*mut u8, Layout)> {
        let h = crate::start::header();
        let off = image_offset();
        let layout = Layout::from_size_align(off + h.tls_size, h.tls_align.max(16)).ok()?;
        let p = unsafe { crate::alloc::alloc_zeroed(layout) };
        if p.is_null() {
            return None;
        }
        unsafe { core::ptr::copy_nonoverlapping(&raw const __tdata_start, p.add(off), h.tdata_size) };
        Some((p, layout))
    }

    /// Make `block` this thread's TLS: TPIDR_EL0, which the kernel keeps per thread.
    pub unsafe fn install(block: *mut u8) {
        unsafe { asm!("msr tpidr_el0, {}", in(reg) block, options(nomem, nostack, preserves_flags)) };
    }
}
