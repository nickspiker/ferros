//! The tide puck's data path on the device, without drawing and without anyone's place (docs/PUCKS.md "What is built"): the bundled catalog and station documents parsed (ferros_tide::station, vsf no_std), the nearest station found from each station's own published place (never the person's: no GPS here), its model made, and the face computed for the clock's now, each step timed.
//! One console line per station (its id, which is public, and microseconds) and a last line with the totals; nothing stored, nothing sent.
//!
//! `ferros-bridge spawn tidetest.bin --name tidetest`. Grant slots: 0 console.

#![no_std]
#![no_main]

extern crate alloc;

use core::cell::UnsafeCell;
use core::fmt::Write;
use ferros_sys::{now, us_since};
use ferros_tide::station::{Catalog, Station};
use ferros_tide::{Face, eagle_from_unix};

const HEAP_SIZE: usize = 1 << 20;
#[repr(C, align(64))]
struct Region<const N: usize>(UnsafeCell<[u8; N]>);
unsafe impl<const N: usize> Sync for Region<N> {}
static HEAP: Region<HEAP_SIZE> = Region(UnsafeCell::new([0; HEAP_SIZE]));

#[global_allocator]
static ALLOC: ferros_heap::LockedHeap = ferros_heap::LockedHeap::empty();

/// The bundled station documents (build.rs: every ferros_tide/stations/*.tide.vsf) and the catalog.
mod bundled {
    include!(concat!(env!("OUT_DIR"), "/stations.rs"));
}

/// A console line.
struct Line {
    b: [u8; 1 << 8],
    n: usize,
}

impl Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        // Every line here is under half the buffer; a longer one is a bug in this file, and the slice panics on it.
        self.b[self.n..self.n + s.len()].copy_from_slice(s.as_bytes());
        self.n += s.len();
        Ok(())
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    unsafe { ALLOC.init(HEAP.0.get() as usize, HEAP_SIZE) };
    let say = |l: &Line| ferros_sys::console(grants, &l.b[..l.n]);
    let Some((unix, _, _)) = ferros_sys::utc_time() else {
        ferros_sys::console(grants, b"tidetest: no clock yet\n");
        ferros_sys::exit(1);
    };
    let at = eagle_from_unix(unix);

    let t0 = now();
    let catalog = match Catalog::parse(bundled::CATALOG) {
        Ok(c) => c,
        Err(e) => {
            let mut l = Line { b: [0; 1 << 8], n: 0 };
            let _ = write!(l, "tidetest: the catalog does not read: {e}\n");
            say(&l);
            ferros_sys::exit(1);
        }
    };
    let catalog_us = us_since(t0);
    let (mut bad, mut parse_us, mut near_us, mut face_us) = (0u32, 0u64, 0u64, 0u64);
    for &(id, doc) in bundled::STATIONS {
        let mut l = Line { b: [0; 1 << 8], n: 0 };
        let t = now();
        let station = match Station::parse(doc) {
            Ok((s, _)) => s,
            Err(e) => {
                let _ = write!(l, "tidetest: {id} does not read: {e}\n");
                say(&l);
                bad += 1;
                continue;
            }
        };
        let p = us_since(t);
        // Its own place: the nearest station to a station is itself.
        let t = now();
        let found = catalog.nearest(station.position.to_xyz()).map(|(k, _)| catalog.stations[k].id == station.id);
        let n = us_since(t);
        let t = now();
        let (tide, unknown) = station.tide();
        let face = Face::at(&tide, at);
        let f = us_since(t);
        parse_us += p;
        near_us += n;
        face_us += f;
        let ok = found == Some(true) && unknown.is_empty() && face.is_some();
        bad += !ok as u32;
        let _ = write!(l, "tidetest: {id}: {}; parsed G#{:X} us, nearest G#{:X} us, face G#{:X} us\n", if ok { "ok" } else { "WRONG" }, p, n, f);
        say(&l);
    }
    let mut l = Line { b: [0; 1 << 8], n: 0 };
    let _ = write!(l, "tidetest: G#{:X} stations, G#{:X} wrong; catalog G#{:X} us, parsing G#{:X}, nearest G#{:X}, faces G#{:X} us\n", bundled::STATIONS.len(), bad, catalog_us, parse_us, near_us, face_us);
    say(&l);
    ferros_sys::exit(bad as u64);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}

// The entry and its image header (ferros_sys::entry!: the kernel sizes the arc from the image and its .bss).
ferros_sys::entry!(payload_main);
