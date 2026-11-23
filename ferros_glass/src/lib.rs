//! The glass console: a few lines of text straight onto whatever buffer the panel scans, for when nothing else can put them there.
//! The kernel's panic screen draws with it, so does a process that died while no display served, and a boot that stalled (ferros_kernel glass.rs).
//!
//! - **Font:** the 8x16 bitmap font (`font.rs`), each font pixel a square of `scale` panel pixels, where `scale` is the span (2wh/(w+h)) over 2^9: 3 on the Pro (24x48 cells, 51 columns), 2 on the 8a (16x32, 61 columns).
//! - **Numbers:** tatau's twelve dozenal digits (U+0010-U+001B), its radix point and sign arrows, rasterised from Oxanium +glyphs at build time into the same grid (build.rs); a dozenal digit takes two cells.
//!   A line's numbers are typed (`Num`, `Say`), so the glass writes them in the device's base while the developer's copy of the same line (the console, the flash log) keeps `A#`/`G#`.
//!   Never a fixed pixel size (AGENT.md "Dimensional Units").
//! - **Wrap:** words wrap at the grid's width; a word that does not fit the rest of a line starts the next one; a word longer than a whole line is cut at every line's end.
//! - **Line returns:** a `\n` ends the line where it stands; a line exactly full followed by a `\n` makes one line, not a line and an empty one.
//! - **Spaces:** a space at a line's end is dropped (it would start the next line indented); spaces after a manual return are kept (indentation).
//! - **Down to the bottom:** lines fill the grid row by row to its last row, the bottom margin above the rounded corners.
//! - **Overrun:** when the text needs more rows than the grid has, one row says `...` and the caller picks which end stays (`Keep`): the earliest lines (a panic: its headline and message come first) or the latest (the kernel's notices, appended as they happen).
//! - **Bytes:** printable ASCII is drawn as is; tatau's digits, point and arrows as their cells; a tab is a space; a carriage return is nothing; each other character (a UTF-8 sequence counts once, by its lead byte) is a `?` cell.
//!
//! No allocation, no locks, and no input makes it panic or reach outside the grid: a panic path draws with it.

#![no_std]

mod font;
pub use font::FONT;

pub const GLYPH_W: usize = 8;
pub const GLYPH_H: usize = 16;

mod num;
pub use num::{Dev, Num, Person, Say};

// The dozenal digits' halves, the radix point and the sign arrows (build.rs).
include!(concat!(env!("OUT_DIR"), "/dozenal.rs"));

/// The cells past ASCII, by id: dozenal digit d's left half `DOZENAL + 2d` and its right half the next, then the point, the up arrow and the down arrow (`EXTRA`'s order).
/// Cell ids are what `lay` hands its callback, never text.
pub const DOZENAL: u8 = 0x80;
pub const POINT: u8 = DOZENAL + 24;
pub const UP: u8 = DOZENAL + 25;
pub const DOWN: u8 = DOZENAL + 26;

/// The cells the character at `text[i]` puts on the grid, how many (0, 1 or 2), and how many bytes it takes: printable ASCII itself; tatau's dozenal digits (U+0010-U+001B, one byte each in UTF-8) two cells, their halves; the radix point and the sign arrows one; a tab a space; nothing for a carriage return or a UTF-8 continuation byte (its character already counted at its lead byte); `?` for anything else.
/// `i` must be inside `text`.
pub fn cells_at(text: &[u8], i: usize) -> ([u8; 2], usize, usize) {
    let b = text[i];
    match b {
        0x20..=0x7E => ([b, 0], 1, 1),
        0x10..=0x1B => {
            let left = DOZENAL + 2 * (b - 0x10);
            ([left, left + 1], 2, 1)
        }
        b'\t' => ([b' ', 0], 1, 1),
        b'\r' | 0x80..=0xBF => ([0, 0], 0, 1),
        _ => {
            let rest = &text[i..];
            if rest.starts_with("\u{B7}".as_bytes()) {
                ([POINT, 0], 1, 2)
            } else if rest.starts_with("\u{2191}".as_bytes()) {
                ([UP, 0], 1, 3)
            } else if rest.starts_with("\u{2193}".as_bytes()) {
                ([DOWN, 0], 1, 3)
            } else {
                ([b'?', 0], 1, 1)
            }
        }
    }
}

/// The glyph a cell id (from `cells_at`) draws.
pub fn glyph(c: u8) -> &'static [u8; GLYPH_H] {
    match c {
        0x20..=0x7E => &FONT[(c - 0x20) as usize],
        DOZENAL..=DOWN => &EXTRA[(c - DOZENAL) as usize],
        // `cells_at` gives no other id; any other draws `?` rather than reading past a table.
        _ => &FONT[(b'?' - 0x20) as usize],
    }
}

/// Which end of a text that needs more rows than the grid has stays on the glass.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Keep {
    /// The first lines, the last row `...`: a panic, whose headline and message come first.
    Earliest,
    /// The last lines, the first row `...`: notices appended over time, the newest the one that matters.
    Latest,
}

/// The text laid out `cols` cells wide, from its first byte: `emit(line, col, cell byte, head)` for every cell, `head` true until the first manual return (the headline).
/// Returns the number of lines the text takes.
/// `cols` must be at least 1 (`lay` makes sure).
fn walk(text: &[u8], cols: usize, mut emit: impl FnMut(usize, usize, u8, bool)) -> usize {
    let (mut line, mut col, mut head) = (0usize, 0usize, true);
    let mut i = 0;
    while i < text.len() {
        match text[i] {
            b'\n' => {
                line += 1;
                col = 0;
                head = false;
                i += 1;
            }
            b' ' | b'\t' => {
                // A space past the line's last cell is dropped: it would indent the next line.
                if col < cols {
                    emit(line, col, b' ', head);
                    col += 1;
                }
                i += 1;
            }
            _ => {
                // The word: up to the next space or return, its width in cells.
                // A multi-byte character's bytes are all G#80 or above, so none is a space or a return and `j` never stops inside one.
                let mut j = i;
                let mut w = 0;
                while j < text.len() && !matches!(text[j], b'\n' | b' ' | b'\t') {
                    let (_, n, len) = cells_at(text, j);
                    w += n;
                    j += len;
                }
                // Not fitting the rest of the line: the next line, whether or not it fits a whole one.
                if col > 0 && col + w > cols {
                    line += 1;
                    col = 0;
                }
                while i < j {
                    let (c, n, len) = cells_at(text, i);
                    i += len;
                    // WHY: a wide character (a dozenal digit, two cells) in a grid one column wide has room for its left half only; `lay` promises no cell at or past `cols`.
                    let n = if n > cols { cols } else { n };
                    if n == 0 {
                        continue;
                    }
                    // Longer than a line: cut at each line's end, a wide character never split across two.
                    if col + n > cols {
                        line += 1;
                        col = 0;
                    }
                    for &cell in &c[..n] {
                        emit(line, col, cell, head);
                        col += 1;
                    }
                }
            }
        }
    }
    // The last line counts when it holds a cell; a final return ends a line, it does not start an empty one.
    line + (col > 0) as usize
}

/// The rows `text` takes in a grid `cols` wide and `rows` tall: all its lines, or the whole grid when it runs over.
pub fn rows_used(text: &[u8], cols: usize, rows: usize) -> usize {
    // WHY: a grid with no columns lays out nothing (and `walk` would put every cell on a new line at column 0).
    if cols == 0 {
        return 0;
    }
    let n = walk(text, cols, |_, _, _, _| {});
    if n > rows { rows } else { n }
}

/// Lay `text` out in a grid `cols` wide and `rows` tall: `cell(row, col, byte, head)` for every cell shown, never a row at or past `rows` nor a column at or past `cols`.
/// When the text runs over, one row is given to `...` (the last row keeping the earliest lines, the first keeping the latest).
/// Returns the rows used.
pub fn lay(text: &[u8], cols: usize, rows: usize, keep: Keep, mut cell: impl FnMut(usize, usize, u8, bool)) -> usize {
    // WHY: an empty grid shows nothing, and `rows - 1` below needs a row.
    if cols == 0 || rows == 0 {
        return 0;
    }
    let total = walk(text, cols, |_, _, _, _| {});
    if total <= rows {
        walk(text, cols, |l, c, b, h| cell(l, c, b, h));
        return total;
    }
    let room = rows - 1;
    let mark = |cell: &mut dyn FnMut(usize, usize, u8, bool), row: usize| {
        // The mark is three cells, or as many as the grid is wide.
        for c in 0..if cols < 3 { cols } else { 3 } {
            cell(row, c, b'.', false);
        }
    };
    match keep {
        Keep::Earliest => {
            walk(text, cols, |l, c, b, h| {
                if l < room {
                    cell(l, c, b, h)
                }
            });
            mark(&mut cell, room);
        }
        Keep::Latest => {
            let skip = total - room;
            walk(text, cols, |l, c, b, h| {
                if l >= skip {
                    cell(l - skip + 1, c, b, h)
                }
            });
            mark(&mut cell, 0);
        }
    }
    rows
}

/// The grid on a panel `width` x `height` pixels: the font's scale and where its cells sit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Glass {
    pub width: usize,
    pub height: usize,
    /// Panel pixels a font pixel.
    pub scale: usize,
    /// The first cell's top left corner.
    pub x0: usize,
    pub y0: usize,
    pub cols: usize,
    pub rows: usize,
}

impl Glass {
    /// The grid for a panel, all from its span (2wh/(w+h), AGENT.md): the font's scale span / 2^9, side margins span / 2^5, the top span / 2^3 (the camera hole and the status rows), the bottom span / 2^4 (the rounded corners).
    /// None for a raster no grid fits: the size comes from the DPU's registers, external input, so it is checked here once and every number below it is proven in range.
    pub fn new(width: usize, height: usize) -> Option<Glass> {
        // WHY: the size is read from the display hardware.
        // Zero would divide by zero below; past 2^14 is outside the registers' fields and not a panel; a span under 2^9 gives a scale of 0, no cell.
        if width == 0 || height == 0 || width > 1 << 14 || height > 1 << 14 {
            return None;
        }
        let span = 2 * width * height / (width + height);
        let scale = span >> 9;
        if scale == 0 {
            return None;
        }
        // PROOF nothing below underflows or is empty: span <= 2·min(w, h), so 2·side <= min/8, top + bottom <= 3/8·min, a cell is at most span/64 wide and span/32 tall; so at least 28 columns and 10 rows fit.
        let (side, top, bottom) = (span >> 5, span >> 3, span >> 4);
        let (cw, ch) = (GLYPH_W * scale, GLYPH_H * scale);
        let cols = (width - 2 * side) / cw;
        let rows = (height - top - bottom) / ch;
        let x0 = side + (width - 2 * side - cols * cw) / 2;
        Some(Glass { width, height, scale, x0, y0: top, cols, rows })
    }

    pub fn cell_w(&self) -> usize {
        GLYPH_W * self.scale
    }
    pub fn cell_h(&self) -> usize {
        GLYPH_H * self.scale
    }
    /// The ground's border above and below the rows of text: a quarter of a cell.
    pub fn pad(&self) -> usize {
        self.cell_h() >> 2
    }
}

/// What a span of pixels is painted with.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Paint {
    /// Behind the text: across the panel's width, over the rows the text takes.
    Ground,
    /// The text.
    Ink,
    /// The headline (the text up to its first return).
    Accent,
}

/// Where the glass console's pixels go: one horizontal run at a time, `x0 < x1 <= width`, `y < height`.
pub trait Surface {
    fn span(&mut self, y: usize, x0: usize, x1: usize, paint: Paint);
}

/// `text` onto `out`: the ground across the panel's width over the rows the text takes (a quarter cell above and below), then the glyphs, laid out by `lay`.
/// Returns the rows used.
pub fn draw(g: &Glass, text: &[u8], keep: Keep, out: &mut impl Surface) -> usize {
    let used = rows_used(text, g.cols, g.rows);
    if used == 0 {
        return 0;
    }
    let (cw, ch, s) = (g.cell_w(), g.cell_h(), g.scale);
    // PROOF in range: pad < top margin and pad < bottom margin (`Glass::new`: a cell is at most span/32 tall, pad a quarter of it; the margins are span/8 and span/16), and the rows end above the bottom margin.
    for y in g.y0 - g.pad()..g.y0 + used * ch + g.pad() {
        out.span(y, 0, g.width, Paint::Ground);
    }
    lay(text, g.cols, g.rows, keep, |row, col, c, head| {
        let paint = if head { Paint::Accent } else { Paint::Ink };
        let (px, py) = (g.x0 + col * cw, g.y0 + row * ch);
        for (gy, &bits) in glyph(c).iter().enumerate() {
            // Each run of lit font pixels in the scanline, one span a panel row.
            let mut gx = 0;
            while gx < GLYPH_W {
                if bits >> (7 - gx) & 1 == 0 {
                    gx += 1;
                    continue;
                }
                let a = gx;
                while gx < GLYPH_W && bits >> (7 - gx) & 1 != 0 {
                    gx += 1;
                }
                for sy in 0..s {
                    out.span(py + gy * s + sy, px + a * s, px + gx * s, paint);
                }
            }
        }
    });
    used
}

/// Text into a fixed buffer, the rest dropped (`core::fmt::Write`): the panic screen formats without the heap.
pub struct Buf<const N: usize> {
    pub buf: [u8; N],
    pub n: usize,
}

impl<const N: usize> Buf<N> {
    pub const fn new() -> Self {
        Buf { buf: [0u8; N], n: 0 }
    }
    pub fn bytes(&self) -> &[u8] {
        &self.buf[..self.n]
    }
    pub fn push(&mut self, s: &[u8]) {
        for &b in s {
            // WHY: the buffer is fixed; what does not fit is dropped rather than written past its end.
            if self.n < N {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
    }
}

impl<const N: usize> Default for Buf<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> core::fmt::Write for Buf<N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.push(s.as_bytes());
        Ok(())
    }
}

#[cfg(test)]
mod tests;
