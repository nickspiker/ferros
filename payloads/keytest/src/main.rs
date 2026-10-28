//! keytest: the two syscalls that use the register-held secrets (keys.rs), end to end from EL0 (development). Two entropy draws must differ; two ira keys for one domain must match, and a second domain must give another. Prints two bytes of each, never more (the domain is a test one, its key guards nothing).
//!
//! ferros-bridge spawn keytest.bin --name keytest --entropy --ira
#![no_std]
#![no_main]

use ferros_sys::{cap, console, exit};

struct Line {
    b: [u8; 160],
    n: usize,
}

impl Line {
    fn s(&mut self, t: &[u8]) {
        for &c in t {
            self.b[self.n] = c;
            self.n += 1;
        }
    }
    fn hex(&mut self, v: &[u8]) {
        let h = b"0123456789ABCDEF";
        for &x in v {
            self.s(&[h[(x >> 4) as usize], h[(x & 15) as usize]]);
        }
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let mut l = Line { b: [0; 160], n: 0 };
    let (mut a, mut b) = ([0u8; 200], [0u8; 200]);
    let ra = ferros_sys::entropy(cap(grants, 1), &mut a);
    let rb = ferros_sys::entropy(cap(grants, 1), &mut b);
    l.s(b"keytest: entropy ");
    if ra == 200 && rb == 200 {
        l.hex(&a[..2]);
        l.s(b" ");
        l.hex(&b[..2]);
        l.s(if a != b { b" differ" } else { b" SAME" });
    } else {
        l.s(b"ERR");
    }
    let (x, y, z) = (ferros_sys::ira_key(cap(grants, 2), "keytest"), ferros_sys::ira_key(cap(grants, 2), "keytest"), ferros_sys::ira_key(cap(grants, 2), "keytest other"));
    l.s(b"; ira ");
    match (x, y, z) {
        (Some(x), Some(y), Some(z)) => {
            l.hex(&x[..2]);
            l.s(if x == y { b" same twice" } else { b" DIFFERS" });
            l.s(if x != z { b", other domain differs" } else { b", other domain SAME" });
        }
        _ => l.s(b"ERR"),
    }
    l.s(b"\n");
    console(grants, &l.b[..l.n]);
    exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
