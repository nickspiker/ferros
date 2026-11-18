//! The client against the fake ip: each wait is one held call (counted per op), each answer read as ip means it.
//! The peer's moves come from a test thread after a pause: the pause is the test's stand-in for the network, never something the client waits on.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use rt_tcp::ip;
use rt_tcp::sys::IP_SLOT;
use rt_tcp::tcp::{Conn, Error, Family, Ip, Until};

const CONNECT: usize = 4;
const SEND: usize = 5;
const RECV: usize = 6;
const CLOSE: usize = 7;
const WATCH: usize = 11;
const ROOM: usize = 14;

fn ipv() -> Ip {
    Ip::at(IP_SLOT)
}

fn v4() -> SocketAddr {
    SocketAddr::new(Ipv4Addr::new(192, 0, 2, 1).into(), 80)
}

/// The peer's move `f`, `ms` from now, on a thread of its own.
fn later(ms: u64, f: impl FnOnce() + Send + 'static) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(ms));
        f();
    })
}

/// A connection open on handle 0, as the tests below start.
fn opened() -> Conn {
    let p = later(1 << 4, || ip::open(0));
    let c = Conn::connect(ipv(), v4(), Until::NEVER).unwrap();
    p.join().unwrap();
    c
}

#[test]
fn connect_waits_for_the_open_in_one_held_call() {
    let _t = ip::take();
    let p = later(1 << 6, || ip::open(0));
    let t = Instant::now();
    let c = Conn::connect(ipv(), v4(), Until::NEVER).unwrap();
    assert!(t.elapsed() >= Duration::from_millis(1 << 6));
    p.join().unwrap();
    assert_eq!(ip::calls(CONNECT), 1);
    assert_eq!(ip::calls(RECV), 1, "the wait for the open is one held receive");
    let k = ip::conn(0);
    assert_eq!((k.family, k.port), (4, 80));
    drop(c);
    assert!(ip::conn(0).fin_queued, "a drop closes");
    assert_eq!(ip::calls(CLOSE), 1);
}

#[test]
fn connect_refused_and_unanswered() {
    let _t = ip::take();
    let p = later(1 << 4, || ip::fail(0, 1));
    assert_eq!(Conn::connect(ipv(), v4(), Until::NEVER).err(), Some(Error::Refused));
    p.join().unwrap();
    // The failed connection's slot is closed by the drop; the next connect takes slot 1.
    let p = later(1 << 4, || ip::fail(1, 2));
    assert_eq!(Conn::connect(ipv(), v4(), Until::NEVER).err(), Some(Error::Unanswered));
    p.join().unwrap();
    assert_eq!(ip::calls(CLOSE), 2);
}

#[test]
fn connect_times_out_at_the_deadline_and_closes() {
    let _t = ip::take();
    let t = Instant::now();
    let r = Conn::connect(ipv(), v4(), Until::after(Duration::from_millis(1 << 6)));
    let e = t.elapsed();
    assert_eq!(r.err(), Some(Error::TimedOut));
    assert!(e >= Duration::from_millis(1 << 6) && e < Duration::from_millis(1 << 9), "{e:?}");
    assert_eq!(ip::calls(RECV), 1, "held to the deadline in one call");
    assert!(ip::conn(0).fin_queued);
}

#[test]
fn a_mapped_ipv4_address_goes_as_ipv4_and_ipv6_as_ipv6() {
    let _t = ip::take();
    let p = later(1 << 4, || ip::open(0));
    let mapped = SocketAddr::new(Ipv4Addr::new(192, 0, 2, 1).to_ipv6_mapped().into(), 80);
    let a = Conn::connect(ipv(), mapped, Until::NEVER).unwrap();
    p.join().unwrap();
    assert_eq!(ip::conn(0).family, 4);
    let p = later(1 << 4, || ip::open(1));
    let b = Conn::connect(ipv(), SocketAddr::new(Ipv6Addr::LOCALHOST.into(), 443), Until::NEVER).unwrap();
    p.join().unwrap();
    assert_eq!((ip::conn(1).family, ip::conn(1).port), (6, 443));
    drop((a, b));
}

#[test]
fn read_is_held_until_data_then_ok_zero_at_the_close() {
    let _t = ip::take();
    let c = opened();
    let before = ip::calls(RECV);
    let p = later(1 << 5, || ip::push(0, b"hello"));
    let mut buf = [0u8; 64];
    assert_eq!(c.read(&mut buf, Until::NEVER), Ok(5));
    assert_eq!(&buf[..5], b"hello");
    p.join().unwrap();
    assert_eq!(ip::calls(RECV) - before, 1);
    let p = later(1 << 5, || ip::peer_close(0));
    assert_eq!(c.read(&mut buf, Until::NEVER), Ok(0));
    p.join().unwrap();
    assert_eq!(ip::calls(RECV) - before, 2);
}

#[test]
fn the_data_before_the_close_is_read_first() {
    let _t = ip::take();
    let c = opened();
    ip::push(0, b"last words");
    ip::peer_close(0);
    let mut buf = [0u8; 4];
    let mut got = Vec::new();
    loop {
        match c.read(&mut buf, Until::NEVER).unwrap() {
            0 => break,
            n => got.extend_from_slice(&buf[..n]),
        }
    }
    assert_eq!(got, b"last words");
}

#[test]
fn a_one_byte_buffer_reads_a_byte_at_a_time() {
    let _t = ip::take();
    let c = opened();
    ip::push(0, b"ab");
    let mut b = [0u8; 1];
    assert_eq!(c.read(&mut b, Until::NEVER), Ok(1));
    assert_eq!(b[0], b'a');
    assert_eq!(c.read(&mut b, Until::NEVER), Ok(1));
    assert_eq!(b[0], b'b');
    assert_eq!(c.read(&mut [], Until::NEVER), Ok(0));
}

#[test]
fn a_quiet_read_times_out_at_its_deadline_in_one_call() {
    let _t = ip::take();
    let c = opened();
    let before = ip::calls(RECV);
    let t = Instant::now();
    let mut buf = [0u8; 16];
    assert_eq!(c.read(&mut buf, Until::after(Duration::from_millis(1 << 6))), Err(Error::TimedOut));
    let e = t.elapsed();
    assert!(e >= Duration::from_millis(1 << 6) && e < Duration::from_millis(1 << 9), "{e:?}");
    assert_eq!(ip::calls(RECV) - before, 1);
    // A deadline already passed answers what is there now, or the timeout.
    ip::push(0, b"x");
    assert_eq!(c.read(&mut buf, Until::at(1)), Ok(1));
    assert_eq!(c.read(&mut buf, Until::at(1)), Err(Error::TimedOut));
}

#[test]
fn reset_and_unanswered_after_the_open() {
    let _t = ip::take();
    let c = opened();
    let p = later(1 << 4, || ip::fail(0, 1));
    assert_eq!(c.read(&mut [0u8; 8], Until::NEVER), Err(Error::Reset));
    p.join().unwrap();
    assert_eq!(c.write(b"more", Until::NEVER), Err(Error::Reset), "a send to a failed connection says why");
    drop(c);
    let p = later(1 << 4, || ip::open(1));
    let c = Conn::connect(ipv(), v4(), Until::NEVER).unwrap();
    p.join().unwrap();
    ip::fail(1, 2);
    assert_eq!(c.read(&mut [0u8; 8], Until::NEVER), Err(Error::Unanswered));
}

#[test]
fn a_full_send_buffer_is_waited_out_at_ip() {
    let _t = ip::take();
    ip::with(|s| s.txb = 1 << 4);
    let c = opened();
    let data: Vec<u8> = (0..(1u32 << 8)).map(|k| k as u8).collect();
    // The peer ACKs 8 bytes every 2 ms until all of it is thru.
    let p = thread::spawn(|| {
        let mut thru = 0usize;
        while thru < 1 << 8 {
            thread::sleep(Duration::from_millis(2));
            let k = ip::conn(0);
            if k.tx_len > 0 {
                let n = k.tx_len.min(8);
                ip::drain(0, n);
                thru += n as usize;
            }
        }
    });
    c.write_all(&data, Until::NEVER).unwrap();
    p.join().unwrap();
    assert_eq!(ip::conn(0).sent, data, "every byte once, in order");
    // Each wait for room is one held call, answered when the room asked for is free: never more than one a SEND that found the buffer full.
    let (sends, rooms) = (ip::calls(SEND), ip::calls(ROOM));
    assert!(rooms <= sends, "{rooms} room waits for {sends} sends");
    assert!(sends <= 1 << 6, "{sends} sends for 256 bytes thru a 16-byte buffer");
}

#[test]
fn a_full_send_buffer_times_out_at_the_deadline() {
    let _t = ip::take();
    ip::with(|s| s.txb = 1 << 3);
    let c = opened();
    assert_eq!(c.write(&[7u8; 64], Until::NEVER), Ok(8));
    let t = Instant::now();
    assert_eq!(c.write(&[7u8; 64], Until::after(Duration::from_millis(1 << 6))), Err(Error::TimedOut));
    assert!(t.elapsed() >= Duration::from_millis(1 << 6));
    assert_eq!(ip::calls(ROOM), 1);
}

#[test]
fn an_ip_without_the_room_op_is_said_not_asked_again() {
    let _t = ip::take();
    ip::with(|s| {
        s.txb = 1 << 3;
        s.room_op = false;
    });
    let c = opened();
    assert_eq!(c.write(&[1u8; 8], Until::NEVER), Ok(8));
    assert_eq!(c.write(&[1u8; 8], Until::NEVER), Err(Error::BadReply));
    assert_eq!(ip::calls(ROOM), 1);
}

#[test]
fn nobody_present_is_waited_out_on_one_watch() {
    let _t = ip::take();
    let c = Arc::new(opened());
    ip::set_person(false);
    let r = Arc::clone(&c);
    let reader = thread::spawn(move || {
        let mut buf = [0u8; 16];
        let n = r.read(&mut buf, Until::NEVER).unwrap();
        buf[..n].to_vec()
    });
    thread::sleep(Duration::from_millis(1 << 5));
    let (recvs, watches) = (ip::calls(RECV), ip::calls(WATCH));
    ip::push(0, b"back");
    ip::set_person(true);
    assert_eq!(reader.join().unwrap(), b"back");
    // While nobody was present: one receive answered with nothing, one watch answered at once with the flags, one held until the person came back.
    assert!(recvs <= 3 && watches <= 2, "{recvs} receives and {watches} watches while nobody was present");
    assert!(ip::calls(WATCH) <= 3);
}

#[test]
fn a_reader_and_a_writer_on_two_threads() {
    let _t = ip::take();
    let c = Arc::new(opened());
    let r = Arc::clone(&c);
    let reader = thread::spawn(move || {
        let mut got = Vec::new();
        let mut buf = [0u8; 8];
        loop {
            match r.read(&mut buf, Until::NEVER).unwrap() {
                0 => return got,
                n => got.extend_from_slice(&buf[..n]),
            }
        }
    });
    // The peer answers once the request is all in.
    let peer = thread::spawn(|| {
        loop {
            if ip::conn(0).sent.len() == 25 {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        ip::push(0, b"HTTP/1.1 200 OK\r\n\r\nok");
        ip::peer_close(0);
    });
    c.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n", Until::NEVER).unwrap();
    peer.join().unwrap();
    assert_eq!(reader.join().unwrap(), b"HTTP/1.1 200 OK\r\n\r\nok");
}

#[test]
fn two_readers_on_one_conn_take_turns() {
    let _t = ip::take();
    let c = Arc::new(opened());
    let readers: Vec<_> = (0..2)
        .map(|_| {
            let r = Arc::clone(&c);
            thread::spawn(move || {
                let mut b = [0u8; 1];
                r.read(&mut b, Until::NEVER).unwrap();
                b[0]
            })
        })
        .collect();
    thread::sleep(Duration::from_millis(1 << 5));
    let before = ip::calls(RECV);
    ip::push(0, b"a");
    thread::sleep(Duration::from_millis(1 << 4));
    ip::push(0, b"b");
    let mut got: Vec<u8> = readers.into_iter().map(|r| r.join().unwrap()).collect();
    got.sort();
    assert_eq!(got, b"ab");
    // The second reader waited on the lock, not at ip: two receives in all, neither answered early by the other.
    assert!(ip::calls(RECV) - before <= 1, "{} more receives", ip::calls(RECV) - before);
}

#[test]
fn resolve_reads_ips_answers() {
    let _t = ip::take();
    ip::with(|s| s.resolve = (0, 4, [203, 0, 113, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
    assert_eq!(ipv().resolve("fgtw.org", Family::Any, Until::NEVER), Ok(Ipv4Addr::new(203, 0, 113, 7).into()));
    let mut a6 = [0u8; 16];
    a6[0] = 0x20;
    a6[1] = 1;
    a6[15] = 9;
    ip::with(|s| s.resolve = (0, 6, a6));
    assert_eq!(ipv().resolve("fgtw.org", Family::V6, Until::NEVER), Ok(Ipv6Addr::from(a6).into()));
    ip::with(|s| s.resolve = (2, 0, [0; 16]));
    assert_eq!(ipv().resolve("no.such", Family::Any, Until::NEVER), Err(Error::NoAddress));
    assert_eq!(ipv().resolve("", Family::Any, Until::NEVER), Err(Error::BadName));
    assert_eq!(ipv().resolve(&"a".repeat(254), Family::Any, Until::NEVER), Err(Error::BadName));
}

#[test]
fn no_grant_is_no_server() {
    let _t = ip::take();
    assert_eq!(Conn::connect(Ip::at(7), v4(), Until::NEVER).err(), Some(Error::NoServer));
}

#[test]
fn every_connection_busy() {
    let _t = ip::take();
    ip::with(|s| s.conns.iter_mut().for_each(|c| c.state = 2));
    assert_eq!(Conn::connect(ipv(), v4(), Until::NEVER).err(), Some(Error::Busy));
}

#[test]
fn conn_is_send_and_sync() {
    fn both<T: Send + Sync>() {}
    both::<Conn>();
}
