//! Ramps: the present stage's test screen, an app (SHELL.md: apps are places) — the first to draw its own pixels. It writes 16-bit working values (gamma 2, white = Q14) into the `app-surface` region and asks the display server to show them (op 22), which publishes them through the one table, E's gains and the dither at full precision. While it is in front it takes the touches itself: a tap shows the column under the finger alone (and a tap shows them all again); two fingers hand the screen back to the map (op 23) and end the app.
//!
//! The columns, left to right, each a ramp from black at the top to its brightest at the bottom, with 4-pixel white lines between them: grey to white dithered and undithered; near-black (light to 1/64 of white) dithered, undithered, and as an 8-bit canvas would hand it over; the headroom (to 16× white) through the roll-off; red, green, blue to white. "Undithered" columns carry the surface's no-dither bit (48). A column alone is named in the black beside it (display op 24: the display's typeface, since apps have none), with what it tests; `PATTERNS` is the list, add a row and a `draw` arm for a new one.
//!
//! Launched by the map (SYS_SPAWN) with its own capabilities. Grant slots: 0 console, 1 display (use), 2 touch (use), 3 app-surface.
#![no_std]
#![no_main]

use ferros_sys::{call, cap, console, exit, freq, now, sleep_until};

const S_DISPLAY: u64 = 1;
const S_TOUCH: u64 = 2;
const S_SURF: u64 = 3;
const COLS: usize = 9;
const LINE: usize = 4;
const WHITE: u64 = 16384;
const NO_DITHER: u64 = 1 << 48;

/// Each column: its name and what to look for, shown when it is alone.
const PATTERNS: [(&str, &str); COLS] = [
    ("grey to white", "dithered: 14 bits over the 8-bit link"),
    ("grey to white", "undithered: the panel's own 8-bit steps"),
    ("near black", "to 1/64 of white's light, dithered"),
    ("near black", "to 1/64 of white's light, undithered"),
    ("near black", "to 1/64, as an 8-bit canvas hands it over"),
    ("headroom", "to 16x white through the roll-off"),
    ("red", "to white"),
    ("green", "to white"),
    ("blue", "to white"),
];

/// A line of text into the surface (display op 24): screen coordinates, size in pixels, grey value Q14.
fn label(display: u64, x: i16, baseline: i16, px: u16, value: u16, s: &str) {
    let mut m = [0u8; 160];
    m[0] = 24;
    m[1..3].copy_from_slice(&x.to_le_bytes());
    m[3..5].copy_from_slice(&baseline.to_le_bytes());
    m[5..7].copy_from_slice(&px.to_le_bytes());
    m[7..9].copy_from_slice(&value.to_le_bytes());
    let n = s.len().min(m.len() - 9);
    m[9..9 + n].copy_from_slice(&s.as_bytes()[..n]);
    let mut r = [0u8; 4];
    call(display, &m[..9 + n], &mut r);
}

/// The labels beside a lone column: its name large, what it tests under it, a third of the way down the left black margin; and the column's number so a tap can be matched to the list.
fn labels(display: u64, w: usize, h: usize, k: usize) {
    let (name, what) = PATTERNS[k];
    let strip = w * 3 / 8;
    let margin = ((w - strip) / 2) as i16;
    let y = (h / 3) as i16;
    let mut num = [b'0' + (k as u8 + 1), 0];
    let n = core::str::from_utf8(&num[..1]).unwrap_or("?");
    label(display, 24, y - 56, 36, 6000, n);
    label(display, 24, y, 44, WHITE as u16, name);
    label(display, 24, y + 48, 28, 6000, what);
    // The right margin says how to leave.
    label(display, margin + strip as i16 + LINE as i16 + 24, y, 28, 4000, "tap: all");
    let _ = &mut num;
}

fn grey(v: u64) -> u64 {
    v | v << 16 | v << 32
}

/// The whole screen into the surface: all nine columns, or `only` alone (a strip three eighths of the width down the middle, white lines each side, black elsewhere).
fn draw(surf: &mut [u64], w: usize, h: usize, only: Option<usize>) {
    let col = (w - LINE * (COLS + 1)) / COLS;
    let strip = w * 3 / 8;
    let (s0, s1) = ((w - strip) / 2, (w + strip) / 2);
    for y in 0..h {
        let f = (y as u64 * 65535 / (h as u64 - 1)) as u64;
        let row = &mut surf[y * w..(y + 1) * w];
        for (x, p) in row.iter_mut().enumerate() {
            let (k, inside, black) = match only {
                Some(k) => (k, x >= s0 && x < s1, x + LINE < s0 || x >= s1 + LINE),
                None => {
                    let k = x.saturating_sub(LINE) / (col + LINE);
                    (k, x >= LINE && (x - LINE) % (col + LINE) < col && k < COLS, false)
                }
            };
            *p = if black {
                0
            } else if !inside {
                grey(WHITE)
            } else {
                let (top, dither, eight) = match k {
                    0 | 1 => (WHITE, k == 0, false),
                    2 | 3 | 4 => (2048, k != 3, k == 4),
                    5 => (65535, true, false),
                    _ => (WHITE, true, false),
                };
                let mut v = f * top / 65535;
                if eight {
                    v = (v * 255 + 8192) / 16384 * 16384 / 255;
                }
                let px = match k {
                    6 => v,
                    7 => v << 16,
                    8 => v << 32,
                    _ => grey(v),
                };
                if dither { px } else { px | NO_DITHER }
            };
        }
    }
}

fn show(display: u64, w: usize, h: usize) -> bool {
    let mut m = [0u8; 9];
    m[0] = 22;
    m[5..7].copy_from_slice(&(w as u16).to_le_bytes());
    m[7..9].copy_from_slice(&(h as u16).to_le_bytes());
    let mut r = [0u8; 4];
    call(display, &m, &mut r);
    r[0] == 0
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let (display, touch) = (cap(grants, S_DISPLAY), cap(grants, S_TOUCH));
    let mut r = [0u8; 256];
    if call(display, &[5], &mut r) < 4 {
        console(grants, b"ramps: no display server\n");
        exit(1);
    }
    let (w, h) = (u16::from_le_bytes([r[0], r[1]]) as usize, u16::from_le_bytes([r[2], r[3]]) as usize);
    let base = ferros_sys::dma(cap(grants, S_SURF));
    if base == 0 || base == u64::MAX || w * h * 8 > 32 << 20 {
        console(grants, b"ramps: no app surface\n");
        exit(2);
    }
    let surf = unsafe { core::slice::from_raw_parts_mut(base as *mut u64, w * h) };
    let mut only: Option<usize> = None;
    draw(surf, w, h, only);
    if !show(display, w, h) {
        console(grants, b"ramps: the display is not in 10 bits\n");
        exit(3);
    }
    console(grants, b"ramps: on the panel (tap: one column alone; two fingers: back to the map)\n");
    // Taps, as the map reads them: down and up without moving past the slop, one finger or two.
    let (mut down, mut moved, mut most) = (false, false, 0usize);
    let mut start = [(false, 0i64, 0i64); 16];
    loop {
        let n = call(touch, &[1], &mut r) as usize;
        let count = if n >= 1 && n != ferros_sys::ERR as usize { (r[0] as usize).min(16) } else { 0 };
        let mut held = 0;
        for k in 0..count {
            if 1 + (k + 1) * 12 > n {
                break;
            }
            let rec = &r[1 + k * 12..1 + (k + 1) * 12];
            let id = (rec[0] as usize) & 15;
            let (x, y) = (u16::from_le_bytes([rec[4], rec[5]]) as i64, u16::from_le_bytes([rec[6], rec[7]]) as i64);
            if !down {
                down = true;
                moved = false;
                most = 0;
                start = [(false, 0, 0); 16];
            }
            if !start[id].0 {
                start[id] = (true, x, y);
            } else if (x - start[id].1).pow(2) + (y - start[id].2).pow(2) > 24 * 24 {
                moved = true;
            }
            held += 1;
        }
        if down {
            most = most.max(held);
        }
        if held == 0 && down {
            down = false;
            if !moved && most == 2 {
                let mut q = [0u8; 4];
                call(display, &[23], &mut q);
                console(grants, b"ramps: back to the map\n");
                exit(0);
            }
            if !moved && most == 1 {
                let x = start.iter().find(|s| s.0).map(|s| s.1).unwrap_or(0) as usize;
                only = match only {
                    Some(_) => None,
                    None => {
                        let col = (w - LINE * (COLS + 1)) / COLS;
                        Some((x.saturating_sub(LINE) / (col + LINE)).min(COLS - 1))
                    }
                };
                draw(surf, w, h, only);
                if let Some(k) = only {
                    labels(display, w, h, k);
                }
                show(display, w, h);
            }
        }
        sleep_until(now() + freq() / 120);
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
