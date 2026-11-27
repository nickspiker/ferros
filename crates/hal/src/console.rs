//! Framebuffer console — text rendering on a linear XRGB8888 framebuffer.
//!
//! Uses the glass console's 8x16 bitmap font: printable ASCII, and tatau's dozenal digits (two cells each), radix point and sign arrows (ferros_glass `cells_at`). All output is hex: no decimal formatting, no `core::fmt` bloat.

/// The 8x16 bitmap font lives in ferros_glass (one copy for this console and the glass console).
use ferros_glass::{GLYPH_H, GLYPH_W};

/// Framebuffer console state.
pub struct Console {
    fb: *mut u32,
    width: usize,
    height: usize,
    stride: usize, // in pixels (not bytes)
    x_off: usize,  // left margin in pixels
    y_off: usize,  // top margin in pixels
    col: usize,    // current text column (pixel X / 8)
    row: usize,    // current text row (pixel Y / 16)
    cols: usize,   // max columns (within margins)
    rows: usize,   // max rows (within margins)
    fg: u32,       // foreground color (XRGB)
    bg: u32,       // background color (XRGB)
    scale: usize,  // glyph magnification (each font pixel becomes scale x scale)
}

impl Console {
    /// Create a new console on a framebuffer with margins for rounded corners/notch.
    ///
    /// # Safety
    /// `fb` must point to a valid XRGB8888 framebuffer of at least `stride * height` pixels.
    pub const unsafe fn new(
        fb: *mut u32,
        width: usize,
        height: usize,
        stride: usize,
        fg: u32,
        bg: u32,
        margins: (usize, usize, usize, usize), // (top, right, bottom, left)
    ) -> Self {
        let (top, right, bottom, left) = margins;
        let text_w = width - left - right;
        let text_h = height - top - bottom;
        Self {
            fb,
            width,
            height,
            stride,
            x_off: left,
            y_off: top,
            col: 0,
            row: 0,
            cols: text_w / (GLYPH_W * Self::SCALE),
            rows: text_h / (GLYPH_H * Self::SCALE),
            fg,
            bg,
            scale: Self::SCALE,
        }
    }

    /// Change the glyph magnification; the cursor is kept and the text grid recomputed.
    pub fn set_scale(&mut self, scale: usize) {
        let scale = scale.max(1);
        self.cols = self.cols * GLYPH_W * self.scale / (GLYPH_W * scale);
        self.rows = self.rows * GLYPH_H * self.scale / (GLYPH_H * scale);
        self.scale = scale;
    }

    /// Change the foreground color for the text that follows.
    pub fn set_fg(&mut self, fg: u32) {
        self.fg = fg;
    }

    /// Move the cursor to a text position.
    pub fn goto(&mut self, col: usize, row: usize) {
        self.col = col.min(self.cols);
        self.row = row.min(self.rows.saturating_sub(1));
    }

    /// Clear the entire screen with the background color.
    pub fn clear(&mut self) {
        for y in 0..self.height {
            for x in 0..self.width {
                unsafe {
                    self.fb.add(y * self.stride + x).write_volatile(self.bg);
                }
            }
        }
        self.col = 0;
        self.row = 0;
    }

    /// Default glyph magnification (2 = double size); `set_scale` changes it per console.
    const SCALE: usize = 2;

    /// Draw a single character at text position (col, row) within the margin area.

    fn draw_char(&self, cell: u8, col: usize, row: usize) {
        let glyph = ferros_glass::glyph(cell);

        let s = self.scale;
        let px = self.x_off + col * GLYPH_W * s;
        let py = self.y_off + row * GLYPH_H * s;

        for gy in 0..GLYPH_H {
            let bits = glyph[gy];
            for sy in 0..s {
                let y = py + gy * s + sy;
                if y >= self.height { return; }
                for gx in 0..GLYPH_W {
                    let pixel = if (bits >> (7 - gx)) & 1 != 0 { self.fg } else { self.bg };
                    for sx in 0..s {
                        let x = px + gx * s + sx;
                        if x >= self.width { break; }
                        unsafe { self.fb.add(y * self.stride + x).write_volatile(pixel); }
                    }
                }
            }
        }
    }

    /// Scroll the text area up by one text row (within margins).
    fn scroll_up(&mut self) {
        let line_h = GLYPH_H * self.scale;
        let text_h = self.rows * GLYPH_H * self.scale;
        let x_end = self.x_off + self.cols * GLYPH_W * self.scale;
        // Move rows 1..rows up to 0..rows-1
        for y in 0..(text_h - line_h) {
            let src_y = self.y_off + y + line_h;
            let dst_y = self.y_off + y;
            for x in self.x_off..x_end {
                let pixel = unsafe {
                    self.fb.add(src_y * self.stride + x).read_volatile()
                };
                unsafe {
                    self.fb.add(dst_y * self.stride + x).write_volatile(pixel);
                }
            }
        }
        // Clear the last row
        let last_row_y = self.y_off + text_h - line_h;
        for y in last_row_y..(last_row_y + line_h) {
            for x in self.x_off..x_end {
                unsafe {
                    self.fb.add(y * self.stride + x).write_volatile(self.bg);
                }
            }
        }
    }

    /// Write a single byte to the console (handles newline, scroll).
    pub fn putc(&mut self, ch: u8) {
        match ch {
            b'\n' => {
                self.col = 0;
                self.row += 1;
                if self.row >= self.rows {
                    self.row = self.rows - 1;
                    self.scroll_up();
                }
            }
            b'\r' => {
                self.col = 0;
            }
            _ => {
                let (c, n, _) = ferros_glass::cells_at(&[ch], 0);
                self.cells(&c[..n]);
            }
        }
    }

    /// One character's cells (one, or two for a dozenal digit), on the next line when they do not fit the rest of this one.
    fn cells(&mut self, c: &[u8]) {
        if c.is_empty() {
            return;
        }
        if self.col + c.len() > self.cols {
            self.col = 0;
            self.row += 1;
            if self.row >= self.rows {
                self.row = self.rows - 1;
                self.scroll_up();
            }
        }
        for &cell in c {
            self.draw_char(cell, self.col, self.row);
            self.col += 1;
        }
    }

    /// Write a string to the console: each character its cells, so tatau's point and arrows (two and three bytes in UTF-8) draw as one cell each.
    pub fn puts(&mut self, s: &str) {
        let b = s.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if matches!(b[i], b'\n' | b'\r') {
                self.putc(b[i]);
                i += 1;
                continue;
            }
            let (c, n, len) = ferros_glass::cells_at(b, i);
            self.cells(&c[..n]);
            i += len;
        }
    }

    /// Write a u64 as hex.
    pub fn put_hex(&mut self, val: u64) {
        if val == 0 { self.putc(b'0'); return; }
        let mut started = false;
        for i in (0..16).rev() {
            let nibble = ((val >> (i * 4)) & 0xF) as u8;
            if nibble != 0 || started {
                let c = if nibble < 10 { b'0' + nibble } else { b'A' + nibble - 10 };
                self.putc(c);
                started = true;
            }
        }
    }

    /// Write a u32 as hex, no leading zeros.
    pub fn put_hex32(&mut self, val: u32) {
        if val == 0 { self.putc(b'0'); return; }
        let mut started = false;
        for i in (0..8).rev() {
            let nibble = ((val >> (i * 4)) & 0xF) as u8;
            if nibble != 0 || started {
                let c = if nibble < 10 { b'0' + nibble } else { b'A' + nibble - 10 };
                self.putc(c);
                started = true;
            }
        }
    }
}

// Console is used from a single-core kernel — no Send/Sync needed for safety, but we need it for static storage.
unsafe impl Send for Console {}
unsafe impl Sync for Console {}
