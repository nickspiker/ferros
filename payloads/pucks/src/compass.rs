//! The compass puck (docs/PUCKS.md; owner, 2026-11-17: "GPS, compass, shit that's useful!").
//!
//! - **Engine**: ferros_compass: the device's top edge's heading from the magnetometers and gravity, tilt-compensated (the vector method), true north from WMM2025's declination at the kept place (against all 100 of NOAA's WMM2025 test values), the magnetometers' hard iron calibrated on the device (a running min and max per axis, its coverage against the field the model predicts there).
//! - **Data path**: the fused magnetometers and gravity from the facts (88-89 and the gravity pair 14-19: the sensors process publishes them, the kernel answers SYS_FACTS from its block; no right is needed, as for the clock's gravity).
//! - **Refresh**: only while its circle is shown (the display's WATCH), at the magnetometers' own rate (the sensors process subscribes them every 2^6 ms): each sample the dial may move. There is no doorbell on a sensor sample yet (the kernel reads the sensors' block in place and rings nothing), so the sample period is the wait, and only while shown; hidden, the puck sleeps until it is shown again. A face is sent only when the heading moved by half a pixel at the dial's size.
//! - **Kept**: its calibration, in its own vault domain as a VSF document (`compass`: offset_x, offset_y, offset_z in Q16 µT (i), samples (u)), written when the calibration has moved by a sixteenth of a microtesla, so a restart turns no dial wrong.

#![no_std]
#![no_main]

extern crate alloc;

#[path = "common.rs"]
mod common;

use ferros_compass::{Calibration, HardIron};
use ferros_faces::compass::{face, Compass};
use vsf::file_format::{VsfHeader, VsfSection};
use vsf::VsfType;

/// The magnetometers' period as the sensors process subscribes them (payloads/sensors main.rs): the wait while shown.
const SAMPLE_S: f64 = (1 << 6) as f64 / 1000.;
/// The domain key of the kept calibration.
const KEY: &[u8] = b"compass";

fn keep(domain: u64, c: &Calibration) -> bool {
    let mut s = VsfSection::new("compass");
    for (name, v) in [("offset_x", c.offset[0]), ("offset_y", c.offset[1]), ("offset_z", c.offset[2])] {
        s.add_field(name, VsfType::i((v * 65536.) as isize));
    }
    s.add_field("samples", VsfType::u(c.samples as usize, false));
    let made = ferros_puck::rt::eagle_now().unwrap_or(0);
    vsf::VsfBuilder::new().creation_time_oscillations(made).provenance_only().add_section_direct(s).build().is_ok_and(|d| ferros_sys::store_set(domain, KEY, &d))
}

fn load(domain: u64) -> Option<Calibration> {
    let mut b = [0u8; 1 << 9];
    let n = ferros_sys::store_get(domain, KEY, &mut b)?;
    // WHY: the document is under 2^8 bytes; a longer value is none of this puck's.
    if n > b.len() {
        return None;
    }
    let doc = &b[..n];
    vsf::verification::is_original(doc).ok()?;
    let (h, end) = VsfHeader::decode(doc).ok()?;
    let sections = h.sections(doc, end).ok()?;
    let s = sections.iter().find(|s| s.name == "compass")?;
    let int = |name: &str| s.get_field(name)?.values.iter().find_map(|v| matches!(v, VsfType::i(_) | VsfType::i3(_) | VsfType::i4(_) | VsfType::i5(_) | VsfType::i6(_)).then(|| v.as_i64()).flatten());
    let samples = s.get_field("samples")?.values.iter().find_map(|v| v.as_u64())?;
    Some(Calibration { offset: [int("offset_x")? as f32 / 65536., int("offset_y")? as f32 / 65536., int("offset_z")? as f32 / 65536.], samples: samples as u32 })
}

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, _input: u64, _input_len: u64) -> ! {
    let link = common::start(grants, false);
    common::start_place(grants);
    let domain = ferros_sys::cap(grants, common::S_DOMAIN);
    let kept = load(domain);
    let mut iron = HardIron::new();
    let mut last_kept = kept.unwrap_or(Calibration::NONE);
    let mut last_n = 0u32;
    let mut shown_heading: Option<f64> = None;
    loop {
        let seen = common::rt::bell();
        let v = common::rt::view();
        let f = ferros_sys::facts();
        let now = common::now_secs();
        if let Some((m, n)) = f.magnetic() {
            if n != last_n {
                last_n = n;
                iron.add(m);
            }
        }
        // The running fit once it has seen more than the kept one; until then the kept one (a restart keeps its calibration).
        let cal = if iron.samples() > last_kept.samples { iron.calibration() } else { last_kept };
        if iron.samples() > last_kept.samples && (0..3).any(|k| (cal.offset[k] - last_kept.offset[k]).abs() >= 1. / 16.) && keep(domain, &cal) {
            last_kept = cal;
        }
        let place = common::place();
        let declination = match (place, now) {
            (Some(p), Some(t)) => ferros_compass::declination(p.lat, p.lon, 0., t),
            _ => 0.,
        };
        let field_ut = match (place, now) {
            (Some(p), Some(t)) => (ferros_compass::field(p.lat, p.lon, 0., t).f / 1000.) as f32,
            // No place: a typical field's strength, for the coverage's scale only.
            _ => 50.,
        };
        let heading = match (f.magnetic(), f.gravity_pair()) {
            (Some((m, _)), Some(((g, _), _))) => ferros_compass::heading(cal.apply(m), g).map(|h| ferros_compass::true_heading(h, declination)),
            _ => None,
        };
        // Half a pixel at the dial's rim (0.84 of the radius): a smaller move changes nothing on the glass. WHY the floor of a pixel: before the display has said the circle's size it is 0, and the step would divide by it.
        let step = 1. / (2. * 0.84 * ((v.d as u32 / 2).max(1)) as f64);
        let moved = match (heading, shown_heading) {
            (Some(a), Some(b)) => {
                let d = a - b - core::f64::consts::TAU * libm::floor((a - b) / core::f64::consts::TAU);
                d.min(core::f64::consts::TAU - d) >= step
            }
            (a, b) => a.is_some() != b.is_some(),
        };
        if moved || shown_heading.is_none() {
            shown_heading = heading;
            let c = Compass { heading, declination, coverage: iron.coverage(field_ut) };
            common::show(link, |w, focus| face(w, &c, focus));
        }
        let deadline = match (v.shown, ferros_puck::rt::eagle_now()) {
            (true, Some(t)) => Some(t + (SAMPLE_S * ferros_puck::OSC as f64) as i64),
            _ => None,
        };
        common::wait(seen, deadline);
    }
}

ferros_sys::entry!(payload_main);
