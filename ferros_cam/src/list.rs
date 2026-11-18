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
}

/// Camera `c`'s section, its IDT's Absolute entry (`idt` the bytes of its `.idt.vsf`) carried in it.
pub fn section(c: &Cam, idt: &[u8]) -> Result<vsf::file_format::VsfSection, IdtError> {
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
    put("width", u(c.width as u64));
    put("height", u(c.height as u64));
    put("cfa", u(c.cfa as u64));
    put("black", u(c.black as u64));
    put("white", u(c.white as u64));
    put("rotation", u(c.rotation as u64));
    put("idt", abs.matrix.iter().map(|v| VsfType::f5(*v)).collect());
    put("sensitivity", sens.iter().map(|v| VsfType::f5(*v)).collect());
    // Exposure: one line to the longest offered: past the init's frame where the camera's margin was measured, the frame and then the line lengthened (ferros_cam ccs `Timing::most_ns`; the main and the ultrawide about 1.02 s, at the longest line time the front end took on the Pro); the init's own integration elsewhere.
    put("exposure_min", u(c.line_ps / 1000));
    put("exposure_max", u(c.listed_timing().most_ns()));
    // Gain (1024 one): the sensor's range as measured from its own capability words (ferros_cam ccs `ARRAYS`' gain, `Gain::range`); the captured code alone (1024 and 1024) for a camera not yet measured.
    let (gmin, gmax) = crate::ccs::ARRAYS[c.id as usize].gain;
    put("gain_min", u(gmin));
    put("gain_max", u(gmax));
    // Focus: the lens actuator's targets over the span measured (the main's G#0800 to G#7800); 0 and 0 where focus is not offered.
    put("focus_min", u(c.focus.0));
    put("focus_max", u(c.focus.1));
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
            for f in ["facing", "width", "height", "cfa", "black", "white", "rotation", "exposure_min", "exposure_max", "gain_min", "gain_max", "focus_min", "focus_max"] {
                assert!(s.uint(f).is_some(), "{} {f}", c.name);
            }
            let (m, sens) = (floats("idt"), floats("sensitivity"));
            assert_eq!((m.len(), sens.len()), (9, 3));
            assert!(!seen.contains(&m), "{} has another camera's matrix", c.name);
            println!("{}: idt {m:?}, sensitivity {sens:?}", c.name);
            seen.push(m);
        }
        assert_eq!(seen.len(), 4);
    }

    #[test]
    fn a_bad_idt_refuses() {
        assert_eq!(section(&cams::MAIN, b"not a vsf").unwrap_err(), IdtError::Document);
    }
}
