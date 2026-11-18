//! What every one of ferros's own pucks shares (docs/PUCKS.md): its heap, its slots, its start, and sending its two faces.
//!
//! Grant slots (ferros_kist `slots`: a capsule that declares no implicit right): 1 display, 2 touch, 3 its surface (empty: a puck holds none), 4 ira, 5 its own vault domain, 6 entropy; then its wants in order, 7 `puck` (its circle), 8 `gps` (asked: its place), 9 `udp` (the weather's alone); then the GPU's three (empty for a puck) and audio.

use core::cell::UnsafeCell;
pub use ferros_puck::rt::{self, Link};
use ferros_puck::{call, Full, Writer, LIST_MAX};
use ferros_glyphs::Glyphs;

// WHY 2 MiB: the typeface's cache (64 KiB of table, 64 KiB of coverage) and swash's scaling beside the engines' own.
const HEAP_SIZE: usize = 2 << 20;
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

/// Relocate, the heap, the typeface, the view's watch: the puck's link to its circle. `frames`: the face moves continuously and is drawn at every frame the deck shows (the clock).
pub fn start(grants: u64, frames: bool) -> Link {
    unsafe { ferros_sys::relocate() };
    unsafe { ALLOC.init(HEAP.0.get() as usize, HEAP_SIZE) };
    unsafe { GLYPHS = Glyphs::with_arena(ferros_glyphs::FONT, 1 << 15) };
    let link = Link { grants, slot: S_PUCK, square: ferros_sys::kist::SURFACE };
    rt::watch(link, frames);
    link
}

/// The typeface, once `start` has made it.
static mut GLYPHS: Option<Glyphs> = None;

/// The typeface, for a puck that draws its square itself (the clock).
#[allow(dead_code)]
pub fn glyphs() -> Option<&'static mut Glyphs> {
    unsafe { (*(&raw mut GLYPHS)).as_mut() }
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

/// When the last face's time-anchored ops next move half a pixel (ferros_puck::draw::next_change), folded into `wait`.
static mut NEXT: Option<i64> = None;

/// Write the face for the circle as it is now (`face(w, large)`: the fuller face when the circle is at least half the panel across) and draw it into the square for the display. Nothing while the circle is not on the glass. A face too big for a list is a bug in this puck: said, and the last square stays.
pub fn show(link: Link, mut face: impl FnMut(&mut Writer, bool) -> Result<(), Full>) {
    let v = rt::view();
    if !v.shown {
        unsafe { NEXT = None };
        return;
    }
    let mut buf = [0u8; LIST_MAX];
    let mut w = Writer::new(&mut buf);
    if face(&mut w, v.large).is_err() {
        ferros_sys::console(link.grants, b"puck: a face did not fit its list; the last one stays\n");
        return;
    }
    let l = w.bytes();
    let Some(glyphs) = (unsafe { (*(&raw mut GLYPHS)).as_mut() }) else {
        ferros_sys::console(link.grants, b"puck: no typeface; nothing drawn\n");
        return;
    };
    if rt::show(link, &v, l, &rt::theme(), glyphs) == Some(call::REFUSED) {
        ferros_sys::console(link.grants, b"puck: the display refused a square\n");
    }
    unsafe { NEXT = rt::eagle_now().and_then(|now| ferros_puck::draw::next_change(l, v.d as f32 * 0.5, now)) };
}

/// Sleep until the bell moves (the view, a tap, a frame, a worker), the content's own `deadline`, or the last face's next move, whichever is first.
pub fn wait(seen: u64, deadline: Option<i64>) -> u64 {
    let next = unsafe { NEXT };
    let at = match (deadline, next) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    rt::wait(seen, at)
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
