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
/// [OP_MAP][release]: with the view (an [OP_OPEN][id][`OPEN_VIEW`] answered with [`VIEW_REPLY`] bytes), give back the buffer `release` names (`NONE` for none), wait for a frame newer than the last mapped and hold its buffer: the reply a [`FRAME_HEADER`] whose samples are read in place thru the view ([`MapHeader`]; docs/CAMERA.md "The view").
pub const OP_MAP: u8 = 5;
/// [OP_RELEASE][buffer]: the held buffer given back. Reply [0], [1] when it was not held.
pub const OP_RELEASE: u8 = 6;
/// The third byte of an open that asks for the view.
pub const OPEN_VIEW: u8 = 1;
/// The mode block's bytes after an open's third (docs/CAMERA.md "Modes, the bench and the slit": [kind][rows: 2][fll: 2][exposure: 2][flags][ring][line length: 2][cols: 2]); all zero is the init's mode, as no block is.
pub const MODE_BLOCK: usize = 13;
/// The open's byte after the mode block (its seventeenth), bit 0: the view write-back (payloads/camera `VIEW_CACHED`, docs/CAMERA.md "A cacheable view"). A driver from before reads no byte there and gives the non-cacheable view, as a kernel from before does.
pub const OPEN_VIEW_CACHED: u8 = 1 << 0;
/// A view reply's length when the open carried a mode block or asked for write-back: the ring's count and the slit's facts after the 48, and at 53 whether the view is write-back.
pub const VIEW_REPLY_MODE: usize = 64;
/// [OP_SLITS][after: u32 LE], a slit ring open: wait for a slit newer than `after`; the reply [`Slits`] (docs/CAMERA.md "The ring drained").
pub const OP_SLITS: u8 = 9;
/// The mode block's kind for a binned slit (docs/CAMERA.md "Modes, the bench and the slit"): `rows` rows across the sensor's middle at the binned mode's width, on the colour filter's period, the driver's windowed default (phase detect and embedded data off, the statistics pipeline detached, the slim program: "The slit runs", 2026-11-18).
pub const KIND_SLIT: u8 = 3;
/// The mode block's kind for the 1:1 full-resolution readout (ferros cam-full-res: the captured init replayed), read by MAP only (no copy).
pub const KIND_FULL: u8 = 2;
/// A slit's rows: 64, the fewest that keep the main's link clean (56 and fewer set ERR_CRC_PH while frames still land; 2026-11-18), at the sensor's floor of about 1218 a second.
pub const SLIT_ROWS: u16 = 1 << 6;
/// The mode block of a binned slit of [`SLIT_ROWS`], every other field the driver's default (its frame the rows and 2^7 lines more, its integration set by SET, a ring of 2^8 buffers).
pub fn slit_block() -> [u8; MODE_BLOCK] {
    let mut b = [0u8; MODE_BLOCK];
    b[0] = KIND_SLIT;
    b[1..3].copy_from_slice(&SLIT_ROWS.to_le_bytes());
    b
}

/// A SLITS answer: the newest slit's count, the ring's buffers, a buffer's bytes, a slit's rows, a line's bytes, its width; slit k is in buffer (k - 1) mod `ring`, its record (count, counter, Eagle ns, landed) right after its rows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slits {
    pub newest: u64,
    pub ring: u64,
    pub buffer: usize,
    pub rows: usize,
    pub line: usize,
    pub width: usize,
}

/// A SLITS reply; None when it is not one (the driver ended: no slit came in its wait, or the camera closed).
pub fn parse_slits(r: &[u8]) -> Option<Slits> {
    if r.len() < 1 << 6 {
        return None;
    }
    let w = |k: usize| u64::from_le_bytes(r[8 * k..8 * k + 8].try_into().unwrap());
    Some(Slits { newest: w(0), ring: w(1), buffer: w(2) as usize, rows: w(3) as usize, line: w(4) as usize, width: w(5) as usize })
}

/// The slits a reader takes from a SLITS answer, after it took every slit up to `after`: those newer than `after` and among the newest half of the ring, as the range of their counts. The driver's rule is the newest ring - 2 (the two oldest buffers are the engine's next and the one patterned for the frame after it), at the answer; the reader reads them after it, while slits go on landing, so it keeps to half the ring: at the main's 1218 a second and 2^8 buffers, the oldest it takes has about 100 ms of slits to go before the engine comes round to it, and a slit is read in microseconds.
pub fn slits_to_take(after: u64, s: &Slits) -> std::ops::RangeInclusive<u64> {
    // WHY the max: the oldest slit to take is the newer of the one after `after` and the oldest in the ring's newest half; early in a run (`newest` under half the ring) that half reaches before slit 1, which the first term then is.
    (after + 1).max((s.newest + 1).max(s.ring / 2) - s.ring / 2)..=s.newest
}

/// No buffer, in [`OP_MAP`]'s release byte.
pub const NONE: u8 = 0xFF;
/// An open's reply with the view: [0], the view's token (32 bytes at 1), the ring's buffers (33), the format (34), width and height (u16 at 36, 38), a line's bytes (u32 at 40), a buffer's bytes in the view (u32 at 44).
pub const VIEW_REPLY: usize = 48;
/// The view's samples as MIPI RAW10: four samples in five bytes, the high eight bits of each, then their low two bits.
pub const FORMAT_RAW10: u8 = 1;

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
    /// The optics (the driver's list since 2026-11-17, Android's Camera2 values; 0 from a driver before, which Lumis's page shows as unknown): the sensor's size and the focal length in mm, the f-number.
    pub sensor_mm: (f32, f32),
    pub focal_mm: f32,
    pub f_number: f32,
    /// The readout (ferros cam-full-res: the list's sections are readouts, a camera's grouped by `id`): its mode block's kind (0 the binned init's; 2 the 1:1 full resolution, [`KIND_FULL`]), its colour filter's period (2 a Bayer cell, 4 a quad: 2 x 2 same-colour blocks, which Lumis reads as Android's maximum-resolution RAW) and its frame in ns; 0, 2 and 0 from a driver before.
    pub mode: u8,
    pub cfa_period: u8,
    pub frame_ns: u64,
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
        // A one-value float field, 0 when absent.
        let one = |name: &str| floats(name).and_then(|v| v.first().copied()).unwrap_or(0.);
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
                // Optional: a driver from before has none, and the page then shows the optics as unknown.
                sensor_mm: (one("sensor_width_mm"), one("sensor_height_mm")),
                focal_mm: one("focal_mm"),
                f_number: one("f_number"),
                mode: u("mode").unwrap_or(0) as u8,
                cfa_period: u("cfa_period").unwrap_or(2) as u8,
                frame_ns: u("frame_ns").unwrap_or(0),
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
    /// The colour filter's period (+14: 2 a Bayer cell, 4 a quad); 0 from a driver before, which is Bayer.
    pub cfa_period: u8,
}

impl FrameHeader {
    /// A quad colour filter (2 x 2 same-colour blocks in a 4 x 4 tile): the integrator's `quad`.
    pub fn quad(&self) -> bool {
        self.cfa_period == 4
    }

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
        cfa_period: r[14],
    };
    (r.len() >= FRAME_HEADER + h.samples_bytes()).then_some(h)
}

/// The view an open gave: the kernel's token for it and the ring's shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub token: [u8; 32],
    pub buffers: u8,
    pub format: u8,
    pub width: usize,
    pub height: usize,
    /// A line's bytes, and a buffer's place from the next one's in the view.
    pub line: usize,
    pub buffer: usize,
    /// The view is mapped write-back (asked with [`OPEN_VIEW_CACHED`] after the mode block, answered 1 at 53 of a 64-byte reply): the driver has each buffer's lines cleaned and invalidated as MAP hands it over, so its rows are read in place thru the caches; else non-cacheable, read in 64-byte loads.
    pub cached: bool,
}

/// The view from an open's reply; None for a plain open (`[0]` alone: the kernel or the driver gave none, and frames come by copy).
pub fn parse_view(r: &[u8]) -> Option<View> {
    if r.len() < VIEW_REPLY || r[0] != 0 {
        return None;
    }
    let u16_at = |k: usize| u16::from_le_bytes([r[k], r[k + 1]]) as usize;
    let u32_at = |k: usize| u32::from_le_bytes([r[k], r[k + 1], r[k + 2], r[k + 3]]) as usize;
    Some(View { token: r[1..33].try_into().unwrap(), buffers: r[33], format: r[34], width: u16_at(36), height: u16_at(38), line: u32_at(40), buffer: u32_at(44), cached: r.len() >= VIEW_REPLY_MODE && r[53] == 1 })
}

/// A MAP's answer: the frame's header, the buffer that holds it (what the next MAP or a RELEASE gives back), and where its samples start in the view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapHeader {
    pub frame: FrameHeader,
    pub buffer: u8,
    pub offset: usize,
    /// The frame's own phase-detect rows in the same buffer (the combined driver, `slit-for-real`): their offset from the image's start (+56) and how many (+60); both 0 from a driver without them.
    pub pd_offset: usize,
    pub pd_rows: usize,
}

/// A MAP's header; None when it is not one (no frame: the driver closed).
pub fn parse_map(r: &[u8]) -> Option<MapHeader> {
    if r.len() < FRAME_HEADER {
        return None;
    }
    let mut h = [0u8; FRAME_HEADER];
    h.copy_from_slice(&r[..FRAME_HEADER]);
    // The header's samples are not in the reply: parsed as a frame of none.
    let frame = parse_frame_header(&h);
    Some(MapHeader {
        frame,
        buffer: r[40],
        offset: u64::from_le_bytes(r[48..56].try_into().unwrap()) as usize,
        pd_offset: u32::from_le_bytes(r[56..60].try_into().unwrap()) as usize,
        pd_rows: u16::from_le_bytes([r[60], r[61]]) as usize,
    })
}

/// One MIPI RAW10 line into 16-bit samples: each five bytes four samples, `(high << 2) | low bits`.
pub fn unpack_raw10(line: &[u8], out: &mut [u16]) {
    for (px, b) in out.chunks_exact_mut(4).zip(line.chunks_exact(5)) {
        let lo = b[4];
        for i in 0..4 {
            px[i] = (b[i] as u16) << 2 | ((lo >> (2 * i)) & 3) as u16;
        }
    }
}

fn parse_frame_header(r: &[u8; FRAME_HEADER]) -> FrameHeader {
    let u16_at = |k: usize| u16::from_le_bytes([r[k], r[k + 1]]);
    let u32_at = |k: usize| u32::from_le_bytes([r[k], r[k + 1], r[k + 2], r[k + 3]]);
    FrameHeader {
        count: u32_at(0),
        width: u16_at(4) as usize,
        height: u16_at(6) as usize,
        bin: r[8],
        cfa: r[9],
        black: u16_at(10),
        white: u16_at(12),
        exposure_ns: u64::from_le_bytes(r[16..24].try_into().unwrap()),
        gain: u32_at(24),
        focus: u32_at(28),
        time: u64::from_le_bytes(r[32..40].try_into().unwrap()) as i64,
        cfa_period: r[14],
    }
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
    h[14] = f.cfa_period;
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
        camera_doc(&[None, Some((2, 8160, 6144, 4, 41670368))])
    }

    /// A list of the main's readouts: each None the binned one as a driver before cam-full-res lists it (no mode fields), each Some((mode, width, height, cfa_period, frame_ns)) one with them.
    fn camera_doc(readouts: &[Option<(u8, u32, u32, u8, u64)>]) -> Vec<u8> {
        let mut b = vsf::VsfBuilder::new();
        for r in readouts {
            b = b.add_section_direct(main_section(*r));
        }
        b.build().expect("the test list builds")
    }

    fn main_section(readout: Option<(u8, u32, u32, u8, u64)>) -> vsf::file_format::VsfSection {
        let f = |v: &[f32]| v.iter().map(|x| VsfType::f5(*x)).collect::<Vec<_>>();
        let (width, height) = readout.map_or((4080, 3072), |r| (r.1, r.2));
        let mut fields = vec![
            ("id".to_string(), vec![VsfType::u3(0)]),
            ("name".to_string(), vec![VsfType::a(vsf::Ascii::lit("main"))]),
            ("facing".to_string(), vec![VsfType::u3(0)]),
            ("width".to_string(), vec![VsfType::u4(width as u16)]),
            ("height".to_string(), vec![VsfType::u4(height as u16)]),
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
            ("sensor_width_mm".to_string(), f(&[9.792])),
            ("sensor_height_mm".to_string(), f(&[7.3728])),
            ("focal_mm".to_string(), f(&[6.9])),
            ("f_number".to_string(), f(&[1.68])),
        ];
        if let Some((mode, _, _, period, frame_ns)) = readout {
            fields.push(("mode".to_string(), vec![VsfType::u3(mode)]));
            fields.push(("cfa_period".to_string(), vec![VsfType::u3(period)]));
            fields.push(("frame_ns".to_string(), vec![VsfType::u6(frame_ns)]));
        }
        let mut s = vsf::file_format::VsfSection::new("camera");
        for (name, values) in fields {
            let mut field = vsf::file_format::VsfField::new(name);
            field.values = values;
            s.fields.push(field);
        }
        s
    }

    #[test]
    fn a_list_reads_back() {
        let cams = parse_list(&main_camera_doc()).unwrap();
        assert_eq!(cams.len(), 2);
        let c = &cams[0];
        assert_eq!((c.id, c.name.as_str(), c.width, c.height, c.black, c.white, c.rotation), (0, "main", 4080, 3072, 64, 1023, 1));
        assert_eq!(c.cfa & 3, 1);
        assert_eq!(c.idt[4], 1.);
        assert_eq!((c.sensor_mm, c.focal_mm, c.f_number), ((9.792, 7.3728), 6.9, 1.68));
        // A driver before cam-full-res lists no mode fields: the binned readout, Bayer.
        assert_eq!((c.mode, c.cfa_period, c.frame_ns), (0, 2, 0));
        // The 1:1 readout: the same camera (id, name), its own size, a quad filter.
        let full = &cams[1];
        assert_eq!((full.id, full.name.as_str(), full.width, full.height, full.mode, full.cfa_period, full.frame_ns), (0, "main", 8160, 6144, KIND_FULL, 4, 41670368));
    }

    #[test]
    fn a_view_and_a_map_read_back() {
        // The driver's OPEN reply with the view (payloads/camera `VIEW_REPLY`).
        let mut r = [0u8; VIEW_REPLY];
        r[1..33].copy_from_slice(&[7u8; 32]);
        r[33] = 4;
        r[34] = FORMAT_RAW10;
        r[36..38].copy_from_slice(&4080u16.to_le_bytes());
        r[38..40].copy_from_slice(&3072u16.to_le_bytes());
        r[40..44].copy_from_slice(&0x1400u32.to_le_bytes());
        r[44..48].copy_from_slice(&(1u32 << 24).to_le_bytes());
        let v = parse_view(&r).unwrap();
        assert_eq!((v.token, v.buffers, v.format, v.width, v.height, v.line, v.buffer, v.cached), ([7; 32], 4, FORMAT_RAW10, 4080, 3072, 0x1400, 1 << 24, false));
        assert!(parse_view(&r[..1]).is_none());
        // The 64-byte reply (a mode block, or write-back asked): 1 at 53 when the view is write-back.
        let mut m = [0u8; VIEW_REPLY_MODE];
        m[..VIEW_REPLY].copy_from_slice(&r);
        assert!(!parse_view(&m).unwrap().cached);
        m[53] = 1;
        assert!(parse_view(&m).unwrap().cached);
        // A MAP's header (payloads/camera `header` at bin 1).
        let f = FrameHeader { count: 9, width: 4080, height: 3072, bin: 1, cfa: 0x49, black: 64, white: 1023, exposure_ns: 1 << 26, gain: GAIN_UNITY, focus: 0x3800, time: 5, cfa_period: 2 };
        let mut h = frame_header(&f);
        h[40] = 2;
        h[48..56].copy_from_slice(&(2u64 << 24).to_le_bytes());
        let m = parse_map(&h).unwrap();
        assert_eq!((m.frame, m.buffer, m.offset), (f, 2, 2 << 24));
    }

    #[test]
    fn slits_read_back_and_keep_to_the_newest_half() {
        let mut r = [0u8; 64];
        for (k, v) in [300u64, 256, 327680, 64, 5120, 4080, 0, 1].iter().enumerate() {
            r[8 * k..8 * k + 8].copy_from_slice(&v.to_le_bytes());
        }
        let s = parse_slits(&r).unwrap();
        assert_eq!(s, Slits { newest: 300, ring: 256, buffer: 327680, rows: 64, line: 5120, width: 4080 });
        // Behind by more than half the ring: from the oldest of the newest 128.
        assert_eq!(slits_to_take(10, &s), 173..=300);
        // Close behind: from the one after the last taken.
        assert_eq!(slits_to_take(290, &s), 291..=300);
        // Early in a run, the newest under half the ring.
        assert_eq!(slits_to_take(0, &Slits { newest: 5, ..s }), 1..=5);
        // Nothing new.
        assert!(slits_to_take(300, &s).is_empty());
        let b = slit_block();
        assert_eq!((b[0], u16::from_le_bytes([b[1], b[2]]), b[3..].iter().all(|&x| x == 0)), (KIND_SLIT, 64, true));
    }

    #[test]
    fn raw10_unpacks() {
        // Samples 0, 1023, 512, 3: high bytes 0, 255, 128, 0; low bits 0, 3, 0, 3 (low byte 0b11_00_11_00).
        let line = [0u8, 255, 128, 0, 0b1100_1100];
        let mut px = [0u16; 4];
        unpack_raw10(&line, &mut px);
        assert_eq!(px, [0, 1023, 512, 3]);
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
        let f = FrameHeader { count: 7, width: 4, height: 2, bin: 2, cfa: 0b01_00_10_01, black: 64, white: 1023, exposure_ns: 1 << 26, gain: GAIN_UNITY, focus: 0x3800, time: -5, cfa_period: 4 };
        let samples: [u16; 8] = [1, 2, 3, 4, 5, 6, 7, 1023];
        let mut r = frame_header(&f).to_vec();
        for s in &samples {
            r.extend_from_slice(&s.to_le_bytes());
        }
        let g = parse_frame(&r).unwrap();
        assert_eq!((g.count, g.width, g.height, g.bin, g.cfa, g.black, g.white, g.exposure_ns, g.gain, g.focus, g.time), (7, 4, 2, 2, f.cfa, 64, 1023, 1 << 26, GAIN_UNITY, 0x3800, -5));
        assert!(g.quad());
        assert_eq!(&r[FRAME_HEADER..], &samples.iter().flat_map(|s| s.to_le_bytes()).collect::<Vec<u8>>()[..]);
        assert!(parse_frame(&r[..r.len() - 1]).is_none());
    }
}
