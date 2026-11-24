//! The tide puck (docs/PUCKS.md "The tide puck"; owner, 2026-11-17: "plus a tide puck").
//!
//! - **Engine**: ferros_tide (NOAA's 37 constituents, Schureman's nodal corrections, highs and lows by root finding; tested against NOAA's published tables at four stations), the engine of record; the sun and the moon from ferros_sky.
//! - **Face**: the owner's tide widget (owner, 2026-11-24: "the tide one, the inksurf/tide android apk/widget is what the tides should look like on ferros"), drawn per pixel into the square (ferros_faces::tide::paint): the day's water and sky centred on now, the sun's and moon's curves, the hours, now's time, the highs and lows, sunrise and sunset; in focus the station's name and distance.
//! - **Station** (the location step): the nearest in the bundled catalog to the puck's kept place (the ~10 km cell of its last fix), by the largest dot product of unit vectors, so no seam at the antimeridian or the poles. One station, never a blend: a tide's phase changes across a coast (amphidromes, estuaries), and an average of two stations' curves is no tide anywhere; NOAA's own practice is one reference station per place. Its distance is shown in focus, so a far station is seen to be far.
//! - **Refresh**: the chart is centred on now, so it is drawn again when now has moved half a pixel along it at the centre, or when now's time turns over in the person's base, whichever is first (ferros_faces::tide::deadline); a new place (the place thread's fix) or a view change wakes it too. Nothing while the circle is off the glass.
//! - Nothing of the place is printed, logged or sent; the station's id is public.

#![no_std]
#![no_main]

extern crate alloc;

#[path = "common.rs"]
mod common;

use ferros_faces::tide;
use ferros_tide::station::{unit, Catalog, Station};
use ferros_tide::Tide;

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
pub extern "C" fn payload_main(grants: u64, input: u64, input_len: u64) -> ! {
    let link = common::start(grants, input, input_len, false);
    common::start_place(grants);
    let Ok(catalog) = Catalog::parse(bundled::CATALOG) else {
        common::say(grants, b"tide: the bundled catalog does not read\n");
        ferros_sys::exit(1);
    };
    let mut using: Option<Using> = None;
    // The year of sky and tide behind the chart's far columns and the face's backgrounds, kept between squares.
    let mut cache = tide::Cache::new();
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
                        let v = common::rt::view();
                        // The great-circle distance from the cosine of its angle (WHY the bound: rounding can put a dot product of unit vectors a hair past 1, outside acos's domain).
                        let distance_m = libm::acos(cos.min(1.)) * 6_371_000.;
                        let chart = tide::Chart {
                            tide: &u.tide,
                            scale: u.scale,
                            place: p,
                            now,
                            zone: ferros_sys::zone(),
                            base: ferros_sys::device_base() & 0x7F,
                            station: v.large.then_some((u.name.as_str(), distance_m)),
                        };
                        if let Some(glyphs) = common::glyphs() {
                            common::show_square(link, |px, d, turn| tide::paint(px, d, turn, &chart, &mut cache, glyphs));
                        }
                        if v.shown {
                            deadline = Some(ferros_faces::osc(tide::deadline(&chart, v.d as usize)));
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
ferros_sys::entry!(payload_main, grows ferros_sys::HEAP_DEVICE, starts common::HEAP_START_LOG2);
