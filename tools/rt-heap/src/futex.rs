//! ferros_rt's futex (its `futex.rs`) on Linux: the same signatures over FUTEX_WAIT and FUTEX_WAKE on the word's low half (little-endian; the lock's words are 0, 1 and 2).

use core::sync::atomic::AtomicU64;
use core::time::Duration;

const SYS_FUTEX: usize = 202;
const FUTEX_WAIT_PRIVATE: usize = 128;
const FUTEX_WAKE_PRIVATE: usize = 129;
const ETIMEDOUT: isize = -110;

#[repr(C)]
struct Timespec {
    sec: i64,
    nsec: i64,
}

unsafe fn futex(word: *const AtomicU64, op: usize, val: usize, timeout: *const Timespec) -> isize {
    let r: isize;
    unsafe {
        core::arch::asm!("syscall", inlateout("rax") SYS_FUTEX as isize => r, in("rdi") word, in("rsi") op, in("rdx") val, in("r10") timeout, in("r8") 0usize, in("r9") 0usize, lateout("rcx") _, lateout("r11") _, options(nostack));
    }
    r
}

/// Returns false only on timeout.
pub fn futex_wait(futex_word: &AtomicU64, expected: u64, timeout: Option<Duration>) -> bool {
    let ts = timeout.map(|d| Timespec { sec: d.as_secs() as i64, nsec: d.subsec_nanos() as i64 });
    let p = ts.as_ref().map_or(core::ptr::null(), |t| t as *const Timespec);
    unsafe { futex(futex_word, FUTEX_WAIT_PRIVATE, expected as u32 as usize, p) != ETIMEDOUT }
}

/// Wake one waiter; true when there was one.
pub fn futex_wake(futex_word: &AtomicU64) -> bool {
    unsafe { futex(futex_word, FUTEX_WAKE_PRIVATE, 1, core::ptr::null()) > 0 }
}

pub fn futex_wake_all(futex_word: &AtomicU64) {
    unsafe { futex(futex_word, FUTEX_WAKE_PRIVATE, i32::MAX as usize, core::ptr::null()) };
}
