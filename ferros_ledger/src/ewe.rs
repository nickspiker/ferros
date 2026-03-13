//! Elastic Width Encoding (EWE) for unbounded integers.
//!
//! VSF uses EWE to encode integers with no fixed ceiling.
//! Format: tag byte + width byte + payload bytes.
//!
//! ```text
//! Value 0-255:      'u' '3' + 1 byte   = 3 bytes
//! Value 256-65535:  'u' '4' + 2 bytes   = 4 bytes
//! Value up to 2^32: 'u' '5' + 4 bytes  = 6 bytes
//! Value up to 2^64: 'u' '6' + 8 bytes  = 10 bytes
//! ```
//!
//! The width byte encodes the payload size as log2(bytes)+3:
//!   '3' = 1 byte, '4' = 2 bytes, '5' = 4 bytes, '6' = 8 bytes.

/// EWE type tag for unsigned integer.
pub const TAG_U: u8 = b'u';

/// Encode a u64 as EWE into `buf`. Returns bytes written.
/// Minimum output: 3 bytes. Maximum: 10 bytes.
pub fn encode_u64(buf: &mut [u8], val: u64) -> usize {
    buf[0] = TAG_U;
    if val <= 0xFF {
        buf[1] = b'3'; // 1-byte payload
        buf[2] = val as u8;
        3
    } else if val <= 0xFFFF {
        buf[1] = b'4'; // 2-byte payload
        let bytes = (val as u16).to_le_bytes();
        buf[2] = bytes[0];
        buf[3] = bytes[1];
        4
    } else if val <= 0xFFFF_FFFF {
        buf[1] = b'5'; // 4-byte payload
        let bytes = (val as u32).to_le_bytes();
        buf[2..6].copy_from_slice(&bytes);
        6
    } else {
        buf[1] = b'6'; // 8-byte payload
        let bytes = val.to_le_bytes();
        buf[2..10].copy_from_slice(&bytes);
        10
    }
}

/// Decode an EWE u64 from `buf`. Returns (value, bytes_consumed) or None.
pub fn decode_u64(buf: &[u8]) -> Option<(u64, usize)> {
    if buf.len() < 3 { return None; }
    if buf[0] != TAG_U { return None; }

    match buf[1] {
        b'3' => {
            Some((buf[2] as u64, 3))
        }
        b'4' => {
            if buf.len() < 4 { return None; }
            let val = u16::from_le_bytes([buf[2], buf[3]]) as u64;
            Some((val, 4))
        }
        b'5' => {
            if buf.len() < 6 { return None; }
            let val = u32::from_le_bytes([buf[2], buf[3], buf[4], buf[5]]) as u64;
            Some((val, 6))
        }
        b'6' => {
            if buf.len() < 10 { return None; }
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&buf[2..10]);
            Some((u64::from_le_bytes(bytes), 10))
        }
        _ => None,
    }
}

/// Returns the number of bytes needed to encode `val` as EWE.
pub fn encoded_len_u64(val: u64) -> usize {
    if val <= 0xFF { 3 }
    else if val <= 0xFFFF { 4 }
    else if val <= 0xFFFF_FFFF { 6 }
    else { 10 }
}
