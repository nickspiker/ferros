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
    /// Its lens actuator's address on the same bus (0: none to drive), the main's protocol (02 <- G#40, 5 ms, 02 <- 0, F1 <- 0, the target in 00/01).
    pub actuator: u8,
    /// The focus targets offered (0 and 0: focus not offered).
    pub focus: (u64, u64),
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
    /// Whether the sensor takes a mode's words only while streaming: written in the hold before stream on, its frame, window and output size are dropped at stream on (the sensor runs its init's mode), so the driver writes the mode after the first frame (payloads/camera `F_MID`).
    /// The tele (2026-11-18, docs/CAMERA.md "Slits on every camera"): frames of 4000 and 8000 lines written before stream on both ran 30 a second, and a 64-row slit sent whole 4032 x 3024 frames (a front end left at the whole frame took them clean); written after the first frame, 4000 lines ran 40 a second, 8000 lines 20, and the slit 559 a second.
    pub mode_live: bool,
    /// Its sensor's CCS facts (ferros_cam ccs `Array`: gain, margin, long-exposure shift), by board: the Pro's `ARRAYS`, the 8a's `AKITA_ARRAYS`.
    pub array: &'static crate::ccs::Array,
    /// Its lens and sensor size (ferros_cam optics `Optics`: the list's sizes, focal length, f-number and each readout's field), by board: the Pro's `OPTICS`, the 8a's `AKITA_OPTICS`.
    pub optics: &'static crate::optics::Optics,
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
    focus: (0x0800, 0x7800),
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
    array: &crate::ccs::ARRAYS[0],
    optics: &crate::optics::OPTICS[0],
};

/// The ultrawide (oksoko, Sony IMX787), as the bring-up harness streamed it on 2026-10-22 (4000 x 3000 RAW10, about 30 frames a second, every frame landing).
/// Measured: the bus, address and identity (G#0787 at the CCS sensor register G#0016), the stream writes (G#0100 alone, 8-bit), the receiver (link 2), the acknowledgement, the tick, the geometry (stride G#13C0, its front-end program's), the outputs (`job::UW_SLOT_BYTES`).
/// From the init, not measured: the line time, 9568 pixel clocks (`line_length_pck`) at the PLL's 1458.2 MHz (24.576 MHz / 3 x 178), 6.5616 µs, which makes its 5081-line frame 33.3 ms, the 30 a second seen; the integration, 4124 lines; the longest offered, the frame less 2^6 lines (Sony's margin is not read).
/// Its mosaic, measured on ferros (2026-11-18 night, a 256 ms frame in the owner's red-lit room, the cell table): rows `261 195` / `195 131`, so R G / G B (its 48 MP readout's 2 x 2 blocks the same), where its calibration capture (cam 3, 4000 x 3000) says B G / G R: the half turn the init's flip (G#0101 = 3, written by both readouts' inits) makes, so that capture's RAW stream ran unflipped and ferros's frames stand a half turn from it (the rotation below, from that capture, is unchecked on a ferros frame). Black 64, white 1023, orientation 6 from that capture. (Lumis's 2026-10-18 captures of it, 8000 x 6000 quad Bayer at orientation 3, were the full-resolution mode with the phone held sideways.)
/// Its lens actuator (G#0F) never acknowledged ferros (docs/oracle/CAMERA.md "Open: the ultrawide's lens actuator"), so focus is not offered and the lens stays at rest.
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
    actuator: 0,
    focus: (0, 0),
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
    cfa: cfa(0, 1, 1, 2),
    black: 64,
    white: 1023,
    rotation: 1,
    alts: &[UW48],
    mode_live: false,
    array: &crate::ccs::ARRAYS[1],
    optics: &crate::optics::OPTICS[1],
};

/// The front camera (dokkaebi, Samsung, G#30A1 at G#0000): its init, receiver (link 0) and job were captured on 2026-11-06; it answers ferros since the LV buck is raised (sub GPIO2, 2026-11-18) and streams with them (docs/CAMERA.md "The other cameras on the Pro").
/// From the init: 3840 x 2736 RAW10 (stride G#12C0 in its front-end program), the integration 1712 lines in a 4001-line frame.
/// Its line, measured (2026-11-18, docs/CAMERA.md "The other cameras on the Pro" and "The I2C side without a flash"): 8448 clocks at 24.576 MHz / 3 x 247 (2023.424 MHz) over vt_pix_clk_div 5 (G#0300) at two pixels a clock, 10.437753 us: ferros took frames of its init's 4001 lines 41.66 to 41.85 ms apart (10.44 us), and Android's 1 s hold (47908 lines of 2^1) runs frames 1.0 s apart. The guess before (8.35 us, 30 a second) reported every exposure 20 % short of what the sensor integrated.
/// From its two captures (cam 7, 3840 x 2736, the size its init sets): cfa G R / B G, black 64, white 1023; orientation 8 (three quarter turns) in the second, none recorded in the first; derived, its init's crop start even and its stream-on's flip 0 the HAL's write.
/// Its acknowledgement and tick are the ultrawide's, as the harness had them: 64 frames thru the view and 64 at bin 8 ran with them (2026-11-18), no error word.
/// Its lens actuator (G#0C) is driven as the main's; focus is not offered until its range is measured.
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
    focus: (0, 0),
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
    array: &crate::ccs::ARRAYS[2],
    optics: &crate::optics::OPTICS[2],
};

/// The folded tele (kraken, Samsung, G#08D5 at G#0000), its 12.2 MP 22-degree readout (4032 x 3024 RAW10, the 48.8 MP array binned 2 x 2): its init from the i2c trace of Lumis on Android opening it (keys/husky/captures/ftrace-2026-11-18, tele22.init, 1142 transfers), its receiver, CPU setup and job from the hv's captures of the same readout (2026-11-18 night, docs/CAMERA.md "Every readout, captured").
/// The receiver: CSI link 6 (G#1C0A), the csis3 lanes, SYSREG G#20 and G#4 (`RX_START_TELE`, its captured stop `RX_STOP_TELE`); the acknowledgement the main's (its frame handler reads back the main's 28 words, no G#1C35_0148).
/// Android's job writes the image as 16-bit samples (loch0's format G#1C32_0550 = 2, stride G#1F80); the job ferros stores is made RAW10 packed as every other camera's (format 4, stride G#13C0, tools/oracle-isp image `--set`).
/// Its line, measured (the Pro, 2026-11-18 night): 5568 clocks at 24.576 MHz / 4 x 371 over vt_pix_clk_div 5 at two pixels a clock (911.7 MHz), 6.1068 us: ferros took its 5457-line frames 33.24 to 33.40 ms apart (30 a second; Lumis on Android showed 15, its own pace), and Android's 16 s hold (40960 lines of 2^6) is 16.0 s at it. Lumis's integration at its open 5332 lines; Android's 16 s is the main's shift words with the frame the integration plus 24.
/// Its mosaic, measured on ferros (2026-11-18 night, a 2 s frame in the owner's red-lit room, the cell table): rows `104 79 104 80` / `164 104 164 104`, so G B / R G (R 164, B 79, G 104; the same room read the front's R over G over B, as its calibration said). Its calibration capture (cam 4, 4032 x 3024) says B G / G R: a column apart, so Android's RAW stream for that capture ran another window or flip than Lumis's 22-degree session the init came from; ferros reads as the init sets it. Black 64, white 1023, orientation 6 (a quarter turn) from that capture.
pub const TELE: Cam = Cam {
    id: 3,
    name: "tele",
    key: "kraken",
    facing: 0,
    bus: 0x108B_0000,
    addr: 0x2D,
    id_reg: [0x00, 0x00],
    id_val: [0x08, 0xD5],
    stream_on: &[0x01, 0x00, 0x01, 0x00],
    stream_off: &[0x01, 0x00, 0x00, 0x00],
    hold: &[0x01, 0x04, 0x01, 0x01],
    release: &[0x01, 0x04, 0x00, 0x01],
    actuator: 0,
    focus: (0, 0),
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
    cfa: cfa(1, 2, 0, 1),
    black: 64,
    white: 1023,
    rotation: 1,
    alts: &[TELE11, TELE48],
    mode_live: true,
    array: &crate::ccs::ARRAYS[3],
    optics: &crate::optics::OPTICS[3],
};

/// The tele's 12.2 MP 11-degree readout: the 48.8 MP array's middle 4032 x 3024 at 0.7 um (its init the 22-degree one but G#0B30 = G#0101 and its own frame, 3377 lines), its PD rows 248 x 1488; written 16-bit by Android's job, packed here as the 22-degree's.
/// Measured on the Pro (2026-11-18 night): a Bayer mosaic (the sensor remosaics; the cell table of a 2 s frame G B / R G as the 22-degree's), frames 33.3 ms apart, its line 9.8712 us (a 2 s exposure planned as 335392 lines landed 3.3107 s apart, its 3377-line frame 33.3 ms).
pub const TELE11: Alt = Alt { tag: "t11", width: 4032, height: 3024, stride: 0x13C0, period: 2, pd: true, rx_start: &crate::rx::RX_START_TELE11, rx_stop: &crate::rx::RX_STOP_TELE, sizes: job::TELE11_SLOT_BYTES, line_ps: 9_871_200, detach: 0, gather: 1 };
/// The tele's 48.8 MP readout, 8064 x 6048 RAW10 at G#2760 a line (its job's own, packed), no PD rows; its frame 6309 lines.
/// Measured on the Pro (2026-11-18 night): the quad array as it is (2 x 2 blocks G B / R G), its line 13.2008 us (a 2 s exposure planned as 335392 lines landed 4.4274 s apart; its 6309-line frame 83.3 ms, 12 a second).
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
    focus: (0, 0),
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
    array: &crate::ccs::AKITA_ARRAYS[0],
    optics: &crate::optics::AKITA_OPTICS[0],
};

/// akita's ultrawide (leshen-uw): a part the device tree does not name, on hsi2c_3 at G#1A, CIS_CLK0 (the oscillator, as every akita init tells its sensor 24.574 MHz).
/// It answers G#0712 at the CCS sensor register G#0016 (PROBE, 2026-11-15): a Sony IMX712.
/// From its init: 4208 x 3120 RAW10 on four lanes, orientation 90 and RGGB in Camera2; CSI link 0 and the csis0 lanes (`rx::RX_START_AKITA_UW`, SYSREG select 1), its job (2026-11-18, isp-uw list 1) writing 16-bit samples, stored RAW10 packed at G#14C0 a line as the main's.
/// Its timing from its init: a line of 5192 clocks at 24.576 MHz / 3 x 122 (5.195 us), a frame of 3269 lines (16.98 ms), the integration 801 lines; the margin taken as the main's 2^6 until read.
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
    focus: (0, 0),
    streams: false,
    rx_start: &crate::rx::RX_START_AKITA_UW,
    rx_stop: &crate::rx::RX_STOP_FRONT,
    ack: &crate::FRAME_ACK,
    tick_us: 1 << 13,
    width: 4208,
    height: 3120,
    stride: 0x14C0,
    sizes: job::AKITA_UW_SLOT_BYTES,
    skip: job::AKITA_UW_SLOT_SKIP,
    line_ps: 5_194_992,
    llp: 5192,
    pll: (3, 122),
    fll: 3269,
    exposure_lines: 801,
    exposure_max_lines: 3269 - (1 << 6),
    cfa: cfa(0, 1, 1, 2),
    black: 64,
    white: 1023,
    rotation: 1,
    alts: &[],
    mode_live: false,
    array: &crate::ccs::AKITA_ARRAYS[1],
    optics: &crate::optics::AKITA_OPTICS[1],
};

/// akita's front camera (leshen): the ultrawide's module family, on hsi2c_2 at G#1A, CIS_CLK1 (the oscillator); it answers G#0712 at G#0016 (PROBE, 2026-11-15): a Sony IMX712 as the ultrawide.
/// Measured from Android's session: 4208 x 3072 RAW10 on four lanes, its image flipped both ways by its init (G#0101 = 3), CSI link 6 and the csis3 lanes (`rx::RX_START_AKITA_FRONT`, SYSREG select G#60), BGGR, orientation 270; its job writes 16-bit samples (format 2, a G#20E0 line), stored RAW10 packed (format 4, the line G#14C0) as the main's.
/// From its init: a line of 5192 clocks at 24.576 MHz / 3 x 119 (5.326 us), a frame of 3128 lines (16.66 ms), the integration 3118 lines; the margin under the frame taken as the main's 2^6 until the sensor's own is read.
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
    focus: (0, 0),
    streams: false,
    rx_start: &crate::rx::RX_START_AKITA_FRONT,
    rx_stop: &crate::rx::RX_STOP_AKITA_FRONT,
    ack: &crate::FRAME_ACK,
    tick_us: 1 << 13,
    width: 4208,
    height: 3072,
    stride: 0x14C0,
    sizes: job::AKITA_FRONT_SLOT_BYTES,
    skip: job::AKITA_FRONT_SLOT_SKIP,
    line_ps: 5_325_958,
    llp: 5192,
    pll: (3, 119),
    fll: 3128,
    exposure_lines: 3118,
    exposure_max_lines: 3128 - (1 << 6),
    cfa: cfa(2, 1, 1, 0),
    black: 64,
    white: 1023,
    rotation: 3,
    alts: &[],
    mode_live: false,
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
        // The captures' facts (2026-10-21, local): their 2 x 2 cfa tensors as cells, every level 64 / 1023, EXIF 6 a quarter turn and 8 three quarters, and each capture's size the readout.
        for (c, cell, turns, size) in [(&MAIN, [1, 2, 0, 1], 1, (4080, 3072)), (&ULTRAWIDE, [0, 1, 1, 2], 1, (4000, 3000)), (&FRONT, [1, 0, 2, 1], 3, (3840, 2736)), (&TELE, [1, 2, 0, 1], 1, (4032, 3024))] {
            assert_eq!(c.cfa, cfa(cell[0], cell[1], cell[2], cell[3]), "{}", c.name);
            assert_eq!((c.rotation, c.black, c.white, (c.width, c.height)), (turns, 64, 1023, size), "{}", c.name);
        }
        // akita's: ids in order, the main streaming (2026-11-19), and each id's bus husky's for the same id (the driver's grant slots are by id).
        for (k, c) in AKITA.iter().enumerate() {
            assert_eq!(c.id as usize, k);
            assert_eq!(c.streams, k == 0, "{}", c.name);
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
