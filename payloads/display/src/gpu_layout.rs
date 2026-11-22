//! The compose on the GPU (docs/GPU-COMPOSE.md, step 2, 2026-11-21): the display's GPU block's map and the compose kernel's uniform slots, shared by the device (gpu.rs) and the kernel's recording on the host (gpu_kernel.rs, run by build.rs).
//!
//! Step 2's frame: today's windows (op 22, each a single surface, `BAKED`, Normal, none overlapping another), the glass and the bars in front of them as one layer the CPU makes (`FRONT_AT`), presented into one phase a job. Everything else (formats 0 and 1, blend modes, overlapping windows, the capture, the hit map's view) stays NEON's.

/// Where each buffer lies in the block (bytes from its start; the GPU sees the block at `ferros_sys::gpu::ARENA_VA`).
/// The queue: a header page and 2^2 records.
pub const Q_AT: usize = 0;
pub const Q_DEPTH_LOG2: u32 = 2;
/// The kernel's words (2^16 bytes of room).
pub const K_AT: usize = 64 << 10;
#[allow(dead_code)]
pub const K_ROOM: usize = 1 << 16;
/// The present's blue-noise thresholds (neon.rs `BLUE`), a byte each, 16 × 16.
pub const BLUE_AT: usize = 128 << 10;
/// The front layer: the glass's edge, the camera hole's knockout and the bars, composed front to back once on the CPU (gpu.rs `front_row`), a u64 a pixel at the screen's width: the channels so far (bits 0-47), the no-dither bit (48), and what of the scene still shows (bits 49-63, Q14, 0 to 2^14), or `PASS` where nothing is in front.
pub const FRONT_AT: usize = 1 << 20;
/// The check's output (op 86's `4`): one phase's words, a u32 a pixel.
pub const CHECK_AT: usize = 33 << 20;
/// The block's size, log 2: 64 MiB (GPU-COMPOSE.md knob 20).
pub const BLOCK_LOG2: u32 = 26;

/// A front pixel with nothing in front of the scene: the scene goes out as it came, its own no-dither bit kept (main.rs `glass_front`'s early return).
pub const PASS: u64 = 0x7FFF << 49;

/// The windows a job composes: at most this many, none overlapping (`gpu.rs` `eligible`).
pub const WINDOWS: usize = 4;

/// The kernel's uniform slots (the caller's: the kernel's own constants follow from `RESERVED`).
pub mod slot {
    /// The phase's word 0 (the job's phase, or the check's buffer).
    pub const PHASE: u8 = 0;
    /// The front layer's pixel 0.
    pub const FRONT: u8 = 1;
    /// The thresholds.
    pub const BLUE: u8 = 2;
    /// [x0 | y0 << 16][x1 | y1 << 16]: the owed rect.
    pub const RECT: u8 = 3;
    /// [16-pixel blocks a row][1 / that, f32 bits]: the grid's shape (a warp a block, rows of blocks one after another).
    pub const GRID: u8 = 4;
    /// [the screen's width][the phase's turn of the thresholds (neon.rs `TURN`) | the channels' masks << 8 (a byte each, R G B: G#FF on, 0 off)].
    pub const SCREEN: u8 = 5;
    /// Window w's four slots from `WIN + 4 w`: its pixel 0's address, [x0 | y0 << 16][x1 | y1 << 16] its rect, [x0 | x1 << 16][y0 | y1 << 16] its ring's rect, [the ring's origin][bit 0 opaque].
    pub const WIN: u8 = 6;
    pub const RESERVED: u8 = WIN + 4 * super::WINDOWS as u8;
}

/// The layered window's kernel's uniform slots (gpu_layers.rs, step 4).
#[allow(dead_code)]
pub mod lslot {
    /// As the step 2 kernel's: the phase, the front layer, the thresholds, the owed rect, [the screen's width][the turn | the channels' masks << 8].
    pub const PHASE: u8 = 0;
    pub const FRONT: u8 = 1;
    pub const BLUE: u8 = 2;
    pub const RECT: u8 = 3;
    pub const SCREEN: u8 = 4;
    /// [first side: 0 right of the axis, 1 left][rows a side]: the grid, a workgroup a row a side.
    pub const SIDE: u8 = 5;
    /// [steps right][steps left]: how far each side reaches from the axis (to the owed rect's far edge on that side).
    pub const STEPS: u8 = 6;
    /// The noise: [its axis column][its scroll, texture rows], [the texture's height][its frame, the twinkle], the walk's mask, speckle and base (16-bit lanes, fluor's `widen_mask` and `widen_base`), [shimmer, 16-bit blue][0], [x0 | y0 << 16][x1 | y1 << 16] its clip.
    pub const AXIS: u8 = 7;
    pub const TEX: u8 = 8;
    pub const MASK: u8 = 9;
    pub const SPECKLE: u8 = 10;
    pub const BASE: u8 = 11;
    pub const SHIMMER: u8 = 12;
    pub const CLIP: u8 = 13;
    /// The bake's tables (bake.rs `Bake::light`, f16 a 16-bit code; `Bake::code`, 2^12 + 2 f32) and its ODT over the light table's scale, nine f32 in slots 16 to 20.
    pub const LIGHT: u8 = 14;
    pub const CODE: u8 = 15;
    pub const ODT: u8 = 16;
    /// Pixel layer l's four slots from `LAYER + 4 l`, as the step 2 kernel's windows: its pixel 0, its rect, its ring's rect, [origin][bit 0 opaque].
    pub const LAYER: u8 = 21;
    pub const RESERVED: u8 = LAYER + 4 * 4;
}
