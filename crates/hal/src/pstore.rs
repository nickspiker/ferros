//! Ramoops / pstore writer.
//!
//! Writes log data into the ramoops reserved-memory region using the `persistent_ram_buffer` header format that Linux's pstore driver recognizes on next boot.
//!
//! Layout of a persistent_ram_buffer zone: offset 0: sig   (u32) = 0x43474244 ("DBGC") offset 4: start (u32) = read pointer in circular buffer offset 8: size  (u32) = total bytes written (may exceed capacity for wrap) offset 12+: data[capacity]
//!
//! The ramoops region is split into zones: [oops records...][console][ftrace][pmsg] We write to the console zone.

const PERSISTENT_RAM_SIG: u32 = 0x4347_4244; // "DBGC" in LE

const PRB_HEADER: usize = 12; // sig + start + size

/// Ramoops console log writer.
pub struct Ramoops {
    base: *mut u8,  // start of console zone
    cap: usize,     // data capacity (zone size - header)
    written: usize, // total bytes written
}

unsafe impl Send for Ramoops {}
unsafe impl Sync for Ramoops {}

/// Configuration parsed from DTB ramoops node.
pub struct RamoopsConfig {
    pub base: u64,
    pub size: usize,
    pub record_size: usize,
    pub console_size: usize,
    pub ftrace_size: usize,
    pub pmsg_size: usize,
}

impl RamoopsConfig {
    /// Compute the byte offset of the console zone within the ramoops region.
    pub fn console_offset(&self) -> usize {
        // oops records fill the space before console/ftrace/pmsg
        let reserved = self.console_size + self.ftrace_size + self.pmsg_size;
        if self.size > reserved {
            self.size - reserved
        } else {
            0
        }
    }
}

impl Ramoops {
    /// Create a ramoops writer for the console zone.
    ///
    /// # Safety
    /// `zone_base` must point to writable DRAM of at least `zone_size` bytes that persists across warm reboot.
    pub unsafe fn new(zone_base: *mut u8, zone_size: usize) -> Self {
        let cap = if zone_size > PRB_HEADER { zone_size - PRB_HEADER } else { 0 };

        let s = Self {
            base: zone_base,
            cap,
            written: 0,
        };

        // Write header
        s.write_u32(0, PERSISTENT_RAM_SIG);
        s.write_u32(4, 0); // start = 0
        s.write_u32(8, 0); // size = 0

        s
    }

    /// Create from a parsed DTB config.
    ///
    /// # Safety
    /// The ramoops region must be writable DRAM.
    pub unsafe fn from_config(cfg: &RamoopsConfig) -> Self {
        let zone_base = (cfg.base as usize + cfg.console_offset()) as *mut u8;
        unsafe { Self::new(zone_base, cfg.console_size) }
    }

    fn write_u32(&self, offset: usize, val: u32) {
        unsafe {
            (self.base.add(offset) as *mut u32).write_volatile(val);
        }
    }

    /// Write a single byte to the log.
    pub fn putc(&mut self, b: u8) {
        if self.cap == 0 { return; }

        let offset = self.written % self.cap;
        unsafe {
            self.base.add(PRB_HEADER + offset).write_volatile(b);
        }
        self.written += 1;

        // Update header: if we've wrapped, start advances
        if self.written <= self.cap {
            self.write_u32(8, self.written as u32);
        } else {
            // Circular: start = write position (oldest data), size = capacity
            let start = self.written % self.cap;
            self.write_u32(4, start as u32);
            self.write_u32(8, self.cap as u32);
        }
    }

    /// Write a string.
    pub fn puts(&mut self, s: &str) {
        for b in s.bytes() {
            self.putc(b);
        }
    }

    /// Write a u64 as G#-prefixed hex.
    pub fn put_hex(&mut self, val: u64) {
        self.puts("G#");
        for i in (0..16).rev() {
            let nibble = ((val >> (i * 4)) & 0xF) as u8;
            let c = if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 };
            self.putc(c);
        }
    }

    /// Write a u32 as G#-prefixed hex (8 digits).
    pub fn put_hex32(&mut self, val: u32) {
        self.puts("G#");
        for i in (0..8).rev() {
            let nibble = ((val >> (i * 4)) & 0xF) as u8;
            let c = if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 };
            self.putc(c);
        }
    }

    /// Bytes written so far.
    pub fn written(&self) -> usize {
        self.written
    }
}
