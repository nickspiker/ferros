//! Microsoft OS 2.0 descriptors (feature `msos`), so Windows binds WinUSB to ferros's USB device with no driver installed (docs/FLASHER.md "Windows and the driver").
//! The device's BOS names the set and the vendor request that returns it (usb.rs); this module is the set itself, apart from the DWC3 driver so the host tests can read it.

/// The vendor request (bmRequestType G#C0, wIndex 7) that returns `SET`; beside the development requests G#5A to G#5D.
pub const VENDOR_REQ: u8 = 0x5E;

pub const SET_LEN: usize = 10 + 20 + 132;

/// The set for this one-interface device: its header, the compatible ID WINUSB (Windows binds WinUSB with no INF), and the registry property DeviceInterfaceGUIDs, {94952487-3D5A-4981-B569-5AA375AE5768} (BLAKE3("ferros usb device interface") with the GUID version and variant bits), the interface class a host program opens.
pub static SET: [u8; SET_LEN] = msos_set();

const fn msos_set() -> [u8; SET_LEN] {
    let mut d = [0u8; SET_LEN];
    // Set header: wLength 10, MS_OS_20_SET_HEADER_DESCRIPTOR (0), dwWindowsVersion, wTotalLength.
    let head: [u8; 10] = [10, 0, 0, 0, 0x00, 0x00, 0x03, 0x06, SET_LEN as u8, (SET_LEN >> 8) as u8];
    // Compatible ID: wLength 20, MS_OS_20_FEATURE_COMPATBLE_ID (3), "WINUSB" padded to 8, no sub-compatible ID.
    let compat: [u8; 20] = [20, 0, 3, 0, b'W', b'I', b'N', b'U', b'S', b'B', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let mut at = 0;
    let mut k = 0;
    while k < 10 {
        d[at] = head[k];
        at += 1;
        k += 1;
    }
    k = 0;
    while k < 20 {
        d[at] = compat[k];
        at += 1;
        k += 1;
    }
    // Registry property: wLength 132, MS_OS_20_FEATURE_REG_PROPERTY (4), REG_MULTI_SZ (7), the name's 42 bytes, the data's 80.
    let prop: [u8; 6] = [132, 0, 4, 0, 7, 0];
    k = 0;
    while k < 6 {
        d[at] = prop[k];
        at += 1;
        k += 1;
    }
    at = utf16z(&mut d, at, b"DeviceInterfaceGUIDs", 1);
    at = utf16z(&mut d, at, b"{94952487-3D5A-4981-B569-5AA375AE5768}", 2);
    assert!(at == SET_LEN);
    d
}

/// `s` as UTF-16LE with `nuls` terminating zero units, its byte length (u16 LE) first; the next free offset.
const fn utf16z(d: &mut [u8; SET_LEN], mut at: usize, s: &[u8], nuls: usize) -> usize {
    let len = (s.len() + nuls) * 2;
    d[at] = len as u8;
    d[at + 1] = (len >> 8) as u8;
    at += 2;
    let mut k = 0;
    while k < s.len() {
        d[at] = s[k];
        d[at + 1] = 0;
        at += 2;
        k += 1;
    }
    at + nuls * 2
}
#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::vec::Vec;

    /// The layout Windows reads (Microsoft OS 2.0 Descriptors Specification): the set's lengths add up, and the property's strings are what Windows looks for. (The BOS that names it is checked by its own const assertions in usb.rs.)
    #[test]
    fn msos_descriptors() {
        assert_eq!(SET[8] as usize | (SET[9] as usize) << 8, SET.len());
        assert_eq!(&SET[10..12], &[20, 0]);
        assert_eq!(&SET[14..20], b"WINUSB");
        let prop = &SET[30..];
        assert_eq!(prop[0] as usize, prop.len());
        let name_len = prop[6] as usize;
        let name: Vec<u8> = prop[8..8 + name_len].iter().step_by(2).copied().collect();
        assert_eq!(name, b"DeviceInterfaceGUIDs\0");
        let data_len = prop[8 + name_len] as usize;
        let data: Vec<u8> = prop[10 + name_len..10 + name_len + data_len].iter().step_by(2).copied().collect();
        assert_eq!(data, b"{94952487-3D5A-4981-B569-5AA375AE5768}\0\0");
        assert_eq!(10 + name_len + data_len, prop.len());
    }
}
