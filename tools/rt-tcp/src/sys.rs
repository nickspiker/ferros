//! A fake kernel for tcp.rs: the arc is the whole address space (base 0, so an offset is the pointer), the counter is nanoseconds since the first look, and CALL hands the request to the fake ip server (`crate::ip`), blocking the calling thread while ip holds it.

use std::sync::OnceLock;
use std::time::Instant;

pub const ERR: u64 = u64::MAX;

pub mod nr {
    pub const CALL: u64 = 1;
    pub const SLEEP: u64 = 9;
}

/// The grant slot the fake ip serves; a call on any other is refused as the kernel refuses a token it does not know.
pub const IP_SLOT: u64 = 1;

pub fn arc_base() -> u64 {
    0
}

pub fn off<T: ?Sized>(p: *const T) -> u64 {
    p as *const u8 as u64
}

fn start() -> Instant {
    static T0: OnceLock<Instant> = OnceLock::new();
    *T0.get_or_init(Instant::now)
}

/// Counter ticks: nanoseconds from the first look, plus one second, so a real deadline is never 0 (a booted device's counter is long past 0 too).
pub fn now() -> u64 {
    start().elapsed().as_nanos() as u64 + (1 << 30)
}

pub fn freq() -> u64 {
    1_000_000_000
}

pub fn svc(n: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
    match n {
        nr::CALL if a0 == IP_SLOT => {
            // SAFETY: the client hands its own buffers by pointer (base 0), alive for the call.
            let req = unsafe { std::slice::from_raw_parts(a1 as *const u8, a2 as usize) };
            let reply = unsafe { std::slice::from_raw_parts_mut(a3 as *mut u8, a4 as usize) };
            let mut out = vec![0u8; 1 << 19];
            let l = crate::ip::serve(req, &mut out);
            // The kernel copies what the server wrote, cut at the caller's length, and says how much it copied (process.rs SYS_REPLY).
            let k = l.min(reply.len());
            reply[..k].copy_from_slice(&out[..k]);
            k as u64
        }
        nr::SLEEP => {
            let t = now();
            if a0 > t {
                std::thread::sleep(std::time::Duration::from_nanos(a0 - t));
            }
            0
        }
        _ => ERR,
    }
}
