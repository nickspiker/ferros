//! Between the departure machine's names and Photon Transport's (fgtw::flow names a device by its key, PT names an address).
//! Pure: no syscall, no clock, so the mapping is tested on the host (the tests below).

use alloc::vec::Vec;
use core::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use photon_core::pt::Addr;

/// A PT address as core's socket address, the form the machine takes a frame's source in (`Event::Frame::from`).
pub fn to_socket(a: &Addr) -> SocketAddr {
    let ip = if a.fam == 4 { IpAddr::V4(Ipv4Addr::new(a.ip[0], a.ip[1], a.ip[2], a.ip[3])) } else { IpAddr::V6(Ipv6Addr::from(a.ip)) };
    SocketAddr::new(ip, a.port)
}

/// A socket address the machine names (`Action::Seen`, `To::Addr`) as PT's.
pub fn from_socket(s: &SocketAddr) -> Addr {
    let mut ip = [0u8; 16];
    let fam = match s.ip() {
        IpAddr::V4(v4) => {
            ip[..4].copy_from_slice(&v4.octets());
            4
        }
        IpAddr::V6(v6) => {
            ip = v6.octets();
            6
        }
    };
    Addr { fam, ip, port: s.port() }
}

/// The machine's frames on their way: where each went, the frame's BLAKE3 (PT's `Sent` names a frame by it) and the device it was for.
/// An answer maps back only to the frame it names, so another frame's acknowledgement (a space push, a chain) never counts as this one taken.
pub struct Sends {
    out: Vec<(Addr, [u8; 32], [u8; 32])>,
}

impl Sends {
    pub const fn new() -> Sends {
        Sends { out: Vec::new() }
    }

    /// A frame for `device` to go to PT at `to`: true when it is not out there already. The same frame to the same address while one is out is not handed to PT again (a device heard every few seconds would otherwise start a transfer each time); the answer to the one out settles it.
    pub fn record(&mut self, to: Addr, hash: [u8; 32], device: [u8; 32]) -> bool {
        if self.out.iter().any(|(a, h, _)| *a == to && *h == hash) {
            return false;
        }
        self.out.push((to, hash, device));
        true
    }

    /// PT's answer for the frame `hash` to `to`: the device it was for, or None when it was not the machine's (dropped).
    pub fn answered(&mut self, to: &Addr, hash: &[u8; 32]) -> Option<[u8; 32]> {
        let i = self.out.iter().position(|(a, h, _)| a == to && h == hash)?;
        Some(self.out.swap_remove(i).2)
    }

    /// Frames out with no answer yet.
    #[cfg(test)]
    pub fn pending(&self) -> usize {
        self.out.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(last: u8, port: u16) -> Addr {
        let mut ip = [0u8; 16];
        ip[..4].copy_from_slice(&[192, 168, 1, last]);
        Addr { fam: 4, ip, port }
    }

    #[test]
    fn socket_round_trip_v4_and_v6() {
        let a = v4(156, 4383);
        let s = to_socket(&a);
        assert_eq!(s, "192.168.1.156:4383".parse::<SocketAddr>().unwrap());
        assert_eq!(from_socket(&s), a);
        let mut ip = [0u8; 16];
        ip[0] = 0xfe;
        ip[1] = 0x80;
        ip[15] = 7;
        let b = Addr { fam: 6, ip, port: 4383 };
        let s = to_socket(&b);
        assert_eq!(s, "[fe80::7]:4383".parse::<SocketAddr>().unwrap());
        assert_eq!(from_socket(&s), b);
    }

    #[test]
    fn an_answer_maps_to_the_device_its_frame_was_for() {
        let mut s = Sends::new();
        let (a, b) = (v4(2, 4383), v4(3, 4383));
        s.record(a, [1; 32], [0xA; 32]);
        s.record(b, [1; 32], [0xB; 32]);
        assert_eq!(s.answered(&b, &[1; 32]), Some([0xB; 32]));
        assert_eq!(s.answered(&a, &[1; 32]), Some([0xA; 32]));
        assert_eq!(s.pending(), 0);
    }

    #[test]
    fn another_frames_answer_is_not_the_machines() {
        let mut s = Sends::new();
        let a = v4(2, 4383);
        s.record(a, [1; 32], [0xA; 32]);
        // A space push to the same device acknowledged: not the departure frame.
        assert_eq!(s.answered(&a, &[2; 32]), None);
        // The same frame answered from another address (a device that moved): not this send either.
        assert_eq!(s.answered(&v4(9, 4383), &[1; 32]), None);
        assert_eq!(s.pending(), 1);
        assert_eq!(s.answered(&a, &[1; 32]), Some([0xA; 32]));
        // Answered once: a second answer for it maps to nothing.
        assert_eq!(s.answered(&a, &[1; 32]), None);
    }

    #[test]
    fn the_same_frame_out_twice_is_one_send() {
        let mut s = Sends::new();
        let a = v4(2, 4383);
        assert!(s.record(a, [1; 32], [0xA; 32]));
        // Heard again while it is out: not handed to PT again.
        assert!(!s.record(a, [1; 32], [0xA; 32]));
        assert_eq!(s.pending(), 1);
        // Answered (given up): the next hand-off goes out.
        assert_eq!(s.answered(&a, &[1; 32]), Some([0xA; 32]));
        assert!(s.record(a, [1; 32], [0xA; 32]));
    }
}
