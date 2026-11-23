use crate::ffi::CStr;
use crate::io;
use crate::num::NonZero;
use crate::thread::ThreadInit;
use crate::time::Duration;
use crate::ptr;

/// Stacks come from the process heap with a guard page under each (ferros_rt::thread), so a thread costs what it asks for and an overflow faults at once.
/// The default is ferros_rt's (1 MiB since 2026-11-14): 256 KiB let Photon's alias thread run 110 KiB past its stack into the heap while signing.
pub const DEFAULT_MIN_STACK_SIZE: usize = ferros_rt::thread::DEFAULT_STACK;

pub struct Thread {
    handle: ferros_rt::thread::Handle,
}

impl Thread {
    // unsafe: see thread::Builder::spawn_unchecked for safety requirements
    pub unsafe fn new(stack: usize, init: Box<ThreadInit>) -> io::Result<Thread> {
        extern "C" fn thread_start(data: u64) {
            // SAFETY: we are simply recreating the box that was leaked earlier.
            let init = unsafe { Box::from_raw(ptr::with_exposed_provenance_mut::<ThreadInit>(data as usize)) };
            let rust_start = init.init();
            rust_start();

            // Run all destructors: std is the only runtime, so it calls them itself.
            unsafe {
                crate::sys::thread_local::destructors::run();
            }
            crate::rt::thread_cleanup();
        }

        let data = Box::into_raw(init);
        match ferros_rt::thread::spawn(thread_start, stack, data.expose_provenance() as u64) {
            Ok(handle) => Ok(Thread { handle }),
            Err(()) => {
                // The thread failed to start and as a result data was not consumed. Therefore, it is
                // safe to reconstruct the box so that it gets deallocated.
                unsafe { drop(Box::from_raw(data)) };
                Err(io::const_error!(io::ErrorKind::Uncategorized, "unable to create thread: the heap or the kernel refused"))
            }
        }
    }

    pub fn join(self) {
        ferros_rt::thread::join(self.handle)
    }
}

/// The kernel's four A715 workers run a process's threads (the X3 runs the kernel and the display).
pub fn available_parallelism() -> io::Result<NonZero<usize>> {
    Ok(NonZero::new(4).unwrap())
}

pub fn current_os_id() -> Option<u64> {
    None
}

pub fn yield_now() {
    ferros_rt::thread::yield_now()
}

pub fn set_name(name: &CStr) {
    // The kernel keeps the name for the thread's FAULT line (SYS_THREAD_NAME); threads are still not addressable by it.
    ferros_rt::thread::set_name(name.to_bytes());
}

pub fn sleep(dur: Duration) {
    let deadline = ferros_rt::time::now().saturating_add(ferros_rt::time::ticks(dur));
    ferros_rt::time::sleep_until(deadline)
}
