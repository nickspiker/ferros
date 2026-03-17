//! Minimal VSF encoder/decoder — no_std, no alloc, writes to fixed buffers.
//!
//! Implements the exact VSF wire format for the subset of types needed by
//! the kernel: hp, hb, u, z, y, l, e, n, b. Any standard VSF reader can
//! parse documents produced by this encoder.
//!
//! ## Wire format reference (from vsf/src/encoding/)
//!
//! ```text
//! Document: RÅ< z(ver) y(compat) b(header_len) e(time) hp(hash) n(count) ...fields... >
//! Magic:    0x52 0xC3 0x85 0x3C  (4 bytes: "RÅ<")
//! Close:    0x3E                  (1 byte: ">")
//!
//! EWE integer encoding (auto-sized, big-endian):
//!   u8:    '3' [1 byte]
//!   u16:   '4' [2 bytes BE]
//!   u32:   '5' [4 bytes BE]
//!   u64:   '6' [8 bytes BE]
//!   u128:  '7' [16 bytes BE]
//!
//! Type encodings:
//!   u(n):      'u' + EWE(n)
//!   z(n):      'z' + EWE(n)
//!   y(n):      'y' + EWE(n)
//!   n(n):      'n' + EWE(n)
//!   b(n):      'b' + EWE(n)
//!   l(s):      'l' + EWE(len) + ASCII bytes
//!   hp(hash):  'h' 'p' + EWE(len-1) + hash bytes
//!   hb(hash):  'h' 'b' + EWE(len-1) + hash bytes
//!   e(time):   'e' + eagle_time_encoding
//! ```
//!
//! Note: hash length is encoded as (len-1) per VSF spec.

/// VSF magic bytes: RÅ< (UTF-8: R=0x52, Å=0xC3 0x85, <=0x3C)
pub const VSF_MAGIC: [u8; 4] = [0x52, 0xC3, 0x85, 0x3C];

/// VSF header close: >
pub const VSF_CLOSE: u8 = 0x3E;

/// Writer that builds a VSF document into a fixed buffer.
pub struct VsfWriter<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> VsfWriter<'a> {
    /// Create a writer over a buffer. Does NOT write magic — call begin_document().
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Current write position.
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// Write raw bytes.
    fn put(&mut self, data: &[u8]) -> bool {
        if self.pos + data.len() > self.buf.len() { return false; }
        self.buf[self.pos..self.pos + data.len()].copy_from_slice(data);
        self.pos += data.len();
        true
    }

    /// Write a single byte.
    fn put_byte(&mut self, b: u8) -> bool {
        if self.pos >= self.buf.len() { return false; }
        self.buf[self.pos] = b;
        self.pos += 1;
        true
    }

    /// Write EWE-encoded unsigned integer (auto-sized, big-endian).
    fn put_ewe_uint(&mut self, val: u64) -> bool {
        if val <= 0xFF {
            self.put_byte(b'3') && self.put_byte(val as u8)
        } else if val <= 0xFFFF {
            self.put_byte(b'4') && self.put(&(val as u16).to_be_bytes())
        } else if val <= 0xFFFF_FFFF {
            self.put_byte(b'5') && self.put(&(val as u32).to_be_bytes())
        } else {
            self.put_byte(b'6') && self.put(&val.to_be_bytes())
        }
    }

    // ----- VSF document structure -----

    /// Write VSF magic: RÅ<
    pub fn magic(&mut self) -> bool {
        self.put(&VSF_MAGIC)
    }

    /// Write version: z(n)
    pub fn version(&mut self, ver: u64) -> bool {
        self.put_byte(b'z') && self.put_ewe_uint(ver)
    }

    /// Write backward compat version: y(n)
    pub fn backward_version(&mut self, ver: u64) -> bool {
        self.put_byte(b'y') && self.put_ewe_uint(ver)
    }

    /// Write header byte length: b(n)
    pub fn header_length(&mut self, len: u64) -> bool {
        self.put_byte(b'b') && self.put_ewe_uint(len)
    }

    /// Write field count: n(count)
    pub fn field_count(&mut self, count: u64) -> bool {
        self.put_byte(b'n') && self.put_ewe_uint(count)
    }

    /// Write header close: >
    pub fn close(&mut self) -> bool {
        self.put_byte(VSF_CLOSE)
    }

    // ----- Sections and fields -----

    /// Open a section: [d("name")
    pub fn section_open(&mut self, name: &str) -> bool {
        self.put_byte(b'[') && self.dict_key(name)
    }

    /// Close a section: ]
    pub fn section_close(&mut self) -> bool {
        self.put_byte(b']')
    }

    /// Open a field: (d("name"):
    pub fn field_open(&mut self, name: &str) -> bool {
        self.put_byte(b'(') && self.dict_key(name) && self.put_byte(b':')
    }

    /// Close a field: )
    pub fn field_close(&mut self) -> bool {
        self.put_byte(b')')
    }

    /// Write internal dictionary key: d(string)
    pub fn dict_key(&mut self, s: &str) -> bool {
        self.put_byte(b'd')
            && self.put_ewe_uint(s.len() as u64)
            && self.put(s.as_bytes())
    }

    // ----- Typed values -----

    /// Write unsigned integer: u(val)
    pub fn uint(&mut self, val: u64) -> bool {
        self.put_byte(b'u') && self.put_ewe_uint(val)
    }

    /// Write ASCII label: l(string)
    pub fn label(&mut self, s: &str) -> bool {
        self.put_byte(b'l')
            && self.put_ewe_uint(s.len() as u64)
            && self.put(s.as_bytes())
    }

    /// Write provenance hash: hp(blake3_hash)
    /// Length encoded as (len-1) per VSF spec.
    pub fn hash_p(&mut self, hash: &[u8; 32]) -> bool {
        self.put_byte(b'h')
            && self.put_byte(b'p')
            && self.put_ewe_uint(31) // len-1 = 31
            && self.put(hash)
    }

    /// Write provenance hash placeholder (32 zero bytes).
    /// Returns the position where the hash starts (for later fill-in).
    pub fn hash_p_placeholder(&mut self) -> Option<usize> {
        if !(self.put_byte(b'h')
            && self.put_byte(b'p')
            && self.put_ewe_uint(31))
        {
            return None;
        }
        let hash_pos = self.pos;
        if !self.put(&[0u8; 32]) { return None; }
        Some(hash_pos)
    }

    /// Write rolling hash: hb(blake3_hash)
    pub fn hash_b(&mut self, hash: &[u8; 32]) -> bool {
        self.put_byte(b'h')
            && self.put_byte(b'b')
            && self.put_ewe_uint(31)
            && self.put(hash)
    }

    /// Write Eagle Time as a u64 QTIMER value.
    /// Uses the 'e' tag with 'u' sub-encoding (eu6 format).
    /// When real Eagle Time is available, this becomes proper EtType.
    pub fn eagle_time_qtimer(&mut self, ticks: u64) -> bool {
        self.put_byte(b'e')
            && self.put_byte(b'u')
            && self.put_ewe_uint(ticks)
    }

    /// Fill in a hash at a previously saved position.
    pub fn fill_hash(&mut self, pos: usize, hash: &[u8; 32]) {
        self.buf[pos..pos + 32].copy_from_slice(hash);
    }
}

/// Reader that parses VSF fields from a buffer.
pub struct VsfReader<'a> {
    buf: &'a [u8],
    pub pos: usize,
}

impl<'a> VsfReader<'a> {
    /// Create a reader over a buffer.
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Remaining bytes.
    fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    /// Read one byte (public for section/field delimiter parsing).
    pub fn read_byte_raw(&mut self) -> Option<u8> {
        if self.pos >= self.buf.len() { return None; }
        let b = self.buf[self.pos];
        self.pos += 1;
        Some(b)
    }

    /// Read one byte (internal alias).
    fn read_byte(&mut self) -> Option<u8> {
        self.read_byte_raw()
    }

    /// Read N bytes.
    fn read_bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.pos + n > self.buf.len() { return None; }
        let slice = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Some(slice)
    }

    /// Read EWE-encoded unsigned integer.
    pub fn read_ewe_uint(&mut self) -> Option<u64> {
        let marker = self.read_byte()?;
        match marker {
            b'3' => Some(self.read_byte()? as u64),
            b'4' => {
                let bytes = self.read_bytes(2)?;
                Some(u16::from_be_bytes([bytes[0], bytes[1]]) as u64)
            }
            b'5' => {
                let bytes = self.read_bytes(4)?;
                Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as u64)
            }
            b'6' => {
                let bytes = self.read_bytes(8)?;
                Some(u64::from_be_bytes([
                    bytes[0], bytes[1], bytes[2], bytes[3],
                    bytes[4], bytes[5], bytes[6], bytes[7],
                ]))
            }
            b'7' => {
                // u128 — read but truncate to u64 (ring doesn't need u128)
                let _bytes = self.read_bytes(16)?;
                Some(u64::MAX) // lossy but ring gen won't exceed u64
            }
            _ => None,
        }
    }

    // ----- VSF document structure -----

    /// Verify and consume VSF magic: RÅ<
    pub fn magic(&mut self) -> bool {
        if self.remaining() < 4 { return false; }
        let m = &self.buf[self.pos..self.pos + 4];
        if m != VSF_MAGIC { return false; }
        self.pos += 4;
        true
    }

    /// Read version: z(n) → returns version number.
    pub fn version(&mut self) -> Option<u64> {
        if self.read_byte()? != b'z' { return None; }
        self.read_ewe_uint()
    }

    /// Read backward version: y(n)
    pub fn backward_version(&mut self) -> Option<u64> {
        if self.read_byte()? != b'y' { return None; }
        self.read_ewe_uint()
    }

    /// Read header length: b(n)
    pub fn header_length(&mut self) -> Option<u64> {
        if self.read_byte()? != b'b' { return None; }
        self.read_ewe_uint()
    }

    /// Read field count: n(count)
    pub fn field_count(&mut self) -> Option<u64> {
        if self.read_byte()? != b'n' { return None; }
        self.read_ewe_uint()
    }

    /// Read and verify close byte: >
    pub fn close(&mut self) -> bool {
        self.read_byte() == Some(VSF_CLOSE)
    }

    // ----- Section/field parsing -----

    /// Read internal dictionary key: d(string) → &str
    pub fn dict_key_str(&mut self) -> Option<&'a str> {
        if self.read_byte()? != b'd' { return None; }
        let len = self.read_ewe_uint()? as usize;
        let bytes = self.read_bytes(len)?;
        core::str::from_utf8(bytes).ok()
    }

    // ----- Typed fields -----

    /// Peek at next type tag without consuming.
    pub fn peek_tag(&self) -> Option<u8> {
        if self.pos < self.buf.len() { Some(self.buf[self.pos]) } else { None }
    }

    /// Read unsigned integer: u(val)
    pub fn uint(&mut self) -> Option<u64> {
        if self.read_byte()? != b'u' { return None; }
        self.read_ewe_uint()
    }

    /// Read ASCII label: l(string) → returns byte slice.
    pub fn label(&mut self) -> Option<&'a [u8]> {
        if self.read_byte()? != b'l' { return None; }
        let len = self.read_ewe_uint()? as usize;
        self.read_bytes(len)
    }

    /// Read provenance hash: hp(32 bytes)
    pub fn hash_p(&mut self) -> Option<&'a [u8; 32]> {
        if self.read_byte()? != b'h' { return None; }
        if self.read_byte()? != b'p' { return None; }
        let len_minus_1 = self.read_ewe_uint()? as usize;
        let hash = self.read_bytes(len_minus_1 + 1)?;
        if hash.len() != 32 { return None; }
        Some(hash.try_into().ok()?)
    }

    /// Read rolling hash: hb(32 bytes)
    pub fn hash_b(&mut self) -> Option<&'a [u8; 32]> {
        if self.read_byte()? != b'h' { return None; }
        if self.read_byte()? != b'b' { return None; }
        let len_minus_1 = self.read_ewe_uint()? as usize;
        let hash = self.read_bytes(len_minus_1 + 1)?;
        if hash.len() != 32 { return None; }
        Some(hash.try_into().ok()?)
    }

    /// Read Eagle Time (QTIMER u64): e(u(ticks))
    pub fn eagle_time_qtimer(&mut self) -> Option<u64> {
        if self.read_byte()? != b'e' { return None; }
        if self.read_byte()? != b'u' { return None; }
        self.read_ewe_uint()
    }

    /// Skip an unknown field (reads type tag + payload).
    /// Returns false if the field can't be skipped (unknown format).
    pub fn skip_field(&mut self) -> bool {
        let tag = match self.read_byte() {
            Some(t) => t,
            None => return false,
        };
        match tag {
            b'u' | b'z' | b'y' | b'n' | b'b' | b'o' | b'm' => {
                self.read_ewe_uint().is_some()
            }
            b'l' => {
                if let Some(len) = self.read_ewe_uint() {
                    self.read_bytes(len as usize).is_some()
                } else {
                    false
                }
            }
            b'h' | b'g' | b'k' => {
                // Two-byte tag: h?, g?, k?
                let _sub = self.read_byte();
                if let Some(len_m1) = self.read_ewe_uint() {
                    self.read_bytes(len_m1 as usize + 1).is_some()
                } else {
                    false
                }
            }
            b'e' => {
                // Eagle Time: e + sub-encoding
                let sub = match self.read_byte() {
                    Some(s) => s,
                    None => return false,
                };
                match sub {
                    b'u' => self.read_ewe_uint().is_some(),
                    _ => false,
                }
            }
            _ => false,
        }
    }
}

/// Read the QTIMER counter (CNTPCT_EL0) — monotonic, 19.2MHz on QCM6490.
#[inline]
pub fn read_qtimer() -> u64 {
    let val: u64;
    unsafe { core::arch::asm!("mrs {}, CNTPCT_EL0", out(reg) val) };
    val
}
