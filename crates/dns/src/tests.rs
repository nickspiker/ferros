//! Host tests (`cargo test -p ferros_dns`): names, the question, answers as servers write them and as an attacker or a broken network might, and the cache.

extern crate std;
use super::*;
use std::vec::Vec;

fn name(h: &str) -> Name {
    Name::from_host(h.as_bytes()).unwrap()
}

/// A response to question `id` for `n`'s `qtype`, built the way a server writes one: the header, the question, then the records (each already in the wire's form).
fn response(id: u16, flags: u16, n: &Name, qtype: u16, answers: &[Vec<u8>], authority: &[Vec<u8>]) -> Vec<u8> {
    let mut m = Vec::new();
    m.extend_from_slice(&id.to_be_bytes());
    m.extend_from_slice(&flags.to_be_bytes());
    m.extend_from_slice(&1u16.to_be_bytes());
    m.extend_from_slice(&(answers.len() as u16).to_be_bytes());
    m.extend_from_slice(&(authority.len() as u16).to_be_bytes());
    m.extend_from_slice(&0u16.to_be_bytes());
    m.extend_from_slice(n.wire());
    m.extend_from_slice(&qtype.to_be_bytes());
    m.extend_from_slice(&1u16.to_be_bytes());
    for r in answers.iter().chain(authority) {
        m.extend_from_slice(r);
    }
    m
}

/// A record: its owner (already in the wire's form, or a pointer), type, TTL and data.
fn rr(owner: &[u8], ty: u16, ttl: u32, data: &[u8]) -> Vec<u8> {
    let mut r = Vec::from(owner);
    r.extend_from_slice(&ty.to_be_bytes());
    r.extend_from_slice(&1u16.to_be_bytes());
    r.extend_from_slice(&ttl.to_be_bytes());
    r.extend_from_slice(&(data.len() as u16).to_be_bytes());
    r.extend_from_slice(data);
    r
}

/// A pointer to the question's name (offset 12, where every response puts it).
const AT_Q: [u8; 2] = [0xC0, 12];
/// A standard response, recursion desired and available, no error.
const OK: u16 = 0x8180;

fn soa(ttl: u32, minimum: u32) -> Vec<u8> {
    let mut d = Vec::from(&b"\x02ns\x00\x05admin\x00"[..]);
    for f in [1u32, 2, 3, 4, minimum] {
        d.extend_from_slice(&f.to_be_bytes());
    }
    rr(&[0], SOA, ttl, &d)
}

#[test]
fn names_in_the_wire_form() {
    assert_eq!(name("Fgtw.ORG").wire(), b"\x04fgtw\x03org\x00");
    assert_eq!(name("fgtw.org.").wire(), name("fgtw.org").wire());
    assert_eq!(name("a-b_c.x9").wire(), b"\x05a-b_c\x02x9\x00");
    for bad in ["", ".", "a..b", ".a", "a b", "a/b", "ä.org", "a.b.."] {
        assert!(Name::from_host(bad.as_bytes()).is_none(), "{bad:?}");
    }
    let l63 = "a".repeat(63);
    assert!(Name::from_host(l63.as_bytes()).is_some());
    assert!(Name::from_host("a".repeat(64).as_bytes()).is_none());
    // 255 bytes in the wire's form is the most: four labels of 63 make 4 * 64 + 1 = 257; three of 63 and one of 61 make 255.
    let max = [l63.as_str(), l63.as_str(), l63.as_str(), &"b".repeat(61)].join(".");
    assert_eq!(name(&max).wire().len(), 255);
    let over = [l63.as_str(), l63.as_str(), l63.as_str(), &"b".repeat(62)].join(".");
    assert!(Name::from_host(over.as_bytes()).is_none());
}

#[test]
fn the_question() {
    let mut q = [0u8; QUERY_MAX];
    let n = query(0xBEEF, &name("fgtw.org"), AAAA, &mut q);
    assert_eq!(&q[..n], b"\xBE\xEF\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00\x04fgtw\x03org\x00\x00\x1C\x00\x01");
    // The longest name fits the buffer.
    let l63 = "a".repeat(63);
    let max = [l63.as_str(), l63.as_str(), l63.as_str(), &"b".repeat(61)].join(".");
    assert_eq!(query(1, &name(&max), A, &mut q), QUERY_MAX);
}

#[test]
fn an_address_found() {
    let n = name("fgtw.org");
    let m = response(7, OK, &n, A, &[rr(&AT_Q, A, 300, &[192, 0, 2, 1])], &[]);
    let mut addr = [0u8; 16];
    addr[..4].copy_from_slice(&[192, 0, 2, 1]);
    assert_eq!(read(&m, 7, &n, A), Some(Reply::Found { addr, ttl: 300 }));
    let v6 = [0x20, 1, 0xd, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
    let m = response(9, OK, &n, AAAA, &[rr(&AT_Q, AAAA, 60, &v6)], &[]);
    assert_eq!(read(&m, 9, &n, AAAA), Some(Reply::Found { addr: v6, ttl: 60 }));
    // The owner written out in another case is the same name.
    let m = response(7, OK, &n, A, &[rr(b"\x04FGTW\x03Org\x00", A, 300, &[192, 0, 2, 1])], &[]);
    assert_eq!(read(&m, 7, &n, A), Some(Reply::Found { addr, ttl: 300 }));
    // A TTL with its high bit set is 0 (RFC 2181 8).
    let m = response(7, OK, &n, A, &[rr(&AT_Q, A, 0x8000_0000, &[192, 0, 2, 1])], &[]);
    assert_eq!(read(&m, 7, &n, A), Some(Reply::Found { addr, ttl: 0 }));
}

#[test]
fn a_chain_of_aliases() {
    let n = name("www.example.com");
    // www.example.com CNAME cdn.example.net (written out at offset 33+12 = past the question), cdn.example.net A, by pointer to the CNAME's data. The question ends at 12 + 17 + 4 = 33; the CNAME's owner pointer, type, class, TTL and length take 12 more, so its data starts at 45.
    let cname = rr(&AT_Q, CNAME, 600, b"\x03cdn\x07example\x03net\x00");
    let a = rr(&[0xC0, 45], A, 120, &[198, 51, 100, 7]);
    let m = response(1, OK, &n, A, &[cname.clone(), a.clone()], &[]);
    let mut addr = [0u8; 16];
    addr[..4].copy_from_slice(&[198, 51, 100, 7]);
    assert_eq!(read(&m, 1, &n, A), Some(Reply::Found { addr, ttl: 120 }));
    // Out of order: the address first, the alias after; the least TTL on the way.
    let cname = rr(&AT_Q, CNAME, 30, b"\x03cdn\x07example\x03net\x00");
    let a = rr(b"\x03cdn\x07example\x03net\x00", A, 120, &[198, 51, 100, 7]);
    let m = response(1, OK, &n, A, &[a, cname], &[]);
    assert_eq!(read(&m, 1, &n, A), Some(Reply::Found { addr, ttl: 30 }));
    // An address for another name than the chain's end is not ours.
    let other = rr(b"\x05other\x03net\x00", A, 120, &[203, 0, 113, 9]);
    let m = response(1, OK, &n, A, &[other], &[soa(60, 30)]);
    assert_eq!(read(&m, 1, &n, A), Some(Reply::NoName { ttl: 30 }));
    // A loop of aliases is a failed answer, not a hang.
    let x = rr(&AT_Q, CNAME, 60, b"\x01y\x00");
    let y = rr(b"\x01y\x00", CNAME, 60, &n.wire()[..]);
    let m = response(1, OK, &n, A, &[x, y], &[]);
    assert_eq!(read(&m, 1, &n, A), Some(Reply::Failed));
}

#[test]
fn negative_answers() {
    let n = name("nothing.example");
    // NXDOMAIN: the SOA's TTL or minimum, the smaller.
    let m = response(3, OK | 3, &n, A, &[], &[soa(900, 60)]);
    assert_eq!(read(&m, 3, &n, A), Some(Reply::NoName { ttl: 60 }));
    let m = response(3, OK | 3, &n, A, &[], &[soa(40, 3600)]);
    assert_eq!(read(&m, 3, &n, A), Some(Reply::NoName { ttl: 40 }));
    // Held to NEG_MAX.
    let m = response(3, OK | 3, &n, A, &[], &[soa(86400, 86400)]);
    assert_eq!(read(&m, 3, &n, A), Some(Reply::NoName { ttl: NEG_MAX }));
    // NODATA (no error, no record of the type) without an SOA.
    let m = response(3, OK, &n, AAAA, &[], &[]);
    assert_eq!(read(&m, 3, &n, AAAA), Some(Reply::NoName { ttl: NEG_NO_SOA }));
    // SERVFAIL, REFUSED: the server could not say.
    assert_eq!(read(&response(3, OK | 2, &n, A, &[], &[]), 3, &n, A), Some(Reply::Failed));
    assert_eq!(read(&response(3, OK | 5, &n, A, &[], &[]), 3, &n, A), Some(Reply::Failed));
    // Cut short (TC) before any address: says nothing.
    assert_eq!(read(&response(3, OK | 0x0200, &n, A, &[], &[]), 3, &n, A), Some(Reply::Failed));
}

#[test]
fn not_our_answer() {
    let n = name("fgtw.org");
    let good = response(7, OK, &n, A, &[rr(&AT_Q, A, 300, &[192, 0, 2, 1])], &[]);
    assert!(read(&good, 7, &n, A).is_some());
    // Another id.
    assert_eq!(read(&good, 8, &n, A), None);
    // Not a response (a question sent to us).
    let mut q = good.clone();
    q[2] &= 0x7F;
    assert_eq!(read(&q, 7, &n, A), None);
    // Another opcode.
    let mut q = good.clone();
    q[2] |= 2 << 3;
    assert_eq!(read(&q, 7, &n, A), None);
    // Another question: the name, the type, the class, or two questions.
    assert_eq!(read(&good, 7, &name("fgtw.net"), A), None);
    assert_eq!(read(&good, 7, &n, AAAA), None);
    let mut q = good.clone();
    q[12 + 10 + 3] = 3;
    assert_eq!(read(&q, 7, &n, A), None);
    let mut q = good.clone();
    q[5] = 2;
    assert_eq!(read(&q, 7, &n, A), None);
    // An answer record running past the end.
    let mut q = good.clone();
    q.truncate(q.len() - 1);
    assert_eq!(read(&q, 7, &n, A), None);
    // More answers counted than the message holds.
    let mut q = good.clone();
    q[7] = 2;
    assert_eq!(read(&q, 7, &n, A), None);
    // Too short for a header.
    assert_eq!(read(&good[..11], 7, &n, A), None);
}

#[test]
fn pointers_only_point_back() {
    let n = name("fgtw.org");
    // An owner pointing at itself, ahead, or into a loop of two.
    for owner in [[0xC0u8, 30], [0xC0, 200]] {
        let m = response(7, OK, &n, A, &[rr(&owner, A, 300, &[192, 0, 2, 1])], &[]);
        assert_eq!(read(&m, 7, &n, A), None, "{owner:?}");
    }
    // A reserved label type (01 or 10 in the high bits).
    let m = response(7, OK, &n, A, &[rr(&[0x41, 0], A, 300, &[192, 0, 2, 1])], &[]);
    assert_eq!(read(&m, 7, &n, A), None);
    // A label then a pointer back to it, over and over, ends at the name's 255 bytes.
    let mut m = response(7, OK, &n, A, &[], &[]);
    m[7] = 1;
    let at = m.len();
    m.extend_from_slice(&[3, b'a', b'b', b'c', 0xC0, at as u8]);
    m.extend_from_slice(&[0, 1, 0, 1, 0, 0, 0, 1, 0, 4, 1, 2, 3, 4]);
    assert_eq!(read(&m, 7, &n, A), None);
}

/// Every prefix of real answers, and random bytes under a valid header, read without a panic (the checks hold for any message).
#[test]
fn anything_reads_without_a_panic() {
    let n = name("www.example.com");
    let cname = rr(&AT_Q, CNAME, 600, b"\x03cdn\x07example\x03net\x00");
    let a = rr(&[0xC0, 45], A, 120, &[198, 51, 100, 7]);
    let m = response(1, OK, &n, A, &[cname, a], &[soa(60, 30)]);
    for k in 0..=m.len() {
        let _ = read(&m[..k], 1, &n, A);
    }
    let mut s: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    for _ in 0..1 << 16 {
        let mut b = m.clone();
        let flips = (next() % 8) as usize;
        for _ in 0..flips {
            let at = 12 + (next() as usize % (b.len() - 12));
            b[at] = next() as u8;
        }
        let k = 12 + (next() as usize % (b.len() - 11));
        let _ = read(&b[..k], 1, &n, A);
    }
}

#[test]
fn the_cache_by_ttl() {
    let mut c: Cache<4> = Cache::new();
    let n = name("fgtw.org");
    let i = c.slot(&n, A, 0).unwrap();
    assert_eq!(c.look(i, 0, 1), Look::Unknown);
    let addr = [192, 0, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    c.found(i, addr, 300, 10, 1);
    assert_eq!(c.slot(&n, A, 20), Some(i));
    assert_eq!(c.look(i, 20, 1), Look::Fresh { addr, left: 290 });
    // Another type is another record.
    assert_ne!(c.slot(&n, AAAA, 20), Some(i));
    // Past its TTL: the last good address, stale.
    assert_eq!(c.look(i, 310, 1), Look::Stale { addr });
    // A lookup that brought nothing keeps it another TTL.
    assert_eq!(c.lost(i, 400, 1), Some((addr, 300)));
    assert_eq!(c.look(i, 400, 1), Look::Fresh { addr, left: 300 });
    // The server's "no such name" replaces it, for the negative TTL.
    c.no_name(i, 8, 800, 1);
    assert_eq!(c.look(i, 807, 1), Look::NoName);
    assert_eq!(c.look(i, 808, 1), Look::Unknown);
    // Nothing kept, nothing to keep.
    assert_eq!(c.lost(i, 900, 1), None);
    // Ticks: 2^10 a second.
    c.found(i, addr, 3, 0, 1 << 10);
    assert_eq!(c.look(i, (1 << 10) + 5, 1 << 10), Look::Fresh { addr, left: 1 });
    assert_eq!(c.look(i, 3 << 10, 1 << 10), Look::Stale { addr });
}

#[test]
fn the_least_recently_asked_gives_way() {
    let mut c: Cache<2> = Cache::new();
    let a = c.slot(&name("a.org"), A, 1).unwrap();
    let b = c.slot(&name("b.org"), A, 2).unwrap();
    // a asked again: b is now the least recent.
    assert_eq!(c.slot(&name("a.org"), A, 3), Some(a));
    assert_eq!(c.slot(&name("c.org"), A, 4), Some(b));
    assert!(c.r[b].name == name("c.org") && c.r[b].have == 0);
    // A record in a lookup never gives way; with every one in a lookup there is no room.
    c.r[a].asking = true;
    assert_eq!(c.slot(&name("d.org"), A, 5), Some(b));
    c.r[b].asking = true;
    assert_eq!(c.slot(&name("e.org"), A, 6), None);
}

#[test]
fn failed_connections_ask_again() {
    let mut c: Cache<4> = Cache::new();
    let addr = [192, 0, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let i = c.slot(&name("fgtw.org"), A, 0).unwrap();
    c.found(i, addr, 300, 0, 1);
    // One failure is the network's; an opening in between starts the count again.
    for _ in 0..FAILURES - 1 {
        c.conn(4, &addr, false, 10);
    }
    c.conn(4, &addr, true, 10);
    for _ in 0..FAILURES - 1 {
        c.conn(4, &addr, false, 10);
    }
    assert_eq!(c.look(i, 10, 1), Look::Fresh { addr, left: 290 });
    // Another family's address of the same bytes is another address.
    c.conn(6, &addr, false, 10);
    assert_eq!(c.look(i, 10, 1), Look::Fresh { addr, left: 290 });
    // The FAILURES-th in a row: stale now, the address still there to use while it is asked for again.
    c.conn(4, &addr, false, 10);
    assert_eq!(c.look(i, 10, 1), Look::Stale { addr });
    assert_eq!(c.r[i].fails, 0);
}
