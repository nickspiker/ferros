//! The lock the heap and the thread registry sit behind: a futex mutex over the kernel's WAIT/WAKE (futex.rs), the shape of std's own Mutex on ferros.
//! A short spin first, then the waiter sleeps in the kernel until the holder's unlock wakes it.
//! The spinlock it replaced (2026-11-11) spun without end: a holder preempted mid-walk left every other thread spinning out its 10 ms slice (Mahere: about 65 % of the process's time in the heap, most of it spinning).
//!
//! Words: 0 free, 1 held, 2 held with a waiter maybe asleep (the unlock then wakes one).

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::futex::{futex_wait, futex_wake};

const FREE: u64 = 0;
const HELD: u64 = 1;
const WAITED: u64 = 2;
/// Loads of the word before sleeping: the heap's operations are O(1), a few hundred nanoseconds, so a holder on another core is usually out within the spin.
const SPIN: u32 = 1 << 7;

pub struct Lock<T> {
    word: AtomicU64,
    value: UnsafeCell<T>,
}

// The lock serialises every access to the value.
unsafe impl<T: Send> Sync for Lock<T> {}
unsafe impl<T: Send> Send for Lock<T> {}

impl<T> Lock<T> {
    pub const fn new(value: T) -> Self {
        Lock { word: AtomicU64::new(FREE), value: UnsafeCell::new(value) }
    }

    #[inline]
    pub fn lock(&self) -> Guard<'_, T> {
        if self.word.compare_exchange(FREE, HELD, Ordering::Acquire, Ordering::Relaxed).is_err() {
            self.contended();
        }
        Guard { lock: self }
    }

    /// The lock only when it is free now (a shrink that would wait behind a grow is not worth waiting for).
    #[inline]
    pub fn try_lock(&self) -> Option<Guard<'_, T>> {
        self.word.compare_exchange(FREE, HELD, Ordering::Acquire, Ordering::Relaxed).ok().map(|_| Guard { lock: self })
    }

    #[cold]
    fn contended(&self) {
        let mut w = self.spin();
        if w == FREE {
            match self.word.compare_exchange(FREE, HELD, Ordering::Acquire, Ordering::Relaxed) {
                Ok(_) => return,
                Err(now) => w = now,
            }
        }
        loop {
            // Mark it waited before sleeping, so the holder's unlock wakes someone; taking it this way leaves it marked, which costs at most one wake with nobody asleep.
            if w != WAITED && self.word.swap(WAITED, Ordering::Acquire) == FREE {
                return;
            }
            futex_wait(&self.word, WAITED, None);
            w = self.spin();
        }
    }

    /// Spin while the word says held with nobody asleep, `SPIN` loads at most; the word last seen.
    fn spin(&self) -> u64 {
        let mut n = SPIN;
        loop {
            let w = self.word.load(Ordering::Relaxed);
            if w != HELD || n == 0 {
                return w;
            }
            core::hint::spin_loop();
            n -= 1;
        }
    }

    #[inline]
    fn unlock(&self) {
        if self.word.swap(FREE, Ordering::Release) == WAITED {
            futex_wake(&self.word);
        }
    }
}

pub struct Guard<'a, T> {
    lock: &'a Lock<T>,
}

impl<T> core::ops::Deref for Guard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> core::ops::DerefMut for Guard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for Guard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.unlock();
    }
}
