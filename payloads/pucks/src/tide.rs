//! The tide puck (docs/PUCKS.md "The tide puck"; owner, 2026-11-17: "plus a tide puck").
//!
//! - **Engine**: ferros_tide (NOAA's 37 constituents, Schureman's nodal corrections, highs and lows by root finding; tested against NOAA's published tables at four stations). It is the engine of record; the owner's inksurf engine is the same method on one hard-coded station, so its look is reused (ferros_faces::tide) and its engine is not.
//! - **Station** (the location step): the nearest in the bundled catalog to the puck's kept place (the ~10 km cell of its last fix), by the largest dot product of unit vectors, so no seam at the antimeridian or the poles. One station, never a blend: a tide's phase changes across a coast (amphidromes, estuaries), and an average of two stations' curves is no tide anywhere; NOAA's own practice is one reference station per place. Its distance is shown in focus, so a far station is seen to be far.
//! - **Refresh**: at the earlier of the next high or low (its words go stale then) and now riding off its curve; between, the display moves now along the curve. A new place (the place thread's fix) or a view change wakes it too.
//! - Nothing of the place is printed, logged or sent; the station's id is public.

#![no_std]
#![no_main]

extern crate alloc;

#[path = "common.rs"]
mod common;

use ferros_faces::tide;
use ferros_tide::station::{unit, Catalog, Station};
use ferros_tide::{Face, Tide};

mod bundled {
    include!(concat!(env!("OUT_DIR"), "/stations.rs"));
}

/// The station in use: its index in the catalog, its tide, its scale.
struct Using {
    k: usize,
    tide: Tide,
    scale: (f32, f32),
    name: alloc::string::String,
}

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, _input: u64, _input_len: u64) -> ! {
    let link = common::start(grants, false);
    common::start_place(grants);
    let Ok(catalog) = Catalog::parse(bundled::CATALOG) else {
        common::say(grants, b"tide: the bundled catalog does not read\n");
        ferros_sys::exit(1);
    };
    let mut using: Option<Using> = None;
    loop {
        let seen = common::rt::bell();
        let mut deadline = None;
        match (common::now_secs(), common::place()) {
            (Some(now), Some(p)) => {
                let at = unit(p.lat.to_degrees(), p.lon.to_degrees());
                if let Some((k, cos)) = catalog.nearest(at) {
                    if using.as_ref().is_none_or(|u| u.k != k) {
                        let id = catalog.stations[k].id.as_str();
                        using = bundled::STATIONS.iter().find(|(sid, _)| *sid == id).and_then(|(_, doc)| Station::parse(doc).ok()).map(|(s, _)| {
                            let (tide, _) = s.tide();
                            Using { k, scale: tide::scale(&tide), tide, name: s.name }
                        });
                    }
                    if let Some(u) = using.as_ref() {
                        if let Some(face) = Face::at(&u.tide, now) {
                            // The great-circle distance from the cosine of its angle (WHY the bound: rounding can put a dot product of unit vectors a hair past 1, outside acos's domain).
                            let distance_m = libm::acos(cos.min(1.)) * 6_371_000.;
                            let about = tide::About { scale: u.scale, place: Some(p), station: &u.name, distance_m };
                            common::show(link, |w, focus| tide::face(w, &face, &about, focus));
                            deadline = Some(ferros_faces::osc(tide::deadline(&face)));
                        }
                    }
                }
            }
            // No place yet (GPS not granted, or no fix so far): the circle shows its sky alone until one comes.
            _ => common::show(link, |w, _| ferros_faces::sky(w).map(|_| ())),
        }
        common::wait(seen, deadline);
    }
}

// The arc grows on demand to 2^26 (64 MiB of addresses): the scratch its square is drawn in before it goes thru the handle, as large as its circle asks (ferros_puck::rt `scratch`; at most two squares of `SIDE_MAX` fill the slot, one fills half of this).
ferros_sys::entry!(payload_main, grows 1 << 26);
