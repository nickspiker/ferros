//! The ferros camera endpoint as Lumis speaks it (docs/CAMERA.md, "The endpoint"): the operations, the camera list (a VSF document, one `camera` section per camera), a frame's header.
//! Lumis's backend on ferros, taken from the first ferros front (Lumis branch `ferros`, rust/lumis-ferros, retired: Lumis itself is the app now). Portable: nothing here makes a call, so the parsing is tested on the host.

/// The cameras the driver can stream, as a VSF document (no input). An empty document is a device whose cameras ferros does not drive yet.
pub const OP_LIST: u8 = 0;
/// [OP_OPEN][id]: start that camera streaming for this caller. Reply [0] open, [1] no such camera, [2] another holds it.
pub const OP_OPEN: u8 = 1;
/// [OP_FRAME][bin]: wait for the next frame and take it, `bin` x `bin` same-colour samples averaged (1 the whole readout). Reply: [`FRAME_HEADER`] bytes, then the mosaic, 16 bits a sample, little-endian, row by row.
pub const OP_FRAME: u8 = 2;
/// [OP_SET][what][value u64 LE]: one of the manual controls ([`SET_EXPOSURE`], [`SET_GAIN`], [`SET_FOCUS`]), taken at the next frame the sensor can. Reply [0] taken, [1] outside the camera's range.
pub const OP_SET: u8 = 3;
/// [OP_CLOSE]: stop streaming. The driver also closes when the holder ends, so Lumis never needs to say it on the way out.
pub const OP_CLOSE: u8 = 4;

/// Exposure per frame, in nanoseconds.
pub const SET_EXPOSURE: u8 = 0;
/// Analogue gain, 1024 = unity.
pub const SET_GAIN: u8 = 1;
/// The lens actuator's position, in the camera's own range (`focus_min`..=`focus_max`).
pub const SET_FOCUS: u8 = 2;

/// Unity gain in [`SET_GAIN`] and a frame's `gain`.
pub const GAIN_UNITY: u32 = 1 << 10;

/// The bytes before a frame's samples.
pub const FRAME_HEADER: usize = 1 << 6;

/// One camera, as the driver describes it in its list.
#[derive(Clone, Debug, PartialEq)]
pub struct Camera {
    /// The driver's number for it ([`OP_OPEN`]).
    pub id: u8,
    /// Its name for people ("main", "ultrawide", "front").
    pub name: String,
    /// 0 faces away from the person holding the device, 1 faces them.
    pub facing: u8,
    /// The whole readout, in samples.
    pub width: u32,
    pub height: u32,
    /// The colour filter: four 2-bit channel codes (0 R, 1 G, 2 B) for the 2 x 2 cell, top left, top right, bottom left, bottom right, from the low bits up.
    pub cfa: u8,
    pub black: u32,
    pub white: u32,
    /// Quarter turns clockwise that bring the readout upright on the panel held in its natural orientation.
    pub rotation: u8,
    /// The sensor's Absolute IDT (its calibration): camera R, G, B above black, each times `sensitivity`, through this matrix (row-major) to linear VSF RGB.
    pub idt: [f32; 9],
    pub sensitivity: [f32; 3],
    pub exposure_min: u64,
    pub exposure_max: u64,
    pub gain_min: u32,
    pub gain_max: u32,
    pub focus_min: u32,
    pub focus_max: u32,
}

/// The list's document, read. A section that misses a field is not a camera Lumis can show, so it is left out and said (a driver bug, not the person's).
pub fn parse_list(doc: &[u8]) -> Result<Vec<Camera>, String> {
    let d = vsf::container::Document::open(doc).map_err(|e| format!("the camera list is not VSF: {e:?}"))?;
    let mut out = Vec::new();
    for entry in d.entries_named("camera") {
        let s = d.section(entry).map_err(|e| format!("a camera section does not read: {e:?}"))?;
        let floats = |name: &str| -> Option<Vec<f32>> {
            let f = s.get_field(name)?;
            f.values.iter().map(|v| match v {
                vsf::VsfType::f5(x) => Some(*x),
                _ => None,
            }).collect()
        };
        let u = |name: &str| s.uint(name);
        let cam = (|| {
            let idt: [f32; 9] = floats("idt")?.try_into().ok()?;
            let sensitivity: [f32; 3] = floats("sensitivity")?.try_into().ok()?;
            Some(Camera {
                id: u("id")? as u8,
                name: s.text("name")?.to_string(),
                facing: u("facing")? as u8,
                width: u("width")? as u32,
                height: u("height")? as u32,
                cfa: u("cfa")? as u8,
                black: u("black")? as u32,
                white: u("white")? as u32,
                rotation: u("rotation")? as u8,
                idt,
                sensitivity,
                exposure_min: u("exposure_min")?,
                exposure_max: u("exposure_max")?,
                gain_min: u("gain_min")? as u32,
                gain_max: u("gain_max")? as u32,
                focus_min: u("focus_min")? as u32,
                focus_max: u("focus_max")? as u32,
            })
        })();
        match cam {
            Some(c) => out.push(c),
            None => println!("lumis: a camera section lacks a field Lumis needs; left out"),
        }
    }
    Ok(out)
}

/// What a frame's header says (the samples follow it, 16 bits each, little-endian, row by row: the bytes `process_frame` takes as they are).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameHeader {
    /// The driver's count of frames since the camera opened.
    pub count: u32,
    pub width: usize,
    pub height: usize,
    pub bin: u8,
    pub cfa: u8,
    pub black: u16,
    pub white: u16,
    pub exposure_ns: u64,
    pub gain: u32,
    pub focus: u32,
    /// When its exposure ended, Eagle time in nanoseconds.
    pub time: i64,
}

impl FrameHeader {
    /// The bytes of the samples that follow the header.
    pub fn samples_bytes(&self) -> usize {
        self.width * self.height * 2
    }
}

/// A frame's header from the reply's bytes; None when the reply is shorter than its own header says (a driver bug).
pub fn parse_frame(r: &[u8]) -> Option<FrameHeader> {
    if r.len() < FRAME_HEADER {
        return None;
    }
    let u16_at = |k: usize| u16::from_le_bytes([r[k], r[k + 1]]);
    let u32_at = |k: usize| u32::from_le_bytes([r[k], r[k + 1], r[k + 2], r[k + 3]]);
    let u64_at = |k: usize| u64::from_le_bytes([r[k], r[k + 1], r[k + 2], r[k + 3], r[k + 4], r[k + 5], r[k + 6], r[k + 7]]);
    let h = FrameHeader {
        count: u32_at(0),
        width: u16_at(4) as usize,
        height: u16_at(6) as usize,
        bin: r[8],
        cfa: r[9],
        black: u16_at(10),
        white: u16_at(12),
        exposure_ns: u64_at(16),
        gain: u32_at(24),
        focus: u32_at(28),
        time: u64_at(32) as i64,
    };
    (r.len() >= FRAME_HEADER + h.samples_bytes()).then_some(h)
}

/// The header's bytes for a frame (the driver's side of [`parse_frame`], kept beside it so the two cannot drift; the tests use it).
pub fn frame_header(f: &FrameHeader) -> [u8; FRAME_HEADER] {
    let mut h = [0u8; FRAME_HEADER];
    h[0..4].copy_from_slice(&f.count.to_le_bytes());
    h[4..6].copy_from_slice(&(f.width as u16).to_le_bytes());
    h[6..8].copy_from_slice(&(f.height as u16).to_le_bytes());
    h[8] = f.bin;
    h[9] = f.cfa;
    h[10..12].copy_from_slice(&f.black.to_le_bytes());
    h[12..14].copy_from_slice(&f.white.to_le_bytes());
    h[16..24].copy_from_slice(&f.exposure_ns.to_le_bytes());
    h[24..28].copy_from_slice(&f.gain.to_le_bytes());
    h[28..32].copy_from_slice(&f.focus.to_le_bytes());
    h[32..40].copy_from_slice(&f.time.to_le_bytes());
    h
}

/// Lumis's colour filter code (Camera2's SENSOR_INFO_COLOR_FILTER_ARRANGEMENT: 0 RGGB, 1 GRBG, 2 GBRG, 3 BGGR) for the driver's 2 x 2 cell codes, None for a cell no Bayer arrangement has.
pub fn bayer_pattern(cfa: u8) -> Option<u32> {
    let cell = [cfa & 3, (cfa >> 2) & 3, (cfa >> 4) & 3, (cfa >> 6) & 3];
    match cell {
        [0, 1, 1, 2] => Some(0),
        [1, 0, 2, 1] => Some(1),
        [1, 2, 0, 1] => Some(2),
        [2, 1, 1, 0] => Some(3),
        _ => None,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use vsf::VsfType;

    fn main_camera_doc() -> Vec<u8> {
        let f = |v: &[f32]| v.iter().map(|x| VsfType::f5(*x)).collect::<Vec<_>>();
        let fields = vec![
            ("id".to_string(), vec![VsfType::u3(0)]),
            ("name".to_string(), vec![VsfType::a(vsf::Ascii::lit("main"))]),
            ("facing".to_string(), vec![VsfType::u3(0)]),
            ("width".to_string(), vec![VsfType::u4(4080)]),
            ("height".to_string(), vec![VsfType::u4(3072)]),
            ("cfa".to_string(), vec![VsfType::u3(1 | 2 << 2 | 0 << 4 | 1 << 6)]),
            ("black".to_string(), vec![VsfType::u3(64)]),
            ("white".to_string(), vec![VsfType::u4(1023)]),
            ("rotation".to_string(), vec![VsfType::u3(1)]),
            ("idt".to_string(), f(&[1., 0., 0., 0., 1., 0., 0., 0., 1.])),
            ("sensitivity".to_string(), f(&[1., 1., 1.])),
            ("exposure_min".to_string(), vec![VsfType::u5(1 << 16)]),
            ("exposure_max".to_string(), vec![VsfType::u5(1 << 26)]),
            ("gain_min".to_string(), vec![VsfType::u4(1 << 10)]),
            ("gain_max".to_string(), vec![VsfType::u5(1 << 14)]),
            ("focus_min".to_string(), vec![VsfType::u4(1 << 11)]),
            ("focus_max".to_string(), vec![VsfType::u4(0x7800)]),
        ];
        let mut b = vsf::VsfBuilder::new();
        let mut s = vsf::file_format::VsfSection::new("camera");
        for (name, values) in fields {
            let mut field = vsf::file_format::VsfField::new(name);
            field.values = values;
            s.fields.push(field);
        }
        b = b.add_section_direct(s);
        b.build().expect("the test list builds")
    }

    #[test]
    fn a_list_reads_back() {
        let cams = parse_list(&main_camera_doc()).unwrap();
        assert_eq!(cams.len(), 1);
        let c = &cams[0];
        assert_eq!((c.id, c.name.as_str(), c.width, c.height, c.black, c.white, c.rotation), (0, "main", 4080, 3072, 64, 1023, 1));
        assert_eq!(c.cfa & 3, 1);
        assert_eq!(c.idt[4], 1.);
    }

    #[test]
    fn the_pro_main_cfa_is_gbrg() {
        // husky's main: G B / R G, G#49 (docs/CAMERA.md).
        assert_eq!(bayer_pattern(0x49), Some(2));
        assert_eq!(bayer_pattern(0b10_01_01_00), Some(0));
        assert_eq!(bayer_pattern(0), None);
    }

    #[test]
    fn a_frame_reads_back() {
        let f = FrameHeader { count: 7, width: 4, height: 2, bin: 2, cfa: 0b01_00_10_01, black: 64, white: 1023, exposure_ns: 1 << 26, gain: GAIN_UNITY, focus: 0x3800, time: -5 };
        let samples: [u16; 8] = [1, 2, 3, 4, 5, 6, 7, 1023];
        let mut r = frame_header(&f).to_vec();
        for s in &samples {
            r.extend_from_slice(&s.to_le_bytes());
        }
        let g = parse_frame(&r).unwrap();
        assert_eq!((g.count, g.width, g.height, g.bin, g.cfa, g.black, g.white, g.exposure_ns, g.gain, g.focus, g.time), (7, 4, 2, 2, f.cfa, 64, 1023, 1 << 26, GAIN_UNITY, 0x3800, -5));
        assert_eq!(&r[FRAME_HEADER..], &samples.iter().flat_map(|s| s.to_le_bytes()).collect::<Vec<u8>>()[..]);
        assert!(parse_frame(&r[..r.len() - 1]).is_none());
    }
}
