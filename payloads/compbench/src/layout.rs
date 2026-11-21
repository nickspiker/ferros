//! The bench's frame and its block's map, shared by the device bench (main.rs), the kernels' recording (kernel.rs, run by build.rs on the host) and the host check (tools/compbench-check).
//!
//! The frame is the owner's question of 2026-11-21: one full 1344 × 2992 frame composed front to back with fluor's `under`, three layers: chrome (a small opaque and antialiased layer at the top), one scrolling pane (an α + darkness buffer taller than the screen, read at a vertical offset) and fluor's background noise generated under whatever is not opaque.

/// The Pro's panel.
pub const W: usize = 1344;
pub const H: usize = 2992;
/// The noise's mirror axis (fluor `background_noise`'s: the frame's centre column).
pub const AXIS: usize = W / 2;
/// The chrome's rows, from the top.
pub const CHROME_ROWS: usize = 1 << 8;
/// The pane's rows: the screen's and 2^10 more to scroll thru.
pub const PANE_ROWS: usize = H + (1 << 10);

/// Where each buffer lies in the block (bytes from its start; the GPU sees the block at `ferros_sys::gpu::ARENA_VA`).
pub const Q_AT: usize = 0;
pub const K_AT: usize = 1 << 20;
/// Each kernel's room for its words.
pub const K_ROOM: usize = 1 << 16;
/// Texture row t's walk seed (fluor noise.rs `seed`), a u64 a row: worked out on the CPU once a frame height, since the GPU has no 64-bit multiply.
pub const SEED_AT: usize = 2 << 20;
/// The bake's light table: a 16-bit g2t code to its light, f16 bits (the display's bake.rs `Bake::light`).
pub const LIGHT_AT: usize = (2 << 20) + (64 << 10);
/// The bake's code table: √p in 2^12 steps to the panel's Q16 code, f32 (bake.rs `Bake::code`), 2^12 + 2 entries.
pub const CODE_AT: usize = (2 << 20) + (256 << 10);
pub const CODE_STEPS: usize = 1 << 12;
/// The present's blue-noise thresholds (the display's neon.rs `BLUE`), a byte each, 16 × 16.
pub const BLUE_AT: usize = (2 << 20) + (512 << 10);
/// Results the bench keeps for itself.
pub const SCRATCH_AT: usize = 3 << 20;
pub const CHROME_AT: usize = 4 << 20;
pub const PANE_AT: usize = 8 << 20;
/// The GPU's output: G2t16 or baked pixels, a full screen.
pub const OUT_AT: usize = 64 << 20;
/// The NEON path's canvas (fluor's G2t16) and its baked surface.
pub const CANVAS_AT: usize = 96 << 20;
pub const BAKED_AT: usize = 128 << 20;
/// The four temporal phases' words (the display's layout: a u32 a pixel, a full screen each).
pub const PHASES_AT: usize = 160 << 20;
pub const PHASE_BYTES: usize = W * H * 4;
/// The block's size, log 2.
pub const BLOCK_LOG2: u32 = 28;

const _: () = assert!(CHROME_AT + W * CHROME_ROWS * 8 <= PANE_AT && PANE_AT + W * PANE_ROWS * 8 <= OUT_AT && OUT_AT + W * H * 8 <= CANVAS_AT && CANVAS_AT + W * H * 8 <= BAKED_AT && BAKED_AT + W * H * 8 <= PHASES_AT && PHASES_AT + 4 * PHASE_BYTES <= 1 << BLOCK_LOG2);

/// The kernels' uniform slots (the caller's: the kernel's own constants follow from `RESERVED`).
pub mod slot {
    pub const OUT: u8 = 0;
    pub const PANE: u8 = 1;
    pub const CHROME: u8 = 2;
    pub const SEED: u8 = 3;
    pub const LIGHT: u8 = 4;
    pub const CODE: u8 = 5;
    pub const BLUE: u8 = 6;
    /// Phases 0-3: slots 7-10.
    pub const PHASE: u8 = 7;
    /// [scroll u32][pane offset u32]
    pub const SCROLL: u8 = 11;
    /// [first row y0 u32][rows a side, padded to 16, u32]
    pub const ROWS: u8 = 12;
    /// [x0 u32][x1 u32]: the columns stored (the clip kernels).
    pub const COLS: u8 = 13;
    /// [first side u32: 0 right, 1 left][rows u32]
    pub const SIDE: u8 = 14;
    /// [steps right u32][steps left u32]: how far each side walks from the axis.
    pub const STEPS: u8 = 15;
    pub const MASK: u8 = 16;
    pub const SPECKLE: u8 = 17;
    pub const BASE: u8 = 18;
    pub const STIR_RIGHT: u8 = 19;
    pub const STIR_LEFT: u8 = 20;
    /// The ODT over 2^14 (the light table's scale), nine f32 by rows, slots 21-25.
    pub const ODT: u8 = 21;
    pub const RESERVED: u8 = 26;
}

/// fluor noise.rs's walk constants (copied: fluor's are private): the step constant, the step's draw, its down draw's shift.
pub const STIR: u64 = 12_345_678_942;
pub const STEP: u64 = 0x0000_01FF_01FF_01FF;
pub const DOWN: u32 = 9;
pub const SPECKLE_BELOW: u64 = u64::MAX >> 8;

/// fluor noise.rs `widen_mask`: a theme mask (a byte a channel) as the walk's 16-bit lanes.
pub fn widen_mask(m8: u32) -> u64 {
    let lane = |s: u32| ((((m8 >> s) & 0xFF) as u64) << 8) | 0xFF;
    (lane(16) << 32) | (lane(8) << 16) | lane(0)
}

/// fluor noise.rs `widen_base`.
pub fn widen_base(b8: u32) -> u64 {
    let lane = |s: u32| (((b8 >> s) & 0xFF) as u64) << 8;
    (lane(16) << 32) | (lane(8) << 16) | lane(0)
}

/// fluor noise.rs `seed`: texture row t's generator in a texture `height` rows tall.
pub fn seed(t: usize, height: usize) -> u64 {
    0xDEAD_BEEF_0123_4567u64 ^ (t as u64).wrapping_sub(height as u64 / 2).wrapping_mul(0x9E37_79B9_4517_B397)
}

/// The chrome layer's pixel (G2t16, premultiplied as a canvas holds it after drawing): an opaque bar over its first 3/4 with glyph-like see-through and antialiased cells, then a shadow whose α falls to nothing over its last quarter.
pub fn chrome_px(x: usize, y: usize) -> u64 {
    let one = 0xFFFFu64;
    let (a, d) = if y < CHROME_ROWS * 3 / 4 {
        // Cells of 16 × 16: most opaque, a glyph's gaps empty, its edges partly covered.
        let c = ((x >> 4) * 7 + (y >> 4) * 13) & 7;
        match c {
            0 => (0, 0),
            1 | 2 => (one / 2 + ((x as u64 * 97) & 0x3FFF), 0x1000 + ((y as u64 * 31) & 0x0FFF)),
            _ => (one, 0x2000 + ((x as u64 * 5 + y as u64 * 3) & 0x7FFF)),
        }
    } else {
        let fall = (CHROME_ROWS - y) as u64;
        let a = fall * (one / (CHROME_ROWS as u64 / 4));
        (a, a >> 1)
    };
    let d = d.min(a);
    a << 48 | d << 32 | (d >> 1) << 16 | (d >> 2)
}

/// The pane's pixel at pane row r (G2t16, straight darkness under its α, as a layer below the canvas is taken): cards of 2^9 rows, 448 of them body and the rest a gap, inset 2^5 + 2^4 columns each side; even cards opaque, odd ones three quarters covered.
pub fn pane_px(x: usize, r: usize) -> u64 {
    let inside = (r & 511) < 448 && x >= 48 && x < W - 48;
    if !inside {
        return 0;
    }
    let a: u64 = if (r >> 9) & 1 == 0 { 0xFFFF } else { 0xC000 };
    let d = |k: usize| ((x * (31 + k) + r * (7 + 2 * k)) & 0x7FFF) as u64 + 0x4000;
    a << 48 | d(0) << 32 | d(1) << 16 | d(2)
}
