//! Threads: the kernel's THREAD_SPAWN on a stack from the heap, each with its own TLS block in TPIDR_EL0 (the kernel carries the register per thread since 2026-10-27), a join word for the futex, and a registry so a detached thread's memory comes back.
//!
//! A thread's last act is in assembly with no stack access: it settles its state word (1 exited-joinable, or 3 exited-detached), wakes a joiner, and traps out — so whoever frees the stack (the joiner, or the next spawn's reaper) can never free it under a still-running thread.

use core::alloc::Layout;
use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::heap::SpinLock;
use crate::sys::{self, ERR, nr, off};

const RUNNING: u64 = 0;
const EXITED: u64 = 1;
const DETACHED: u64 = 2;
const DEAD: u64 = 3;

/// The 64 KiB stack the kernel gives the entry thread at the arc's top (process.rs STACK); the heap stops below it.
pub const ENTRY_STACK: usize = 64 * 1024;

pub struct Block {
    state: AtomicU64,
    next: *mut Block,
    stack: *mut u8,
    stack_layout: Layout,
    tls: *mut u8,
    tls_layout: Layout,
    entry: extern "C" fn(u64),
    arg: u64,
}

/// A spawned thread: join it, or drop it to detach.
pub struct Handle(*mut Block);
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

struct Head(*mut Block);
unsafe impl Send for Head {}

static REGISTRY: SpinLock<Head> = SpinLock::new(Head(core::ptr::null_mut()));

/// Start `entry(arg)` on a fresh stack of at least `stack_bytes`. Err when the heap or the kernel refuses.
pub fn spawn(entry: extern "C" fn(u64), stack_bytes: usize, arg: u64) -> Result<Handle, ()> {
    reap();
    let stack_layout = Layout::from_size_align((stack_bytes.max(16 * 1024) + 4095) & !4095, 16).map_err(|_| ())?;
    let stack = unsafe { crate::alloc::alloc(stack_layout) };
    if stack.is_null() {
        return Err(());
    }
    let Some((tls, tls_layout)) = tls::new_block() else {
        unsafe { crate::alloc::dealloc(stack, stack_layout) };
        return Err(());
    };
    let block_layout = Layout::new::<Block>();
    let block = unsafe { crate::alloc::alloc(block_layout) } as *mut Block;
    if block.is_null() {
        unsafe {
            crate::alloc::dealloc(stack, stack_layout);
            crate::alloc::dealloc(tls, tls_layout);
        }
        return Err(());
    }
    unsafe { block.write(Block { state: AtomicU64::new(RUNNING), next: core::ptr::null_mut(), stack, stack_layout, tls, tls_layout, entry, arg }) };
    {
        let mut head = REGISTRY.lock();
        unsafe { (*block).next = head.0 };
        head.0 = block;
    }
    let top = unsafe { stack.add(stack_layout.size()) };
    if sys::svc(nr::THREAD_SPAWN, off(trampoline as *const u8), off(top), off(block as *const u8), 0, 0) == ERR {
        unlink(block);
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
fn finish(block: *mut Block) -> ! {
    let state = unsafe { &raw const (*block).state } as *const u8;
    unsafe {
        asm!(
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
