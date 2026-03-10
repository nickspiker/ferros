//! Minimal DTB (Device Tree Blob) parser.
//!
//! We only need to extract a few things from the DTB that ABL passes:
//! - `simple-framebuffer` node (address, size, format)
//! - `reserved-memory/ferros-anchor-key` (for Phase 2 ABL handoff)
//! - Memory regions
//!
//! This is NOT a full DTB parser. It's a scanner that finds specific
//! nodes by name and reads their properties. Full FDT parsing is
//! overkill for boot — we know exactly what we're looking for.
//!
//! ## DTB Format (condensed)
//!
//! ```text
//! Header (40 bytes):
//!   magic: 0xD00DFEED (big-endian)
//!   totalsize, off_dt_struct, off_dt_strings, ...
//!
//! Structure block (tokens, big-endian u32):
//!   FDT_BEGIN_NODE (0x01) + name\0 + padding
//!   FDT_PROP       (0x03) + len + nameoff + data + padding
//!   FDT_END_NODE   (0x02)
//!   FDT_END        (0x09)
//!
//! Strings block: null-terminated property names
//! ```

/// DTB header magic.
const FDT_MAGIC: u32 = 0xD00D_FEED;

/// DTB structure tokens.
const FDT_BEGIN_NODE: u32 = 0x01;
const FDT_END_NODE: u32 = 0x02;
const FDT_PROP: u32 = 0x03;
const FDT_NOP: u32 = 0x04;
const FDT_END: u32 = 0x09;

/// A property found in the DTB.
#[derive(Clone, Copy)]
pub struct DtbProp<'a> {
    pub name: &'a [u8],
    pub data: &'a [u8],
}

impl<'a> DtbProp<'a> {
    /// Read property as a big-endian u32.
    pub fn as_u32(&self) -> Option<u32> {
        if self.data.len() >= 4 {
            Some(u32::from_be_bytes([
                self.data[0], self.data[1], self.data[2], self.data[3],
            ]))
        } else {
            None
        }
    }

    /// Read property as a big-endian u64.
    pub fn as_u64(&self) -> Option<u64> {
        if self.data.len() >= 8 {
            Some(u64::from_be_bytes([
                self.data[0], self.data[1], self.data[2], self.data[3],
                self.data[4], self.data[5], self.data[6], self.data[7],
            ]))
        } else {
            None
        }
    }

    /// Read property as a null-terminated string.
    pub fn as_str(&self) -> Option<&'a str> {
        let end = self.data.iter().position(|&b| b == 0).unwrap_or(self.data.len());
        core::str::from_utf8(&self.data[..end]).ok()
    }
}

/// Read a big-endian u32 from a byte slice at offset.
fn be_u32(data: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
}

/// Align up to 4-byte boundary.
fn align4(n: usize) -> usize {
    (n + 3) & !3
}

/// A minimal DTB scanner.
///
/// Call `find_node_prop()` to locate a property inside a named node.
pub struct Dtb<'a> {
    data: &'a [u8],
    struct_offset: usize,
    strings_offset: usize,
}

impl<'a> Dtb<'a> {
    /// Parse a DTB from raw bytes.
    ///
    /// # Safety
    /// The data must be a valid FDT blob.
    pub fn from_bytes(data: &'a [u8]) -> Option<Self> {
        if data.len() < 40 {
            return None;
        }
        if be_u32(data, 0) != FDT_MAGIC {
            return None;
        }
        let struct_offset = be_u32(data, 8) as usize;
        let strings_offset = be_u32(data, 12) as usize;
        Some(Self { data, struct_offset, strings_offset })
    }

    /// Look up a string in the strings block.
    fn string_at(&self, offset: usize) -> &'a [u8] {
        let start = self.strings_offset + offset;
        let end = self.data[start..].iter().position(|&b| b == 0)
            .map(|p| start + p)
            .unwrap_or(self.data.len());
        &self.data[start..end]
    }

    /// Find a property `prop_name` inside a node whose name starts with `node_name`.
    ///
    /// Scans the structure block linearly. Returns the first match.
    /// `node_name` matches if the DTB node name starts with it (handles
    /// unit addresses like `framebuffer@9c000000`).
    pub fn find_node_prop(&self, node_name: &[u8], prop_name: &[u8]) -> Option<DtbProp<'a>> {
        let mut pos = self.struct_offset;
        let mut in_target_node = false;
        let mut depth: i32 = 0;
        let mut target_depth: i32 = 0;

        while pos + 4 <= self.data.len() {
            let token = be_u32(self.data, pos);
            pos += 4;

            match token {
                FDT_BEGIN_NODE => {
                    depth += 1;
                    // Node name is null-terminated, then aligned
                    let name_start = pos;
                    let name_end = self.data[pos..].iter().position(|&b| b == 0)
                        .map(|p| pos + p)
                        .unwrap_or(self.data.len());
                    let name = &self.data[name_start..name_end];

                    if name.len() >= node_name.len() && &name[..node_name.len()] == node_name {
                        in_target_node = true;
                        target_depth = depth;
                    }

                    pos = align4(name_end + 1); // skip null + align
                }
                FDT_END_NODE => {
                    if in_target_node && depth == target_depth {
                        in_target_node = false;
                    }
                    depth -= 1;
                }
                FDT_PROP => {
                    if pos + 8 > self.data.len() { return None; }
                    let len = be_u32(self.data, pos) as usize;
                    let nameoff = be_u32(self.data, pos + 4) as usize;
                    pos += 8;

                    if pos + len > self.data.len() { return None; }
                    let data = &self.data[pos..pos + len];
                    let name = self.string_at(nameoff);

                    if in_target_node && depth == target_depth && name == prop_name {
                        return Some(DtbProp { name, data });
                    }

                    pos = align4(pos + len);
                }
                FDT_NOP => {}
                FDT_END => break,
                _ => break,
            }
        }

        None
    }

    /// Parse the `simple-framebuffer` node into an FbConfig.
    pub fn parse_simplefb(&self) -> Option<crate::fb::FbConfig> {
        // Find the node — could be "framebuffer" or "simple-framebuffer"
        let node = b"framebuffer";

        let width = self.find_node_prop(node, b"width")?.as_u32()?;
        let height = self.find_node_prop(node, b"height")?.as_u32()?;
        let stride = self.find_node_prop(node, b"stride")?.as_u32()?;

        // Parse "reg" property for physical address (assumes #address-cells=2, #size-cells=2)
        let reg = self.find_node_prop(node, b"reg")?;
        let phys_base = if reg.data.len() >= 16 {
            // 64-bit address
            reg.as_u64()?
        } else {
            reg.as_u32()? as u64
        };

        let format_str = self.find_node_prop(node, b"format")?.as_str()?;
        let format = match format_str {
            "a8r8g8b8" => crate::fb::PixelFormat::Argb8888,
            "x8r8g8b8" => crate::fb::PixelFormat::Xrgb8888,
            "r5g6b5" => crate::fb::PixelFormat::Rgb565,
            _ => return None,
        };

        Some(crate::fb::FbConfig { phys_base, width, height, stride, format })
    }
}
