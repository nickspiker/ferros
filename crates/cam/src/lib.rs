//! The camera's front end, the parts both the kernel's camera tools and the camera driver process need (docs/CAMERA.md "The driver"): the CSI receiver's tables (`rx`), the front end's per-frame acknowledgement (`FRAME_ACK`), the sensor init's records and the CPU's setup writes around the PDMA start (`inputs`), and the PDMA job moved into a region of the camera's own frames (`job`).
//! No allocation and no I/O: the driver reads its inputs from the vault and hands the bytes in.
//! Host-tested (`cargo test -p ferros_cam`), with the local job and bundle (target/cam, the vendor's settings, never in the repository) when they are on this machine.
#![cfg_attr(not(test), no_std)]

pub mod cams;
pub mod ccs;
pub mod geom;
pub mod inputs;
pub mod job;
#[cfg(any(feature = "list", test))]
pub mod list;
pub mod optics;
pub mod raw;
pub mod rx;

/// The front end's per-frame interrupt acknowledgement, in the order a camera open does it: read the word, write the same value straight back (docs/oracle/CAMERA.md "One frame reaches the front end").
/// Under Linux these look like bookkeeping, which is why the captured CPU program leaves them out: it stops deliberately "before the per-frame interrupt handling".
/// They are not bookkeeping: unacknowledged, the front end latches its first frame and never advances (2026-10-20).
pub const FRAME_ACK: [usize; 28] = [
    0x1C35_0160, 0x1C35_0178, 0x1C35_0190,
    0x1C32_008C,
    0x1C32_2004, 0x1C32_2024, 0x1C32_2038, 0x1C32_204C,
    0x1C32_0404, 0x1C32_0424, 0x1C32_0438, 0x1C32_044C,
    0x1C32_0110,
    0x1C35_0058, 0x1C35_0088, 0x1C35_00B8, 0x1C35_00E8, 0x1C35_0118, 0x1C35_01AC,
    0x1C33_1300, 0x1C33_3300, 0x1C33_5300, 0x1C33_7300, 0x1C33_9300, 0x1C33_B300, 0x1C33_C300, 0x1C33_D300, 0x1C33_E300,
];

/// The front end's aggregate interrupt word (csis_core_cmn_ctl): loch0's image path at bit 0, its PD path at bit 7 (docs/oracle/CAMERA.md "From the ground up").
pub const CMN_STATUS: usize = 0x1C32_008C;
/// loch0's image path status (isp_fe_loch0_csis_core_img_proc, zuma-isp-event-info.dtsi): SOF bit 0, EOF bit 1, SHADOW_UPDATE bit 3, DMA_DONE bit 4.
pub const IMG_STATUS: usize = 0x1C32_0424;
pub const IMG_SOF: u32 = 1 << 0;
pub const IMG_EOF: u32 = 1 << 1;
pub const IMG_SHADOW: u32 = 1 << 3;
pub const IMG_DMA_DONE: u32 = 1 << 4;
/// The core's buffer index: counts the frames it took (+G#49C of the image path).
pub const IMG_INDEX: usize = 0x1C32_049C;
/// loch0's path enable: G#81 (image bit 0, PD bit 7), which Linux writes right after acknowledging the first SHADOW_UPDATE (docs/oracle/CAMERA.md "First frame"). A plain write, not an acknowledgement.
pub const LOCH_ENABLE: usize = 0x1C32_00A0;
pub const LOCH_ENABLE_IMG_PD: u32 = 0x81;

/// The PDMA engine (G#1C39_0000) and its registers, in the order a camera open drives them (oracle capture 2026-10-19): +G#2C = 1, +G#40 = G#1F, +G#20 = 0, +4 the ring's base (low), +8 its high half, +C the count, +G#10 the descriptor index, +0 = 1 start, +G#34 = 0, +G#28 = 1.
pub const PDMA: usize = 0x1C39_0000;
pub const PDMA_CTRL: usize = 0x00;
pub const PDMA_BASE_LO: usize = 0x04;
pub const PDMA_BASE_HI: usize = 0x08;
pub const PDMA_COUNT: usize = 0x0C;
pub const PDMA_INDEX: usize = 0x10;

/// The main sensor's line time at its init's 6320-clock line: 3.1489 us, the PLL at 24.576 MHz / 3 x 245 (2007.04 MHz), measured on the Pro (b933, 2026-11-17) from the long-exposure ladder's frame spacings (16.004 s over 65487 lines of 2^6 at 7664 clocks) and the init's frames 67.3 ms apart (21362 lines). Until then 3.2237 us, read in October from a 24 MHz reference and a one-frame-resolution count (30 frames in 2.034 s): 2.3 % long (docs/CAMERA.md "Long exposures").
pub const MAIN_LINE_PS: u64 = 3_148_916;
/// The init's frame length in lines (CCS frame_length_lines G#0340 = G#5372) and its longest integration (G#535E, 21342 lines): the exposure cannot pass the frame.
pub const MAIN_FRAME_LINES: u64 = 0x5372;
pub const MAIN_EXPOSURE_MAX_LINES: u64 = 0x535E;
/// The least blanking (lines past a readout's rows) the driver streams a frame with: a slit's default, and the floor of a frame length SET (payloads/camera `BLANK`); the ultrawide's whole frames need about this many (2026-11-16: 32 gave no frame, 128 ran 48 a second). The list's `fll_least` is a readout's rows and this.
pub const BLANK_LEAST: u16 = 1 << 7;
