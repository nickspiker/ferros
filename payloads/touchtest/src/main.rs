//! A client of the `touch` endpoint: asks what the panel is, then reports contacts for eight seconds — every change as a line, and the held table's pressure while a finger stays down. This proves the endpoint the way `shelltest` proves `display`.
//!
//! Spawn after `touch`: ferros-bridge spawn touchtest.bin 00 --name touchtest --use touch
//! Grant slots: 0 console, 1 touch (use).

#![no_std]
#![no_main]

use ferros_sys::{call, cap, console, freq, now, sleep_until};

const S_TOUCH: u64 = 1;
const REC: usize = 12;

/// Append `v` as decimal. The project writes numbers as `A#` decimal or `G#` hex, never a bare numeral.
fn num(buf: &mut [u8], at: &mut usize, v: u64) {
    buf[*at] = b'A';
    buf[*at + 1] = b'#';
    *at += 2;
    if v == 0 {
        buf[*at] = b'0';
        *at += 1;
        return;
    }
    let mut d = [0u8; 20];
    let mut n = 0;
    let mut v = v;
    while v > 0 {
        d[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    while n > 0 {
        n -= 1;
        buf[*at] = d[n];
        *at += 1;
    }
}

fn text(buf: &mut [u8], at: &mut usize, s: &[u8]) {
    for &c in s {
        buf[*at] = c;
        *at += 1;
    }
}

fn u16le(b: &[u8]) -> u16 {
    b[0] as u16 | (b[1] as u16) << 8
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _input: u64, _input_len: u64) -> ! {
    let ep = cap(grants, S_TOUCH);
    let mut r = [0u8; 256];
    let mut line = [0u8; 256];

    let n = call(ep, &[2], &mut r);
    let mut at = 0;
    if n >= 5 {
        text(&mut line, &mut at, b"touchtest: panel ");
        num(&mut line, &mut at, u16le(&r[0..2]) as u64);
        text(&mut line, &mut at, b" by ");
        num(&mut line, &mut at, u16le(&r[2..4]) as u64);
        text(&mut line, &mut at, b", ");
        num(&mut line, &mut at, r[4] as u64);
        text(&mut line, &mut at, b" contact slots\n");
    } else {
        text(&mut line, &mut at, b"touchtest: no touch server\n");
    }
    console(grants, &line[..at]);

    let (mut events, mut lines, mut pmax) = (0u64, 0u32, 0u64);
    let tick = freq() / 64;
    let end = now() + freq() * 8;
    while now() < end {
        let n = call(ep, &[0], &mut r);
        if n >= 1 {
            let k = r[0] as usize;
            for i in 0..k {
                let c = &r[1 + i * REC..1 + (i + 1) * REC];
                events += 1;
                if c[3] as u64 > pmax {
                    pmax = c[3] as u64;
                }
                if lines < 32 {
                    let mut at = 0;
                    text(&mut line, &mut at, b"  finger ");
                    num(&mut line, &mut at, c[0] as u64);
                    text(&mut line, &mut at, match c[1] {
                        0 => b" down   ",
                        1 => b" moves  ",
                        _ => b" up     ",
                    });
                    text(&mut line, &mut at, b"x ");
                    num(&mut line, &mut at, u16le(&c[4..6]) as u64);
                    text(&mut line, &mut at, b" y ");
                    num(&mut line, &mut at, u16le(&c[6..8]) as u64);
                    text(&mut line, &mut at, b" pressure ");
                    num(&mut line, &mut at, c[3] as u64);
                    text(&mut line, &mut at, b" axes ");
                    num(&mut line, &mut at, u16le(&c[8..10]) as u64);
                    text(&mut line, &mut at, b" by ");
                    num(&mut line, &mut at, u16le(&c[10..12]) as u64);
                    text(&mut line, &mut at, b"\n");
                    console(grants, &line[..at]);
                    lines += 1;
                }
            }
        }
        sleep_until(now() + tick);
    }

    let mut at = 0;
    text(&mut line, &mut at, b"touchtest: ");
    num(&mut line, &mut at, events);
    text(&mut line, &mut at, b" contact events in 8 s, hardest press ");
    num(&mut line, &mut at, pmax);
    text(&mut line, &mut at, b" of A#255\n");
    console(grants, &line[..at]);
    ferros_sys::exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
