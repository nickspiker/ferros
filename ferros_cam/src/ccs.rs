//! The rear sensors' readout modes thru their MIPI CCS registers (owner, 2026-11-15: "straight as fast as possible full frame 1:1 and 2x binned for all back cams ... Then I want the slitscan wired up"; docs/CAMERA.md "Modes, the bench and the slit").
//!
//! Each sensor's captured init (`cam.<key>.init`, Android's own) leaves it in Android's binned mode; of its writes only about twenty define the mode, all standard CCS registers on the sensor's register page (G#4000 on the Samsung parts, reached directly once G#FCFC selects it, or thru the indirect window G#6028/G#602A/G#6F12).
//! `Mode::captured` reads them back out of an init, so every mode here starts from the one Android ran and changes only what it must: the window and output size (a slit), the binning and the increments (1:1), the frame and line length, the integration, the data format (RAW12).
//! `Mode::writes` gives the bytes the driver sends inside the sensor's grouped-parameter hold, after the init and before stream on.
//!
//! What is measured and what is a guess, per camera (`ARRAYS`), and the trap that shaped the order: on 2026-10-20/21 a geometry written before stream on never started, and one written mid-stream took once and left the sensor stuck until a cold boot.
//! Those writes went without the hold (the harness wrote `w16(G#0104, 1)`, which sets G#0104 to 0 and G#0105 to 1: the release, not the hold) and with the init's integration (21342 lines) left in a frame of 68 to 576 lines; both are fixed here (the hold as the camera's own `hold` bytes; an integration inside the frame, refused otherwise), which is the experiment the bench runs first.

use crate::cams::Cam;

/// CCS registers (MIPI CCS 1.1, the mainline driver's ccs-regs.h), big-endian on the bus, 16 bits unless said.
pub const FRAME_COUNT: u16 = 0x0005; // 8 bits: counts every frame the sensor sends
pub const DATA_FORMAT_MODEL_TYPE: u16 = 0x00C0; // 8 bits
pub const DATA_FORMAT_MODEL_SUBTYPE: u16 = 0x00C1; // 8 bits: the descriptors' count in its low nibble
pub const DATA_FORMAT_DESCRIPTOR_0: u16 = 0x00C2; // 16 bits each: (uncompressed bits << 8) | compressed bits
pub const MODE_SELECT: u16 = 0x0100; // 8 bits
pub const GROUPED_HOLD: u16 = 0x0104; // 8 bits
pub const CSI_DATA_FORMAT: u16 = 0x0112; // (uncompressed << 8) | compressed: G#0A0A RAW10, G#0C0C RAW12
pub const COARSE_INTEGRATION: u16 = 0x0202;
pub const FRAME_LENGTH_LINES: u16 = 0x0340;
pub const LINE_LENGTH_PCK: u16 = 0x0342;
pub const X_ADDR_START: u16 = 0x0344;
pub const Y_ADDR_START: u16 = 0x0346;
pub const X_ADDR_END: u16 = 0x0348;
pub const Y_ADDR_END: u16 = 0x034A;
pub const X_OUTPUT_SIZE: u16 = 0x034C;
pub const Y_OUTPUT_SIZE: u16 = 0x034E;
pub const X_EVEN_INC: u16 = 0x0380;
pub const X_ODD_INC: u16 = 0x0382;
pub const Y_EVEN_INC: u16 = 0x0384;
pub const Y_ODD_INC: u16 = 0x0386;
pub const DIGITAL_CROP_X: u16 = 0x0408;
pub const DIGITAL_CROP_Y: u16 = 0x040A;
pub const DIGITAL_CROP_W: u16 = 0x040C;
pub const DIGITAL_CROP_H: u16 = 0x040E;
pub const BINNING_MODE: u16 = 0x0900; // 8 bits, then binning_type at G#0901 and (written by the Sony part) binning_weighting at G#0902
pub const COARSE_INTEGRATION_MIN: u16 = 0x1004;
pub const COARSE_INTEGRATION_MAX_MARGIN: u16 = 0x1006;
/// The Samsung parts' page select: G#4000 is the CCS page.
pub const SAMSUNG_PAGE: u16 = 0xFCFC;
pub const CCS_PAGE: u16 = 0x4000;

/// The data formats (CCS csi_data_format).
pub const RAW10: u16 = 0x0A0A;
pub const RAW12: u16 = 0x0C0C;

/// A rear sensor's pixel array as its CCS addresses see it, from its captured init and Android's characteristics (Lumis docs/google-bug-tele-maxres-fov.md: PIXEL_ARRAY and PIXEL_ARRAY_MAXIMUM_RESOLUTION).
#[derive(Clone, Copy, Debug)]
pub struct Array {
    /// Address units per pixel on each axis: the main's addresses run over 16320 x 12288 for its 8160 x 6144 pixels (x_addr G#10 to G#3FCF, increments of 4 for a 2 x 2 binned output pixel), the others one a pixel.
    pub unit: u16,
    /// The colour filter's period in pixels: every rear sensor is quad Bayer (a 4 x 4 cell of 2 x 2 same-colour blocks), whose binned output is plain Bayer (a 2 x 2 cell).
    pub period: u16,
    /// The 1:1 readout Android offers (SENSOR_PIXEL_MODE_MAXIMUM_RESOLUTION, RAW10), width and height.
    pub full: (u16, u16),
    /// The Samsung parts select the CCS page with G#FCFC first; the Sony part needs nothing.
    pub samsung: bool,
    /// Whether its init writes the increments (the Samsung parts) and the digital crop and binning weighting (the Sony part): a mode writes back only what the init set.
    pub incs: bool,
    pub crop: bool,
    pub weighting: bool,
    /// The binning bytes (G#0900, G#0901) a 1:1 mode writes: a guess on every sensor until a capture of Android's maximum-resolution stream (docs/CAMERA.md "The 1:1 capture").
    pub unbinned: [u8; 2],
    /// The gain the list offers (1024 one): the sensor's own range (`Gain::range`, read at each open and said on the console), written here once measured on the device so the list, built with no camera powered, can carry it; 1024 and 1024 (the captured code alone) until then.
    pub gain: (u64, u64),
}

/// By camera id (ferros_cam cams): the main (Samsung, G#08EF), the ultrawide (Sony IMX787), the front (not a rear sensor: no 1:1 mode offered), the tele (Samsung, G#08D5).
pub const ARRAYS: [Array; 4] = [
    // The main's gain, its own words read on the Pro (2026-11-17): analogue code / 32 over codes 32 to 5120 (1x to 160x), digital 1x to 128x in whole steps, measured linear from 8x to 128x (twice the signal a doubling, 2.35 to 2.03). Offered: the analogue range alone, 1024 to 163840 (owner, 2026-11-17: digital gain is after the ADC, so analogue only); the noise test (camtest 4, b915) puts the real analogue at the stated 160x: no empty codes at any step, and the dark noise grows 1.67 times from 64x to 128x and 1.10 from 128x to 160x (2 and 1.25 if the gain were after the ADC).
    Array { unit: 2, period: 4, full: (8160, 6144), samsung: true, incs: true, crop: false, weighting: false, unbinned: [0x00, 0x11], gain: (UNITY, 160 * UNITY) },
    // The ultrawide's gain, its own words read on the Pro (b915, 2026-11-17): capability 0, gain = 1024 / (1024 - code) over codes 0 to 1008 (1x to 64x analogue), digital never used. The noise test (camtest 4): no empty codes up to 64x, and the dark noise grows 4.8 times from 8x to 64x (8 times if the gain were after the ADC), so analogue to its stated most.
    Array { unit: 1, period: 4, full: (8000, 6000), samsung: false, incs: false, crop: true, weighting: true, unbinned: [0x00, 0x11], gain: (UNITY, 64 * UNITY) },
    Array { unit: 1, period: 2, full: (0, 0), samsung: true, incs: true, crop: false, weighting: false, unbinned: [0x00, 0x11], gain: (UNITY, UNITY) },
    Array { unit: 1, period: 4, full: (8064, 6048), samsung: true, incs: true, crop: false, weighting: false, unbinned: [0x00, 0x11], gain: (UNITY, UNITY) },
];

/// A readout mode: every CCS register a mode change writes, as the sensor holds it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mode {
    pub x_start: u16,
    pub y_start: u16,
    pub x_end: u16,
    pub y_end: u16,
    pub x_out: u16,
    pub y_out: u16,
    /// x even, x odd, y even, y odd increments (the Samsung parts write them).
    pub inc: [u16; 4],
    /// G#0900, G#0901, G#0902.
    pub binning: [u8; 3],
    /// The digital crop: x and y offset, width and height (the Sony part writes it).
    pub crop: [u16; 4],
    pub fll: u16,
    pub llp: u16,
    pub coarse: u16,
    pub format: u16,
    /// Output pixels per array pixel on each axis' inverse: 2 for the binned modes, 1 for 1:1.
    pub bin: u16,
}

/// The registers `Mode::captured` reads back out of an init, and their widths.
const READ: [(u16, u8); 27] = [
    (X_ADDR_START, 2), (Y_ADDR_START, 2), (X_ADDR_END, 2), (Y_ADDR_END, 2), (X_OUTPUT_SIZE, 2), (Y_OUTPUT_SIZE, 2),
    (X_EVEN_INC, 2), (X_ODD_INC, 2), (Y_EVEN_INC, 2), (Y_ODD_INC, 2),
    (BINNING_MODE, 1), (BINNING_MODE + 1, 1), (BINNING_MODE + 2, 1),
    (DIGITAL_CROP_X, 2), (DIGITAL_CROP_Y, 2), (DIGITAL_CROP_W, 2), (DIGITAL_CROP_H, 2),
    (FRAME_LENGTH_LINES, 2), (LINE_LENGTH_PCK, 2), (COARSE_INTEGRATION, 2), (CSI_DATA_FORMAT, 2),
    (0x0306, 2), (0x0304, 2), (0x0300, 2), (0x0136, 2), (ANALOGUE_GAIN, 2), (DIGITAL_GAIN, 2),
];

/// An init table's final values on the CCS page, for the registers in `READ` (the PLL's last four too, for the console): bytes written directly while the page select reads G#4000 (or on a part with no page select), or thru the indirect window into page G#4000; each byte the last one written. None for a register the init never wrote whole.
pub struct Page {
    vals: [(u16, u32, u8); READ.len()],
}

impl Page {
    pub fn of(init: &[u8]) -> Page {
        let mut vals = [(0u16, 0u32, 0u8); READ.len()];
        for (k, &(a, _)) in READ.iter().enumerate() {
            vals[k].0 = a;
        }
        let mut page = CCS_PAGE;
        let (mut hi, mut lo) = (0u16, 0u16);
        let put = |vals: &mut [(u16, u32, u8); READ.len()], at: u32, bytes: &[u8]| {
            for (j, &b) in bytes.iter().enumerate() {
                let a = at + j as u32;
                if a >> 16 != CCS_PAGE as u32 {
                    continue;
                }
                let a = a as u16;
                for (k, &(r, w)) in READ.iter().enumerate() {
                    if a >= r && a < r + w as u16 {
                        let byte = (r + w as u16 - 1 - a) as u32;
                        vals[k].1 = vals[k].1 & !(0xFF << (8 * byte)) | (b as u32) << (8 * byte);
                        vals[k].2 |= 1 << byte;
                    }
                }
            }
        };
        for rec in crate::inputs::records(init) {
            let Ok(crate::inputs::Rec::Write(w)) = rec else { continue };
            if w.len() < 3 {
                continue;
            }
            let a = (w[0] as u16) << 8 | w[1] as u16;
            let d = &w[2..];
            let v16 = if d.len() >= 2 { (d[0] as u16) << 8 | d[1] as u16 } else { d[0] as u16 };
            match a {
                SAMSUNG_PAGE => page = v16,
                0x6028 => hi = v16,
                0x602A => lo = v16,
                0x6F12 => {
                    put(&mut vals, (hi as u32) << 16 | lo as u32, d);
                    lo = lo.wrapping_add(d.len() as u16);
                }
                _ => put(&mut vals, (page as u32) << 16 | a as u32, d),
            }
        }
        Page { vals }
    }

    /// Register `a`'s final value, when the init wrote all its bytes.
    pub fn get(&self, a: u16) -> Option<u32> {
        let k = READ.iter().position(|r| r.0 == a)?;
        let full = (1u8 << READ[k].1) - 1;
        (self.vals[k].2 == full).then_some(self.vals[k].1)
    }
}

/// Why a mode could not be made.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Error {
    /// The init leaves this register unwritten: the mode it set is not known whole.
    Missing(u16),
    /// No 1:1 readout is known for this camera.
    NoFullMode,
    /// A slit of no rows, an odd count, or one past the readout.
    Rows(u16),
    /// The integration (lines) does not fit the frame (lines) with its margin: CCS asks coarse <= frame_length_lines - max_margin, and the 2026-10-20 slit ran the init's 21342 lines in frames of 68 to 576.
    Integration(u16, u16),
    /// A frame shorter than the rows it carries.
    Frame(u16, u16),
}

/// The integration's margin under the frame, in lines: the captured main mode runs 21342 lines in a 21362-line frame (20), so 2^5 holds it with room; the sensor's own (CCS G#1006) is read at open where the part answers it.
pub const MARGIN: u16 = 1 << 5;

impl Mode {
    /// The mode an init leaves the sensor in (Android's binned mode); `bin` 2 (every rear camera's init is its binned mode).
    pub fn captured(init: &[u8], a: &Array) -> Result<Mode, Error> {
        let p = Page::of(init);
        let g = |r: u16| p.get(r).map(|v| v as u16).ok_or(Error::Missing(r));
        let opt = |r: u16| p.get(r).map(|v| v as u16).unwrap_or(0);
        let inc = if a.incs { [g(X_EVEN_INC)?, g(X_ODD_INC)?, g(Y_EVEN_INC)?, g(Y_ODD_INC)?] } else { [0; 4] };
        let crop = if a.crop { [g(DIGITAL_CROP_X)?, g(DIGITAL_CROP_Y)?, g(DIGITAL_CROP_W)?, g(DIGITAL_CROP_H)?] } else { [0; 4] };
        let binning = [g(BINNING_MODE)? as u8, g(BINNING_MODE + 1)? as u8, if a.weighting { g(BINNING_MODE + 2)? as u8 } else { 0 }];
        Ok(Mode {
            x_start: g(X_ADDR_START)?,
            y_start: g(Y_ADDR_START)?,
            x_end: g(X_ADDR_END)?,
            y_end: g(Y_ADDR_END)?,
            x_out: g(X_OUTPUT_SIZE)?,
            y_out: g(Y_OUTPUT_SIZE)?,
            inc,
            binning,
            crop,
            fll: g(FRAME_LENGTH_LINES)?,
            llp: g(LINE_LENGTH_PCK)?,
            coarse: g(COARSE_INTEGRATION)?,
            format: opt(CSI_DATA_FORMAT),
            bin: 2,
        })
    }

    /// Address units per output row (and column): the increment the init set, or the binning times the unit where the part bins in its binning registers alone (the Sony part).
    pub fn units_per_row(&self, a: &Array) -> u16 {
        if a.incs { self.inc[2] } else { self.bin * a.unit }
    }

    /// The 1:1 readout of the same array: every pixel, binning off, the increments one pixel, the output Android's maximum-resolution size (a guess on every sensor, built from the CCS registers alone; the vendor's setfile may carry mode timing this does not change: docs/CAMERA.md "The 1:1 capture").
    /// The line length doubles by default: a 1:1 line carries twice the pixels of a binned one at the same link rate (the binned main already sends 4080 RAW10 pixels each 3.2 us, about 13 Gbit/s).
    pub fn unbinned(&self, a: &Array) -> Result<Mode, Error> {
        if a.full.0 == 0 {
            return Err(Error::NoFullMode);
        }
        let (w, h) = a.full;
        let mut m = *self;
        m.bin = 1;
        m.binning = [a.unbinned[0], a.unbinned[1], 0];
        if a.incs {
            m.inc = [a.unit; 4];
        }
        m.x_out = w;
        m.y_out = h;
        if a.crop {
            // The Sony part reads its window binned and crops after: 1:1 the window is the same, the crop twice the binned one's offsets and the full size.
            m.crop = [self.crop[0] * 2, self.crop[1] * 2, w, h];
        }
        m.llp = self.llp * 2;
        m.fll = h + (1 << 8);
        m.coarse = MARGIN;
        Ok(m)
    }

    /// A slit of `rows` output rows across the whole width, centred on the array's middle (the captured window's: the optical axis as Android crops it) and started on the colour filter's period, so its rows begin a whole cell (binned: a 2 x 2 Bayer cell is two rows; 1:1, a 4 x 4 quad cell is four), and the frame cut to `fll` lines with `coarse` lines of integration.
    /// The middle: the window's centre rounded to the nearest period, so the slit lies half a period off the true middle when its height is an odd number of periods.
    pub fn slit(&self, a: &Array, rows: u16, fll: u16, coarse: u16) -> Result<Mode, Error> {
        let cell = if self.bin == 1 { a.period } else { a.period / 2 };
        if rows == 0 || rows % cell != 0 || rows > self.y_out {
            return Err(Error::Rows(rows));
        }
        let mut m = *self;
        let upr = self.units_per_row(a);
        // Address rows: the slit's own, and the period's (a period of pixels is `period * unit` addresses).
        let span = rows * upr;
        let period = a.period * a.unit;
        // The captured window's top and its whole height in address rows; the Sony part's crop trims rows off the binned window, which the slit leaves out (its crop is set to the slit itself below).
        let top = self.y_start;
        let height = self.y_end + 1 - self.y_start;
        // The first period-aligned start whose slit is centred to the nearest period: (height - span) / 2 rounded to a period.
        let off = ((height - span) / 2 + period / 2) / period * period;
        m.y_start = top + off;
        m.y_end = m.y_start + span - 1;
        m.y_out = rows;
        if a.crop {
            m.crop[1] = 0;
            m.crop[3] = rows;
        }
        m.fll = fll;
        m.coarse = coarse;
        m.check()?;
        Ok(m)
    }

    /// The frame cut to `fll` lines and `coarse` of integration: refused when the integration would pass the frame's margin or the frame its rows.
    pub fn timed(&self, fll: u16, coarse: u16) -> Result<Mode, Error> {
        let mut m = *self;
        m.fll = fll;
        m.coarse = coarse;
        m.check()?;
        Ok(m)
    }

    fn check(&self) -> Result<(), Error> {
        if self.fll < self.y_out {
            return Err(Error::Frame(self.fll, self.y_out));
        }
        if self.coarse == 0 || self.coarse + MARGIN > self.fll {
            return Err(Error::Integration(self.coarse, self.fll));
        }
        Ok(())
    }

    /// RAW12 instead of RAW10 (the sensor's data format; the receiver's data type and the front end's stride follow it: `crate::geom`).
    pub fn raw12(&self) -> Mode {
        let mut m = *self;
        m.format = RAW12;
        m
    }

    /// Bits a sample on the link.
    pub fn bits(&self) -> u16 {
        self.format >> 8
    }

    /// The mode's writes, in order: the page select (Samsung), the hold, every mode register, the release (the camera's own hold and release bytes, `Cam::hold`). At most `WRITES_MOST`; each `(len, bytes)` one I2C write of a 16-bit address and its data.
    pub fn writes(&self, a: &Array, c: &Cam) -> Writes {
        let mut w = Writes { list: [(0, [0; 6]); WRITES_MOST], n: 0 };
        if a.samsung {
            w.push16(SAMSUNG_PAGE, CCS_PAGE);
        }
        w.push_raw(c.hold);
        if self.format != 0 {
            w.push16(CSI_DATA_FORMAT, self.format);
        }
        w.push16(LINE_LENGTH_PCK, self.llp);
        w.push16(FRAME_LENGTH_LINES, self.fll);
        w.push16(X_ADDR_START, self.x_start);
        w.push16(Y_ADDR_START, self.y_start);
        w.push16(X_ADDR_END, self.x_end);
        w.push16(Y_ADDR_END, self.y_end);
        if a.incs {
            w.push16(X_EVEN_INC, self.inc[0]);
            w.push16(X_ODD_INC, self.inc[1]);
            w.push16(Y_EVEN_INC, self.inc[2]);
            w.push16(Y_ODD_INC, self.inc[3]);
        }
        if a.weighting {
            w.push_raw(&[(BINNING_MODE >> 8) as u8, BINNING_MODE as u8, self.binning[0], self.binning[1], self.binning[2]]);
        } else {
            w.push_raw(&[(BINNING_MODE >> 8) as u8, BINNING_MODE as u8, self.binning[0], self.binning[1]]);
        }
        if a.crop {
            w.push16(DIGITAL_CROP_X, self.crop[0]);
            w.push16(DIGITAL_CROP_Y, self.crop[1]);
            w.push16(DIGITAL_CROP_W, self.crop[2]);
            w.push16(DIGITAL_CROP_H, self.crop[3]);
        }
        w.push16(X_OUTPUT_SIZE, self.x_out);
        w.push16(Y_OUTPUT_SIZE, self.y_out);
        w.push16(COARSE_INTEGRATION, self.coarse);
        w.push_raw(c.release);
        w
    }
}

/// The most writes a mode change makes (the page, the hold, 19 registers, the release).
pub const WRITES_MOST: usize = 1 << 5;

/// A mode change's I2C writes.
pub struct Writes {
    list: [(u8, [u8; 6]); WRITES_MOST],
    n: usize,
}

impl Writes {
    fn push16(&mut self, a: u16, v: u16) {
        self.push_raw(&[(a >> 8) as u8, a as u8, (v >> 8) as u8, v as u8]);
    }
    fn push_raw(&mut self, b: &[u8]) {
        let mut e = [0u8; 6];
        e[..b.len()].copy_from_slice(b);
        self.list[self.n] = (b.len() as u8, e);
        self.n += 1;
    }
    pub fn iter(&self) -> impl Iterator<Item = &[u8]> {
        self.list[..self.n].iter().map(|(n, b)| &b[..*n as usize])
    }
    pub fn len(&self) -> usize {
        self.n
    }
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }
}

// ---------------------------------------------------------------- gain (owner, 2026-11-17: "the ISO doesn't do anything in lumis")

/// The analogue gain's capability block (CCS: G#0080 analogue_gain_capability, 0 global; G#0084 code min, G#0086 code max, G#0088 code step, G#008A type, G#008C m0, G#008E c0, G#0090 m1, G#0092 c1, the last four signed), read as one 20-byte run from G#0080.
pub const GAIN_CAPS: u16 = 0x0080;
pub const GAIN_CAPS_LEN: usize = 20;
/// The digital gain's limits (CCS: G#1084 digital_gain_min, G#1086 max, G#1088 step), one 6-byte run; its value is 8.8 fixed point (G#0100 one), as both inits write G#020E.
pub const DGAIN_CAPS: u16 = 0x1084;
pub const DGAIN_CAPS_LEN: usize = 6;
pub const ANALOGUE_GAIN: u16 = 0x0204;
pub const DIGITAL_GAIN: u16 = 0x020E;
/// The endpoint's gain unit: 1024 one (the list's gain_min and gain_max, SET's value, a frame's gain).
pub const UNITY: u64 = 1 << 10;

/// A sensor's gain as its CCS capability words state it: analogue gain = (m0 code + c0) / (m1 code + c1) over codes min to max in steps (CCS's model; Sony's 1024 / (1024 - code) is m0 0, c0 1024, m1 -1, c1 1024), then digital gain in 8.8 from d_min to d_max.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gain {
    pub code_min: u16,
    pub code_max: u16,
    pub step: u16,
    pub m0: i16,
    pub c0: i16,
    pub m1: i16,
    pub c1: i16,
    pub d_min: u16,
    pub d_max: u16,
    pub d_step: u16,
}

/// A gain split for the sensor: the analogue code, the digital gain (8.8) and the gain they make (1024 one).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GainSet {
    pub code: u16,
    pub digital: u16,
    pub gain: u64,
}

impl Gain {
    /// The model from the sensor's own words (`GAIN_CAPS` and `DGAIN_CAPS` read big-endian), or None when they do not describe one: a capability other than 0 or 2, no step, codes out of order, a denominator that reaches zero or goes negative over the codes, an analogue gain falling with the code, or digital limits out of order (a sensor whose capability block aliases, as the main's limit block at G#1100 does, says so here instead of driving the gain off a wrong model).
    pub fn of(a: &[u8; GAIN_CAPS_LEN], d: &[u8; DGAIN_CAPS_LEN]) -> Option<Gain> {
        let be = |b: &[u8], k: usize| u16::from_be_bytes([b[k], b[k + 1]]);
        let mut g = Gain {
            code_min: be(a, 4),
            code_max: be(a, 6),
            step: be(a, 8),
            m0: be(a, 12) as i16,
            c0: be(a, 14) as i16,
            m1: be(a, 16) as i16,
            c1: be(a, 18) as i16,
            d_min: be(d, 0),
            d_max: be(d, 2),
            d_step: be(d, 4),
        };
        // No digital step stated: digital gain not offered, left at one.
        if g.d_step == 0 {
            (g.d_min, g.d_max, g.d_step) = (0x100, 0x100, 1);
        }
        let den = |c: u16| g.m1 as i64 * c as i64 + g.c1 as i64;
        // The capability 0 (global) or 2: the main answers 2 (the Pro, 2026-11-17: words 2 0 20 1400 1 0 1 0 0 20, codes 32 to 5120 at code / 32, its init at G#400) and its init drives G#0204 all the same, so the code model stands; the curve measured on the device says whether it holds.
        let ok = matches!(be(a, 0), 0 | 2)
            && g.step != 0
            && g.code_min <= g.code_max
            && den(g.code_min) > 0
            && den(g.code_max) > 0
            && g.analogue(g.code_min) >= 1
            && g.analogue(g.code_max) >= g.analogue(g.code_min)
            && g.d_min != 0
            && g.d_min <= g.d_max;
        ok.then_some(g)
    }

    /// The analogue gain a code gives (1024 one).
    pub fn analogue(&self, code: u16) -> u64 {
        let num = self.m0 as i64 * code as i64 + self.c0 as i64;
        let den = self.m1 as i64 * code as i64 + self.c1 as i64;
        (num * UNITY as i64 / den) as u64
    }

    /// The least and the most gain offered (1024 one): the analogue range alone, the least code to the most (owner, 2026-11-17: digital gain is after the ADC, so analogue only; the digital limits are read and said, never used).
    pub fn range(&self) -> (u64, u64) {
        (self.analogue(self.code_min), self.analogue(self.code_max))
    }

    /// The code whose gain is the most not past `want` (1024 one), on the code's step from the least: the model solved for the code, then walked down a step while it overshoots (integer division can land one step high).
    fn code_under(&self, want: u64) -> u16 {
        // want = 1024 (m0 c + c0) / (m1 c + c1), so c = (want c1 - 1024 c0) / (1024 m0 - want m1).
        let (w, u) = (want as i64, UNITY as i64);
        let den = u * self.m0 as i64 - w * self.m1 as i64;
        let c = (w * self.c1 as i64 - u * self.c0 as i64) / den;
        let lo = self.code_min as i64;
        let k = (c - lo) / self.step as i64;
        let mut code = (lo + k * self.step as i64) as u16;
        while code > self.code_min && self.analogue(code) > want {
            code -= self.step;
        }
        code
    }

    /// `want` (1024 one) for the sensor: the analogue code whose gain is the most not past it, digital gain left at one (G#0100, written so a digital gain anything else left is undone); None outside `range`, which SET answers as outside the range: refused, never capped (as an exposure past the frame is).
    pub fn split(&self, want: u64) -> Option<GainSet> {
        let (lo, hi) = self.range();
        if want < lo || want > hi {
            return None;
        }
        let code = self.code_under(want);
        Some(GainSet { code, digital: 0x100, gain: self.analogue(code) })
    }
}

/// A mode's frame period in picoseconds, from the captured mode's measured line time `line_ps` at its own line length: the line time scales with `llp` (the pixel clock is the PLL's and does not move).
pub fn frame_ps(m: &Mode, captured_llp: u16, line_ps: u64) -> u64 {
    m.fll as u64 * line_ps * m.llp as u64 / captured_llp as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cams;

    fn local(name: &str) -> Option<Vec<u8>> {
        let b = std::fs::read(format!("/mnt/Harbor/Code/ferros/target/cam/{name}")).ok()?;
        // The main's init is the bundle's first part (cam.frame, a 'CFRM').
        Some(match crate::inputs::split_bundle(&b) {
            Some((init, _)) => init.to_vec(),
            None => b,
        })
    }

    /// The main's init (target/cam/cam.frame, local): Android's binned mode as docs/oracle/CAMERA.md decoded it by hand in October.
    #[test]
    fn the_main_mode() {
        let Some(init) = local("cam.frame") else { return };
        let a = &ARRAYS[0];
        let m = Mode::captured(&init, a).unwrap();
        println!("main: {m:?}");
        assert_eq!((m.x_start, m.x_end, m.y_start, m.y_end), (16, 0x3FCF, 16, 0x300F));
        assert_eq!((m.x_out, m.y_out, m.inc), (4080, 3072, [4; 4]));
        assert_eq!((m.binning[0], m.binning[1]), (0x32, 0x44));
        assert_eq!((m.fll, m.llp, m.coarse, m.format), (0x5372, 6320, 0x535E, RAW10));
        // The CCS addresses span 16320 x 12288, two to a pixel of the 8160 x 6144 array.
        assert_eq!(((m.x_end + 1 - m.x_start) / a.unit, (m.y_end + 1 - m.y_start) / a.unit), a.full);
        let p = Page::of(&init);
        assert_eq!((p.get(0x0136), p.get(0x0304), p.get(0x0306), p.get(0x0300)), (Some(0x1800), Some(3), Some(245), Some(8)));
    }

    #[test]
    fn the_ultrawide_and_tele_modes() {
        if let Some(init) = local("uw.init") {
            let m = Mode::captured(&init, &ARRAYS[1]).unwrap();
            println!("ultrawide: {m:?}");
            assert_eq!((m.x_out, m.y_out, m.binning, m.crop), (4000, 3000, [1, 0x22, 8], [312, 4, 4000, 3000]));
            assert_eq!((m.fll, m.llp, m.coarse), (5081, 9568, 4124));
        }
        if let Some(init) = local("tele.init") {
            let m = Mode::captured(&init, &ARRAYS[3]).unwrap();
            println!("tele: {m:?}");
            assert_eq!((m.x_out, m.y_out, m.inc, m.binning[0], m.binning[1]), (4032, 3024, [2; 4], 0x22, 0x22));
            assert_eq!((m.x_end + 1 - m.x_start, m.fll, m.llp), (8064, 5457, 5568));
        }
    }

    /// A slit lies on the colour filter's period, centred to the nearest one, with the rows asked for; a slit of rows that break the cell is refused, as an integration past the frame is.
    #[test]
    fn slits_centre_on_the_period() {
        let Some(init) = local("cam.frame") else { return };
        let a = &ARRAYS[0];
        let m = Mode::captured(&init, a).unwrap();
        for rows in [2u16, 4, 8, 16, 32, 64] {
            let s = m.slit(a, rows, rows + (1 << 7), 1 << 4).unwrap();
            let span = s.y_end + 1 - s.y_start;
            assert_eq!(span, rows * 4);
            // A whole quad cell (8 addresses) from the window's top, so the mosaic's phase is the captured mode's.
            assert_eq!((s.y_start - m.y_start) % 8, 0, "rows {rows}");
            let centre = (s.y_start as i32 + s.y_end as i32 + 1) / 2;
            let middle = (m.y_start as i32 + m.y_end as i32 + 1) / 2;
            assert!((centre - middle).abs() <= 4, "rows {rows}: centre {centre} middle {middle}");
            println!("binned slit of {rows}: y G#{:X} to G#{:X}", s.y_start, s.y_end);
        }
        assert_eq!(m.slit(a, 3, 128, 16), Err(Error::Rows(3)));
        assert_eq!(m.slit(a, 4, 64, 64), Err(Error::Integration(64, 64)));
        let u = m.unbinned(a).unwrap();
        assert_eq!((u.x_out, u.y_out, u.inc, u.bin), (8160, 6144, [2; 4], 1));
        for rows in [4u16, 8, 64] {
            let s = u.slit(a, rows, rows + (1 << 7), 1 << 4).unwrap();
            assert_eq!(s.y_end + 1 - s.y_start, rows * 2);
            assert_eq!((s.y_start - m.y_start) % 8, 0);
        }
        // 1:1, a slit of two rows would cut the quad cell in half.
        assert_eq!(u.slit(a, 2, 128, 16), Err(Error::Rows(2)));
    }

    /// The writes: the page, the hold as the camera's own bytes, the registers big-endian, the release; on the Sony part no page and its three binning bytes.
    #[test]
    fn the_writes() {
        let Some(init) = local("cam.frame") else { return };
        let (a, c) = (&ARRAYS[0], &cams::MAIN);
        let m = Mode::captured(&init, a).unwrap().slit(a, 4, 132, 16).unwrap();
        let ws = m.writes(a, c);
        let w: Vec<&[u8]> = ws.iter().collect();
        assert_eq!(w[0], &[0xFC, 0xFC, 0x40, 0x00]);
        assert_eq!(w[1], c.hold);
        assert_eq!(*w.last().unwrap(), c.release);
        assert!(w.contains(&&[0x03u8, 0x4E, 0x00, 0x04][..]));
        assert!(w.contains(&&[0x03u8, 0x40, 0x00, 132][..]));
        assert!(w.contains(&&[0x09u8, 0x00, 0x32, 0x44][..]));
        assert!(w.len() <= WRITES_MOST);
        if let Some(init) = local("uw.init") {
            let (a, c) = (&ARRAYS[1], &cams::ULTRAWIDE);
            let m = Mode::captured(&init, a).unwrap();
            let s = m.slit(a, 2, 160, 16).unwrap();
            assert_eq!((s.crop[1], s.crop[3], s.y_end + 1 - s.y_start), (0, 2, 4));
            let ws = s.writes(a, c);
            let w: Vec<&[u8]> = ws.iter().collect();
            assert_eq!(w[0], c.hold);
            assert!(w.contains(&&[0x09u8, 0x00, 1, 0x22, 8][..]));
        }
    }

    /// The main's line time scales the frame: the captured mode's 21362 lines at 3.2237 us are its measured 68.9 ms; a 132-line slit is about 426 us of lines (the sensor kept a floor of about 825 us on 2026-10-21).
    #[test]
    fn gain_models() {
        let caps = |min: u16, max: u16, step: u16, m: [i16; 4]| {
            let mut a = [0u8; GAIN_CAPS_LEN];
            for (k, v) in [0u16, 0, min, max, step, 0, m[0] as u16, m[1] as u16, m[2] as u16, m[3] as u16].iter().enumerate() {
                a[2 * k..2 * k + 2].copy_from_slice(&v.to_be_bytes());
            }
            a
        };
        let d = |min: u16, max: u16, step: u16| {
            let mut b = [0u8; DGAIN_CAPS_LEN];
            b[0..2].copy_from_slice(&min.to_be_bytes());
            b[2..4].copy_from_slice(&max.to_be_bytes());
            b[4..6].copy_from_slice(&step.to_be_bytes());
            b
        };
        // Sony's convention: gain = 1024 / (1024 - code), codes 0 to 960 (16x); digital 1x to 16x in 1/256.
        let sony = Gain::of(&caps(0, 960, 1, [0, 1024, -1, 1024]), &d(0x100, 0x1000, 1)).unwrap();
        assert_eq!(sony.range(), (1024, 16384));
        assert_eq!(sony.split(2048), Some(GainSet { code: 512, digital: 0x100, gain: 2048 }));
        assert_eq!(sony.split(16384), Some(GainSet { code: 960, digital: 0x100, gain: 16384 }));
        // Past the analogue most: refused, never digital (owner, 2026-11-17).
        assert_eq!(sony.split(16385), None);
        // A gain between codes takes the one under it.
        let s = sony.split(3000).unwrap();
        assert!(s.gain <= 3000 && sony.analogue(s.code + 1) > 3000, "{s:?}");
        assert_eq!(sony.split(1000), None);
        // A linear model (gain = code / 32), codes 32 to 512; no digital step stated: no digital gain.
        let lin = Gain::of(&caps(32, 512, 1, [1, 0, 0, 32]), &d(0x100, 0x100, 0)).unwrap();
        assert_eq!(lin.range(), (1024, 16384));
        assert_eq!(lin.split(1024).unwrap().code, 32);
        assert_eq!(lin.split(5000).unwrap(), GainSet { code: 156, digital: 0x100, gain: 4992 });
        assert_eq!(lin.split(16385), None);
        // The main's own words (the Pro, 2026-11-17): capability 2, codes G#20 to G#1400 step 1, gain = code / 32; digital 1x to 128x in whole steps. Its init's G#400 is 32x.
        let main_a: [u8; GAIN_CAPS_LEN] = [0, 2, 0, 0, 0, 0x20, 0x14, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0x20];
        let main = Gain::of(&main_a, &d(0x100, 0x8000, 0x100)).unwrap();
        assert_eq!((main.analogue(0x20), main.analogue(0x400), main.analogue(0x1400)), (1024, 32768, 163840));
        assert_eq!(main.split(2048).unwrap().code, 64);
        assert_eq!(main.range(), (1024, 163840));
        assert_eq!(main.split(163841), None);
        assert!(main.split(163840).is_some_and(|s| s.digital == 0x100));
        // Words that describe no model: the aliasing the main's limit block shows (every word G#4140).
        assert_eq!(Gain::of(&[0x41, 0x40].repeat(10).try_into().unwrap(), &d(0x100, 0x100, 1)), None);
    }

    #[test]
    fn frame_times() {
        let Some(init) = local("cam.frame") else { return };
        let a = &ARRAYS[0];
        let m = Mode::captured(&init, a).unwrap();
        let ps = frame_ps(&m, m.llp, crate::MAIN_LINE_PS);
        assert!((68_000_000_000..69_500_000_000).contains(&ps), "{ps}");
        let s = m.slit(a, 4, 132, 16).unwrap();
        println!("slit frame {} ns", frame_ps(&s, m.llp, crate::MAIN_LINE_PS) / 1000);
    }
}
