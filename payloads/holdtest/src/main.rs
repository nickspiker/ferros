//! The hold's proof on the device (docs/HOLD-API.md): a development spawn with the `items` right (`ferros-bridge spawn holdtest.bin <mode> --name holdtest --items`), making the calls a kist makes thru `SYS_STORE` bit 7, never the bench's.
//! Input 00: a 3 MiB still kept, found by kind, read back by ranges and checked by its BLAKE3, the device's usage read, the still removed and found gone.
//! Input 01: a clip recorded straight to its own extent (the extent's SYS_BLOCK capability), its bytes those of the newest RAW video item already in the hold (the bench's `items put "RAW video" <clip> --clip`), read from that item and written here a MiB at a time with a pause between, so the bench can kill it mid-clip; left alone it seals the copy and checks it.
//!
//! Input 02: the person locked mid-session (see `lock`).
//!
//! Grant slots: 0 console, 1 items, 2 unlock (input 02 only).
#![no_std]
#![no_main]

extern crate alloc;

use ferros_items::kist::{Hold, Sys};
use ferros_items::{kinds, Find, Proposed, ORDER_NEWEST};
use ferros_sys::{cap, console, freq, now};

/// The heap's ceiling in the image header: what the device holds (ferros_sys::HEAP_DEVICE, docs/NO-CEILINGS.md step 3; the kernel takes the least of it and its pool), refused only when the device is out. Addresses only: the kernel maps the start at spawn and the rest as the heap grows (SYS_ARC, ferros_heap::ArcHeap). Until docs/NO-CEILINGS.md step 1 the heap was a static array of 16 MiB, held whole from spawn to end.
const HEAP_CEILING: usize = ferros_sys::HEAP_DEVICE;
/// A MiB: the hold's transfer, a clip's write.
const MIB: usize = 1 << 20;


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
    main = sym hold_main,
    heap = const HEAP_CEILING
);

unsafe extern "C" {
    /// The end of .bss (payload.ld): the heap starts there, in the zeroed tail the kernel maps after it.
    static __bss_end: u8;
}

#[global_allocator]
static ALLOC: ferros_heap::ArcHeap<ferros_heap::Kernel> = ferros_heap::ArcHeap::new(ferros_heap::Kernel);

const S_ITEMS: u64 = 1;
static mut GRANTS: u64 = 0;

struct Log {
    b: [u8; 384],
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
        let mut l = Log { b: [0; 384], n: 0 };
        let _ = core::fmt::Write::write_fmt(&mut l, format_args!($($a)*));
        let _ = core::fmt::Write::write_str(&mut l, "\n");
        console(unsafe { GRANTS }, &l.b[..l.n]);
    }};
}

/// Eagle time now, in oscillations (as the vault process reads it): 0 until the kernel has the wall clock.
fn eagle() -> i64 {
    const EPOCH_UNIX: i128 = -14_182_940;
    let Some((secs, ticks, hz)) = ferros_sys::utc_time() else { return 0 };
    let ops = vsf::OSCILLATIONS_PER_SECOND as i128;
    ((secs as i128 - EPOCH_UNIX) * ops + ticks as i128 * ops / hz as i128) as i64
}

fn ms(t: u64) -> u64 {
    (now() - t) * 1000 / freq()
}

fn hex8(b: &[u8; 32]) -> [u8; 16] {
    let d = b"0123456789ABCDEF";
    let mut o = [0u8; 16];
    for k in 0..8 {
        o[2 * k] = d[(b[k] >> 4) as usize];
        o[2 * k + 1] = d[(b[k] & 15) as usize];
    }
    o
}

#[unsafe(no_mangle)]
pub extern "C" fn hold_main(g: u64, input: u64, input_len: u64) -> ! {
    // The mode byte before anything else (x28, the arc base, is read once here).
    let mode = if input_len > 0 { unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input) as *const u8) } } else { 0 };
    unsafe { ferros_sys::relocate() };
    unsafe { ALLOC.init((&raw const __bss_end) as usize, input as usize, input_len as usize) };
    unsafe { GRANTS = g };
    let mut hold = Hold::new(Sys { cap: cap(g, S_ITEMS) }, eagle);
    match mode {
        0 => still(&mut hold),
        1 => clip(&mut hold),
        2 => lock(&mut hold),
        m => say!("holdtest: no mode {}", m),
    }
    ferros_sys::exit(0)
}

fn still(hold: &mut Hold<Sys>) {
    let body: alloc::vec::Vec<u8> = (0..3 * MIB as u32).map(|i| (i.wrapping_mul(2654435761) >> 11) as u8).collect();
    let want = blake3::hash(&body);
    let t = now();
    let p = Proposed { kind: kinds::PHOTO, title: Some("holdtest still".into()), ..Default::default() };
    let (h, c) = match hold.keep(&p, &body) {
        Ok(r) => r,
        Err(e) => return say!("holdtest: keep refused: {:?}", e),
    };
    say!("holdtest: kept a still of {} bytes in {} ms, its BLAKE3 the body's {}", c.size, ms(t), c.body == *want.as_bytes());
    let found = match hold.find(&Find { kinds: alloc::vec![kinds::PHOTO], words: Some("holdtest".into()), order: ORDER_NEWEST, ..Default::default() }) {
        Ok(f) => f,
        Err(e) => return say!("holdtest: find refused: {:?}", e),
    };
    say!("holdtest: found {} photos titled holdtest; the first is this one {}", found.total, found.items.first().is_some_and(|i| i.1.id == c.id));
    let mut back = alloc::vec![0u8; body.len()];
    let t = now();
    match hold.read(&h, 0, &mut back) {
        Ok((n, size)) => say!("holdtest: read {} of {} bytes in {} ms, equal {}", n, size, ms(t), back == body),
        Err(e) => say!("holdtest: read refused: {:?}", e),
    }
    let mut mid = [0u8; 5000];
    match hold.read(&h, (MIB as u64) - 1234, &mut mid) {
        Ok((n, _)) => say!("holdtest: a range across the first MiB's end: {} bytes, equal {}", n, mid[..] == body[MIB - 1234..MIB - 1234 + 5000]),
        Err(e) => say!("holdtest: range refused: {:?}", e),
    }
    match hold.usage() {
        Ok(u) => say!("holdtest: usage: {} kinds in the hold, the clips area {:?}, {} kists", u.kinds.len(), u.clips, u.kists.len()),
        Err(e) => say!("holdtest: usage refused: {:?}", e),
    }
    match hold.remove(&h) {
        Ok(()) => say!("holdtest: removed; its card now {:?}", hold.card(&h).err()),
        Err(e) => say!("holdtest: remove refused: {:?}", e),
    }
}

/// Input 02 (grant slot 2 the unlock right, `--unlock` after `--items`): a find while the person is present, then the identity locked (SYS_UNLOCK op 2), then the same find and a card by the handle from before, both refused (the person's items domain has no keys while they are away). The next boot unlocks again (unlock at boot).
fn lock(hold: &mut Hold<Sys>) {
    let before = hold.find(&Find::default());
    let h = before.as_ref().ok().and_then(|f| f.items.first().map(|i| i.0));
    say!("holdtest: before the lock: {:?} items", before.map(|f| f.total));
    let r = ferros_sys::svc(ferros_sys::nr::UNLOCK, cap(unsafe { GRANTS }, 2), 2, 0, 0, 0);
    say!("holdtest: the identity locked: {}", r == 0);
    say!("holdtest: after the lock: find {:?}, the card from before {:?}", hold.find(&Find::default()).map(|f| f.total), h.map(|h| hold.card(&h).map(|c| c.size)));
}

fn clip(hold: &mut Hold<Sys>) {
    let src = match hold.find(&Find { kinds: alloc::vec![kinds::RAW_VIDEO], order: ORDER_NEWEST, count: 1, ..Default::default() }) {
        Ok(f) if !f.items.is_empty() => f.items[0].clone(),
        Ok(_) => return say!("holdtest: no RAW video in the hold to copy (bench: items put \"RAW video\" <clip> --clip)"),
        Err(e) => return say!("holdtest: find refused: {:?}", e),
    };
    let (sh, sc) = src;
    let blocks = sc.size.div_ceil(4096);
    let p = Proposed { kind: kinds::RAW_VIDEO, title: Some("holdtest copy".into()), ..Default::default() };
    let c = match hold.make_clip(&p, blocks) {
        Ok(c) => c,
        Err(e) => return say!("holdtest: make_clip refused: {:?}", e),
    };
    say!("holdtest: a clip of {} blocks made, straight to the flash {}; copying {} bytes a MiB at a time", c.blocks, c.direct(), sc.size);
    let mut buf = alloc::vec![0u8; MIB];
    let mut h = blake3::Hasher::new();
    let mut at = 0u64;
    let t = now();
    while at < sc.size {
        buf.fill(0);
        let (n, _) = match hold.read(&sh, at, &mut buf) {
            Ok(r) => r,
            Err(e) => return say!("holdtest: source read refused at {}: {:?}", at, e),
        };
        h.update(&buf[..n]);
        // Whole blocks: the last one padded with zeros.
        let w = n.div_ceil(4096) << 12;
        if let Err(e) = hold.write_clip(&c, at >> 12, &buf[..w]) {
            return say!("holdtest: clip write refused at block {}: {:?}", at >> 12, e);
        }
        at += n as u64;
        if at % (16 * MIB as u64) == 0 {
            say!("holdtest: {} MiB written", at >> 20);
        }
        // A pause a MiB, so the bench can kill this mid-clip.
        ferros_sys::sleep_until(now() + (freq() >> 4));
    }
    let hash = *h.finalize().as_bytes();
    say!("holdtest: {} bytes copied in {} ms, BLAKE3 {}", at, ms(t), core::str::from_utf8(&hex8(&hash)).unwrap_or("?"));
    match hold.seal_clip(c, at, &hash) {
        Ok(card) => say!("holdtest: sealed: {} bytes, its BLAKE3 the source's {}", card.size, card.body == sc.body),
        Err(e) => say!("holdtest: seal refused: {:?}", e),
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    say!("holdtest: panic: {}", info);
    ferros_sys::exit(1)
}
