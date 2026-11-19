//! Host tests against packets of the 2026-11-18 captures that carry no place: the link's setup and the AP's requests.

use super::*;

/// The AP's transport reset, as captured (24 bytes on the wire: 20 of packet, 4 of the CRC; no padding, it ends on a word).
const RESET_HEAD: [u8; 20] = [0x68, 0x43, 0x00, 0x10, 0x00, 0x00, 0x0A, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x05, 0x01, 0x00, 0xE8, 0x03];

#[test]
fn reset_frames_as_captured() {
    let mut out = [0u8; PACKET_MAX];
    let n = Link::reset(&mut out);
    assert_eq!(n, 24);
    assert_eq!(&out[..20], &RESET_HEAD[..]);
    let mut s: Scanner<{ PACKET_MAX * 2 }> = Scanner::new();
    s.feed(&out[..n]);
    match s.next() {
        Found::Packet(h, p) => {
            assert_eq!(h, Header { flags: 0, code: RESET, ack: 0, seq: 0, len: 10 });
            assert_eq!(p, &LINK_CONFIG);
        }
        Found::More => panic!("no packet"),
    }
}

#[test]
fn crc_is_ieee() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
}

/// The chip's bare ack as read on 2026-11-18 (the loud capture's first read: `68 43 00 00 0B 00 00 00 00 00`, then its CRC `AA 2F E5 0F`).
#[test]
fn captured_bare_ack_checks() {
    let wire: [u8; 16] = [0x68, 0x43, 0x00, 0x00, 0x0B, 0x00, 0x00, 0x00, 0x00, 0x00, 0xAA, 0x2F, 0xE5, 0x0F, 0, 0];
    let mut s: Scanner<{ PACKET_MAX * 2 }> = Scanner::new();
    s.feed(&wire);
    match s.next() {
        Found::Packet(h, p) => {
            assert_eq!((h.ack, h.seq, h.len), (0x0B, 0, 0));
            assert!(p.is_empty());
        }
        Found::More => panic!("the captured ack did not check"),
    }
    let mut out = [0u8; 16];
    let l = Link { chip_ack: 0, next_from_chip: 0x0B };
    assert_eq!(l.ack(&mut out), 16);
    assert_eq!(out, wire);
}

#[test]
fn scanner_spans_reads_and_skips_fill() {
    let mut a = [0u8; PACKET_MAX];
    let n = frame(0, 0, 5, 4, &OPEN, &mut a);
    let mut s: Scanner<{ PACKET_MAX * 2 }> = Scanner::new();
    let mut read = [0u8; 64];
    // The packet's first 7 bytes at the end of one read, the rest at the start of the next, zeros around.
    read[57..64].copy_from_slice(&a[..7]);
    s.feed(&read);
    assert_eq!(s.next(), Found::More);
    let mut read2 = [0u8; 64];
    read2[..n - 7].copy_from_slice(&a[7..n]);
    s.feed(&read2);
    match s.next() {
        Found::Packet(h, p) => {
            assert_eq!((h.ack, h.seq, h.len), (5, 4, 6));
            assert_eq!(p, &OPEN);
        }
        Found::More => panic!("the packet across two reads was not found"),
    }
    assert_eq!(s.next(), Found::More);
    assert_eq!(s.bad, 0);
}

#[test]
fn ap_datagram_matches_capture() {
    // The fix session's G#04D4 message as gnssd sent it (47 bytes).
    let want: [u8; 47] = [
        0x10, 0x02, 0x00, 0x00, 0x02, 0x00, 0xD4, 0x04, 0x02, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x14, 0x00, 0x01, 0x00, 0x0F, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    let mut out = [0u8; 128];
    let n = ap_datagram(&[AP_SESSION_ASK], &mut out);
    assert_eq!(&out[..n], &want[..]);
    assert_eq!(ap_datagram(&[AP_SESSION_OPEN], &mut out), 46);
    assert_eq!(ap_datagram(&[AP_START], &mut out), 50);
}

#[test]
fn discovery_answer_names_betp() {
    let ans: [u8; 42] = [
        0x0F, 0x01, 0x00, 0x00, 0x01, 0x00, 0xB4, 0x34, 0xC1, 0x4D, 0x9C, 0x66, 0x46, 0xC7, 0x8F, 0xF9, 0xF5, 0xD9, 0x10, 0xA4, 0xD2, 0xBD, 0x42, 0x45, 0x54, 0x50, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x01, 0x00, 0x00, 0x00,
    ];
    assert!(discovered_betp(&ans));
}

#[test]
fn datagram_joins_and_drops_resends() {
    let mut d: Datagram<64> = Datagram::new();
    let h = |seq, more| Header { flags: if more { MORE } else { 0 }, code: 0, ack: 0, seq, len: 3 };
    assert_eq!(d.take(&h(5, true), &[1, 2, 3]), Took::Part);
    assert_eq!(d.take(&h(5, true), &[1, 2, 3]), Took::Resend);
    assert_eq!(d.take(&h(6, false), &[4, 5, 6]), Took::Whole);
    assert_eq!(d.bytes(), &[1, 2, 3, 4, 5, 6]);
    assert_eq!(d.take(&h(7, false), &[7]), Took::Whole);
    assert_eq!(d.bytes(), &[7]);
}

#[test]
fn chip_records_walk() {
    // Two records, 16-byte heads: the first's next offset 16 + 2, the second's 0.
    let mut b = [0u8; 16 + 2 + 16 + 1];
    b[0..4].copy_from_slice(&0x0002_6538u32.to_le_bytes());
    b[8..10].copy_from_slice(&18u16.to_le_bytes());
    b[12..14].copy_from_slice(&16u16.to_le_bytes());
    b[14..16].copy_from_slice(&2u16.to_le_bytes());
    b[16] = 0xAA;
    b[17] = 0xBB;
    b[18..22].copy_from_slice(&0x0002_3448u32.to_le_bytes());
    b[30..32].copy_from_slice(&16u16.to_le_bytes());
    b[32..34].copy_from_slice(&1u16.to_le_bytes());
    b[34] = 0xCC;
    let mut it = chip_records(&b);
    let r0 = it.next().unwrap();
    let r1 = it.next().unwrap();
    assert!(it.next().is_none());
    assert_eq!((r0.word, r0.data), (0x0002_6538, &[0xAA, 0xBB][..]));
    assert_eq!((r1.word, r1.data), (0x0002_3448, &[0xCC][..]));
}
