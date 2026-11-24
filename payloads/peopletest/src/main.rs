//! peopletest: the People page's paging on the device (docs/NO-CEILINGS.md step 7, fgtw op 36), as the map reads it (`People::fetch`): every page from where the last ended until fgtw has said them all, and a line with the state, the total fgtw counts, the rows read and the pages it took. No name is printed (a person's handle is theirs).
//! `ferros-bridge spawn peopletest.bin --name peopletest --use fgtw` once the person has attested (fgtw serves then).
#![no_std]
#![no_main]

extern crate alloc;

use ferros_sys::{cap, console, ERR};

const HEAP_CEILING: usize = ferros_sys::HEAP_DEVICE;

core::arch::global_asm!(
    r#"
    .section .text.entry, "ax"
    .globl _entry
    .type _entry, %function
_entry:
    b {main}
    .4byte 1
    .ascii "ferrosRT"
    .quad __ferros_tail
    .quad 0
    .quad 0
    .quad 0
    .quad {heap}
    .size _entry, . - _entry
"#,
    main = sym test_main,
    heap = const HEAP_CEILING
);

unsafe extern "C" {
    static __bss_end: u8;
}

#[global_allocator]
static ALLOC: ferros_heap::ArcHeap<ferros_heap::Kernel> = ferros_heap::ArcHeap::new(ferros_heap::Kernel);

const S_FGTW: u64 = 1;
/// A page's buffer: the map's (`PEOPLE_PAGE`).
const PAGE: usize = 1 << 12;
static mut GRANTS: u64 = 0;

struct Log {
    b: [u8; 256],
    n: usize,
}

impl core::fmt::Write for Log {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let k = s.len().min(self.b.len() - self.n);
        self.b[self.n..self.n + k].copy_from_slice(&s.as_bytes()[..k]);
        self.n += k;
        Ok(())
    }
}

macro_rules! say {
    ($($a:tt)*) => {{
        let mut l = Log { b: [0; 256], n: 0 };
        let _ = core::fmt::Write::write_fmt(&mut l, format_args!($($a)*));
        let _ = core::fmt::Write::write_str(&mut l, "\n");
        console(unsafe { GRANTS }, &l.b[..l.n]);
    }};
}

#[unsafe(no_mangle)]
pub extern "C" fn test_main(g: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    unsafe { ALLOC.init((&raw const __bss_end) as usize, input as usize, input_len as usize) };
    unsafe { GRANTS = g };
    let mut r = [0u8; PAGE];
    let (mut read, mut pages, mut total, mut state) = (0usize, 0u32, 0usize, 0u8);
    loop {
        let mut q = [0u8; 5];
        q[0] = 36;
        q[1..].copy_from_slice(&(read as u32).to_le_bytes());
        let k = ferros_sys::call_live(cap(g, S_FGTW), &q, &mut r);
        if k == ERR || k < 7 {
            say!("peopletest: fgtw did not answer op 36 (answer G#{k:X}) after A#{pages} pages");
            ferros_sys::exit(1);
        }
        pages += 1;
        let got = (k as usize).min(r.len());
        state = r[0];
        total = u32::from_le_bytes([r[1], r[2], r[3], r[4]]) as usize;
        let rows = u16::from_le_bytes([r[5], r[6]]) as usize;
        let mut at = 7;
        let before = read;
        for _ in 0..rows {
            // WHY: the lengths are fgtw's words; a row past the reply ends the page.
            if at + 33 > got || at + 33 + r[at + 32] as usize > got {
                break;
            }
            at += 33 + r[at + 32] as usize;
            read += 1;
        }
        if read >= total || read == before {
            break;
        }
    }
    say!("peopletest: state A#{state}, fgtw counts A#{total} people, A#{read} read in A#{pages} pages{}", if read == total { ", all of them" } else { ", NOT all" });
    ferros_sys::exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::exit(0xDEAD)
}
