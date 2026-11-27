//! Elastic Width Encoding (EWE) for unbounded integers.
//!
//! VSF uses EWE to encode integers with no fixed ceiling. Format: type tag + width marker + big-endian payload.
//!
//! ```text
//! Value 0-255:      'u' '3' + 1 byte   = 3 bytes Value 256-65535:  'u' '4' + 2 bytes   = 4 bytes Value up to 2^32: 'u' '5' + 4 bytes   = 6 bytes Value up to 2^64: 'u' '6' + 8 bytes   = 10 bytes
//! ```
//!
//! Width marker = log2(payload_bytes) + 3: '3' = 1 byte, '4' = 2 bytes, '5' = 4 bytes, '6' = 8 bytes.
//!
//! All payloads are big-endian, matching VSF canonical encoding.

/// EWE type tag for unsigned integer.
pub const TAG_U: u8 = b'u';

/// Encode a u64 as EWE into `buf`. Returns bytes written. Minimum output: 3 bytes. Maximum: 10 bytes. Payload is big-endian per VSF spec.
pub fn encode_u64(buf: &mut [u8], val: u64) -> usize {
    buf[0] = TAG_U;
    if val <= 0xFF {
        buf[1] = b'3'; // 1-byte payload
        buf[2] = val as u8;
        3
    } else if val <= 0xFFFF {
        buf[1] = b'4'; // 2-byte payload
        let bytes = (val as u16).to_be_bytes();
        buf[2] = bytes[0];
        buf[3] = bytes[1];
        4
    } else if val <= 0xFFFF_FFFF {
        buf[1] = b'5'; // 4-byte payload
        let bytes = (val as u32).to_be_bytes();
        buf[2..6].copy_from_slice(&bytes);
        6
    } else {
        buf[1] = b'6'; // 8-byte payload
        let bytes = val.to_be_bytes();
        buf[2..10].copy_from_slice(&bytes);
        10
    }
}

/// Decode an EWE u64 from `buf`. Returns (value, bytes_consumed) or None.
pub fn decode_u64(buf: &[u8]) -> Option<(u64, usize)> {
    if buf.len() < 3 {
        return None;
    }
    if buf[0] != TAG_U {
        return None;
    }

    match buf[1] {
        b'3' => Some((buf[2] as u64, 3)),
        b'4' => {
            if buf.len() < 4 {
                return None;
            }
            let val = u16::from_be_bytes([buf[2], buf[3]]) as u64;
            Some((val, 4))
        }
        b'5' => {
            if buf.len() < 6 {
                return None;
            }
            let val = u32::from_be_bytes([buf[2], buf[3], buf[4], buf[5]]) as u64;
            Some((val, 6))
        }
        b'6' => {
            if buf.len() < 10 {
                return None;
            }
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&buf[2..10]);
            Some((u64::from_be_bytes(bytes), 10))
        }
        _ => None,
    }
}

/// Returns the number of bytes needed to encode `val` as EWE.
pub fn encoded_len_u64(val: u64) -> usize {
    if val <= 0xFF {
        3
    } else if val <= 0xFFFF {
        4
    } else if val <= 0xFFFF_FFFF {
        6
    } else {
        10
    }
}

/// Returns the EWE width (payload bytes only, no tag) for a given count. Used by PT to determine per-packet sequence field width from SPEC count.
///
/// e.g. seq_width(500) = 2, so DATA packets use 2-byte BE sequence numbers.
pub fn seq_width(count: u64) -> usize {
    if count <= 0xFF {
        1
    } else if count <= 0xFFFF {
        2
    } else if count <= 0xFFFF_FFFF {
        4
    } else {
        8
    }
}

/// Encode a raw big-endian sequence number at a known width (no tag bytes). Used for DATA packet sequence fields where width is implicit from SPEC.
pub fn encode_seq(buf: &mut [u8], val: u64, width: usize) -> usize {
    let be = val.to_be_bytes();
    // Take the last `width` bytes of the 8-byte BE representation
    let start = 8 - width;
    buf[..width].copy_from_slice(&be[start..]);
    width
}

/// Decode a raw big-endian sequence number at a known width (no tag bytes).
pub fn decode_seq(buf: &[u8], width: usize) -> Option<u64> {
    if buf.len() < width {
        return None;
    }
    let mut bytes = [0u8; 8];
    let start = 8 - width;
    bytes[start..].copy_from_slice(&buf[..width]);
    Some(u64::from_be_bytes(bytes))
}

// ---------------------------------------------------------------------------
// Lean EWE — width marker + payload only, no type tag. For PT control packets where field types are known by position.
// ---------------------------------------------------------------------------

/// Encode a u64 as lean EWE (width marker + big-endian payload, no 'u' tag). Minimum output: 2 bytes. Maximum: 9 bytes.
pub fn encode_lean(buf: &mut [u8], val: u64) -> usize {
    if val <= 0xFF {
        buf[0] = b'3';
        buf[1] = val as u8;
        2
    } else if val <= 0xFFFF {
        buf[0] = b'4';
        let bytes = (val as u16).to_be_bytes();
        buf[1] = bytes[0];
        buf[2] = bytes[1];
        3
    } else if val <= 0xFFFF_FFFF {
        buf[0] = b'5';
        let bytes = (val as u32).to_be_bytes();
        buf[1..5].copy_from_slice(&bytes);
        5
    } else {
        buf[0] = b'6';
        let bytes = val.to_be_bytes();
        buf[1..9].copy_from_slice(&bytes);
        9
    }
}

/// Decode a lean EWE u64 (width marker + payload, no 'u' tag). Returns (value, bytes_consumed) or None.
pub fn decode_lean(buf: &[u8]) -> Option<(u64, usize)> {
    if buf.is_empty() {
        return None;
    }
    match buf[0] {
        b'3' => {
            if buf.len() < 2 { return None; }
            Some((buf[1] as u64, 2))
        }
        b'4' => {
            if buf.len() < 3 { return None; }
            let val = u16::from_be_bytes([buf[1], buf[2]]) as u64;
            Some((val, 3))
        }
        b'5' => {
            if buf.len() < 5 { return None; }
            let val = u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]) as u64;
            Some((val, 5))
        }
        b'6' => {
            if buf.len() < 9 { return None; }
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&buf[1..9]);
            Some((u64::from_be_bytes(bytes), 9))
        }
        _ => None,
    }
}
