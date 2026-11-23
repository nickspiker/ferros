//! The Pro's four cameras as the driver drives them (docs/CAMERA.md "The other cameras"): each one's sensor bus and address, identity, stream and hold writes, CSI receiver, the front end's per-frame acknowledgement, readout geometry, its job's region and the facts its list section carries.
//! The kernel's `camera` switch takes the camera's id as its module (camera_power.rs), so the id here is also the module the switch powers.
//! What is measured and what is a guess is said per field; the guesses are for the owner's run to settle (docs/CAMERA.md "Device test plan").

use crate::job::{self, SLOTS};

/// One camera.
pub struct Cam {
    /// The endpoint's id (OPEN `[1][id]`) and the `camera` switch's module.
    pub id: u8,
    /// Its name in the list ("main", "ultrawide", "front", "tele").
    pub name: &'static str,
    /// The module's name in the device tree, and the vault's names for its inputs: `cam.<key>.init` (the sensor's init), `cam.<key>.prog` (the CPU's setup writes, a 'CPR2'), `cam.<key>.pdma` (the front end's job, a 'CPR3').
    pub key: &'static str,
    /// 0 away from the person, 1 toward.
    pub facing: u8,
    /// Its sensor's HS-I2C controller and 7-bit address (zuma-husky-camera.dtsi).
    pub bus: usize,
    pub addr: u8,
    /// The register its identity is read at and what it answers (two bytes).
    pub id_reg: [u8; 2],
    pub id_val: [u8; 2],
    /// Stream on and off, and the grouped-parameter hold and its release, as its Linux driver writes them (16-bit values on the Samsung parts, 8-bit on the Sony).
    pub stream_on: &'static [u8],
    pub stream_off: &'static [u8],
    pub hold: &'static [u8],
    pub release: &'static [u8],
    /// Its lens actuator's address on the same bus (0: none to drive), and the protocol it takes (`Lens`).
    pub actuator: u8,
    pub lens: Lens,
    /// The focus targets offered, the actuator's whole travel (0 and 0: focus not offered).
    pub focus: (u64, u64),
    /// The lens's calibration: the actuator's target at infinity and at the module's near point, and that point's distance in mm (all 0: not known; the distance alone 0: not known).
    /// The direction follows from which target is larger; the travel past infinity is the span from the infinity target to the travel's end on the far side.
    pub focus_cal: FocusCal,
    /// Whether its receiver and job are known, so it can stream (the tele's are not captured).
    pub streams: bool,
    pub rx_start: &'static [(usize, u32)],
    pub rx_stop: &'static [(usize, u32)],
    /// The front end's per-frame acknowledgement for this camera's stream.
    pub ack: &'static [usize],
    /// How often the frame thread answers the front end on its own tick when its interrupt line stays quiet (µs): under the sensor's frame period, or the front end stops for good (the main at 50 ms held at 14.5 frames a second; the ultrawide at 50 ms took one frame and stopped, at 15 ms it ran, 2026-10-22).
    pub tick_us: u64,
    /// The readout: width and height in samples, a line's bytes (RAW10 packed).
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    /// Its job's region: each October slot's bytes and the granule offset it starts at (ferros_cam job).
    pub sizes: [u64; SLOTS],
    pub skip: [u64; SLOTS],
    /// A line's time in picoseconds (0: not known, exposure not offered), the init's integration and the longest the endpoint offers, in lines.
    pub line_ps: u64,
    /// The captured init's line length in pixel clocks (CCS line_length_pck), the line `line_ps` was measured at: a longer line takes proportionally longer (ferros_cam ccs `Timing`).
    pub llp: u16,
    /// The captured init's video timing PLL, (pre_pll_clk_div G#0304, pll_multiplier G#0306), the clock `line_ps` was measured at: an init on another PLL (the main's 1:1, multiplier 215 against the binned 245) runs every line that much longer or shorter (ccs `line_ps_for`). (0, 0) where never read.
    pub pll: (u16, u16),
    /// The captured init's frame length in lines (CCS frame_length_lines): its frame period at `line_ps`, the list's `frame_ns` (the main's 21362 lines, 67.27 ms, measured at 67.3 ms apart; the ultrawide's 5081, 33.3 ms, its 30 a second seen; the front's and the tele's read from their inits, never streamed).
    pub fll: u16,
    pub exposure_lines: u64,
    pub exposure_max_lines: u64,
    /// The 2 x 2 cell's channels, two bits each, top left first (0 R, 1 G, 2 B); black and white; quarter turns clockwise to stand upright.
    pub cfa: u8,
    pub black: u16,
    pub white: u16,
    pub rotation: u8,
    /// Its other readouts, each replayed whole as Android ran it (`Alt`), opened by the mode block's kind `ALT + k`.
    pub alts: &'static [Alt],
    /// The least blanking a whole frame of its captured readout takes, lines past its rows (the list's `fll_least`, a client's fastest whole rate): the driver's `BLANK_LEAST`, or more where the sensor pads its frame further (the tele).
    pub blank_least: u16,
    /// A binned slit's least rows that reach memory, and its least frame (ns): measured on the Pro (2026-11-22, b1134, docs/CAMERA.md "Two-row slits, 2:1 and 1:1"), 0 where not measured.
    /// Every sensor sends 2-row frames, but the receive side takes only these heights and up: the main 16 rows (12 and under, the link never takes a frame end, with ERR_CRC_PH, also with the front end left at the whole frame), the ultrawide 4 (2: the link's VRESOL_MISMATCH on the image channel, its PD channel silent), the front 2 (since its PD channel is a quarter of the rows rounded up, geom `Geom::of`), the tele 2 (since its phase detect is off in a slit, ccs `ARRAYS` 3).
    /// The least frame at those rows: the main about 812 us whatever the frame or the line asked (1231 a second; a time, not a line count: a 5888-clock line ran 1244, not the 1321 a line count would give); the ultrawide 104 lines of 6.5616 us (1465 a second at 4 rows; a whole frame, not a blanking: 64 rows ran 120 lines, 1269 a second); the front about 121 lines of 10.4378 us (791 a second at 2 and 4 rows, any frame asked under it); the tele 231 lines of 6.1068 us at 2 rows (713 a second drained), its frame the rows plus about 229 lines (16 rows 670 a second, 64 rows 559).
    pub slit_rows: u16,
    pub slit_frame_ns: u64,
    /// Whether the sensor takes a mode's words only while streaming: written in the hold before stream on, its frame, window and output size are dropped at stream on (the sensor runs its init's mode), so the driver writes the mode after the first frame (payloads/camera `F_MID`).
    /// The tele (2026-11-18, docs/CAMERA.md "Slits on every camera"): frames of 4000 and 8000 lines written before stream on both ran 30 a second, and a 64-row slit sent whole 4032 x 3024 frames (a front end left at the whole frame took them clean); written after the first frame, 4000 lines ran 40 a second, 8000 lines 20, and the slit 559 a second.
    pub mode_live: bool,
    /// Its sensor's CCS facts (ferros_cam ccs `Array`: gain, margin, long-exposure shift), by board: the Pro's `ARRAYS`, the 8a's `AKITA_ARRAYS`.
    pub array: &'static crate::ccs::Array,
    /// Its lens and sensor size (ferros_cam optics `Optics`: the list's sizes, focal length, f-number and each readout's field), by board: the Pro's `OPTICS`, the 8a's `AKITA_OPTICS`.
    pub optics: &'static crate::optics::Optics,
}

/// A lens's focus calibration (`Cam::focus_cal`): the actuator's targets at infinity and at the near point, and the near point's distance in mm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FocusCal {
    pub infinity: u64,
    pub near: u64,
    pub near_mm: u64,
}

/// No calibration known.
pub const FOCUS_UNKNOWN: FocusCal = FocusCal { infinity: 0, near: 0, near_mm: 0 };

/// A lens actuator's protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lens {
    /// No lens to drive.
    None,
    /// The Pro's cornerfolk actuators (the main's, the ultrawide's, the front's: 8-bit registers): reset 02 <- G#40, 5 ms, 02 <- 0, F1 <- 0; the target big-endian in 00/01; the position at 84 (docs/oracle/CAMERA.md "Focus").
    Cornerfolk,
    /// The tele's humbaba (its OIS controller, G#61: 16-bit register addresses, 8-bit values), decoded from Android focusing it (2026-11-22, docs/CAMERA.md "The tele's focus from Android"): G#0201 reads 1 when ready, then G#0200 <- 1, G#0B00 <- 1, G#0200 <- 0, G#0200 <- 1; the target little-endian in G#0204 (low) and G#0205 (high), each its own write; the position in G#0208 (low) and G#0209 (high).
    Humbaba,
}

/// The Pro's nearest focus of each lens, from its HAL (Camera2 LENS_INFO_MINIMUM_FOCUS_DISTANCE, `dumpsys media.camera` on the Pro, 2026-11-22, its calibration APPROXIMATE): the main 9.5238 dioptres, the ultrawide 50, the front 10, the tele 1.25.
/// The tele's capture put its factory near code at exactly that distance (Android drove its slider's near end, 1.25 dioptres, to the factory's code less the same 382 as its infinity); the cornerfolk lenses' near codes are taken at theirs the same way, unmeasured.
pub const NEAR_MM: [u64; 4] = [105, 20, 100, 800];

/// The Pro's three cornerfolk actuators (the main's G#0C, the ultrawide's G#0F, the front's G#0C) take a 12-bit code left-aligned in their 16-bit target (00/01): the target is the code times 2^4.
/// Measured two ways: the main's sharpest over a floor about a metre away was G#3800 (docs/oracle/CAMERA.md), code 896, a seventh of the way from its infinity code 509 to its near code 3274 (about 1.4 dioptres if the near point is 10 cm); and the owner found the front's slider over G#0800 to G#7800 (codes 128 to 1920) going from about infinity to far past it with nothing near in focus, its infinity code 1642 (G#66A0) near that span's top.
/// At 2^3 the front's infinity would sit at G#3350 and its near point at G#6000, both inside that span, which the owner's slider refutes; Android's two captured targets (G#5D10 and G#5120, docs/oracle/CAMERA.md) end in a zero nibble, as the code times 2^4 does.
pub const CORNERFOLK_SHIFT: u32 = 4;

/// The whole 12-bit code's travel as targets (G#0000 to G#FFF0): offered so a client reaches the near point (every one of the three past G#C000) and the lens's span past infinity.
/// Unmeasured past G#7800: the position (G#84) read back every target of G#0800 to G#7800 within G#64 on the front (2026-11-21) and within 30 ms on the main (2026-10-21); its two ends are for a device check (the position read back at G#0000 and G#FFF0).
pub const CORNERFOLK_TRAVEL: (u64, u64) = (0, 0xFFF << CORNERFOLK_SHIFT);

/// The module's factory AF calibration as this Pro's persist partition holds it (`/camera/AF_calibration.cal`, written at manufacture, read from the 2026-11-18 partition dump in keys/husky; one unit, as the IDTs are): its infinity and near ("macro") codes, the HAL's 12-bit codes for the cornerfolk actuators.
/// The near point's distance is not in that file: it is the lens's HAL minimum focus distance (`NEAR_MM`).
const fn cornerfolk_cal(infinity: u64, near: u64, near_mm: u64) -> FocusCal {
    FocusCal { infinity: infinity << CORNERFOLK_SHIFT, near: near << CORNERFOLK_SHIFT, near_mm }
}

/// The mode block's kind that opens a camera's readout `alts[k]`: `ALT + k`.
pub const ALT: u8 = 8;

/// A readout other than the binned one, replayed whole as Android ran it (docs/CAMERA.md "Every readout, captured"): its own sensor init, CPU setup, job and receiver, from the hv's 42-page capture, the PDMA dump and the i2c trace of Lumis on Android opening it (2026-11-18 night), nothing rewritten: the sensor runs the init's mode, the front end the job's geometry.
/// Its inputs in the vault: `cam.<key>.init.<tag>`, `cam.<key>.prog.<tag>`, `cam.<key>.pdma.<tag>`.
pub struct Alt {
    pub tag: &'static str,
    /// The readout: width and height in samples, a line's bytes (RAW10 packed, as the job writes it).
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    /// The colour filter's period in pixels (2 Bayer, 4 the quad array as it is).
    pub period: u16,
    /// Whether its job writes the PD rows (loch0's PD path): the ring's buffers then hold them after the image.
    pub pd: bool,
    pub rx_start: &'static [(usize, u32)],
    pub rx_stop: &'static [(usize, u32)],
    /// Its job's slots (ferros_cam job), as `Cam::sizes`, each from the start of its granule.
    pub sizes: [u64; SLOTS],
    /// Its line, measured at its own init's line length (ps; 0: the camera's line scaled by the init's PLL, `line_ps_for`): the tele's 11-degree and 48.8 MP readouts run longer lines than their CCS words say (their vendor tables).
    pub line_ps: u64,
    /// The processing blocks' switches its job runs with (ferros_cam geom `DETACH`; 0 none): the front's 8.42 MP stalls the engine in its second program unless FC ctx3 is off (bit 0).
    pub detach: u8,
    /// Array pixels each output pixel gathers on each axis (2 binned, 1 at 1:1), so its field is its size times this (ferros_cam optics `Optics::field`, the list's `field_*` and `fov_*`).
    /// Every captured readout is 1: the tele's 11 degrees is the array's middle 4032 x 3024 at 1:1 (its CCS words say the whole array binned as the 22 degrees' do; the crop is the vendor's G#0B30 = G#0101, and Android's virtual camera 6 reports half the tele's sensor, 2.8224 x 2.1168 mm), its 48.8 MP and the ultrawide's 48 MP the whole array at 1:1, the front's 8.42 MP the middle 3440 x 2448 of its 3840 x 2736 (its init's output crop, G#0350/G#0352 = 268 and 144).
    pub gather: u16,
}

impl Alt {
    pub const fn buffer_bytes(&self) -> u64 {
        job::buffer_bytes(self.height as u64 + if self.pd { self.height as u64 / 4 } else { 0 }, self.stride as u64)
    }
}

const fn cfa(a: u8, b: u8, c: u8, d: u8) -> u8 {
    a | b << 2 | c << 4 | d << 6
}

/// The main camera's per-frame acknowledgement (`crate::FRAME_ACK`) with the PDAF statistics status G#1C35_0148 first, as the ultrawide's frame handler does it (capture of its stream, 2026-10-22): without it the front end took one frame and stopped.
pub const FRAME_ACK_UW: [usize; 29] = [
    0x1C35_0148, 0x1C35_0160, 0x1C35_0178, 0x1C35_0190,
    0x1C32_008C,
    0x1C32_2004, 0x1C32_2024, 0x1C32_2038, 0x1C32_204C,
    0x1C32_0404, 0x1C32_0424, 0x1C32_0438, 0x1C32_044C,
    0x1C32_0110,
    0x1C35_0058, 0x1C35_0088, 0x1C35_00B8, 0x1C35_00E8, 0x1C35_0118, 0x1C35_01AC,
    0x1C33_1300, 0x1C33_3300, 0x1C33_5300, 0x1C33_7300, 0x1C33_9300, 0x1C33_B300, 0x1C33_C300, 0x1C33_D300, 0x1C33_E300,
];

// The mosaic, levels and orientation of every camera, from the Android captures the IDTs were solved from (2026-10-21, `pixel 8 pro calibration data/*average.vsf`, local only: a per-unit fingerprint; only these numbers come from them).
// Each capture's plane: `cfa` a 2 x 2 tensor over R G B (0 R, 1 G, 2 B), black 64 and white 1023 on all three (10 bits), `orientation` EXIF's (6 a quarter turn clockwise to stand upright, 8 three quarters), its readout size.
// Whether ferros reads out the same way: each capture's size is exactly what the init ferros replays sets (CCS x_output_size and y_output_size), the init being Android's own, captured from the same HAL opening the same camera; so the mode, its crop start and its flip are Android's.
// Every crop start in the four inits is even (x_addr_start, y_addr_start: the main 16, 16; the ultrawide 0, 464; the front 136, 0; the tele 8, 0), so no start moves the 2 x 2 phase; the flip (CCS image_orientation G#0101) ferros writes is the HAL's own write (the main's 3 in its stream-on, the ultrawide's 3 in its init; the front's 0 in its stream-on; the tele's in no captured write).
// The camera's register state while Android took each capture was not itself captured: that the HAL ran the same mode for its RAW stream as for the session the init came from rests on the matching size alone (docs/CAMERA.md "The other cameras").

/// The main camera (boitata), as the driver has streamed it since 2026-11-14: every field measured (docs/oracle/CAMERA.md).
/// Its capture (cam 2, 4080 x 3072): cfa G B / R G, black 64, white 1023, orientation 6; ferros's frames developed with it in true colour (2026-10-21, "First colour") and stand upright in Lumis at a quarter turn (2026-11-14).
pub const MAIN: Cam = Cam {
    id: 0,
    name: "main",
    key: "boitata",
    facing: 0,
    bus: 0x1088_0000,
    addr: 0x3D,
    id_reg: [0x00, 0x00],
    id_val: [0x08, 0xEF],
    // The main's stream write carries the image orientation (G#0101 = 3) as its Linux driver writes it.
    stream_on: &[0x01, 0x00, 0x01, 0x03],
    stream_off: &[0x01, 0x00, 0x00, 0x00],
    // G#0104 is one byte; G#0105 stays 1.
    hold: &[0x01, 0x04, 0x01, 0x01],
    release: &[0x01, 0x04, 0x00, 0x01],
    actuator: 0x0C,
    lens: Lens::Cornerfolk,
    focus: CORNERFOLK_TRAVEL,
    // Factory: infinity 509, near 3274 ("rear wide"): G#1FD0 and G#CCA0, 509 codes (G#1FD0 of target) past infinity to the travel's end.
    focus_cal: cornerfolk_cal(509, 3274, NEAR_MM[0]),
    streams: true,
    rx_start: &crate::rx::RX_START,
    rx_stop: &crate::rx::RX_STOP,
    ack: &crate::FRAME_ACK,
    tick_us: 1 << 15,
    width: 4080,
    height: 3072,
    stride: 0x1400,
    sizes: job::MAIN_SLOT_BYTES,
    skip: job::NO_SKIP,
    line_ps: crate::MAIN_LINE_PS,
    llp: 6320,
    pll: (3, 245),
    fll: 0x5372,
    exposure_lines: crate::MAIN_EXPOSURE_MAX_LINES,
    exposure_max_lines: crate::MAIN_EXPOSURE_MAX_LINES,
    cfa: cfa(1, 2, 0, 1),
    black: 64,
    white: 1023,
    rotation: 1,
    alts: &[],
    mode_live: false,
    blank_least: crate::BLANK_LEAST,
    slit_rows: 16,
    slit_frame_ns: 812_300,
    array: &crate::ccs::ARRAYS[0],
    optics: &crate::optics::OPTICS[0],
};

/// The ultrawide (oksoko, Sony IMX787), as the bring-up harness streamed it on 2026-10-22 (4000 x 3000 RAW10, about 30 frames a second, every frame landing).
/// Measured: the bus, address and identity (G#0787 at the CCS sensor register G#0016), the stream writes (G#0100 alone, 8-bit), the receiver (link 2), the acknowledgement, the tick, the geometry (stride G#13C0, its front-end program's), the outputs (`job::UW_SLOT_BYTES`).
/// From the init, not measured: the line time, 9568 pixel clocks (`line_length_pck`) at the PLL's 1458.2 MHz (24.576 MHz / 3 x 178), 6.5616 µs, which makes its 5081-line frame 33.3 ms, the 30 a second seen; the integration, 4124 lines; the longest offered, the frame less 2^6 lines (Sony's margin is not read).
/// Its mosaic B G / G R (its 48 MP readout's 2 x 2 blocks the same), as its calibration capture says (cam 3, 4000 x 3000), whose channel labelled red peaks at 604 nm in the response solved from it: the IMX787's own R G / G B (Camera2's RGGB on the 8a, whose init writes no flip) turned a half turn by the init's G#0101 = 3, which both readouts' inits write, on Android as on ferros.
/// The owner saw red and blue swapped in Lumis with R G / G B (2026-11-22, the list of 2026-11-18 to then), and ferros's own frame of 2026-10-22 (target/cam/uw1.raw10, the same init and job) develops a wooden floor tan with B G / G R and blue-grey with R G / G B.
/// The R G / G B before came from a cell table that took the brightest place for red in the owner's room (2026-11-18 night, rows `261 195` / `195 131`); the rear cameras looked at that room's blue light (the main, whose G B / R G is measured in true colour, read its blue place brightest there the same day, `162 181` / `104 162`), so the brightest place was blue.
/// Black 64, white 1023, orientation 6 from that capture. (Lumis's 2026-10-18 captures of it, 8000 x 6000 quad Bayer at orientation 3, were the full-resolution mode with the phone held sideways.)
/// Its lens actuator (G#0F, the main's protocol) answers since its power-up raises the LV buck (2026-11-21; before, it never acknowledged ferros, docs/oracle/CAMERA.md "Open: the ultrawide's lens actuator"): a sweep over G#0800 to G#7800 moved the lens (the frame's sharpness peaked mid-range, its mean breathing 110, 92, 110).
pub const ULTRAWIDE: Cam = Cam {
    id: 1,
    name: "ultrawide",
    key: "oksoko",
    facing: 0,
    bus: 0x108A_0000,
    addr: 0x1A,
    id_reg: [0x00, 0x16],
    id_val: [0x07, 0x87],
    stream_on: &[0x01, 0x00, 0x01],
    stream_off: &[0x01, 0x00, 0x00],
    hold: &[0x01, 0x04, 0x01],
    release: &[0x01, 0x04, 0x00],
    actuator: 0x0F,
    lens: Lens::Cornerfolk,
    focus: CORNERFOLK_TRAVEL,
    // Factory: infinity 883, near 3224 ("rear uw"): G#3730 and G#C980, 883 codes past infinity.
    focus_cal: cornerfolk_cal(883, 3224, NEAR_MM[1]),
    streams: true,
    rx_start: &crate::rx::RX_START_UW,
    rx_stop: &crate::rx::RX_STOP_UW,
    ack: &FRAME_ACK_UW,
    tick_us: 1 << 13,
    width: 4000,
    height: 3000,
    stride: 0x13C0,
    sizes: job::UW_SLOT_BYTES,
    skip: job::UW_SLOT_SKIP,
    line_ps: 6_561_622,
    llp: 9568,
    pll: (3, 178),
    fll: 5081,
    exposure_lines: 4124,
    exposure_max_lines: 5081 - (1 << 6),
    cfa: cfa(2, 1, 1, 0),
    black: 64,
    white: 1023,
    rotation: 1,
    alts: &[UW48],
    mode_live: false,
    blank_least: crate::BLANK_LEAST,
    slit_rows: 4,
    slit_frame_ns: 682_400,
    array: &crate::ccs::ARRAYS[1],
    optics: &crate::optics::OPTICS[1],
};

/// The front camera (dokkaebi, Samsung, G#30A1 at G#0000): its init, receiver (link 0) and job were captured on 2026-11-06; it answers ferros since the LV buck is raised (sub GPIO2, 2026-11-18) and streams with them (docs/CAMERA.md "The other cameras on the Pro").
/// From the init: 3840 x 2736 RAW10 (stride G#12C0 in its front-end program), the integration 1712 lines in a 4001-line frame.
/// Its line, measured (2026-11-18, docs/CAMERA.md "The other cameras on the Pro" and "The I2C side without a flash"): 8448 clocks at 24.576 MHz / 3 x 247 (2023.424 MHz) over vt_pix_clk_div 5 (G#0300) at two pixels a clock, 10.437753 us: ferros took frames of its init's 4001 lines 41.66 to 41.85 ms apart (10.44 us), and Android's 1 s hold (47908 lines of 2^1) runs frames 1.0 s apart. The guess before (8.35 us, 30 a second) reported every exposure 20 % short of what the sensor integrated.
/// From its two captures (cam 7, 3840 x 2736, the size its init sets): cfa G R / B G, black 64, white 1023; orientation 8 (three quarter turns) in the second, none recorded in the first; derived, its init's crop start even and its stream-on's flip 0 the HAL's write.
/// Its acknowledgement and tick are the ultrawide's, as the harness had them: 64 frames thru the view and 64 at bin 8 ran with them (2026-11-18), no error word.
/// Its lens actuator (G#0C) takes the main's protocol: on the Pro (2026-11-21) its position (G#84) read back each target of a sweep over G#0800 to G#7800 within G#64 (G#0864 for G#0800, G#782C for G#7800).
/// That span was the main's, never the front's: its infinity is G#66A0 and its near point G#C000 (`focus_cal`), so the owner's slider over it ran from about infinity to far past it (2026-11-22).
pub const FRONT: Cam = Cam {
    id: 2,
    name: "front",
    key: "dokkaebi",
    facing: 1,
    bus: 0x1089_0000,
    addr: 0x10,
    id_reg: [0x00, 0x00],
    id_val: [0x30, 0xA1],
    stream_on: &[0x01, 0x00, 0x01, 0x00],
    stream_off: &[0x01, 0x00, 0x00, 0x00],
    hold: &[0x01, 0x04, 0x01, 0x01],
    release: &[0x01, 0x04, 0x00, 0x01],
    actuator: 0x0C,
    lens: Lens::Cornerfolk,
    focus: CORNERFOLK_TRAVEL,
    // Factory: infinity 1642, near 3072 ("front normal"): G#66A0 and G#C000, 1642 codes past infinity.
    focus_cal: cornerfolk_cal(1642, 3072, NEAR_MM[2]),
    streams: true,
    rx_start: &crate::rx::RX_START_FRONT,
    rx_stop: &crate::rx::RX_STOP_FRONT,
    ack: &FRAME_ACK_UW,
    tick_us: 1 << 13,
    width: 3840,
    height: 2736,
    stride: 0x12C0,
    sizes: job::FRONT_SLOT_BYTES,
    skip: job::NO_SKIP,
    line_ps: 10_437_753,
    llp: 8448,
    pll: (3, 247),
    fll: 4001,
    exposure_lines: 1712,
    exposure_max_lines: 1712,
    cfa: cfa(1, 0, 2, 1),
    black: 64,
    white: 1023,
    rotation: 3,
    alts: &[FRONT842],
    mode_live: false,
    blank_least: crate::BLANK_LEAST,
    slit_rows: 2,
    slit_frame_ns: 1_264_200,
    array: &crate::ccs::ARRAYS[2],
    optics: &crate::optics::OPTICS[2],
};

/// The folded tele (kraken, Samsung, G#08D5 at G#0000), its 12.2 MP 22-degree readout (4032 x 3024 RAW10, the 48.8 MP array binned 2 x 2): its init from the i2c trace of Lumis on Android opening it (keys/husky/captures/ftrace-2026-11-18, tele22.init, 1142 transfers), its receiver, CPU setup and job from the hv's captures of the same readout (2026-11-18 night, docs/CAMERA.md "Every readout, captured").
/// The receiver: CSI link 6 (G#1C0A), the csis3 lanes, SYSREG G#20 and G#4 (`RX_START_TELE`, its captured stop `RX_STOP_TELE`); the acknowledgement the main's (its frame handler reads back the main's 28 words, no G#1C35_0148).
/// Android's job writes the image as 16-bit samples (loch0's format G#1C32_0550 = 2, stride G#1F80); the job ferros stores is made RAW10 packed as every other camera's (format 4, stride G#13C0, tools/oracle-isp image `--set`).
/// Its line, measured (the Pro, 2026-11-18 night): 5568 clocks at 24.576 MHz / 4 x 371 over vt_pix_clk_div 5 at two pixels a clock (911.7 MHz), 6.1068 us: ferros took its 5457-line frames 33.24 to 33.40 ms apart (30 a second; Lumis on Android showed 15, its own pace), and Android's 16 s hold (40960 lines of 2^6) is 16.0 s at it. Lumis's integration at its open 5332 lines; Android's 16 s is the main's shift words with the frame the integration plus 24.
/// Its stream on writes the image orientation G#0101 = 2 (the vertical flip), as Android's HAL writes it at all three of its readouts (`01 00 01 02`, keys/husky/captures/ftrace-2026-11-18; no tele init writes G#0101): with 0 every tele readout stood mirrored left to right against the main in Lumis (NCC 0.935 mirrored, 0.40 straight), with 2 all three match the main straight (22 degrees NCC 0.947, 48.8 MP 0.947, 11 degrees against 22 degrees 0.997; the Lumis session, 2026-11-19, docs/CAMERA.md "The tele's orientation"). The tele's quarter turn puts the sensor's rows across the screen, so its vertical flip is the screen's mirror.
/// Its mosaic with that flip B G / G R (the 48.8 MP quad's 2 x 2 blocks the same), as the sensor itself reports it: CCS pixel_order (G#0006) read back on ferros after the first frame says BGGR with G#0101 = 2 and GRBG with 0 (2026-11-19, `regs_say`), and the calibration capture (cam 4, 4032 x 3024, Android writing the same G#0101 = 2) says B G / G R, its channel labelled red peaking at 604 nm in the response solved from it.
/// The owner saw red and blue swapped in Lumis with R G / G B (2026-11-22).
/// The R G / G B before came from cell tables that took the brightest place for red (2026-11-18 night, unflipped rows `104 79` / `164 104`; 2026-11-19 flipped `257 94` / `93 79`): the rear cameras saw the room's blue light (the main the same day read its blue place brightest), so that place was blue, and every reading agreed with the sensor's own pixel_order once read as blue.
/// Black 64, white 1023, orientation 6 (a quarter turn) from that capture.
pub const TELE: Cam = Cam {
    id: 3,
    name: "tele",
    key: "kraken",
    facing: 0,
    bus: 0x108B_0000,
    addr: 0x2D,
    id_reg: [0x00, 0x00],
    id_val: [0x08, 0xD5],
    stream_on: &[0x01, 0x00, 0x01, 0x02],
    stream_off: &[0x01, 0x00, 0x00, 0x00],
    hold: &[0x01, 0x04, 0x01, 0x01],
    release: &[0x01, 0x04, 0x00, 0x01],
    actuator: 0x61,
    lens: Lens::Humbaba,
    // Its travel, measured on the Pro (2026-11-22, b1137, every target read back): from G#0100 (256, read G#00FF) to G#3F00 (16128, read G#3EEC), every target G#100 apart below infinity within 7 of it and G#400 apart above the near point up to G#3B00 within 2; past G#3F00 the position reads G#3FF2 to G#3FFF whatever the target (its 14-bit end).
    // So 6412 below Android's infinity (6668: about 1.24 dioptres past it at Android's 5190 a dioptre) and 2973 past its near point (13155, 80 cm: to about 1.82 dioptres, 55 cm); 0 itself unmeasured.
    focus: (256, 16128),
    // The factory's AF calibration ("rear tele", persist /camera/AF_calibration.cal) is infinity 7050 and near 13538 in humbaba's own units; Android drives both 382 lower, so these are Android's.
    focus_cal: FocusCal { infinity: 6668, near: 13155, near_mm: NEAR_MM[3] },
    streams: true,
    rx_start: &crate::rx::RX_START_TELE,
    rx_stop: &crate::rx::RX_STOP_TELE,
    ack: &crate::FRAME_ACK,
    tick_us: 1 << 15,
    width: 4032,
    height: 3024,
    stride: 0x13C0,
    sizes: job::TELE_SLOT_BYTES,
    skip: job::NO_SKIP,
    line_ps: 6_106_805,
    llp: 5568,
    pll: (4, 0x173),
    fll: 5457,
    exposure_lines: 5332,
    exposure_max_lines: 5457 - 4,
    cfa: cfa(2, 1, 1, 0),
    black: 64,
    white: 1023,
    rotation: 1,
    alts: &[TELE11, TELE48],
    mode_live: true,
    // Measured on the Pro (2026-11-21): asked 3211 lines (51 a second) its frames came 19.95 ms apart (about 3267 lines), so the sensor pads its frame past the driver's 128; asked 3275 (50 a second) they came 20.0 ms apart. 250 past its 3024 rows makes 50 its fastest whole rate.
    blank_least: 250,
    slit_rows: 2,
    slit_frame_ns: 1_410_670,
    array: &crate::ccs::ARRAYS[3],
    optics: &crate::optics::OPTICS[3],
};

/// The tele's 12.2 MP 11-degree readout: the 48.8 MP array's middle 4032 x 3024 at 0.7 um (its init the 22-degree one but G#0B30 = G#0101 and its own frame, 3377 lines), its PD rows 248 x 1488; written 16-bit by Android's job, packed here as the 22-degree's.
/// Measured on the Pro (2026-11-18 night): a Bayer mosaic (the sensor remosaics; its cell tables, read with blue as their brightest place, the 22-degree's: G R / B G unflipped, B G / G R with the stream on's G#0101 = 2), frames 33.3 ms apart, its line 9.8712 us (a 2 s exposure planned as 335392 lines landed 3.3107 s apart, its 3377-line frame 33.3 ms).
pub const TELE11: Alt = Alt { tag: "t11", width: 4032, height: 3024, stride: 0x13C0, period: 2, pd: true, rx_start: &crate::rx::RX_START_TELE11, rx_stop: &crate::rx::RX_STOP_TELE, sizes: job::TELE11_SLOT_BYTES, line_ps: 9_871_200, detach: 0, gather: 1 };
/// The tele's 48.8 MP readout, 8064 x 6048 RAW10 at G#2760 a line (its job's own, packed), no PD rows; its frame 6309 lines.
/// Measured on the Pro (2026-11-18 night): the quad array as it is (2 x 2 blocks G R / B G unflipped, B G / G R with the stream on's G#0101 = 2: the 4 x 4 cell B B G G / B B G G / G G R R / G G R R, its cell tables read with blue as their brightest place), its line 13.2008 us (a 2 s exposure planned as 335392 lines landed 4.4274 s apart; its 6309-line frame 83.3 ms, 12 a second).
pub const TELE48: Alt = Alt { tag: "t48", width: 8064, height: 6048, stride: 0x2760, period: 4, pd: false, rx_start: &crate::rx::RX_START_TELE48, rx_stop: &crate::rx::RX_STOP_TELE, sizes: job::TELE48_SLOT_BYTES, line_ps: 13_200_800, detach: 0, gather: 1 };
/// The ultrawide's 48 MP readout, 8000 x 6000 RAW10 at G#2720 a line, its PD rows 4000 x 1500; its link at the slower op PLL (`RX_START_UW48`).
pub const UW48: Alt = Alt { tag: "w48", width: 8000, height: 6000, stride: 0x2720, period: 4, pd: true, rx_start: &crate::rx::RX_START_UW48, rx_stop: &crate::rx::RX_STOP_UW, sizes: job::UW48_SLOT_BYTES, line_ps: 5_406_700, detach: 0, gather: 1 };
/// The front's 8.42 MP readout, 3440 x 2448, the sensor's own scaler; written 16-bit by Android's job (stride G#1AE0), packed here (G#1100).
pub const FRONT842: Alt = Alt { tag: "f8", width: 3440, height: 2448, stride: 0x1100, period: 2, pd: true, rx_start: &crate::rx::RX_START_FRONT842, rx_stop: &crate::rx::RX_STOP_FRONT, sizes: job::FRONT842_SLOT_BYTES, line_ps: 0, detach: 1, gather: 1 };

/// Every camera of the Pro (husky), by id.
pub const CAMS: [&Cam; 4] = [&MAIN, &ULTRAWIDE, &FRONT, &TELE];

// ---------------------------------------------------------------- akita (Pixel 8a)

// akita's three cameras (docs/CAMERA.md "akita's cameras"), from its device tree (the factory image's MP overlay, dtbo 18, "ZUMA AKITA MP", the same as zuma-akita-camera.dtsi): every one on its own bus at G#1A, 16-bit register addresses, 8-bit values.
// The buses are husky's (hsi2c_1, hsi2c_3, hsi2c_2), so the driver's grant slots by id hold on both boards.
// From the hv captures of Android opening each (2026-11-15 and 2026-11-18, docs/CAMERA.md "akita's cameras"): every sensor's init (I2C, integrity clean), the receivers and PDMA jobs, the front end's CPU programs (merged from captures that lost lines, docs/CAMERA.md "The 8a's main streams"); the main streams since 2026-11-19.
// Each readout is its init's CCS output size, each line its job's stride (the ultrawide's derived: no job of its was captured), each mosaic and orientation Android's Camera2 characteristics of the same mode (dumpsys media.camera: colorFilterArrangement, sensor.orientation), black 64 and white 1023 its blackLevelPattern and whiteLevel.

/// akita's main camera (oksoko): a Sony IMX787 (the same part as husky's ultrawide, G#0787 at the CCS sensor register G#0016), on hsi2c_1 at G#1A.
/// Its master clock is CIS_CLK3, asked at 26 MHz by the device tree, but Android's init tells the sensor 24.574 MHz (EXCK_FREQ G#1893): Linux leaves it on the 24.576 MHz oscillator, as the cold boot has it (CMU_TOP G#2604_101C = 0).
/// Measured from Android's session: 4624 x 3472 RAW10 on three lanes (CCS G#0114 = 2), CSI link 1 and the csis1 lanes as husky's main (`rx::RX_START_AKITA_MAIN`), RGGB, orientation 90.
/// Android's job writes its image in 16-bit samples (format 2 at G#1C32_0550, a line of G#2420); its stored job has them RAW10 packed as every other path (format 4, the line G#16C0: cam-oracle `job-set`), so the whole driver reads it as the Pro's.
/// Streams on ferros since 2026-11-19 (docs/CAMERA.md "The 8a's main streams"): 29.5 frames a second, every frame released by the kernel at DMA_DONE, the 2 x 2 cell measured R G / G B (the greens equal on the anti-diagonal; red from the IMX787's mosaic on the Pro, unflipped here).
/// From its init: a line of 9568 clocks at 24.576 MHz / 3 x 166 (7.036 µs), a frame of 4826 lines (34.0 ms, 29.5 a second), the integration 1184 lines (Android's in a dark room), the longest offered the frame less 2^6 lines as the ultrawide's.
/// Its lens is driven by "nessie" at G#24 (the actuator, the OIS and the EEPROM share that address, 32-bit values): not the main's protocol, so no focus here.
pub const AKITA_MAIN: Cam = Cam {
    id: 0,
    name: "main",
    key: "oksoko",
    facing: 0,
    bus: 0x1088_0000,
    addr: 0x1A,
    id_reg: [0x00, 0x16],
    id_val: [0x07, 0x87],
    stream_on: &[0x01, 0x00, 0x01],
    stream_off: &[0x01, 0x00, 0x00],
    hold: &[0x01, 0x04, 0x01],
    release: &[0x01, 0x04, 0x00],
    actuator: 0,
    lens: Lens::None,
    focus: (0, 0),
    focus_cal: FOCUS_UNKNOWN,
    streams: true,
    rx_start: &crate::rx::RX_START_AKITA_MAIN,
    rx_stop: &crate::rx::RX_STOP,
    ack: &crate::FRAME_ACK,
    tick_us: 1 << 13,
    width: 4624,
    height: 3472,
    stride: 0x16C0,
    sizes: job::AKITA_MAIN_SLOT_BYTES,
    skip: job::AKITA_MAIN_SLOT_SKIP,
    line_ps: 7_035_956,
    llp: 9568,
    pll: (3, 166),
    fll: 4826,
    exposure_lines: 1184,
    exposure_max_lines: 4826 - (1 << 6),
    cfa: cfa(0, 1, 1, 2),
    black: 64,
    white: 1023,
    rotation: 1,
    alts: &[],
    mode_live: false,
    blank_least: crate::BLANK_LEAST,
    slit_rows: 0,
    slit_frame_ns: 0,
    array: &crate::ccs::AKITA_ARRAYS[0],
    optics: &crate::optics::AKITA_OPTICS[0],
};

/// akita's ultrawide (leshen-uw): a part the device tree does not name, on hsi2c_3 at G#1A, CIS_CLK0 (the oscillator, as every akita init tells its sensor 24.574 MHz).
/// It answers G#0712 at the CCS sensor register G#0016 (PROBE, 2026-11-15): a Sony IMX712.
/// From its init: 4208 x 3120 RAW10 on four lanes, orientation 90 and RGGB in Camera2; CSI link 0 and the csis0 lanes (`rx::RX_START_AKITA_UW`, SYSREG select 1), its job (2026-11-18, isp-uw list 1) writing 16-bit samples, stored RAW10 packed at G#14C0 a line as the main's.
/// Its timing: a line of 5192 clocks at 24.576 MHz / 3 x 122 counted in two (10.390 us: its frames 33.89 to 34.04 ms apart at 3269 lines, 2026-11-19), the integration 801 lines, the margin 10 (Android's holds).
/// Streams on ferros since 2026-11-19 (docs/CAMERA.md "The 8a's front and ultrawide stream"): its program's loch0 input on link 0 (G#1C32_048C G#200) and its job cut to Android's G#1828 bytes; 29.4 a second, R G / G B, the shift's ceiling 7 (87.16 s).
pub const AKITA_ULTRAWIDE: Cam = Cam {
    id: 1,
    name: "ultrawide",
    key: "leshen-uw",
    facing: 0,
    bus: 0x108A_0000,
    addr: 0x1A,
    id_reg: [0x00, 0x16],
    id_val: [0x07, 0x12],
    stream_on: &[0x01, 0x00, 0x01],
    stream_off: &[0x01, 0x00, 0x00],
    hold: &[0x01, 0x04, 0x01],
    release: &[0x01, 0x04, 0x00],
    actuator: 0,
    lens: Lens::None,
    focus: (0, 0),
    focus_cal: FOCUS_UNKNOWN,
    streams: true,
    rx_start: &crate::rx::RX_START_AKITA_UW,
    rx_stop: &crate::rx::RX_STOP_FRONT,
    ack: &crate::FRAME_ACK,
    tick_us: 1 << 13,
    width: 4208,
    height: 3120,
    stride: 0x14C0,
    sizes: job::AKITA_UW_SLOT_BYTES,
    skip: job::AKITA_UW_SLOT_SKIP,
    line_ps: 10_389_984,
    llp: 5192,
    pll: (3, 122),
    fll: 3269,
    exposure_lines: 801,
    exposure_max_lines: 3269 - 10,
    cfa: cfa(0, 1, 1, 2),
    black: 64,
    white: 1023,
    rotation: 1,
    alts: &[],
    mode_live: false,
    blank_least: crate::BLANK_LEAST,
    slit_rows: 0,
    slit_frame_ns: 0,
    array: &crate::ccs::AKITA_ARRAYS[1],
    optics: &crate::optics::AKITA_OPTICS[1],
};

/// akita's front camera (leshen): the ultrawide's module family, on hsi2c_2 at G#1A, CIS_CLK1 (the oscillator); it answers G#0712 at G#0016 (PROBE, 2026-11-15): a Sony IMX712 as the ultrawide.
/// Measured from Android's session: 4208 x 3072 RAW10 on four lanes, its image flipped both ways by its init (G#0101 = 3), CSI link 6 and the csis3 lanes (`rx::RX_START_AKITA_FRONT`, SYSREG select G#60), BGGR, orientation 270; its job writes 16-bit samples (format 2, a G#20E0 line), stored RAW10 packed (format 4, the line G#14C0) as the main's.
/// Its timing: a line of 5192 clocks at 24.576 MHz / 3 x 119 counted in two (10.652 us: its frames 33.88 to 34.02 ms apart at 3187 lines, 2026-11-19), the integration 3118 lines, the margin 10 (Android's holds).
/// Streams on ferros since 2026-11-19 (docs/CAMERA.md "The 8a's front and ultrawide stream"): its program's loch0 input on link 6 (G#1C32_048C G#0600_0200); 29.5 a second, B G / G R, the shift's ceiling 7 (89.35 s).
pub const AKITA_FRONT: Cam = Cam {
    id: 2,
    name: "front",
    key: "leshen",
    facing: 1,
    bus: 0x1089_0000,
    addr: 0x1A,
    id_reg: [0x00, 0x16],
    id_val: [0x07, 0x12],
    stream_on: &[0x01, 0x00, 0x01],
    stream_off: &[0x01, 0x00, 0x00],
    hold: &[0x01, 0x04, 0x01],
    release: &[0x01, 0x04, 0x00],
    actuator: 0,
    lens: Lens::None,
    focus: (0, 0),
    focus_cal: FOCUS_UNKNOWN,
    streams: true,
    rx_start: &crate::rx::RX_START_AKITA_FRONT,
    rx_stop: &crate::rx::RX_STOP_AKITA_FRONT,
    ack: &crate::FRAME_ACK,
    tick_us: 1 << 13,
    width: 4208,
    height: 3072,
    stride: 0x14C0,
    sizes: job::AKITA_FRONT_SLOT_BYTES,
    skip: job::AKITA_FRONT_SLOT_SKIP,
    line_ps: 10_651_916,
    llp: 5192,
    pll: (3, 119),
    fll: 3128,
    exposure_lines: 3118,
    exposure_max_lines: 3128 - 10,
    cfa: cfa(2, 1, 1, 0),
    black: 64,
    white: 1023,
    rotation: 3,
    alts: &[],
    mode_live: false,
    blank_least: crate::BLANK_LEAST,
    slit_rows: 0,
    slit_frame_ns: 0,
    array: &crate::ccs::AKITA_ARRAYS[2],
    optics: &crate::optics::AKITA_OPTICS[2],
};

/// Every camera of the 8a (akita), by id: there is no tele.
pub const AKITA: [&Cam; 3] = [&AKITA_MAIN, &AKITA_ULTRAWIDE, &AKITA_FRONT];

/// The board the table is for: ferros_sys's fact 84 numbers (1 husky, 2 akita), set once by whoever reads the table (the kernel from its board, the driver from its facts) before any read; 0 or 1 is husky's.
static BOARD: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(1);
pub const BOARD_HUSKY: u8 = 1;
pub const BOARD_AKITA: u8 = 2;

/// Which board's cameras the table names from now on.
pub fn set_board(board: u8) {
    BOARD.store(board, core::sync::atomic::Ordering::Relaxed);
}

/// The board's cameras, by id.
pub fn all() -> &'static [&'static Cam] {
    if BOARD.load(core::sync::atomic::Ordering::Relaxed) == BOARD_AKITA { &AKITA } else { &CAMS }
}

/// The camera with this id on this board.
/// WHY `get`: the id is the caller's byte (an OPEN or PROBE on the endpoint), so one past the table is answered "no such camera", not indexed.
pub fn by_id(id: u8) -> Option<&'static Cam> {
    all().get(id as usize).copied()
}

/// The widest readout, the most rows and the longest line of any camera on either board: the driver's buffers are sized for them (akita's main, 4624 x 3472 at G#16C0, since 2026-11-15; husky's main was 4080 x 3072 at G#1400).
pub const W_MOST: usize = 4624;
pub const H_MOST: usize = 3472;
pub const STRIDE_MOST: usize = 0x16C0;

impl Cam {
    /// An image buffer for its captured readout: its rows of `stride` bytes in whole frames of the pool (the Pro's main 16 MiB; a 1:1 mode or a slit sizes its own from the mode, ferros_cam job `buffer_bytes`, `slit_bytes`; the 8a's main, 4624 x 3472 at G#1700, 22 MiB). One constant (`job::IMAGE_BYTES`) held every camera until 2026-11-15.
    pub const fn buffer_bytes(&self) -> u64 {
        job::buffer_bytes(self.height as u64, self.stride as u64)
    }
    /// Its region with an image ring of `n` buffers of its captured readout.
    pub const fn layout(&self, n: usize) -> job::Layout {
        job::with_image(&self.sizes, &self.skip, n as u64 * self.buffer_bytes())
    }
    /// The array pixels its captured (binned) readout reads, width and height: each output pixel gathers the array's pixels over its own (ferros_cam optics `array_px`: the rear sensors' 2 x 2, Android's PIXEL_ARRAY 4080 x 3072 against PIXEL_ARRAY_MAXIMUM_RESOLUTION 8160 x 6144 and the like; the front's one; the 8a's, whose Camera2 arrays are their binned readouts, one), so every binned readout reads the whole array.
    /// Its init's window says the same: the main's x G#10 to G#3FCF, 16320 addresses at 2 a pixel, 8160; the tele's x G#8 to G#1F87, 8064, its rows 6080 binned to 3040 and 3024 kept (G#0352 = 8 at each end); the ultrawide's 4000 x 3000 binned out of 8000 x 6000 by its digital crop.
    pub const fn binned_field(&self) -> (u32, u32) {
        let o = self.optics;
        let g = o.array_px.0 / self.width as u32;
        (self.width as u32 * g, self.height as u32 * g)
    }
    /// The binned readout at `bin`: whole 2 x 2 cells of `bin` x `bin` blocks, so the mosaic keeps its phase (the ultrawide's 3000 rows at bin 8 are 374, not 375).
    pub const fn binned(&self, bin: usize) -> (usize, usize) {
        (self.width / (2 * bin) * 2, self.height / (2 * bin) * 2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_holds() {
        for (k, c) in CAMS.iter().enumerate() {
            assert_eq!(c.id as usize, k);
            assert_eq!(by_id(c.id).unwrap().name, c.name);
            // A line holds its RAW10 samples, and a frame fits an image buffer.
            assert!(c.width * 5 / 4 <= c.stride && (c.height * c.stride) as u64 <= c.buffer_bytes(), "{}", c.name);
            // A ring of four of its buffers and its region fit the kernel's camera window (cachemap `CAM_VA`, 512 MiB) with the engine's area.
            assert!(c.layout(4).bytes + 3 * job::FRAME <= 512 << 20, "{}", c.name);
            if c.streams {
                assert!(c.width <= W_MOST && c.height <= H_MOST && c.stride <= STRIDE_MOST, "{}", c.name);
                assert!(!c.rx_start.is_empty() && !c.rx_stop.is_empty());
                // The driver copies a line in 64-byte loads, and turns an exposure in ns into lines by the line time.
                assert!(c.stride % 64 == 0 && c.line_ps > 0, "{}", c.name);
                // Every bin the endpoint offers gives whole 2 x 2 cells.
                for bin in [1, 2, 4, 8] {
                    let (w, h) = c.binned(bin);
                    assert!(w % 2 == 0 && h % 2 == 0 && w * bin <= c.width && h * bin <= c.height);
                }
            }
            assert!(c.exposure_lines <= c.exposure_max_lines);
        }
        // The captures' facts (2026-10-21, local): their 2 x 2 cfa tensors as cells, every level 64 / 1023, EXIF 6 a quarter turn and 8 three quarters, and each capture's size the readout; ferros reads every camera as Android did (the ultrawide's init flips both ways on both, the tele's stream on flips its rows on both).
        // The tele's stream on carries Android's G#0101 = 2 (its frames mirrored against the main without it).
        assert_eq!(TELE.stream_on, &[0x01, 0x00, 0x01, 0x02]);
        for (c, cell, turns, size) in [(&MAIN, [1, 2, 0, 1], 1, (4080, 3072)), (&ULTRAWIDE, [2, 1, 1, 0], 1, (4000, 3000)), (&FRONT, [1, 0, 2, 1], 3, (3840, 2736)), (&TELE, [2, 1, 1, 0], 1, (4032, 3024))] {
            assert_eq!(c.cfa, cfa(cell[0], cell[1], cell[2], cell[3]), "{}", c.name);
            assert_eq!((c.rotation, c.black, c.white, (c.width, c.height)), (turns, 64, 1023, size), "{}", c.name);
        }
        // akita's: ids in order, all three streaming (the main, then the front and ultrawide, 2026-11-19), and each id's bus husky's for the same id (the driver's grant slots are by id).
        for (k, c) in AKITA.iter().enumerate() {
            assert_eq!(c.id as usize, k);
            assert!(c.streams, "{}", c.name);
            assert_eq!(c.bus, CAMS[k].bus, "{}", c.name);
            // Each line holds its RAW10 samples on a 64-byte boundary, each frame fits its buffer, and each readout fits the driver's buffers.
            assert!(c.width * 5 / 4 <= c.stride && c.stride % 64 == 0 && (c.height * c.stride) as u64 <= c.buffer_bytes(), "{}", c.name);
            assert!(c.width <= W_MOST && c.height <= H_MOST && c.stride <= STRIDE_MOST, "{}", c.name);
        }
        // akita's main's line: 9568 clocks at 24.576 MHz / 3 x 166, and its buffer 20 MiB (3472 rows of G#16C0 RAW10 packed, 20.2 MB, in whole 2 MiB frames of the pool).
        let ps = 9568u64 * 3 * 1_000_000_000_000 / (24_576_000 * 166);
        assert!(AKITA_MAIN.line_ps.abs_diff(ps) <= 1, "{ps}");
        assert_eq!(AKITA_MAIN.buffer_bytes(), 20 << 20);
        set_board(BOARD_AKITA);
        assert_eq!(by_id(0).unwrap().key, "oksoko");
        assert!(by_id(3).is_none());
        set_board(BOARD_HUSKY);
        assert_eq!(by_id(0).unwrap().key, "boitata");
        assert_eq!(MAIN.binned(2), (2040, 1536));
        assert_eq!(ULTRAWIDE.binned(8), (500, 374));
        // The ultrawide's line: 9568 clocks at 24.576 MHz / 3 x 178.
        let ps = 9568u64 * 3 * 1_000_000_000_000 / (24_576_000 * 178);
        assert!(ULTRAWIDE.line_ps.abs_diff(ps) <= 1, "{ps}");
        // The front's line: 8448 clocks at 24.576 MHz / 3 x 247 over vt_pix_clk_div 5, two pixels a clock (measured: 41.66 to 41.85 ms a 4001-line frame).
        let ps = 8448u64 * 3 * 5 * 1_000_000_000_000 / (2 * 24_576_000 * 247);
        assert!(FRONT.line_ps.abs_diff(ps) <= 1, "{ps}");
        // The tele's line: 5568 clocks at 24.576 MHz / 4 x 371 over vt_pix_clk_div 5, two pixels a clock (measured: 33.24 to 33.40 ms a 5457-line frame).
        let ps = 5568u64 * 4 * 5 * 1_000_000_000_000 / (2 * 24_576_000 * 371);
        assert!(TELE.line_ps.abs_diff(ps) <= 1, "{ps}");
        // Every captured readout: its line holds its RAW10 samples on 64 bytes, a ring of four of its buffers and its job fit the window, its slots name the image's (2).
        for c in CAMS.iter() {
            for a in c.alts.iter() {
                assert!(a.width * 5 / 4 <= a.stride && a.stride % 32 == 0 && a.sizes[job::OLD_SLOT_IMAGE] != 0, "{}", a.tag);
                assert!(job::with_image(&a.sizes, &job::NO_SKIP, 4 * a.buffer_bytes()).bytes + 3 * job::FRAME <= 512 << 20, "{}", a.tag);
            }
        }
    }
}
