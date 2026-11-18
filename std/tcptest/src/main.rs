//! ferros_rt's TCP client on the device (docs/STD-TARGET.md "TCP"): fgtw.org thru ip's resolver, a GET of /status on port 80 read to the peer's close, the same over a second connection read on its own thread (held at ip) while the main thread writes, and a read held to a deadline on a connection that brings nothing.
//! Spawn: `ferros-bridge spawn tcptest.bin --name tcptest --use udp --entropy`: slot 0 the console, 1 the `udp` endpoint (`UDP_SLOT`).
//! Each step is a console line; the exit code is the number of steps that failed (0 all passed).
//! Addresses are not printed (ip's own rule for its lookups: the person's), only their family.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ferros_rt::tcp::{Conn, Error, Family, Ip, Until};

/// The `udp` endpoint's grant slot: first after the console in the spawn line above.
const UDP_SLOT: u64 = 1;
const HOST: &str = "fgtw.org";
/// The exchange's deadline, as fgtw's post has (WHY a deadline: a server gone silent mid-reply would hold the read for ever).
const EXCHANGE: Duration = Duration::from_secs(1 << 4);
/// The deadline the held read is given on a connection that sends nothing: 2 s.
const QUIET: Duration = Duration::from_secs(1 << 1);

fn request() -> Vec<u8> {
    format!("GET /status HTTP/1.1\r\nHost: {HOST}\r\nUser-Agent: ferros tcptest\r\nConnection: close\r\n\r\n").into_bytes()
}

/// Everything up to the peer's close.
fn read_to_close(c: &Conn, until: Until) -> Result<Vec<u8>, Error> {
    let mut got = Vec::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        match c.read(&mut buf, until)? {
            0 => return Ok(got),
            n => got.extend_from_slice(&buf[..n]),
        }
    }
}

/// The reply's status line and length, said; whether it was a 200.
fn say_reply(what: &str, got: &[u8], t: Instant) -> bool {
    let line = got.split(|&b| b == b'\r').next().unwrap_or(&[]);
    println!("tcptest: {what}: A#{} bytes in A#{} ms, \"{}\"", got.len(), t.elapsed().as_millis(), String::from_utf8_lossy(line));
    line.starts_with(b"HTTP/1.1 200")
}

fn main() {
    let ip = Ip::at(UDP_SLOT);
    let mut failed = 0;

    // Step 0: the name.
    let t = Instant::now();
    let addr = match ip.resolve(HOST, Family::Any, Until::after(EXCHANGE)) {
        Ok(a) => {
            println!("tcptest: 0 resolve {HOST}: IPv{} in A#{} ms", if a.is_ipv4() { 4 } else { 6 }, t.elapsed().as_millis());
            SocketAddr::new(a, 80)
        }
        Err(e) => {
            println!("tcptest: 0 resolve {HOST}: {e:?}; nothing else to try");
            std::process::exit(4);
        }
    };

    // Step 1: one exchange on one thread: connect, write, read to the close, all within one deadline.
    let t = Instant::now();
    let until = Until::after(EXCHANGE);
    let one = Conn::connect(ip, addr, until).and_then(|c| {
        println!("tcptest: 1 connected in A#{} ms", t.elapsed().as_millis());
        c.write_all(&request(), until)?;
        read_to_close(&c, until)
    });
    match one {
        Ok(got) if say_reply("1 GET /status", &got, t) => {}
        Ok(_) => failed += 1,
        Err(e) => {
            println!("tcptest: 1 GET /status: {e:?}");
            failed += 1;
        }
    }

    // Step 2: the read held on a thread of its own (no deadline: until the peer speaks or closes) while this thread writes.
    let t = Instant::now();
    match Conn::connect(ip, addr, Until::after(EXCHANGE)) {
        Ok(c) => {
            let c = Arc::new(c);
            let r = Arc::clone(&c);
            let reader = std::thread::Builder::new().name("tcp-read".into()).spawn(move || read_to_close(&r, Until::NEVER)).expect("the reader thread");
            let wrote = c.write_all(&request(), Until::after(EXCHANGE));
            match (wrote, reader.join().expect("the reader thread ended")) {
                (Ok(()), Ok(got)) if say_reply("2 GET /status, read on its own thread", &got, t) => {}
                (Ok(()), Ok(_)) => failed += 1,
                (w, r) => {
                    println!("tcptest: 2 threads: write {w:?}, read {:?}", r.map(|g| g.len()));
                    failed += 1;
                }
            }
        }
        Err(e) => {
            println!("tcptest: 2 connect: {e:?}");
            failed += 1;
        }
    }

    // Step 3: a connection that is sent nothing brings nothing: the read is held to its deadline exactly and says so.
    match Conn::connect(ip, addr, Until::after(EXCHANGE)) {
        Ok(c) => {
            let t = Instant::now();
            let mut buf = [0u8; 64];
            match c.read(&mut buf, Until::after(QUIET)) {
                Err(Error::TimedOut) => println!("tcptest: 3 quiet read: timed out after A#{} ms (asked A#{})", t.elapsed().as_millis(), QUIET.as_millis()),
                other => {
                    println!("tcptest: 3 quiet read: {other:?} after A#{} ms, not the timeout", t.elapsed().as_millis());
                    failed += 1;
                }
            }
        }
        Err(e) => {
            println!("tcptest: 3 connect: {e:?}");
            failed += 1;
        }
    }

    println!("tcptest: A#{failed} of 4 steps failed");
    std::process::exit(failed);
}
