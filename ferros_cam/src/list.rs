//! The `camera` endpoint's list (docs/CAMERA.md "The endpoint"), built by the driver at LIST from the cameras it can stream and each one's IDT as the vault holds it (`cam.<key>.idt`, the device's calibration beside the sensor's init, never in a kist): a VSF document with a section `camera` per camera.
//! The IDT's Absolute entry gives the section its `idt` (row-major, camera to VSF RGB) and `sensitivity`; everything else comes from ferros_cam cams.

extern crate alloc;

use crate::cams::Cam;
use alloc::vec::Vec;
use vsf::VsfType;

/// Why a camera's IDT does not read.
#[derive(Debug, PartialEq)]
pub enum IdtError {
    /// Not a VSF document, or no `characterization` section in it.
    Document,
    /// The profile does not parse, or has no Absolute entry, or that entry no scale.
    Profile,
    /// The 1:1 init (`cam.<key>.init.full`) leaves no 1:1 mode (ferros_cam ccs `Mode::captured_full`).
    Mode(crate::ccs::Error),
}

/// One listed readout of a camera: the OPEN mode block's kind that opens it, its size, its mosaic's period, its timing.
/// A camera has a section per readout, each with the camera's `id` and `name` (the lens: what a client groups them by) and its own `mode`, size, `cfa_period`, exposure range and frame period.
pub struct Readout {
    /// The mode block's kind: 0 the binned init's own (the same as an OPEN with no block), 2 the 1:1 init's (payloads/camera `K_FULL`).
    pub kind: u8,
    pub width: u16,
    pub height: u16,
    /// The colour filter's period in pixels: 2 a Bayer mosaic (the binned modes: each binned pixel one 2 x 2 same-colour block of the quad array), 4 the quad array as it is at 1:1 (2 x 2 blocks of one colour, the blocks in `cfa`'s order).
    pub period: u16,
    pub timing: crate::ccs::Timing,
    /// Its own frame period (ns): the init's frame of lines at its line, the frame rate while an exposure fits it.
    pub frame_ns: u64,
    /// The array pixels it reads, width and height (its size times the pixels each output pixel gathers): its field of view (ferros_cam optics `Optics::field`).
    pub covered: (u32, u32),
}

/// The kind of the mode block that opens a 1:1 readout (payloads/camera `K_FULL`).
pub const KIND_FULL: u8 = 2;

impl Readout {
    /// The camera's binned readout, its captured init's (as the list has offered since 2026-11-14).
    pub fn binned(c: &Cam) -> Readout {
        Readout { kind: 0, width: c.width as u16, height: c.height as u16, period: 2, timing: c.listed_timing(), frame_ns: c.fll as u64 * c.line_ps / 1000, covered: c.binned_field() }
    }
    /// The camera's 1:1 readout from its 1:1 init (`cam.<key>.init.full`): the mode it leaves (`Mode::captured_full`), at the line time its PLL gives (`line_ps_for`).
    pub fn full(c: &Cam, init: &[u8]) -> Result<Readout, IdtError> {
        let a = c.array;
        let m = crate::ccs::Mode::captured_full(init, a).map_err(IdtError::Mode)?;
        let line = crate::ccs::line_ps_for(c, &crate::ccs::Page::of(init));
        let timing = c.listed_mode_timing(line, &m);
        Ok(Readout { kind: KIND_FULL, width: m.x_out, height: m.y_out, period: a.period, timing, frame_ns: timing.frame_ns(&timing.own(1)), covered: (m.x_out as u32, m.y_out as u32) })
    }
    /// The camera's captured readout `alts[k]` (kind `cams::ALT + k`) from its own init (`cam.<key>.init.<tag>`): its size and period, the init's line and frame at the line its PLL gives (`line_ps_for`).
    pub fn alt(c: &Cam, k: usize, init: &[u8]) -> Readout {
        let a = &c.alts[k];
        let p = crate::ccs::Page::of(init);
        let llp = p.get(crate::ccs::LINE_LENGTH_PCK).map_or(c.llp, |v| v as u16);
        let fll = p.get(crate::ccs::FRAME_LENGTH_LINES).map_or(c.fll, |v| v as u16);
        // A readout's own measured line is at its own line length (the tele's share the 22-degree's, 5568 clocks); else the camera's, scaled by the init's PLL.
        let line = if a.line_ps != 0 { a.line_ps * c.llp as u64 / llp as u64 } else { crate::ccs::line_ps_for(c, &p) };
        let timing = c.listed_timing_at(line, llp, fll);
        Readout { kind: crate::cams::ALT + k as u8, width: a.width as u16, height: a.height as u16, period: a.period, timing, frame_ns: timing.frame_ns(&timing.own(1)), covered: (a.width as u32 * a.gather as u32, a.height as u32 * a.gather as u32) }
    }
}

/// Camera `c`'s section for its binned readout, its IDT's Absolute entry (`idt` the bytes of its `.idt.vsf`) carried in it.
pub fn section(c: &Cam, idt: &[u8]) -> Result<vsf::file_format::VsfSection, IdtError> {
    section_of(c, idt, &Readout::binned(c))
}

/// Camera `c`'s section for readout `r`.
pub fn section_of(c: &Cam, idt: &[u8], r: &Readout) -> Result<vsf::file_format::VsfSection, IdtError> {
    let doc = vsf::container::Document::open(idt).map_err(|_| IdtError::Document)?;
    let sec = doc.section(doc.entry("characterization").ok_or(IdtError::Document)?).map_err(|_| IdtError::Document)?;
    let prof = vsf::spectral_image::profile_from_fields(&sec.fields).map_err(|_| IdtError::Profile)?;
    let abs = prof.entries.iter().find(|e| e.class == vsf::spectral_image::IdtClass::Absolute).ok_or(IdtError::Profile)?;
    let sens = abs.scale.as_ref().ok_or(IdtError::Profile)?.sensitivity;

    let mut s = vsf::file_format::VsfSection::new("camera");
    let mut put = |name: &str, values: Vec<VsfType>| {
        let mut f = vsf::file_format::VsfField::new(name);
        f.values = values;
        s.fields.push(f);
    };
    let u = |v: u64| alloc::vec![VsfType::u(v as usize, false)];
    put("id", u(c.id as u64));
    put("name", alloc::vec![VsfType::a(vsf::Ascii::lit(c.name))]);
    put("facing", u(c.facing as u64));
    // The readout: the OPEN mode block's kind that opens it (0 the binned init's, as no block; 2 the 1:1), its size, the mosaic's period (2 Bayer, 4 the quad array at 1:1, its 2 x 2 blocks in `cfa`'s order).
    put("mode", u(r.kind as u64));
    put("width", u(r.width as u64));
    put("height", u(r.height as u64));
    put("cfa", u(c.cfa as u64));
    put("cfa_period", u(r.period as u64));
    put("black", u(c.black as u64));
    put("white", u(c.white as u64));
    put("rotation", u(c.rotation as u64));
    put("idt", abs.matrix.iter().map(|v| VsfType::f5(*v)).collect());
    put("sensitivity", sens.iter().map(|v| VsfType::f5(*v)).collect());
    // Exposure: one line to the longest offered, per readout: past the init's frame where the camera's margin was measured, the frame lengthened, then the vendor shift up to the longest its frame counter holds (ferros_cam ccs `Timing::most_ns`, `Lte::frame_bits`: the main 13524 s binned and 28286 s at 1:1, measured on the Pro 2026-11-18; the ultrawide, no shift, its 16-bit frame, 0.430 s); the init's own integration elsewhere. SET refuses past it.
    let t = r.timing;
    put("exposure_min", u(t.line_ps_at(t.llp) / 1000));
    put("exposure_max", u(t.most_ns()));
    // The mode's own frame period (ns): its frame of lines at its line, the frame rate while the exposure fits it (the binned main 67.3 ms, its 1:1 41.7 ms).
    put("frame_ns", u(r.frame_ns));
    // The readout's line (ps) at its own line length, and the shortest whole frame the driver streams it at (its rows and `BLANK_LEAST`): what a client paces any rate from (ferros_clip `Line::of_ps`, `Pacing::of_line`; 2026-11-21, RAW video on every camera, whose lines differ: the main 3.149 us, the ultrawide 6.562, the front 10.438, the tele 6.107).
    put("line_ps", u(t.line_ps_at(t.llp)));
    put("fll_least", u(r.height as u64 + crate::BLANK_LEAST as u64));
    // The readout at the longest exposure (ns), first row to last, the rolling shutter's skew (owner, 2026-11-17: a camera that ever stretches its line says what its readout costs; the default route never does, so this is the mode's own).
    put("readout", u(t.readout_ps(&t.own(1), r.height as u64) / 1000));
    // Gain (1024 one): the sensor's range as measured from its own capability words (ferros_cam ccs `ARRAYS`' gain, `Gain::range`); the captured code alone (1024 and 1024) for a camera not yet measured.
    let (gmin, gmax) = c.array.gain;
    put("gain_min", u(gmin));
    put("gain_max", u(gmax));
    // Focus: the lens actuator's targets over the span measured (the main's G#0800 to G#7800); 0 and 0 where focus is not offered.
    put("focus_min", u(c.focus.0));
    put("focus_max", u(c.focus.1));
    // The optics (ferros_cam optics, the camera's `Cam::optics`: Android's Camera2 values, on the Pro as Lumis recorded them, on the 8a its dumpsys): the sensor's size in mm, the focal length in mm, the f-number.
    let o = c.optics;
    put("sensor_width_mm", alloc::vec![VsfType::f5(o.sensor_mm.0)]);
    put("sensor_height_mm", alloc::vec![VsfType::f5(o.sensor_mm.1)]);
    put("focal_mm", alloc::vec![VsfType::f5(o.focal_mm)]);
    put("f_number", alloc::vec![VsfType::f5(o.f_number)]);
    // The readout's own field (ferros_cam optics `Optics::field`, owner via the Lumis session, 2026-11-18: what tells two readouts of one lens apart in a menu, the tele's two 4032 x 3024 at 22 and 11 degrees): the sensor's part it reads in mm (its pixels over the array's, `Readout::covered`), the angles across its width, height and diagonal in degrees, and its 35 mm equivalent focal length.
    let fd = o.field(r.covered);
    put("field_width_mm", alloc::vec![VsfType::f5(fd.mm.0)]);
    put("field_height_mm", alloc::vec![VsfType::f5(fd.mm.1)]);
    put("fov_width_deg", alloc::vec![VsfType::f5(fd.fov_deg.0)]);
    put("fov_height_deg", alloc::vec![VsfType::f5(fd.fov_deg.1)]);
    put("fov_diagonal_deg", alloc::vec![VsfType::f5(fd.fov_deg.2)]);
    put("focal_equiv_mm", alloc::vec![VsfType::f5(fd.focal_equiv_mm)]);
    Ok(s)
}

/// The list document from its sections (none: an empty document).
pub fn document(sections: Vec<vsf::file_format::VsfSection>) -> Option<Vec<u8>> {
    let mut b = vsf::VsfBuilder::new();
    for s in sections {
        b = b.add_section_direct(s);
    }
    b.build().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cams;

    /// Each camera's IDT file in the repository, by id (the calibration's module numbers: cam2 the main, cam3 the ultrawide, cam7 the front, cam4 the tele).
    const IDT: [&str; 4] = ["cam2-main", "cam3-ultrawide", "cam7-front", "cam4-tele"];

    fn idt(c: &Cam) -> Vec<u8> {
        std::fs::read(format!("{}/../pixel 8 pro calibration data/derived/{}.idt.vsf", env!("CARGO_MANIFEST_DIR"), IDT[c.id as usize])).unwrap()
    }

    /// Every camera's list section from its own IDT file, read back as Lumis's parser reads it (std/lumis camera.rs `parse_list`): every field present, its own matrix and sensitivity, and no two cameras with the same matrix.
    #[test]
    fn the_list_reads_as_lumis_reads_it() {
        let secs: Vec<_> = cams::CAMS.iter().map(|c| section(c, &idt(c)).unwrap()).collect();
        let doc = document(secs).unwrap();
        let d = vsf::container::Document::open(&doc[..]).unwrap();
        let mut seen: Vec<Vec<f32>> = Vec::new();
        for (k, e) in d.entries_named("camera").into_iter().enumerate() {
            let s = d.section(e).unwrap();
            let c = cams::CAMS[k];
            let floats = |name: &str| -> Vec<f32> { s.get_field(name).unwrap().values.iter().map(|v| if let VsfType::f5(x) = v { *x } else { panic!() }).collect() };
            assert_eq!(s.uint("id"), Some(c.id as u64));
            assert_eq!(s.text("name"), Some(c.name));
            for f in ["facing", "mode", "width", "height", "cfa", "cfa_period", "frame_ns", "line_ps", "fll_least", "black", "white", "rotation", "exposure_min", "exposure_max", "readout", "gain_min", "gain_max", "focus_min", "focus_max"] {
                assert!(s.uint(f).is_some(), "{} {f}", c.name);
            }
            let o = c.optics;
            assert_eq!((floats("sensor_width_mm"), floats("sensor_height_mm"), floats("focal_mm"), floats("f_number")), (vec![o.sensor_mm.0], vec![o.sensor_mm.1], vec![o.focal_mm], vec![o.f_number]), "{}", c.name);
            let (m, sens) = (floats("idt"), floats("sensitivity"));
            assert_eq!((m.len(), sens.len()), (9, 3));
            assert!(!seen.contains(&m), "{} has another camera's matrix", c.name);
            println!("{}: idt {m:?}, sensitivity {sens:?}", c.name);
            seen.push(m);
        }
        assert_eq!(seen.len(), 4);
    }

    /// Every readout's field as the list carries it, read back from the document: the binned readouts and the 1:1 ones read the whole array (Android's diagonal angles: the main 83.2, the ultrawide 121.7, the tele 22.2), the tele's 11 degrees its middle quarter (Android's virtual camera 6, 11.2), the front's 8.42 MP its middle 3440 x 2448; no two readouts of one lens with the same size and the same field.
    /// The captured readouts' sections need no init for the field (an empty one gives the camera's own line and frame).
    #[test]
    fn every_readouts_field() {
        let mut secs = Vec::new();
        let mut want = Vec::new();
        for c in cams::CAMS.iter() {
            secs.push(section(c, &idt(c)).unwrap());
            want.push((c.id, 0u8, [83.2f32, 121.7, 0., 22.2][c.id as usize]));
            if let (0, Ok(full)) = (c.id, std::fs::read("/mnt/Harbor/Code/ferros/target/cam/main-full.init")) {
                secs.push(section_of(c, &idt(c), &Readout::full(c, &full).unwrap()).unwrap());
                want.push((0, KIND_FULL, 83.2));
            }
            for k in 0..c.alts.len() {
                secs.push(section_of(c, &idt(c), &Readout::alt(c, k, &[])).unwrap());
                want.push((c.id, cams::ALT + k as u8, match (c.id, k) { (3, 0) => 11.2, (2, 0) => 0., _ => [83.2f32, 121.7, 0., 22.2][c.id as usize] }));
            }
        }
        let doc = document(secs).unwrap();
        let d = vsf::container::Document::open(&doc[..]).unwrap();
        let f = |s: &vsf::file_format::VsfSection, name: &str| -> f32 { if let [VsfType::f5(x)] = s.get_field(name).unwrap().values[..] { x } else { panic!("{name}") } };
        let mut seen: Vec<(u64, u64, u64, i64)> = Vec::new();
        for (e, &(id, kind, diag)) in d.entries_named("camera").into_iter().zip(want.iter()) {
            let s = d.section(e).unwrap();
            assert_eq!((s.uint("id"), s.uint("mode")), (Some(id as u64), Some(kind as u64)));
            let (fw, fh, aw, ah, ad, eq) = (f(&s, "field_width_mm"), f(&s, "field_height_mm"), f(&s, "fov_width_deg"), f(&s, "fov_height_deg"), f(&s, "fov_diagonal_deg"), f(&s, "focal_equiv_mm"));
            println!("{} mode {}: {} x {}, field {fw:.4} x {fh:.4} mm, {aw:.2} x {ah:.2} degrees, {ad:.2} diagonal, {eq:.2} mm equivalent", s.text("name").unwrap(), kind, s.uint("width").unwrap(), s.uint("height").unwrap());
            if diag != 0. {
                assert!((ad - diag).abs() < 0.05, "camera {id} mode {kind}: {ad} against {diag}");
            }
            assert!(aw < ad && ah < aw && fh < fw);
            let key = (id as u64, s.uint("width").unwrap(), s.uint("height").unwrap(), (ad * 16.) as i64);
            assert!(!seen.contains(&key), "two readouts alike: {key:?}");
            seen.push(key);
        }
        // The front's two: 10.5 MP its whole array (the 35 mm equivalent its captures recorded, 20.61), 8.42 MP its middle 3440 x 2448, narrower by 3440 / 3840 on the width.
        let front: Vec<_> = d.entries_named("camera").into_iter().map(|e| d.section(e).unwrap()).filter(|s| s.uint("id") == Some(2)).collect();
        assert!((f(&front[0], "focal_equiv_mm") - 20.6108).abs() < 0.01);
        assert!((f(&front[1], "field_width_mm") / f(&front[0], "field_width_mm") - 3440. / 3840.).abs() < 1e-5);
        // The tele's two 4032 x 3024: 22 and 11 degrees, a quarter of the array's area apart.
        let tele: Vec<_> = d.entries_named("camera").into_iter().map(|e| d.section(e).unwrap()).filter(|s| s.uint("id") == Some(3)).collect();
        assert_eq!((tele[0].uint("width"), tele[1].uint("width")), (Some(4032), Some(4032)));
        assert!((f(&tele[0], "field_width_mm") - 2. * f(&tele[1], "field_width_mm")).abs() < 1e-5);
    }

    /// The 8a's main lists its own sensor and lens (ferros_cam optics `AKITA_OPTICS`, its Camera2 report), not the Pro's main's of the same id, and its binned readout reads its whole array; its IDT the Pro ultrawide's stand-in, as the 8a's vault holds it (docs/CAMERA.md "The 8a's main streams").
    #[test]
    fn the_8as_main_lists_its_own_optics() {
        let c = &cams::AKITA_MAIN;
        let doc = document(vec![section(c, &idt(&cams::ULTRAWIDE)).unwrap()]).unwrap();
        let d = vsf::container::Document::open(&doc[..]).unwrap();
        let s = d.section(d.entries_named("camera").next().unwrap()).unwrap();
        let f = |name: &str| -> f32 { if let [VsfType::f5(x)] = s.get_field(name).unwrap().values[..] { x } else { panic!("{name}") } };
        let o = crate::optics::AKITA_OPTICS[0];
        assert_eq!((f("sensor_width_mm"), f("sensor_height_mm"), f("focal_mm"), f("f_number")), (o.sensor_mm.0, o.sensor_mm.1, o.focal_mm, o.f_number));
        assert_eq!(c.binned_field(), o.array_px);
        assert_eq!(f("fov_diagonal_deg"), o.field(o.array_px).fov_deg.2);
        assert!((f("fov_diagonal_deg") - crate::optics::OPTICS[0].field(crate::optics::OPTICS[0].array_px).fov_deg.2).abs() > 1.);
        for c in cams::AKITA.iter() {
            let fd = c.optics.field(c.binned_field());
            println!("the 8a's {}: {} x {}, field {:.4} x {:.4} mm, {:.2} x {:.2} degrees, {:.2} diagonal, {:.2} mm equivalent", c.name, c.width, c.height, fd.mm.0, fd.mm.1, fd.fov_deg.0, fd.fov_deg.1, fd.fov_deg.2, fd.focal_equiv_mm);
        }
    }

    #[test]
    fn a_bad_idt_refuses() {
        assert_eq!(section(&cams::MAIN, b"not a vsf").unwrap_err(), IdtError::Document);
    }

    /// The main's two readouts as the driver lists them with its 1:1 init in the vault (target/cam/main-full.init, local): two sections with the same `id` and `name`, the binned first; the 1:1 opened by kind 2, 8160 x 6144, the quad period, its own frame (41.7 ms against the binned 67.3) and exposure range; the binned one as before. A binned init under the 1:1 name lists nothing.
    #[test]
    fn the_mains_two_readouts() {
        let Ok(full) = std::fs::read("/mnt/Harbor/Code/ferros/target/cam/main-full.init") else { return };
        let c = &cams::MAIN;
        let r = Readout::full(c, &full).unwrap();
        let doc = document(vec![section(c, &idt(c)).unwrap(), section_of(c, &idt(c), &r).unwrap()]).unwrap();
        let d = vsf::container::Document::open(&doc[..]).unwrap();
        let s: Vec<_> = d.entries_named("camera").into_iter().map(|e| d.section(e).unwrap()).collect();
        assert_eq!(s.len(), 2);
        let one = document(vec![section(c, &idt(c)).unwrap()]).unwrap();
        println!("the list of the main's two readouts: {} bytes, its binned one alone {}", doc.len(), one.len());
        let get = |k: usize, f: &str| s[k].uint(f).unwrap();
        assert_eq!((get(0, "id"), get(1, "id"), s[0].text("name"), s[1].text("name")), (0, 0, Some("main"), Some("main")));
        assert_eq!((get(0, "mode"), get(0, "width"), get(0, "height"), get(0, "cfa_period")), (0, 4080, 3072, 2));
        assert_eq!((get(1, "mode"), get(1, "width"), get(1, "height"), get(1, "cfa_period"), get(1, "cfa")), (2, 8160, 6144, 4, c.cfa as u64));
        assert!((67_200_000..67_300_000).contains(&get(0, "frame_ns")), "{}", get(0, "frame_ns"));
        assert!((41_600_000..41_750_000).contains(&get(1, "frame_ns")), "{}", get(1, "frame_ns"));
        // A line of each: 3.149 us binned, 6.586 us 1:1; the 1:1's readout 6144 of them, 40.5 ms.
        assert_eq!((get(0, "exposure_min"), get(1, "exposure_min")), (3148, 6586));
        assert!((40_400_000..40_500_000).contains(&get(1, "readout")), "{}", get(1, "readout"));
        assert!(get(1, "exposure_max") > get(0, "exposure_max"));
        for k in 0..2 {
            println!("main readout {k}: mode {} {} x {}, period {}, a frame {} ns, exposure {} to {} ns, readout {} ns", get(k, "mode"), get(k, "width"), get(k, "height"), get(k, "cfa_period"), get(k, "frame_ns"), get(k, "exposure_min"), get(k, "exposure_max"), get(k, "readout"));
        }
        if let Ok(b) = std::fs::read("/mnt/Harbor/Code/ferros/target/cam/cam.frame") {
            let (binned, _) = crate::inputs::split_bundle(&b).unwrap();
            assert!(matches!(Readout::full(c, binned), Err(IdtError::Mode(_))));
        }
    }
}
