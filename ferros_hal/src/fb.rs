//! SimpleFB — ABL's framebuffer as a pixel surface.
//!
//! ABL initializes the display panel (DSI lane training, panel init
//! sequence, backlight, voltage rails) and leaves a linear framebuffer
//! running. It advertises the address via a DTB `simple-framebuffer` node.
//!
//! We just map that physical address and write pixels. Zero display
//! driver code needed for Phase A.
//!
//! ## Pixel Format
//!
//! ABL typically uses ARGB8888 (a8r8g8b8):
//! ```text
//! byte 0: Blue
//! byte 1: Green
//! byte 2: Red
//! byte 3: Alpha (ignored, 0xFF)
//! ```
//!
//! ## FP5 Display
//! - 1224 x 2700 @ 90Hz
//! - Stride = 1224 * 4 = 4896 bytes
//! - Total = 4896 * 2700 = 13,219,200 bytes (~12.6 MB)

/// Pixel format of the framebuffer.
#[derive(Clone, Copy, Debug)]
pub enum PixelFormat {
    /// ARGB8888: [B, G, R, A] per pixel (little-endian u32 = 0xAARRGGBB)
    Argb8888,
    /// XRGB8888: same layout, alpha ignored
    Xrgb8888,
    /// RGB565: 16-bit, [RRRRRGGG, GGGBBBBB]
    Rgb565,
}

impl PixelFormat {
    pub const fn bytes_per_pixel(&self) -> usize {
        match self {
            Self::Argb8888 | Self::Xrgb8888 => 4,
            Self::Rgb565 => 2,
        }
    }
}

/// Framebuffer configuration, typically parsed from DTB.
#[derive(Clone, Debug)]
pub struct FbConfig {
    /// Physical base address of the framebuffer.
    pub phys_base: u64,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Stride in bytes (may be > width * bpp due to alignment).
    pub stride: u32,
    /// Pixel format.
    pub format: PixelFormat,
}

impl FbConfig {
    /// Total framebuffer size in bytes.
    pub fn size_bytes(&self) -> usize {
        self.stride as usize * self.height as usize
    }
}

/// A mapped framebuffer — raw pixel access.
///
/// The `buf` pointer is the virtual address mapping of the physical
/// framebuffer. On bare metal with identity mapping, phys == virt.
pub struct Framebuffer {
    pub config: FbConfig,
    /// Raw pointer to the mapped framebuffer memory.
    /// On bare metal with identity map: this equals phys_base.
    buf: *mut u8,
}

impl Framebuffer {
    /// Create a framebuffer handle.
    ///
    /// # Safety
    /// `buf` must point to mapped, writable memory of at least
    /// `config.size_bytes()` bytes. The framebuffer must remain
    /// valid for the lifetime of this struct.
    pub unsafe fn new(config: FbConfig, buf: *mut u8) -> Self {
        Self { config, buf }
    }

    /// Write a single pixel (ARGB8888 / XRGB8888).
    #[inline]
    pub fn put_pixel(&mut self, x: u32, y: u32, r: u8, g: u8, b: u8) {
        if x >= self.config.width || y >= self.config.height {
            return;
        }
        let offset = (y as usize * self.config.stride as usize)
            + (x as usize * self.config.format.bytes_per_pixel());
        unsafe {
            match self.config.format {
                PixelFormat::Argb8888 | PixelFormat::Xrgb8888 => {
                    *self.buf.add(offset) = b;
                    *self.buf.add(offset + 1) = g;
                    *self.buf.add(offset + 2) = r;
                    *self.buf.add(offset + 3) = 0xFF;
                }
                PixelFormat::Rgb565 => {
                    let val: u16 = ((r as u16 & 0xF8) << 8)
                        | ((g as u16 & 0xFC) << 3)
                        | ((b as u16) >> 3);
                    *(self.buf.add(offset) as *mut u16) = val;
                }
            }
        }
    }

    /// Fill the entire screen with a solid color.
    pub fn clear(&mut self, r: u8, g: u8, b: u8) {
        for y in 0..self.config.height {
            for x in 0..self.config.width {
                self.put_pixel(x, y, r, g, b);
            }
        }
    }

    /// Fill a rectangle.
    pub fn fill_rect(&mut self, x: u32, y: u32, w: u32, h: u32, r: u8, g: u8, b: u8) {
        for dy in 0..h {
            for dx in 0..w {
                self.put_pixel(x + dx, y + dy, r, g, b);
            }
        }
    }

    /// Raw pointer to the buffer (for bulk operations / DMA).
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.buf
    }

    /// Raw slice of the entire framebuffer.
    ///
    /// # Safety
    /// Caller must ensure no concurrent writes from DPU scanout.
    pub unsafe fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.buf, self.config.size_bytes()) }
    }
}
