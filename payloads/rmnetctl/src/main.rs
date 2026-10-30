//! One request to the modem driver's `rmnet` endpoint from the host, its reply on the console: the driver's state (4), a call reset (6), a power-down (5) — payloads/modem's header has the requests.
//!
//! Spawn: ferros-bridge spawn rmnetctl.bin <request byte in hex> --name rmnetctl --use rmnet
//! Grant slots: 0 console, 1 rmnet (use). A reply that is text (STATE) is printed as it is; any other as its first bytes in hex. INFO (3) is never printed: its addresses are the line's.

#![no_std]
#![no_main]

use ferros_sys::{cap, console, exit, CALL_LIVE, ERR};

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, input: u64, input_len: u64) -> ! {
    let base = ferros_sys::arc_base();
    // The request is the input (up to 16 bytes: TRACE takes a count after its byte); none is STATE.
    let mut q = [4u8; 16];
    let ql = (input_len as usize).min(q.len()).max(1);
    for k in 0..input_len.min(16) as usize {
        q[k] = unsafe { core::ptr::read_volatile((base + input + k as u64) as *const u8) };
    }
    let req = q[0];
    if req == 3 {
        console(grants, b"rmnetctl: INFO is not printed (the call's addresses)\n");
        exit(0)
    }
    let mut r = [0u8; 2048];
    // CALL_LIVE: no driver serving rmnet is an answer too, not a wait.
    let n = ferros_sys::svc(ferros_sys::nr::CALL, cap(grants, 1), ferros_sys::off(q.as_ptr()), ql as u64, ferros_sys::off(r.as_mut_ptr()), r.len() as u64 | CALL_LIVE);
    if n == ERR {
        console(grants, b"rmnetctl: no driver serves rmnet\n");
        exit(1)
    }
    let n = n as usize;
    if req == 4 {
        console(grants, &r[..n]);
    } else {
        let h = b"0123456789ABCDEF";
        let mut line = [0u8; 64];
        let mut k = 0;
        for &b in r[..n.min(16)].iter() {
            line[k] = h[(b >> 4) as usize];
            line[k + 1] = h[(b & 15) as usize];
            k += 2;
        }
        console(grants, b"rmnetctl: reply G#");
        console(grants, &line[..k]);
        console(grants, b"\n");
    }
    exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
