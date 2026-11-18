//! What every one of ferros's own pucks shares (docs/PUCKS.md): its heap, its slots, its start, and sending its two faces.
//!
//! Grant slots (ferros_kist `slots`: a capsule that declares no implicit right): 1 display, 2 touch, 3 its surface (empty: a puck holds none), 4 ira, 5 its own vault domain, 6 entropy; then its wants in order, 7 `puck` (its circle), 8 `gps` (asked: its place), 9 `udp` (the weather's alone); then the GPU's three (empty for a puck) and audio.

use core::cell::UnsafeCell;
pub use ferros_puck::rt::{self, Link};
use ferros_puck::{call, Full, Writer, LIST_MAX};

const HEAP_SIZE: usize = 1 << 20;
#[repr(C, align(64))]
struct Region<const N: usize>(UnsafeCell<[u8; N]>);
unsafe impl<const N: usize> Sync for Region<N> {}
static HEAP: Region<HEAP_SIZE> = Region(UnsafeCell::new([0; HEAP_SIZE]));
#[global_allocator]
static ALLOC: ferros_heap::LockedHeap = ferros_heap::LockedHeap::empty();

/// Its circle (`puck`), its place (`gps`, when the person grants it), the network (`udp`, the weather's).
pub const S_PUCK: u64 = ferros_sys::kist::asked(0);
pub const S_GPS: u64 = ferros_sys::kist::asked(1);
#[allow(dead_code)]
pub const S_UDP: u64 = ferros_sys::kist::asked(2);
/// Its own vault domain: the place it keeps.
pub const S_DOMAIN: u64 = ferros_sys::kist::DOMAIN;

/// Relocate, the heap, the view's watch: the puck's link to its circle.
pub fn start(grants: u64) -> Link {
    unsafe { ferros_sys::relocate() };
    unsafe { ALLOC.init(HEAP.0.get() as usize, HEAP_SIZE) };
    let link = Link { grants, slot: S_PUCK };
    rt::watch(link);
    link
}

/// The place shared by the place-bound pucks: the kept cell at start, and a fresh fix while shown when the person granted GPS (ferros_puck::rt::place).
#[allow(dead_code)]
pub fn start_place(grants: u64) {
    rt::place::start(grants, ferros_sys::cap(grants, S_DOMAIN), Some(S_GPS));
}

/// The place as the engines take it: the kept cell's centre, never finer than the cell.
#[allow(dead_code)]
pub fn place() -> Option<ferros_sky::Place> {
    let (cell, _) = rt::place::current()?;
    let (lat, lon) = cell.centre();
    Some(ferros_sky::Place::deg(lat, lon))
}

/// Now in Eagle seconds (the engines'), None before the kernel has read the clock.
#[allow(dead_code)]
pub fn now_secs() -> Option<f64> {
    rt::eagle_now().map(|t| t as f64 / ferros_puck::OSC as f64)
}

/// The lists last sent, at rest and in focus: a list unchanged is not sent again.
static mut SENT: [[u8; LIST_MAX]; 2] = [[0; LIST_MAX]; 2];
static mut SENT_LEN: [usize; 2] = [0; 2];
static mut GENERATION: u32 = 0;

/// Write the face both ways (`face(w, focus)`) and send each that changed. A face too big for a list is a bug in this puck: said, and the last good one stays on the glass.
pub fn show(link: Link, mut face: impl FnMut(&mut Writer, bool) -> Result<(), Full>) {
    for (v, focus) in [(call::REST, false), (call::FOCUS, true)] {
        let mut buf = [0u8; LIST_MAX];
        let mut w = Writer::new(&mut buf);
        if face(&mut w, focus).is_err() {
            ferros_sys::console(link.grants, b"puck: a face did not fit its list; the last one stays\n");
            continue;
        }
        let l = w.bytes();
        let (sent, len) = unsafe { (&mut (*(&raw mut SENT))[v as usize], &mut (*(&raw mut SENT_LEN))[v as usize]) };
        if &sent[..*len] == l {
            continue;
        }
        let g = unsafe {
            GENERATION = GENERATION.wrapping_add(1);
            GENERATION
        };
        if link.send(v, g, l) == call::REFUSED {
            ferros_sys::console(link.grants, b"puck: the display refused a face\n");
            continue;
        }
        sent[..l.len()].copy_from_slice(l);
        *len = l.len();
    }
}

/// A console line.
#[allow(dead_code)]
pub fn say(grants: u64, s: &[u8]) {
    ferros_sys::console(grants, s);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
