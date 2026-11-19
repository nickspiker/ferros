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
};

/// The ultrawide (oksoko, Sony IMX787), as the bring-up harness streamed it on 2026-10-22 (4000 x 3000 RAW10, about 30 frames a second, every frame landing).
/// Measured: the bus, address and identity (G#0787 at the CCS sensor register G#0016), the stream writes (G#0100 alone, 8-bit), the receiver (link 2), the acknowledgement, the tick, the geometry (stride G#13C0, its front-end program's), the outputs (`job::UW_SLOT_BYTES`).
/// From the init, not measured: the line time, 9568 pixel clocks (`line_length_pck`) at the PLL's 1458.2 MHz (24.576 MHz / 3 x 178), 6.5616 µs, which makes its 5081-line frame 33.3 ms, the 30 a second seen; the integration, 4124 lines; the longest offered, the frame less 2^6 lines (Sony's margin is not read).
/// From its capture (cam 3, 4000 x 3000, the binned mode ferros's init sets): cfa B G / G R, black 64, white 1023, orientation 6, so a quarter turn as the main; derived, its init's crop start even and its flip (G#0101 = 3) the HAL's. Not yet seen on a ferros frame. (Lumis's 2026-10-18 captures of it, 8000 x 6000 quad Bayer at orientation 3, were the full-resolution mode with the phone held sideways.)
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
    cfa: cfa(2, 1, 1, 0),
    black: 64,
    white: 1023,
    rotation: 1,
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
};

/// The folded tele (kraken, Samsung, G#08D5 at G#0000 under Linux's boot probe): only its init is captured (target/cam/tele.init, 4032 x 3024 RAW10 from its CCS writes); its receiver and job need a capture of Android streaming it, which needs a bright scene (docs/oracle/CAMERA.md: in dim light the app's 5x is a crop of the main).
/// So it does not stream; the endpoint's PROBE powers it and reads its identity (docs/CAMERA.md "The other cameras").
/// From its capture (cam 4, 4032 x 3024, the size its init sets): cfa B G / G R, black 64, white 1023, orientation 6 (a quarter turn); derived, its init's crop start even, its flip in no captured write (its stream-on was never captured).
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
    streams: false,
    rx_start: &[],
    rx_stop: &[],
    ack: &crate::FRAME_ACK,
    tick_us: 1 << 13,
    width: 4032,
    height: 3024,
    stride: 0x13B0,
    sizes: [0; SLOTS],
    skip: job::NO_SKIP,
    line_ps: 0,
    llp: 5568,
    pll: (4, 0x173),
    fll: 5457,
    exposure_lines: 2232,
    exposure_max_lines: 2232,
    cfa: cfa(2, 1, 1, 0),
    black: 64,
    white: 1023,
    rotation: 1,
};

/// Every camera, by id.
pub const CAMS: [&Cam; 4] = [&MAIN, &ULTRAWIDE, &FRONT, &TELE];

/// The camera with this id.
/// WHY `get`: the id is the caller's byte (an OPEN or PROBE on the endpoint), so one past the table is answered "no such camera", not indexed.
pub fn by_id(id: u8) -> Option<&'static Cam> {
    CAMS.get(id as usize).copied()
}

/// The widest readout, the most rows and the longest line of any camera that streams: the driver's buffers are sized for them.
pub const W_MOST: usize = 4080;
pub const H_MOST: usize = 3072;
pub const STRIDE_MOST: usize = 0x1400;

impl Cam {
    /// An image buffer for its captured readout: its rows of `stride` bytes in whole frames of the pool (the Pro's main 16 MiB; a 1:1 mode or a slit sizes its own from the mode, ferros_cam job `buffer_bytes`, `slit_bytes`; the 8a's main, 4624 x 3472 at G#1700, 22 MiB). One constant (`job::IMAGE_BYTES`) held every camera until 2026-11-15.
    pub const fn buffer_bytes(&self) -> u64 {
        job::buffer_bytes(self.height as u64, self.stride as u64)
    }
    /// Its region with an image ring of `n` buffers of its captured readout.
    pub const fn layout(&self, n: usize) -> job::Layout {
        job::with_image(&self.sizes, &self.skip, n as u64 * self.buffer_bytes())
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
        for (c, cell, turns, size) in [(&MAIN, [1, 2, 0, 1], 1, (4080, 3072)), (&ULTRAWIDE, [2, 1, 1, 0], 1, (4000, 3000)), (&FRONT, [1, 0, 2, 1], 3, (3840, 2736)), (&TELE, [2, 1, 1, 0], 1, (4032, 3024))] {
            assert_eq!(c.cfa, cfa(cell[0], cell[1], cell[2], cell[3]), "{}", c.name);
            assert_eq!((c.rotation, c.black, c.white, (c.width, c.height)), (turns, 64, 1023, size), "{}", c.name);
        }
        assert_eq!(MAIN.binned(2), (2040, 1536));
        assert_eq!(ULTRAWIDE.binned(8), (500, 374));
        // The ultrawide's line: 9568 clocks at 24.576 MHz / 3 x 178.
        let ps = 9568u64 * 3 * 1_000_000_000_000 / (24_576_000 * 178);
        assert!(ULTRAWIDE.line_ps.abs_diff(ps) <= 1, "{ps}");
        // The front's line: 8448 clocks at 24.576 MHz / 3 x 247 over vt_pix_clk_div 5, two pixels a clock (measured: 41.66 to 41.85 ms a 4001-line frame).
        let ps = 8448u64 * 3 * 5 * 1_000_000_000_000 / (2 * 24_576_000 * 247);
        assert!(FRONT.line_ps.abs_diff(ps) <= 1, "{ps}");
    }
}
