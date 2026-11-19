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
    /// The binning bytes (G#0900, G#0901) a 1:1 mode writes: a guess on every sensor until a capture of Android's maximum-resolution stream (docs/CAMERA.md "The 1:1 capture"). The guess (`Mode::unbinned`) only; the main's capture says G#22 G#22, with its vendor words.
    pub unbinned: [u8; 2],
    /// The gain the list offers (1024 one): the sensor's own range (`Gain::range`, read at each open and said on the console), written here once measured on the device so the list, built with no camera powered, can carry it; 1024 and 1024 (the captured code alone) until then.
    pub gain: (u64, u64),
    /// The integration's margin under the frame (CCS coarse_integration_time_max_margin, G#1006) as the sensor answered it on the device, so the list, built with no camera powered, can offer the long exposures it makes (`Timing`); None where never read, and there the frame never moves for an exposure (the init's own integration is the longest offered).
    /// The driver reads the sensor's own word at each open and drives with that (`Timing::margin`); the console says so when it differs from this.
    pub margin: Option<u16>,
    /// The vendor's long-exposure shift (`Lte`), once a capture of Android's own long exposure shows how it is written; None until then.
    pub lte: Option<Lte>,
    /// The sensor's side channels (`Side`): phase detect and embedded data, which a windowed mode turns off at the sensor; None where not known.
    pub side: Option<Side>,
}

/// A sensor vendor's long-exposure shift: the frame and the integration each counted in units of 2^shift lines, past the 16 bits of CCS frame_length_lines and coarse_integration_time (owner, 2026-11-17: "this needs done", up to 16 s on the main as on Android).
/// The main's per-frame block under Android writes two such words in its hold every frame, G#0702 and G#0704, both 0 at its ordinary exposures (target/hv/cam-open3, the 2026-10-19 open capture, local).
/// What a long exposure writes there (which byte carries the shift, its most, whether the stream stops around the change) comes only from the capture of Android at 16 s (docs/CAMERA.md "Long exposures"), so no camera has one yet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lte {
    /// The frame length's shift register and the integration's.
    pub fll_reg: u16,
    pub coarse_reg: u16,
    /// The 16-bit word written for a shift: the shift moved left by this many bits (8 when the shift is the register's high byte).
    pub at: u8,
    /// The most shift the sensor takes (the field's width).
    pub most: u8,
    /// The sensor's frame counter in bits: the frame in lines, the frame length shifted (frame_length_lines << shift), kept in this many bits and wrapping past them.
    /// A frame of 2^bits lines or more is not a long frame on the sensor: it runs the frame modulo 2^bits lines (at the sensor's floor where that is 0), so the longest exposure is the longest integration whose frame stays under 2^bits lines (`Timing::shift_most`).
    pub frame_bits: u8,
    /// The shift alone as one byte at `fll_reg`, `coarse_reg` never written (Sony's G#3100 on the ultrawide: Android's 16 s hold writes it beside the frame and the integration, both counted in 2^shift lines, every register a byte; G#3101 is another register); else a 16-bit word into each of the two (Samsung's G#0702 and G#0704).
    pub byte: bool,
}

impl Lte {
    pub fn word(&self, shift: u8) -> u16 {
        (shift as u16) << self.at
    }

    /// Whether a frame of `fll` lines of 2^`shift` stays inside the frame counter (`frame_bits`).
    /// In 128 bits: `shift` is at most `most` (the table's 31, or a device test's override byte), and 16 bits of frame shifted that far pass 64 bits past a shift of 48.
    pub fn holds(&self, fll: u16, shift: u8) -> bool {
        (fll as u128) << shift >> self.frame_bits == 0
    }
}

/// A sensor's side channels on its CSI link besides the image: phase detect (the PD rows, the main's channel 1, 4080 x 768) and embedded data (channel 2, 4 lines on the main).
/// Both are laid out for the whole binned frame (the main's PD block says 768 rows whatever the window), so a mode with its own window (a slit, a narrower x) turns both off at the sensor (owner, 2026-11-17: "phase detect off at the sensor" for slits, kept for whole frames).
/// Read off the captured inits by diffing, not guessed (docs/CAMERA.md "The slit for real"):
/// - embedded data: G#0118 is two bytes, an enable (1) and the line count, the count matching each camera's link exactly (the main and the tele G#0104 with 4 embedded lines on the link, G#40FF0 at link +G#64; the front G#0102 with 2, G#20F00); off is G#0000;
/// - phase detect: the main's G#0720 to G#072E, G#0720 = 1 beside a block that describes the PD channel (G#0728/G#072A = 2040 x 768, G#072C/G#072E = 508 x 768: the 768 is the link's channel 1 height, G#3000FF0), written the same in the binned and the 1:1 inits; off is G#0720 = 0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Side {
    /// Phase detect's enable word (16 bits, 0 off), where known.
    pub pd: Option<u16>,
    /// Embedded data's control word (16 bits: enable byte and lines; 0 off).
    pub emb: u16,
}

/// By camera id (ferros_cam cams): the main (Samsung, G#08EF), the ultrawide (Sony IMX787), the front (not a rear sensor: no 1:1 mode offered), the tele (Samsung, G#08D5).
pub const ARRAYS: [Array; 4] = [
    // The main's gain, its own words read on the Pro (2026-11-17): analogue code / 32 over codes 32 to 5120 (1x to 160x), digital 1x to 128x in whole steps, measured linear from 8x to 128x (twice the signal a doubling, 2.35 to 2.03). Offered: the analogue range alone, 1024 to 163840 (owner, 2026-11-17: digital gain is after the ADC, so analogue only); the noise test (camtest 4, b915) puts the real analogue at the stated 160x: no empty codes at any step, and the dark noise grows 1.67 times from 64x to 128x and 1.10 from 128x to 160x (2 and 1.25 if the gain were after the ADC).
    // Its integration least and margin, CAPS on the Pro (2026-11-16, b897): 2 and 2.
    // Its long-exposure shift, Android's own at 16 s (the capture of 2026-11-17, Lumis on Android in its 8160 x 6144 mode, integrity 0): G#0702 and G#0704 both carry the shift in their high byte, written in every hold beside the integration and frame, which count in 2^shift lines (frame = integration + 36 in those units); 0, then 3, then 6 at 16 s, mid-stream, no stream off or on around it, the line length untouched. 6 is the most Android showed; 7 (16 s) and 8 (32 s) ran on ferros at the mode's line (b933: frames 16.4 s and 32.8 s apart, the sensor reading back its shifted counters and the line 6320), so 8 was offered (52.8 s) until the field's width was found.
    // The field is 5 bits (the Pro, 2026-11-17 night): shifts 21, 22 and 23 ran (frames 26.4 s, 52.8 s, 105.66 s apart, the long-exposure sweep), so bit 4 counts, while the registers keep all 16 bits written (read back G#FFFF). So the most is 31.
    // But the frame is counted in 32 bits of lines (the Pro, 2026-11-18, camtest's ceiling sweep, docs/CAMERA.md "The shift's ceiling"): frame_length_lines << shift wraps modulo 2^32. At 32768 lines of integration (a 32770-line frame) shifts 14 to 16 held, and 17 to 20 gave frames 2^18, 2^19, 2^20 and 2^21 lines apart (825.46 ms, 1650.9 ms, 3301.8 ms at the binned line; 1726.50 ms at 1:1 for 17): (32770 << s) mod 2^32 = 2^(s+1) lines. Shift 31 with an even frame wraps to 0 and the sensor runs at its floor (10.31 ms binned, 41.6 ms at 1:1), as on 2026-11-18 evening; with an odd frame (5 lines at shifts 29 to 31) it held. A frame of exactly 2^32 lines (4 at shift 30, 32768 at 17) ran at the floor, 2^32 - 2^17 and 2^32 - 2^16 lines held. The same in lines at 1:1, so the counter is lines, not time.
    // So the longest exposure is 65533 lines of 2^16 (its frame 65535 << 16, under 2^32): 13524 s, about 3.76 hours, binned (3.15 us a line); 28286 s, about 7.86 hours, at 1:1 (6.59 us). The shift 32 of 2026-11-17 (16 lines of 2^32) was this wrap too, not bit 5 ignored.
    Array { unit: 2, period: 4, full: (8160, 6144), samsung: true, incs: true, crop: false, weighting: false, unbinned: [0x00, 0x11], gain: (UNITY, 160 * UNITY), margin: Some(2), lte: Some(Lte { fll_reg: 0x0702, coarse_reg: 0x0704, at: 8, most: 31, frame_bits: 32, byte: false }), side: Some(Side { pd: Some(0x0720), emb: 0x0118 }) },
    // The ultrawide's gain, its own words read on the Pro (b915, 2026-11-17): capability 0, gain = 1024 / (1024 - code) over codes 0 to 1008 (1x to 64x analogue), digital never used. The noise test (camtest 4): no empty codes up to 64x, and the dark noise grows 4.8 times from 8x to 64x (8 times if the gain were after the ADC), so analogue to its stated most.
    // Its integration least and margin, CAPS on the Pro (2026-11-16, b898): 1 and 64. Its shift: Sony's G#3100, one byte, as Android's 16 s holds write it (6 at 16 s, the frame the integration plus 64 in 2^6 lines: docs/CAMERA.md "The I2C side without a flash"); its most the 6 Android uses, the field's width and the frame counter unmeasured (65471 lines of 2^6 is under 2^22).
    Array { unit: 1, period: 4, full: (8000, 6000), samsung: false, incs: false, crop: true, weighting: true, unbinned: [0x00, 0x11], gain: (UNITY, 64 * UNITY), margin: Some(64), lte: Some(Lte { fll_reg: 0x3100, coarse_reg: 0x3100, at: 0, most: 6, frame_bits: 32, byte: true }), side: None },
    // The front: its margin 5 read at its first opens on the Pro (2026-11-18, b976); its shift is the main's pair of words (G#0702, G#0704, the shift in the high byte), as Android's 1 s hold writes them (shift 1, docs/CAMERA.md "The I2C side without a flash"). Its most is the shift Android uses, 1 (65535 lines of 2^1, about 1.37 s); the field's width and the frame counter are unmeasured (at shift 1 every frame is under 2^17 lines).
    Array { unit: 1, period: 2, full: (0, 0), samsung: true, incs: true, crop: false, weighting: false, unbinned: [0x00, 0x11], gain: (UNITY, UNITY), margin: Some(5), lte: Some(Lte { fll_reg: 0x0702, coarse_reg: 0x0704, at: 8, most: 1, frame_bits: 32, byte: false }), side: Some(Side { pd: None, emb: 0x0118 }) },
    // The tele: Android's 16 s (the i2c trace of 2026-11-18, docs/CAMERA.md "The I2C side without a flash") is the main's pair of shift words (6 at 16 s: integration G#9FE8, frame G#A000 in 2^6 lines; Android's frame the integration plus 24).
    // Measured on the Pro (2026-11-18 night, docs/CAMERA.md "Every readout, captured"): its own words say integration least 4 and margin 4 (CAPS), gain code / 32 over codes 32 to 2048 (1x to 64x analogue; no noise test yet); frames 1 s to 65.5 s the exposure apart (shifts 1 to 8, the mean doubling each step); the shift field holds 29 (frames of 8 << 29 and 32770 << 29 lines held, 2^32 lines and past: no 32-bit wrap as the main's), while 30 and 31 run every frame at the sensor's floor (19.9 ms) whatever the frame. So its most is 29; the frame is held to 2^32 lines (`frame_bits` 32, 7.3 hours at the 22-degree line) as the longest waited out by inference, none past 65.5 s watched land.
    Array { unit: 1, period: 4, full: (8064, 6048), samsung: true, incs: true, crop: false, weighting: false, unbinned: [0x00, 0x11], gain: (UNITY, 64 * UNITY), margin: Some(4), lte: Some(Lte { fll_reg: 0x0702, coarse_reg: 0x0704, at: 8, most: 29, frame_bits: 32, byte: false }), side: Some(Side { pd: None, emb: 0x0118 }) },
];

// ---------------------------------------------------------------- long exposures (owner, 2026-11-17: "this needs done": up to 16 s on the main, as Lumis exposes on Android)

/// The integration's least and margin, one 4-byte read from G#1004 (CCS coarse_integration_time_min, coarse_integration_time_max_margin).
pub const COARSE_CAPS: u16 = COARSE_INTEGRATION_MIN;
/// The most CCS's 16-bit frame length and line length hold.
pub const LINES_MOST: u64 = (1 << 16) - 1;
/// A stretched line's step in pixel clocks: every line length this sensor has run on ferros is a multiple of 2^4 (6320 the init's, 5888, 5632, 5376 and 4864 the 2026-11-16 sweep), so a stretched one is kept on that step (CCS states no step, and the main's limit block does not answer).
pub const LLP_STEP: u64 = 1 << 4;
/// The longest line the 16 bits hold, on the step.
pub const LLP_MOST: u64 = LINES_MOST & !(LLP_STEP - 1);
/// The longest line time the front end has taken (ps): on the Pro (b932, 2026-11-17) the main's stream ran on at 30640 clocks a line (15.27 us, a 1 s exposure) and stopped at 61280 (30.53 us, 2 s), the frame begun and never ended. The limit is the front end's, so it is a time, not a clock count, and holds for every camera behind it until each is measured; between the two it is not measured. The value is the main's 30640 clocks at its 3.1489 us per 6320 (`MAIN_LINE_PS`), rounded up so it gives back 30640 exactly.
pub const LINE_PS_MOST: u64 = 15_266_264;

/// An open camera's frame timing, what an exposure in nanoseconds turns into (`plan`).
/// The line time is measured at the captured line length (`Cam::line_ps` at `Cam::llp`) and scales with the line length, the pixel clock being the PLL's (`frame_ps`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timing {
    pub line_ps: u64,
    pub captured_llp: u16,
    /// The open mode's own line length and frame length: the init's, or a mode's (OPEN's mode block). An exposure that fits this frame leaves both alone, so the frame rate stays the mode's.
    pub llp: u16,
    pub fll: u16,
    /// The sensor's integration least and margin (CCS G#1004, G#1006).
    pub coarse_min: u16,
    pub margin: u16,
    /// Whether the frame may move for an exposure: the sensor's margin read at this open, and the camera offered long exposures (`Array::margin`). Otherwise an exposure must fit the mode's own frame less `MARGIN`, as before 2026-11-17.
    pub long: bool,
    pub lte: Option<Lte>,
    /// Whether the line may be lengthened past the shift (or without one): never by default, as a longer line is a longer readout and more rolling-shutter skew (owner, 2026-11-17: "keep the readout clean ... it must never be the silent default"). A camera that offers it says the readout it costs (the list's `readout`).
    pub stretch: bool,
}

/// The sensor's timing for one exposure: line length, frame length and integration in their registers' units, and the vendor shift (0 without one).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    pub llp: u16,
    pub fll: u16,
    pub coarse: u16,
    pub shift: u8,
}

impl Timing {
    /// The longest line offered (clocks, on the step): the front end's longest line time (`LINE_PS_MOST`) at this sensor's pixel clock, or the 16 bits, whichever is shorter.
    pub fn llp_most(&self) -> u64 {
        let front = (LINE_PS_MOST * self.captured_llp as u64 / self.line_ps) & !(LLP_STEP - 1);
        // WHY the shorter: two ceilings, the register's 16 bits and the line time past which the front end stopped (b932); a line past either is one the sensor cannot hold or the stream does not survive.
        if front < LLP_MOST { front } else { LLP_MOST }
    }

    /// The line time at line length `llp` (ps).
    pub fn line_ps_at(&self, llp: u16) -> u64 {
        self.line_ps * llp as u64 / self.captured_llp as u64
    }

    /// The mode's own timing with `coarse` lines of integration.
    pub fn own(&self, coarse: u16) -> Plan {
        Plan { llp: self.llp, fll: self.fll, coarse, shift: 0 }
    }

    /// The frame for `coarse` integration in units of 2^`shift` lines (CCS: coarse <= frame_length_lines - margin, both in those units).
    /// WHY the choice of the larger: the frame has two floors, the mode's own frame in real lines (its rows and the blanking the front end needs: 128 lines on the ultrawide, 2026-11-16), here in the shift's units rounded up, and the integration plus the sensor's margin; it is the larger of the two, never cut under either.
    fn frame(&self, coarse: u64, shift: u8) -> u16 {
        let floor = (self.fll as u64).div_ceil(1 << shift);
        if coarse + self.margin as u64 > floor { (coarse + self.margin as u64) as u16 } else { floor as u16 }
    }

    /// A device test of the shift field's reach (owner, 2026-11-17: "no artificial exposure cap anywhere"): `ns` at exactly `shift`, the mode's line, whatever the least shift would be; None when the integration does not fit 16 bits at that shift or falls under the sensor's least, or the camera has no shift.
    /// Not held to the sensor's frame counter (`Lte::holds`): a frame past it runs wrapped, which is how the counter was measured (the Pro, 2026-11-18); `plan` is.
    pub fn plan_shift(&self, ns: u64, shift: u8) -> Option<Plan> {
        self.lte?;
        // WHY checked: `shift` is the caller's byte: past 63 a shift of 64 bits has no meaning, and is refused.
        let lines = self.lines(ns);
        let c = lines.checked_shr(shift as u32)?;
        if c < self.coarse_min as u64 || c > LINES_MOST - self.margin as u64 {
            return None;
        }
        Some(Plan { llp: self.llp, fll: self.frame(c, shift), coarse: c as u16, shift })
    }

    /// The timing for an exposure of `ns`, in the order that changes the least: (0) the mode's own frame when the integration fits it; (1) the frame lengthened, to the 16 bits CCS gives it (the main: about 0.21 s); (2) with the vendor's shift, the frame and integration counted in 2^shift lines at the mode's line length (Android's way, `Lte`); (3) without one, the line lengthened on `LLP_STEP` to `llp_most` (the main: about 1.02 s).
    /// None: shorter than the sensor's least integration, or longer than the most this camera takes (refused, never capped, as a gain past the range is).
    pub fn plan(&self, ns: u64) -> Option<Plan> {
        // In 128 bits: `ns` is the caller's eight bytes (SET), and past 2^64 / 1000 the product in 64 would wrap and read as a short exposure.
        let ps = ns as u128 * 1000;
        let lines = self.lines(ns);
        if lines < self.coarse_min as u64 {
            return None;
        }
        let room = if self.long { self.margin as u64 } else { MARGIN as u64 };
        if lines + room <= self.fll as u64 {
            return Some(self.own(lines as u16));
        }
        if !self.long {
            return None;
        }
        // The most integration 16 bits of frame hold with the margin.
        let most = LINES_MOST - self.margin as u64;
        if lines <= most {
            return Some(Plan { llp: self.llp, fll: self.frame(lines, 0), coarse: lines as u16, shift: 0 });
        }
        // With the vendor's shift: the least shift that brings the integration under 16 bits at the mode's line (Android's choice on 2026-11-17: shift 3 for an exposure 3 needs, 6 at 16 s, the line untouched). The shift scales the sensor's counters only: the line, and with it the readout, stay the mode's (owner, 2026-11-17: "keep the readout clean").
        // No further than `shift_most`: past it the frame no longer fits the sensor's frame counter for any exposure the shifts before it could not hold (the frame in lines is the exposure plus the margin either way), and the sensor would run the frame wrapped (`Lte::frame_bits`).
        let mut shift = 0u8;
        if let Some(l) = self.lte {
            let top = self.shift_most(&l);
            shift = 1;
            while shift <= top {
                let c = lines >> shift;
                if c <= most {
                    let fll = self.frame(c, shift);
                    // WHY the test: at the least shift a frame past the counter is an exposure past the longest it holds (`most_ns`); refused, never capped, as past the range.
                    if !l.holds(fll, shift) {
                        break;
                    }
                    return Some(Plan { llp: self.llp, fll, coarse: c as u16, shift });
                }
                shift += 1;
            }
            shift = top;
        }
        // The line lengthened: the last resort, and only where asked for (`stretch`), as it lengthens the readout and the rolling shutter's skew with it (`readout_ps`).
        if !self.stretch {
            return None;
        }
        // The least line on the step whose `most` lines (in 2^shift) hold the exposure: llp >= ps x captured / (line_ps x most x 2^shift), rounded up to the step; in 128 bits, as ps x captured passes 64 for an exposure past about 48 minutes.
        let exact = (ps as u128 * self.captured_llp as u128).div_ceil(self.line_ps as u128 * ((most as u128) << shift));
        let llp = exact.div_ceil(LLP_STEP as u128) * LLP_STEP as u128;
        if llp > self.llp_most() as u128 {
            return None;
        }
        let c = (ps / self.line_ps_at(llp as u16) as u128) as u64 >> shift;
        let fll = self.frame(c, shift);
        // WHY the test: the stretched line keeps the frame's lines under the counter at `shift_most` by construction (`most` lines of 2^shift), but the mode's own frame as the floor (`frame`) is checked as well rather than assumed.
        if self.lte.is_some_and(|l| !l.holds(fll, shift)) {
            return None;
        }
        Some(Plan { llp: llp as u16, fll, coarse: c as u16, shift })
    }

    /// The most integration (in units of 2^`shift` lines) whose frame, the integration plus the margin, fits both the 16 bits of CCS's frame length and the sensor's frame counter (`Lte::frame_bits`) at `shift`; None when no integration does.
    fn coarse_most(&self, l: &Lte, shift: u8) -> Option<u64> {
        let regs = LINES_MOST;
        let counter = ((1u128 << l.frame_bits) - 1) >> shift;
        // WHY the shorter: two ceilings on the frame, the register's 16 bits and the frame counter's lines in units of the shift; a frame past either is one the sensor does not hold (the counter's: it runs the frame wrapped, the Pro, 2026-11-18).
        let frame = if counter < regs as u128 { counter as u64 } else { regs };
        let floor = (self.fll as u64).div_ceil(1 << shift);
        // WHY the test: a frame ceiling under the margin plus the least integration, or under the mode's own frame in these units, leaves no integration this shift can hold.
        if frame < self.margin as u64 + self.coarse_min as u64 || frame < floor {
            return None;
        }
        Some(frame - self.margin as u64)
    }

    /// The shift that holds the longest integration (`coarse_most` lines of 2^shift), up to the field's `most`: the least of the longest, so the planner's least shift never passes it.
    /// The main: 16 (65533 lines of 2^16, its frame 65535 << 16 under 2^32); a shift past it holds less, the counter halving the frame faster than the shift doubles it.
    pub fn shift_most(&self, l: &Lte) -> u8 {
        let mut best = (0u128, 0u8);
        for s in 0..=l.most {
            if let Some(c) = self.coarse_most(l, s) {
                if (c as u128) << s > best.0 {
                    best = ((c as u128) << s, s);
                }
            }
        }
        best.1
    }

    /// The exposure a plan gives (ns): its integration's lines, shifted, at its line's time.
    /// In 128 bits: at the main's widest shift (31) 65535 lines of 2^31 at 3.15 us is about 4.4 x 10^20 ps, past 64 bits; in ns it fits (14 years).
    pub fn ns(&self, p: &Plan) -> u64 {
        (((p.coarse as u128) << p.shift) * self.line_ps_at(p.llp) as u128 / 1000) as u64
    }

    /// A plan's frame period (ns), in 128 bits as `ns`.
    pub fn frame_ns(&self, p: &Plan) -> u64 {
        (((p.fll as u128) << p.shift) * self.line_ps_at(p.llp) as u128 / 1000) as u64
    }

    /// Whether a new frame of `new_ns`, begun at counter tick `now`, ends more than `margin_us` before the frame in flight (begun at tick `at`, `flight_ns` long) does, at `hz` ticks a second: the camera driver then cuts the frame in flight rather than let it finish (payloads/camera `exposure_cut`; docs/CAMERA.md "Serving while a frame waits").
    /// In 128 bits: a frame of the main's widest shift is 14 years of ns, and its product with the counter's 24.576 MHz passes 64 bits; the ticks themselves fit.
    pub fn cut_due(at: u64, flight_ns: u64, now: u64, new_ns: u64, hz: u64, margin_us: u64) -> bool {
        let end = at as u128 + flight_ns as u128 * hz as u128 / 1_000_000_000;
        let new_end = now as u128 + new_ns as u128 * hz as u128 / 1_000_000_000 + margin_us as u128 * hz as u128 / 1_000_000;
        end > new_end
    }

    /// Whole lines of the mode's line in `ns`, in 128 bits (any u64 of ns over a line of at least 1 ns fits back in 64 bits).
    fn lines(&self, ns: u64) -> u64 {
        (ns as u128 * 1000 / self.line_ps_at(self.llp) as u128) as u64
    }

    /// The longest exposure offered (ns): the most integration at the most shift (0 without one), at the mode's line, or at the longest line where stretching is allowed; with no margin read, the mode's own frame less `MARGIN`.
    pub fn most_ns(&self) -> u64 {
        if !self.long {
            return (self.fll as u64 - MARGIN as u64) * self.line_ps_at(self.llp) / 1000;
        }
        // With a shift, the longest integration the frame counter holds (`shift_most`), not the field's widest shift: past the counter the sensor runs the frame wrapped (the Pro, 2026-11-18).
        let most = match self.lte {
            Some(l) => {
                let s = self.shift_most(&l);
                // None only where no shift holds even the least integration (a counter narrower than the margin): then the longest offered is 0.
                (self.coarse_most(&l, s).unwrap_or(0) as u128) << s
            }
            None => (LINES_MOST - self.margin as u64) as u128,
        };
        let llp = if self.stretch { self.llp_most() as u16 } else { self.llp };
        (most * self.line_ps_at(llp) as u128 / 1000) as u64
    }

    /// A plan's readout (ps): `rows` lines at its line's time, from the first row read to the last, the rolling shutter's skew (the main binned: 3072 rows of 3.15 us, 9.67 ms, at every exposure the default route takes).
    pub fn readout_ps(&self, p: &Plan, rows: u64) -> u64 {
        rows * self.line_ps_at(p.llp)
    }
}

/// The video timing PLL's words (CCS pre_pll_clk_div, pll_multiplier): the pixel clock is the reference over the first times the second.
pub const PRE_PLL_CLK_DIV: u16 = 0x0304;
pub const PLL_MULTIPLIER: u16 = 0x0306;

/// Camera `c`'s line time (ps, at its captured line length `Cam::llp`) under the PLL the init in `p` leaves: the measured `Cam::line_ps` scaled by the clocks' ratio, so a line of the same clocks on a slower PLL takes longer.
/// The main's 1:1 init runs its PLL at 215 where the binned one runs 245 (both over 3 from 24.576 MHz), so its 11600-clock line is 6.586 us, its 6327-line frame 41.7 ms, 24 a second (derived from the init, docs/CAMERA.md "1:1 on the main"; the binned line's 3.1489 us was measured, b933, and is exactly 6320 clocks of 24.576 MHz / 3 x 245).
/// The camera's own line time where the init leaves the camera's own PLL, or either word unwritten.
pub fn line_ps_for(c: &Cam, p: &Page) -> u64 {
    let (pre, mult) = c.pll;
    match (p.get(PRE_PLL_CLK_DIV), p.get(PLL_MULTIPLIER)) {
        // WHY the zero tests: the words are the init's (vault data) and the camera's (0, 0) means its PLL was never read; a zero in either would divide by zero, and with no measured clock there is nothing to scale from.
        (Some(q), Some(m)) if pre != 0 && mult != 0 && m != 0 && (q as u16, m as u16) != c.pll => c.line_ps * mult as u64 * q as u64 / (pre as u64 * m as u64),
        _ => c.line_ps,
    }
}

impl Cam {
    /// The camera's timing as the list offers it, with no sensor powered: the captured init's line and frame, the margin and shift as measured (`ARRAYS`); the driver builds its own at the open from the sensor's own words.
    pub fn listed_timing(&self) -> Timing {
        self.listed_timing_at(self.line_ps, self.llp, (self.exposure_max_lines + MARGIN as u64) as u16)
    }

    /// A mode's timing as the list offers it: `m`'s own line and frame (the 1:1 init's, `Mode::captured_full`) at the line time its init's PLL gives (`line_ps_for`), the margin and shift as `listed_timing`'s.
    /// The main's shift was captured in its 1:1 mode (Android at 16 s, 2026-11-17), so the 1:1 offers it as the binned mode does.
    pub fn listed_mode_timing(&self, line_ps: u64, m: &Mode) -> Timing {
        self.listed_timing_at(line_ps, m.llp, m.fll)
    }

    pub fn listed_timing_at(&self, line_ps: u64, llp: u16, fll: u16) -> Timing {
        let a = &ARRAYS[self.id as usize];
        Timing {
            line_ps,
            captured_llp: self.llp,
            llp,
            fll,
            coarse_min: 1,
            margin: a.margin.unwrap_or(MARGIN),
            long: a.margin.is_some(),
            lte: a.lte,
            stretch: false,
        }
    }
}

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
    /// The side channels (`Side`) turned off at the sensor in the hold: phase detect and embedded data, for a mode whose window is not the init's.
    pub quiet: bool,
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
    /// The 1:1 init (`cam.<key>.init.full`) leaves an output of this many columns and rows that is not one pixel per array pixel over its window, or not the array's maximum resolution (`Array::full`): not a 1:1 mode.
    NotFull(u16, u16),
    /// A slit of no rows, an odd count, or one past the readout.
    Rows(u16),
    /// A window of no columns, one not on 2^4 columns (the RAW10 groups and the front end's line), or one past the readout.
    Cols(u16),
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
            quiet: false,
        })
    }

    /// The 1:1 mode a 1:1 init leaves the sensor in (`cam.<key>.init.full`, Android's own maximum-resolution init: the main's from the 16 s capture of 2026-11-17, docs/CAMERA.md "1:1 on the main"), `bin` 1.
    /// Refused unless its output is one pixel per array pixel across its whole window and the array's maximum resolution (`Array::full`): a binned init stored under the 1:1 name would otherwise be driven as 1:1 (a slit's cell, the list's size).
    pub fn captured_full(init: &[u8], a: &Array) -> Result<Mode, Error> {
        if a.full.0 == 0 {
            return Err(Error::NoFullMode);
        }
        let mut m = Mode::captured(init, a)?;
        // In u32 and added, never subtracted: the window's ends are the init's (vault data), and an end before its start would wrap a u16 difference.
        let whole = |out: u16, start: u16, end: u16| out as u32 * a.unit as u32 + start as u32 == end as u32 + 1;
        // A part that crops digitally (the Sony ultrawide: its 48 MP init reads the 9248 x 6016 window and crops 8000 x 6000 at 624, 8, Android's own, docs/CAMERA.md "The I2C side without a flash") is 1:1 when its binning is off and its crop, inside the window, is the output and the array's maximum resolution.
        let cropped = |out: u16, off: u16, size: u16, start: u16, end: u16| size == out && (off as u32 + size as u32) * a.unit as u32 + start as u32 <= end as u32 + 1;
        let full = if a.crop {
            m.binning[0] == 0 && cropped(m.x_out, m.crop[0], m.crop[2], m.x_start, m.x_end) && cropped(m.y_out, m.crop[1], m.crop[3], m.y_start, m.y_end)
        } else {
            whole(m.x_out, m.x_start, m.x_end) && whole(m.y_out, m.y_start, m.y_end)
        };
        if !full || (m.x_out, m.y_out) != a.full {
            return Err(Error::NotFull(m.x_out, m.y_out));
        }
        m.bin = 1;
        Ok(m)
    }

    /// Address units per output row (and column): the increment the init set, or the binning times the unit where the part bins in its binning registers alone (the Sony part).
    pub fn units_per_row(&self, a: &Array) -> u16 {
        if a.incs { self.inc[2] } else { self.bin * a.unit }
    }

    /// The 1:1 readout of the same array: every pixel, binning off, the increments one pixel, the output Android's maximum-resolution size (a guess on every sensor, built from the CCS registers alone; the vendor's setfile may carry mode timing this does not change: docs/CAMERA.md "The 1:1 capture").
    /// Measured on the Pro (2026-11-16): neither the main nor the ultrawide runs it whole (the main ERR_CRC_PH, the ultrawide its binned line width); the 1:1 init Android writes differs from the binned one in its PLL, line, binning (the main G#2222, not G#0011) and about 170 vendor words (docs/CAMERA.md "1:1 on the main"). The driver no longer writes it: its 1:1 kinds replay that init (`captured_full`). Kept for the host tests and the record.
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

    /// Address units per output column: the x increment the init set, or the binning times the unit (the Sony part).
    pub fn units_per_col(&self, a: &Array) -> u16 {
        if a.incs { self.inc[0] } else { self.bin * a.unit }
    }

    /// The window narrowed to `cols` output columns across the array's middle, started on the colour filter's period as a slit's rows are (`slit`), so the mosaic keeps its phase: fewer pixels a line on the link, for a shorter line (owner, 2026-11-17: "stupid fast", the link the ceiling, not the sensor).
    /// `cols` on 2^4 (whole RAW10 groups of four, and a line the front end takes in 20-byte steps), at most the mode's own width.
    pub fn cols(&self, a: &Array, cols: u16) -> Result<Mode, Error> {
        if cols == 0 || cols % (1 << 4) != 0 || cols > self.x_out {
            return Err(Error::Cols(cols));
        }
        let mut m = *self;
        let span = cols * self.units_per_col(a);
        let period = a.period * a.unit;
        let width = self.x_end + 1 - self.x_start;
        let off = ((width - span) / 2 + period / 2) / period * period;
        m.x_start = self.x_start + off;
        m.x_end = m.x_start + span - 1;
        m.x_out = cols;
        if a.crop {
            m.crop[0] = 0;
            m.crop[2] = cols;
        }
        Ok(m)
    }

    /// Phase detect and embedded data off at the sensor (`Side`), for a mode whose window is not the init's: both are laid out for the whole binned frame.
    pub fn quiet(&self) -> Mode {
        let mut m = *self;
        m.quiet = true;
        m
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
        if self.quiet {
            if let Some(s) = a.side {
                w.push16(s.emb, 0);
                if let Some(p) = s.pd {
                    w.push16(p, 0);
                }
            }
        }
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
        assert_eq!(m.llp, cams::MAIN.llp);
        // The CCS addresses span 16320 x 12288, two to a pixel of the 8160 x 6144 array.
        assert_eq!(((m.x_end + 1 - m.x_start) / a.unit, (m.y_end + 1 - m.y_start) / a.unit), a.full);
        let p = Page::of(&init);
        assert_eq!((p.get(0x0136), p.get(0x0304), p.get(0x0306), p.get(0x0300)), (Some(0x1800), Some(3), Some(245), Some(8)));
    }

    /// The main's 1:1 init (target/cam/main-full.init, local: tools/oracle-i2c `--init` from the 16 s capture of 2026-11-17, integrity 0): Android's 8160 x 6144 mode over the binned mode's own window, one pixel an output pixel, its PLL at 215 (the binned 245), its line 11600 clocks, its frame 6327 lines with 4570 of integration as its last hold before the stream on left them; so 6.586 us a line, 41.7 ms a frame, 24 a second.
    /// The binned init under the 1:1 name is refused, and a 1:1 slit keeps the quad cell (4 rows).
    #[test]
    fn the_main_full_mode() {
        let Some(full) = local("main-full.init") else { return };
        let (a, c) = (&ARRAYS[0], &cams::MAIN);
        let m = Mode::captured_full(&full, a).unwrap();
        println!("main 1:1: {m:?}");
        assert_eq!((m.x_start, m.x_end, m.y_start, m.y_end), (16, 0x3FCF, 16, 0x300F));
        assert_eq!((m.x_out, m.y_out, m.inc, m.bin), (8160, 6144, [2; 4], 1));
        assert_eq!((m.binning[0], m.binning[1]), (0x22, 0x22));
        assert_eq!((m.fll, m.llp, m.coarse, m.format), (0x18B7, 11600, 0x11DA, RAW10));
        let p = Page::of(&full);
        assert_eq!((p.get(PRE_PLL_CLK_DIV), p.get(PLL_MULTIPLIER), p.get(0x0300), p.get(ANALOGUE_GAIN)), (Some(3), Some(215), Some(8), Some(0x400)));
        let line = line_ps_for(c, &p);
        // The clock's own arithmetic: 6320 clocks at 2007.04 MHz is the measured 3.1489 us; 11600 at 1761.28 MHz.
        let exact = 11600u64 * 3 * 1_000_000_000_000 / (24_576_000 * 215);
        let t = c.listed_mode_timing(line, &m);
        assert!(t.line_ps_at(11600).abs_diff(exact) <= 2, "{} {exact}", t.line_ps_at(11600));
        let frame = t.frame_ns(&t.own(m.coarse));
        assert!((41_600_000..41_750_000).contains(&frame), "{frame}");
        println!("main 1:1: a line {} ps, a frame {} ns ({} a second), readout {} us", t.line_ps_at(11600), frame, 1_000_000_000 / frame, t.readout_ps(&t.own(1), 6144) / 1_000_000);
        // The binned init's own clock leaves the measured line alone.
        if let Some(binned) = local("cam.frame") {
            assert_eq!(line_ps_for(c, &Page::of(&binned)), c.line_ps);
            assert_eq!(Mode::captured_full(&binned, a), Err(Error::NotFull(4080, 3072)));
        }
        // Long exposures as the binned mode's: Android's shift ran in this very mode (16 s at shift 6).
        let p16 = t.plan(16_000_000_000).unwrap();
        // Android at 16 s in this mode wrote integration G#9446 at shift 6 (the capture's third hold): the plan agrees to a line.
        assert_eq!((p16.llp, p16.shift), (11600, 6));
        assert!((p16.coarse as i32 - 0x9446).abs() <= 1, "{:X}", p16.coarse);
        // The longest, the frame counter's (65533 lines of 2^16 at the 1:1 line): about 7.86 hours, twice the binned mode's 3.76 for its longer line.
        assert!((28_285_000_000_000..28_287_000_000_000).contains(&t.most_ns()), "{}", t.most_ns());
        // A 1:1 slit: whole quad cells.
        assert_eq!(m.slit(a, 2, 128, 16), Err(Error::Rows(2)));
        let s = m.slit(a, 64, 64 + (1 << 7), 16).unwrap();
        assert_eq!((s.y_end + 1 - s.y_start, (s.y_start - m.y_start) % 8, s.bin), (128, 0, 1));
        // The mode as the driver writes it in the hold: the init's own registers, phase detect and embedded data off.
        let ws = m.quiet().writes(a, c);
        let w: Vec<&[u8]> = ws.iter().collect();
        assert!(w.contains(&&[0x09u8, 0x00, 0x22, 0x22][..]) && w.contains(&&[0x03u8, 0x42, 0x2D, 0x50][..]) && w.contains(&&[0x07u8, 0x20, 0, 0][..]));
    }

    #[test]
    fn the_ultrawide_and_tele_modes() {
        if let Some(init) = local("uw.init") {
            let m = Mode::captured(&init, &ARRAYS[1]).unwrap();
            println!("ultrawide: {m:?}");
            assert_eq!((m.x_out, m.y_out, m.binning, m.crop), (4000, 3000, [1, 0x22, 8], [312, 4, 4000, 3000]));
            assert_eq!((m.fll, m.llp, m.coarse), (5081, 9568, 4124));
            assert_eq!(m.llp, cams::ULTRAWIDE.llp);
            // The binned init is no 1:1 mode.
            assert_eq!(Mode::captured_full(&init, &ARRAYS[1]), Err(Error::NotFull(4000, 3000)));
        }
        // Android's 48 MP init (the 2026-11-18 i2c trace, oracle-i2c --ftrace, local): the whole window, binning off, the crop the output.
        if let Some(init) = local("uw-full.init") {
            let m = Mode::captured_full(&init, &ARRAYS[1]).unwrap();
            println!("ultrawide 48 MP: {m:?}");
            assert_eq!((m.x_out, m.y_out, m.binning, m.crop, m.bin), (8000, 6000, [0, 0x11, 0x0A], [624, 8, 8000, 6000], 1));
            assert_eq!(m.llp, 10320);
        }
        if let Some(init) = local("tele.init") {
            let m = Mode::captured(&init, &ARRAYS[3]).unwrap();
            println!("tele: {m:?}");
            assert_eq!((m.x_out, m.y_out, m.inc, m.binning[0], m.binning[1]), (4032, 3024, [2; 4], 0x22, 0x22));
            assert_eq!((m.x_end + 1 - m.x_start, m.fll, m.llp), (8064, 5457, 5568));
            assert_eq!(m.llp, cams::TELE.llp);
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

    /// A quiet mode turns phase detect and embedded data off inside the hold, after the mode's registers and before the release; a mode left loud writes neither. The main's PD block and embedded word are read off its init as the side channels say (`Side`).
    #[test]
    fn side_channels_off_in_the_hold() {
        let Some(init) = local("cam.frame") else { return };
        let (a, c) = (&ARRAYS[0], &cams::MAIN);
        let m = Mode::captured(&init, a).unwrap();
        assert!(!m.quiet);
        let loud: Vec<Vec<u8>> = m.slit(a, 4, 132, 16).unwrap().writes(a, c).iter().map(|w| w.to_vec()).collect();
        assert!(!loud.iter().any(|w| w[..2] == [0x07, 0x20] || w[..2] == [0x01, 0x18]));
        let s = m.slit(a, 4, 132, 16).unwrap().quiet();
        let ws = s.writes(a, c);
        let w: Vec<&[u8]> = ws.iter().collect();
        let n = w.len();
        assert_eq!(w[n - 3], &[0x01, 0x18, 0, 0]);
        assert_eq!(w[n - 2], &[0x07, 0x20, 0, 0]);
        assert_eq!(w[n - 1], c.release);
        assert!(n <= WRITES_MOST);
        // The init's own: embedded data on with 4 lines (the link's channel 2: G#40FF0), phase detect on with its 768-row block (the link's channel 1: G#3000FF0).
        let mut emb = None;
        let mut pd = None;
        for rec in crate::inputs::records(&init) {
            let Ok(crate::inputs::Rec::Write(w)) = rec else { continue };
            match w {
                [0x01, 0x18, d @ ..] => emb = Some(d.to_vec()),
                [0x07, 0x2A, d @ ..] => pd = Some(d.to_vec()),
                _ => {}
            }
        }
        assert_eq!(emb.as_deref(), Some(&[0x01, 0x04][..]));
        assert_eq!(pd.as_deref(), Some(&[0x03, 0x00][..]));
    }

    /// An x window lies on the period, centred, `cols` wide; a width off 2^4 or past the readout is refused.
    #[test]
    fn columns_centre_on_the_period() {
        let Some(init) = local("cam.frame") else { return };
        let a = &ARRAYS[0];
        let m = Mode::captured(&init, a).unwrap();
        for cols in [16u16, 256, 1280, 2048, 4080] {
            let s = m.cols(a, cols).unwrap();
            assert_eq!(s.x_end + 1 - s.x_start, cols * 4);
            assert_eq!((s.x_start - m.x_start) % 8, 0);
            let centre = (s.x_start as i32 + s.x_end as i32 + 1) / 2;
            let middle = (m.x_start as i32 + m.x_end as i32 + 1) / 2;
            assert!((centre - middle).abs() <= 4, "cols {cols}");
            assert_eq!((s.x_out, s.y_out), (cols, m.y_out));
        }
        assert_eq!(m.cols(a, 1288), Err(Error::Cols(1288)));
        assert_eq!(m.cols(a, 4096), Err(Error::Cols(4096)));
    }

    /// The main's line time scales the frame: the captured mode's 21362 lines at 3.1489 us are its measured 67.3 ms; a 132-line slit is about 426 us of lines (the sensor kept a floor of about 825 us on 2026-10-21).
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

    /// The main's timing at its init's mode: llp 6320, fll 21362, the margin and least integration it answered (2 and 2).
    fn main_timing() -> Timing {
        Timing { line_ps: crate::MAIN_LINE_PS, captured_llp: 6320, llp: 6320, fll: 0x5372, coarse_min: 2, margin: 2, long: true, lte: None, stretch: true }
    }

    /// Long exposures, step by step: inside the init's frame nothing but the integration moves; past it the frame grows to 16 bits (about 0.21 s); past that, with no shift, refused unless the stretch is asked for (`stretch`, set here: the line grows on its step to the front end's longest line, about 1.0 s; 2 s stopped the stream on b932). Every plan keeps the integration inside the frame by the sensor's margin, and gives back the exposure asked to within a line.
    #[test]
    fn long_exposures_plan() {
        let t = main_timing();
        // Within the frame: the mode's own frame and line.
        let p = t.plan(17_200_000).unwrap();
        assert_eq!((p.llp, p.fll, p.shift), (6320, 0x5372, 0));
        assert_eq!(p.coarse as u64, 17_200_000_000 / crate::MAIN_LINE_PS);
        // The init's own integration (21342 lines) fits its frame with the sensor's margin.
        assert_eq!(t.plan(0x535E * crate::MAIN_LINE_PS / 1000 + 1).unwrap().fll, 0x5372);
        // 100 ms: the frame lengthened, the line not.
        let p = t.plan(100_000_000).unwrap();
        assert_eq!((p.llp, p.fll as u64), (6320, p.coarse as u64 + 2));
        // The most 16 bits of frame give at the init's line: about 0.206 s.
        let edge = (LINES_MOST - 2) * crate::MAIN_LINE_PS / 1000;
        assert_eq!(t.plan(edge).unwrap().llp, 6320);
        assert!((205_000_000..208_000_000).contains(&edge), "{edge}");
        // The main's longest line is the one the front end took on the Pro (30640 clocks, the 1 s step of b932).
        assert_eq!(t.llp_most(), 30640);
        // 0.5 s and 1 s: the line lengthened on its step, the frame at most 16 bits.
        for ns in [208_000_000u64, 500_000_000, 1_000_000_000] {
            let p = t.plan(ns).unwrap();
            assert!(p.llp > 6320 && p.llp as u64 % LLP_STEP == 0 && p.llp as u64 <= t.llp_most(), "{ns}: {p:?}");
            assert!(p.coarse as u64 + 2 <= p.fll as u64 && p.fll as u64 <= LINES_MOST, "{ns}: {p:?}");
            // The exposure given is the one asked, short of it by less than a line.
            let got = t.ns(&p);
            assert!(got <= ns && ns - got < t.line_ps_at(p.llp) / 1000 + 1, "{ns}: {got}");
        }
        // A second: the longest line, 65503 lines of 15.27 us.
        let p = t.plan(1_000_000_000).unwrap();
        assert_eq!((p.llp, p.fll, p.coarse), (30640, 65505, 65503));
        // The most offered, about 1.000 s, is taken; one more line is not; 2 s (where the stream stopped, at 61280 clocks) is refused.
        let most = t.most_ns();
        assert!((1_000_000_000..1_001_000_000).contains(&most), "{most}");
        assert!(t.plan(most).is_some());
        assert_eq!(t.plan(most + t.line_ps_at(t.llp_most() as u16) / 1000 + 1), None);
        assert_eq!(t.plan(2_048_000_000), None);
        // 16 s takes the vendor's shift, which no camera has until the capture.
        assert_eq!(t.plan(16_000_000_000), None);
        // Under the sensor's least integration (2 lines), and a value that would wrap: refused.
        assert_eq!(t.plan(crate::MAIN_LINE_PS / 1000), None);
        assert_eq!(t.plan(u64::MAX), None);
        // The frame periods: the init's 67.3 ms; at the most, 65535 lines of 15.27 us.
        assert_eq!(t.frame_ns(&t.own(16)), 0x5372 * crate::MAIN_LINE_PS / 1000);
        let p = t.plan(most).unwrap();
        assert!((1_000_000_000..1_001_000_000).contains(&t.frame_ns(&p)), "{}", t.frame_ns(&p));
    }

    /// With a shift (an example: the shift in the high byte, to 2^7, as the capture may show), 16 s is the mode's own line with the frame and integration counted in 2^shift lines, the least shift that fits.
    #[test]
    fn long_exposures_with_a_shift() {
        let l = Lte { fll_reg: 0x0702, coarse_reg: 0x0704, at: 8, most: 7, frame_bits: 32, byte: false };
        let t = Timing { lte: Some(l), ..main_timing() };
        let p = t.plan(16_000_000_000).unwrap();
        assert_eq!((p.llp, p.shift), (6320, 7));
        assert!(p.coarse as u64 + 2 <= p.fll as u64);
        let got = t.ns(&p);
        assert!(got <= 16_000_000_000 && 16_000_000_000 - got < (crate::MAIN_LINE_PS << 7) / 1000 + 1, "{got}");
        assert_eq!(l.word(7), 0x0700);
        // A second takes shift 3: 310200 lines over 8 is under 16 bits, over 4 is not.
        assert_eq!(t.plan(1_000_000_000).unwrap().shift, 3);
        // The most: 65533 lines of 2^7 at the longest line (15.27 us), about 128 s.
        assert!((128_000_000_000..128_100_000_000).contains(&t.most_ns()), "{}", t.most_ns());
    }

    /// The main's own shift on the default route (the 16 s capture, 2026-11-17: high byte; 7 and 8 run on ferros, b933; owner: the line never stretched by default): every exposure past the 16-bit frame is the least shift at the mode's 6320-clock line, so the readout stays 3072 lines of 3.15 us (9.67 ms); 6 reaches 13.2 s, 8 52.8 s; the stretch, asked for explicitly, carries 16 s at shift 6 on a longer line and says its longer readout.
    #[test]
    fn the_mains_shift() {
        let t = cams::MAIN.listed_timing();
        let t = Timing { fll: 0x5372, coarse_min: 2, ..t };
        assert!(!t.stretch && t.lte == ARRAYS[0].lte);
        assert_eq!(ARRAYS[0].lte.unwrap().word(6), 0x0600);
        for (ns, shift) in [(250_000_000u64, 1u8), (1_000_000_000, 3), (4_000_000_000, 5), (13_000_000_000, 6)] {
            let p = t.plan(ns).unwrap();
            assert_eq!((p.llp, p.shift), (6320, shift), "{ns}");
            assert!(p.coarse as u64 + 2 <= p.fll as u64 && p.fll as u64 <= LINES_MOST, "{p:?}");
            let got = t.ns(&p);
            assert!(got <= ns && ns - got < (t.line_ps_at(p.llp) << shift) / 1000 + 1, "{ns}: {got}");
            // The readout is the mode's at every exposure: 3072 rows of the 6320-clock line.
            assert!((9_600_000_000..9_700_000_000).contains(&t.readout_ps(&p, 3072)), "{}", t.readout_ps(&p, 3072));
        }
        // 16 s at shift 7 and 32 s at shift 8, both at the mode's line (b933), 105 s at 9; the most, 65533 lines of 2^16 (the frame counter's 32 bits, the Pro 2026-11-18), 13524 s, about 3.76 hours; its frame period in ns; past it refused.
        for (ns, shift) in [(16_000_000_000u64, 7u8), (32_000_000_000, 8), (105_000_000_000, 9)] {
            assert_eq!(t.plan(ns).map(|p| (p.llp, p.shift)), Some((6320, shift)), "{ns}");
        }
        assert_eq!(t.most_ns(), (65533u64 << 16) * crate::MAIN_LINE_PS / 1000);
        assert!((13_523_000_000_000..13_525_000_000_000).contains(&t.most_ns()), "{}", t.most_ns());
        // The most in whole ns lies a fraction of a line under 65533 x 2^16 lines, so it plans 65532 of 2^16.
        let p = t.plan(t.most_ns()).unwrap();
        assert_eq!((p.shift, p.coarse, p.fll), (16, 65532, 65534));
        assert_eq!(t.frame_ns(&p), (65534u64 << 16) * crate::MAIN_LINE_PS / 1000);
        // 65534 lines of 2^16 (the least past the most, whose frame at the least shift that holds it, 32769 << 17, passes 2^32) is refused.
        assert_eq!(t.plan(((65534u64 << 16) * crate::MAIN_LINE_PS).div_ceil(1000)), None);
        // Held to 6 (Android's most): 16 s refused at the mode's line, and 13.2 s the most.
        let l6 = Lte { most: 6, ..ARRAYS[0].lte.unwrap() };
        let t6 = Timing { lte: Some(l6), ..t };
        assert_eq!(t6.plan(16_000_000_000), None);
        assert!((13_200_000_000..13_220_000_000).contains(&t6.most_ns()), "{}", t6.most_ns());
        // The stretch, only when asked: 16 s at shift 6 on a longer line, its readout longer by as much.
        let ts = Timing { stretch: true, ..t6 };
        let p = ts.plan(16_000_000_000).unwrap();
        assert_eq!(p.shift, 6);
        assert!(p.llp > 6320 && (p.llp as u64) < 8000 && p.llp as u64 % LLP_STEP == 0, "{p:?}");
        assert!(ts.readout_ps(&p, 3072) > 11_000_000_000, "{}", ts.readout_ps(&p, 3072));
    }

    /// The cut (docs/CAMERA.md "Serving while a frame waits"): the owner's shutter dragged to the end (a 3.76-hour frame since the counter's ceiling; a 14-year one was planned before) and back to 1 s cuts the frame in flight, at any point of it, with the counter's 24.576 MHz (no 64-bit wrap reads it as short, tested at the 14-year frame the arithmetic must still take); "a 64 s exposure, 5 s in I want 12 s" cuts (the new frame would end 47 s sooner); a new frame ending under the margin (2^20 us) sooner, or later, does not; a frame begun just now at 64 s and asked for 63.5 s does not.
    #[test]
    fn the_cut() {
        let t = cams::MAIN.listed_timing();
        let t = Timing { fll: 0x5372, coarse_min: 2, ..t };
        let hz = 24_576_000u64;
        let margin = 1u64 << 20;
        let s = |x: u64| x * hz;
        let hours = t.frame_ns(&t.plan(t.most_ns()).unwrap());
        // The 14-year frame of shift 31 (the field's width, before the counter was measured): forced, the device test's way.
        let years = t.frame_ns(&t.plan_shift(443_150_242_145_049_247, 31).unwrap());
        let one = t.frame_ns(&t.plan(1_000_000_000).unwrap());
        let at = 1u64 << 40;
        // A second into the 3.76-hour frame, and 3 hours into it; a second and 13 years into the 14-year one.
        assert!(Timing::cut_due(at, hours, at + s(1), one, hz, margin));
        assert!(Timing::cut_due(at, hours, at + s(3 * 3600), one, hz, margin));
        assert!(Timing::cut_due(at, years, at + s(1), one, hz, margin));
        assert!(Timing::cut_due(at, years, at + s(13 * 365 * 86400), one, hz, margin));
        // 64 s, 5 s in, now 12 s: about 47 s sooner.
        let f64s = t.frame_ns(&t.plan(64_000_000_000).unwrap());
        let f12 = t.frame_ns(&t.plan(12_000_000_000).unwrap());
        assert!(Timing::cut_due(at, f64s, at + s(5), f12, hz, margin));
        // 59 s in: the frame in flight ends in about 5 s, before a 12 s one would; let it finish.
        assert!(!Timing::cut_due(at, f64s, at + s(59), f12, hz, margin));
        // Inside the margin: 64 s just begun, 63.5 s asked.
        let f63 = t.frame_ns(&t.plan(63_500_000_000).unwrap());
        assert!(f63 < f64s && !Timing::cut_due(at, f64s, at, f63, hz, margin));
        // Longer never cuts.
        assert!(!Timing::cut_due(at, one, at, years, hz, margin));
    }


    /// The sensor's frame counter (the Pro, 2026-11-18, docs/CAMERA.md "The shift's ceiling"): frames of 32770 lines at shifts 14 to 16 held, at 17 to 20 ran wrapped modulo 2^32 lines; exactly 2^32 ran at the floor, 2^32 - 2^16 and 2^32 - 2^17 held; the same in lines at 1:1.
    /// So the plan stops at shift 16 (65533 lines of 2^16): 3.76 hours binned, 7.86 hours at 1:1; every held exposure plans, every wrapped one is refused; a forced shift (the device test's) still reaches past the counter, as the measurement needs.
    #[test]
    fn the_frame_counter() {
        let l = ARRAYS[0].lte.unwrap();
        assert_eq!((l.most, l.frame_bits), (31, 32));
        // What ran: held, and wrapped.
        for s in 14..=16 {
            assert!(l.holds(32770, s), "{s}");
        }
        for s in 17..=31 {
            assert!(!l.holds(32770, s), "{s}");
        }
        assert!(l.holds(65535, 16) && l.holds(32767, 17) && l.holds(5, 29));
        assert!(!l.holds(32768, 17) && !l.holds(4, 30) && !l.holds(4, 31) && !l.holds(5, 30));
        let t = Timing { fll: 0x5372, coarse_min: 2, ..cams::MAIN.listed_timing() };
        assert_eq!(t.shift_most(&l), 16);
        // The run's exposures, 32768 lines of 2^s and half a unit more as camtest asks them: shifts 14 to 16 plan at that shift, 17 and past are refused.
        let at = |s: u8| (((32768u128 << s) + (1u128 << s >> 1)) * crate::MAIN_LINE_PS as u128 / 1000) as u64;
        for s in 14..=16 {
            let p = t.plan(at(s)).unwrap();
            assert_eq!((p.shift, p.coarse, p.fll, p.llp), (s, 32768, 32770, 6320), "{s}");
        }
        for s in 17..=31 {
            assert_eq!(t.plan(at(s)), None, "{s}");
        }
        // Three hours plans at shift 16; four do not.
        assert_eq!(t.plan(3 * 3600 * 1_000_000_000).map(|p| p.shift), Some(16));
        assert_eq!(t.plan(4 * 3600 * 1_000_000_000), None);
        // The 1:1 readout: the same lines at its 6.586 us line (the 1:1 init's PLL, 215 for 245), its own frame of 6327 lines at 11600 clocks: 28286 s.
        let full = Timing { line_ps: crate::MAIN_LINE_PS * 245 / 215, llp: 11600, fll: 6327, ..t };
        assert_eq!(full.line_ps_at(11600), 6_586_118);
        assert_eq!(full.shift_most(&l), 16);
        assert!((28_285_000_000_000..28_287_000_000_000).contains(&full.most_ns()), "{}", full.most_ns());
        let p = full.plan(full.most_ns()).unwrap();
        assert!(p.shift == 16 && l.holds(p.fll, p.shift), "{p:?}");
        assert_eq!(full.plan(((65534u64 << 16) * 6_586_118).div_ceil(1000)), None);
        // The device test's forced shift is not held to the counter: it is how the wrap was found.
        assert_eq!(t.plan_shift(at(17), 17).map(|p| (p.coarse, p.fll)), Some((32768, 32770)));
        // A stretched line with the shift stays inside the counter too.
        let ts = Timing { stretch: true, ..t };
        let p = ts.plan(ts.most_ns()).unwrap();
        assert!(p.llp > 6320 && l.holds(p.fll, p.shift), "{p:?}");
        assert_eq!(ts.plan(((65534u64 << 16) * ts.line_ps_at(ts.llp_most() as u16)).div_ceil(1000)), None);
    }

    /// The forced shift of the reach test: 16 lines of integration at shift 12 on the main is a frame of 18 (the mode's 21362 lines are 6 at that shift), at the mode's line; under the least (one line at shift 16) refused, and past the field.
    #[test]
    fn a_forced_shift() {
        let t = Timing { fll: 0x5372, coarse_min: 2, ..cams::MAIN.listed_timing() };
        let ns = (16u64 << 12) * t.line_ps_at(6320) / 1000 + 1;
        assert_eq!(t.plan_shift(ns, 12), Some(Plan { llp: 6320, fll: 18, coarse: 16, shift: 12 }));
        assert_eq!(t.plan_shift(ns, 16), None);
        assert_eq!(t.plan_shift(ns, 200), None);
        // At shift 9 the mode's frame is the floor: 21362 lines are 42 of 2^9.
        let p = t.plan_shift((16u64 << 9) * t.line_ps_at(6320) / 1000 + 1, 9).unwrap();
        assert_eq!((p.fll, p.coarse), (42, 16));
    }
    /// A camera whose margin was never read keeps to its frame: the list offers what it did before (the init's integration), and SET refuses past the frame less `MARGIN`.
    #[test]
    fn short_cameras_keep_their_frame() {
        // The tele, since its capture and run (2026-11-18 night): the main's words, its margin 4, its frame held to 2^32 lines: 65531 lines of 2^16 at its 6.11 us line, about 7.3 hours.
        let f = cams::TELE.listed_timing();
        assert!(f.long);
        assert_eq!(f.most_ns(), (65535 - 4) * (1 << 16) * cams::TELE.line_ps / 1000);
        // The front, since its margin and shift (2026-11-18): 65535 - 5 lines of 2^1 at its 10.44 us line, about 1.37 s, past Android's 1 s.
        let f = cams::FRONT.listed_timing();
        assert!(f.long);
        assert_eq!(f.most_ns(), (65535 - 5) * 2 * cams::FRONT.line_ps / 1000);
        let m = cams::MAIN.listed_timing();
        assert!(m.long && m.most_ns() > 13_000_000_000);
        let u = cams::ULTRAWIDE.listed_timing();
        // The ultrawide, its shift (Sony's one byte G#3100, 6 the most) and no stretch by default: (65535 - 64) lines of 2^6 at its own 6.56 us line, 27.49 s; stretched (asked for) the line to 22256 clocks, longer still.
        assert!((27_400_000_000..27_600_000_000).contains(&u.most_ns()), "{}", u.most_ns());
        let us = Timing { stretch: true, ..u };
        assert_eq!(us.llp_most(), 22256);
        assert!(us.most_ns() > u.most_ns(), "{}", us.most_ns());
        assert_eq!((ARRAYS[1].lte.unwrap().word(6), ARRAYS[1].lte.unwrap().byte), (6, true));
        let t = Timing { long: false, ..main_timing() };
        assert!(t.plan(t.most_ns()).is_some());
        assert_eq!(t.plan(t.most_ns() + 2 * crate::MAIN_LINE_PS / 1000), None);
    }

    #[test]
    fn frame_times() {
        let Some(init) = local("cam.frame") else { return };
        let a = &ARRAYS[0];
        let m = Mode::captured(&init, a).unwrap();
        let ps = frame_ps(&m, m.llp, crate::MAIN_LINE_PS);
        assert!((67_000_000_000..67_500_000_000).contains(&ps), "{ps}");
        let s = m.slit(a, 4, 132, 16).unwrap();
        println!("slit frame {} ns", frame_ps(&s, m.llp, crate::MAIN_LINE_PS) / 1000);
    }
}
