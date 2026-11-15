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

/// The main camera (boitata), as the driver has streamed it since 2026-11-14: every field measured (docs/oracle/CAMERA.md).
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
/// From the Lumis captures of this module (cam 3, 8000 x 6000 quad Bayer, black 64 x 43 frames): the mosaic B G / G R once binned, black 64, white 1023; its colour order is not yet confirmed on a known scene (docs/oracle/CAMERA.md "Continuous frames and colour").
/// A guess: the rotation, the main's (both rear modules; Lumis's captures of it on 2026-10-18 say a half turn, on a rig whose orientation is not recorded).
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
    exposure_lines: 4124,
    exposure_max_lines: 5081 - (1 << 6),
    cfa: cfa(2, 1, 1, 0),
    black: 64,
    white: 1023,
    rotation: 1,
};

/// The front camera (dokkaebi, Samsung, G#30A1 at G#0000): its init, receiver (link 0) and job were captured on 2026-11-06, but its sensor has never answered ferros (docs/CAMERA.md "The other cameras"), so nothing here has run.
/// From the init: 3840 x 2736 RAW10 (stride G#12C0 in its front-end program), the integration 1712 lines in a 4001-line frame.
/// Its line time is a guess: the PLL's 2023 MHz over 8448 pixel clocks is 4.18 µs at one pixel a clock (60 frames a second) or 8.35 µs at a half (30, taken here); so the exposure is offered only up to the init's own until a run measures the frame rate.
/// Guesses: the mosaic (the main's, unmeasured), the rotation (three quarter turns, Android's usual for a front module), black 64 and white 1023 (RAW10 as the others); its acknowledgement and tick the ultrawide's, as the harness had them.
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
    line_ps: 8_350_202,
    exposure_lines: 1712,
    exposure_max_lines: 1712,
    cfa: cfa(1, 2, 0, 1),
    black: 64,
    white: 1023,
    rotation: 3,
};

/// The folded tele (kraken, Samsung, G#08D5 at G#0000 under Linux's boot probe): only its init is captured (target/cam/tele.init, 4032 x 3024 RAW10 from its CCS writes); its receiver and job need a capture of Android streaming it, which needs a bright scene (docs/oracle/CAMERA.md: in dim light the app's 5x is a crop of the main).
/// So it does not stream; the endpoint's PROBE powers it and reads its identity (docs/CAMERA.md "The other cameras").
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
    exposure_lines: 2232,
    exposure_max_lines: 2232,
    cfa: cfa(1, 2, 0, 1),
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
    /// Its region with an image ring of `n` buffers.
    pub const fn layout(&self, n: usize) -> job::Layout {
        job::ring(&self.sizes, &self.skip, n)
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
            assert!(c.width * 5 / 4 <= c.stride && c.height * c.stride <= job::IMAGE_BYTES as usize, "{}", c.name);
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
        assert_eq!(MAIN.binned(2), (2040, 1536));
        assert_eq!(ULTRAWIDE.binned(8), (500, 374));
        // The ultrawide's line: 9568 clocks at 24.576 MHz / 3 x 178.
        let ps = 9568u64 * 3 * 1_000_000_000_000 / (24_576_000 * 178);
        assert!(ULTRAWIDE.line_ps.abs_diff(ps) <= 1, "{ps}");
        // The front's guess: 8448 clocks at half of 24.576 MHz / 3 x 247.
        let ps = 8448u64 * 6 * 1_000_000_000_000 / (24_576_000 * 247);
        assert!(FRONT.line_ps.abs_diff(ps) <= 1, "{ps}");
    }
}
