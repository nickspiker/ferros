//! The map (SHELL.md): the phone is a map of your life. Every person, message, photo and app is a circle on one plane; the view is a camera over it, and the camera is the only thing that moves.
//!
//! This is the first slice. The circles are the first apps — Photon, Mahere, Lumis, Opsin — placed once by a seeded random fit with generous air, so they land in the same place every time. One finger pans; two fingers pinch to zoom about the point between them. The display server keeps the scene, so a frame costs this process three numbers: the camera.
//!
//! What is still illustrative, and why: area is the share of the owner's time each thing takes, set from the owner's own ordering (Photon, Mahere, Lumis, then Opsin) until time is measured. The rings (importance) and the one lit circle (changed since last seen) are set by hand here to show all four channels; importance becomes the owner's to declare and lit becomes Photon's to report. Positions are deterministic rather than written to the vault, which keeps them fixed across boots until the vault holds them.
//!
//! Spawn after `display` and `touch`: ferros-bridge spawn map.bin 00 --name map --use display --use touch
//! Grant slots: 0 console, 1 display (use), 2 touch (use).

#![no_std]
#![no_main]

use ferros_sys::{call, cap, console, freq, now, sleep_until};

const S_DISPLAY: u64 = 1;
const S_TOUCH: u64 = 2;

/// One circle: its label, its weight of ring (0 none, 1 thin, 2 heavy — importance), whether it is lit (changed since last seen), and its share of the owner's time in thousandths — the circle's area is that share of the map.
struct Spec {
    label: &'static str,
    ring: u8,
    lit: u8,
    share: u32,
}

/// The shares are the owner's ordering until time is measured (2026-10-22: "Photon the most, then Mahere, then Lumis"): 40 %, 25 %, 20 %, 15 %.
const APPS: [Spec; 4] = [
    Spec { label: "Photon", ring: 2, lit: 1, share: 400 },
    Spec { label: "Mahere", ring: 1, lit: 0, share: 250 },
    Spec { label: "Lumis", ring: 1, lit: 0, share: 200 },
    Spec { label: "Opsin", ring: 1, lit: 0, share: 150 },
];

/// Names under the circles: off. An app's icon identifies it (owner, 2026-10-21: "icons identify just fine"), so words are an option for whoever wants them, not the default. Settings will own this switch once settings exist.
const SHOW_LABELS: bool = false;

/// The map's total circle area, as four circles of radius 110 world units: each circle gets its share of it, so area is time.
const BASE_RADIUS: i64 = 110;

/// A circle's radius in world units: area = share x the map's total, so r = 110 x sqrt(4 x share). Integer square root, no floats.
fn radius_of(share: u32) -> i64 {
    isqrt(BASE_RADIUS * BASE_RADIUS * 4 * share as i64 / 1000)
}

fn isqrt(v: i64) -> i64 {
    if v <= 0 {
        return 0;
    }
    let mut x = v;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + v / x) / 2;
    }
    x
}

/// A small, fixed-seed generator: the same seed places the same circles in the same places on every boot, which is what "nothing ever moves" asks of a layout that is not yet stored.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// Uniform in [-span, span).
    fn signed(&mut self, span: i64) -> i64 {
        ((self.next() >> 33) as i64 % (2 * span.max(1))) - span
    }
}

/// Place the circles one at a time, biggest first, each at the first random spot that keeps clear of every circle already placed by at least the larger of the two radii again: more air is better, and no two ever overlap.
fn place(rs: &[i64], xs: &mut [i64], ys: &mut [i64]) {
    let mut rng = Rng(0x6D61_7020_6C69_6665);
    for i in 0..rs.len() {
        let mut spread = rs[i];
        loop {
            let (x, y) = (rng.signed(spread), rng.signed(spread * 8 / 5));
            let clear = (0..i).all(|j| {
                let gap = rs[i] + rs[j] + rs[i].max(rs[j]);
                let (dx, dy) = (x - xs[j], y - ys[j]);
                dx * dx + dy * dy >= gap * gap
            });
            if clear {
                xs[i] = x;
                ys[i] = y;
                break;
            }
            // Widen the search slowly, so the circles stay as close as the air allows rather than scattering.
            spread += 4;
        }
    }
}


/// The distance between two touch points, for the pinch (still floating point, as the camera's zoom is; the layout above is integer). Written out because `f32::powi` and friends exist only with std.
fn dist(ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let (dx, dy) = (ax - bx, ay - by);
    libm::sqrtf(dx * dx + dy * dy)
}

fn camera(grants: u64, cx: f32, cy: f32, zoom: f32) {
    let mut m = [0u8; 13];
    m[0] = 14;
    m[1..5].copy_from_slice(&(cx as i32).to_le_bytes());
    m[5..9].copy_from_slice(&(cy as i32).to_le_bytes());
    m[9..13].copy_from_slice(&((zoom * 65536.0) as u32).to_le_bytes());
    let mut r = [0u8; 4];
    call(cap(grants, S_DISPLAY), &m, &mut r);
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _input: u64, _input_len: u64) -> ! {
    // The circles' labels are `&'static str`, absolute addresses fixed at link time; linked position-independent, they are relocations this applies before anything reads them. Without it the first label read faulted at its link-time address, G#D9B (2026-10-21).
    unsafe { ferros_sys::relocate() };
    let display = cap(grants, S_DISPLAY);
    let touch = cap(grants, S_TOUCH);
    let mut r = [0u8; 256];

    // The panel's size, from the display server.
    if call(display, &[5], &mut r) < 4 {
        console(grants, b"map: no display server\n");
        ferros_sys::exit(1);
    }
    let (w, h) = (u16::from_le_bytes([r[0], r[1]]) as f32, u16::from_le_bytes([r[2], r[3]]) as f32);

    // The circles, sized by share and placed once, biggest first.
    let rs: [i64; 4] = core::array::from_fn(|i| radius_of(APPS[i].share));
    let mut xs = [0i64; 4];
    let mut ys = [0i64; 4];
    place(&rs, &mut xs, &mut ys);

    // Hand the scene to the display server: clear, then the circles in one call.
    call(display, &[12], &mut r);
    let mut m = [0u8; 2 + 30 * 4];
    m[0] = 13;
    m[1] = APPS.len() as u8;
    for (i, a) in APPS.iter().enumerate() {
        let at = 2 + i * 30;
        m[at..at + 4].copy_from_slice(&(xs[i] as i32).to_le_bytes());
        m[at + 4..at + 8].copy_from_slice(&(ys[i] as i32).to_le_bytes());
        m[at + 8..at + 10].copy_from_slice(&(rs[i] as u16).to_le_bytes());
        m[at + 10] = a.ring;
        m[at + 11] = a.lit;
        // Each app's icon, by its place in tools/app-icons.py's bundle, which follows this table's order.
        m[at + 12] = i as u8 + 1;
        let label = a.label.as_bytes();
        let len = if SHOW_LABELS { label.len().min(16) } else { 0 };
        m[at + 13] = len as u8;
        m[at + 14..at + 14 + len].copy_from_slice(&label[..len]);
    }
    call(display, &m, &mut r);

    // Home: the camera over the middle of the circles, one world unit to a pixel.
    let (mut cx, mut cy) = (xs.iter().sum::<i64>() as f32 / 4.0, ys.iter().sum::<i64>() as f32 / 4.0);
    let mut zoom = 1.0f32;
    camera(grants, cx, cy, zoom);
    console(grants, b"map: home drawn; one finger pans, two fingers pinch\n");

    // The previous frame's held contacts, by contact id: present, x, y.
    let mut prev = [(false, 0f32, 0f32); 16];
    loop {
        let n = call(touch, &[1], &mut r) as usize;
        let mut cur = [(false, 0f32, 0f32); 16];
        let mut ids = [0usize; 16];
        let mut held = 0usize;
        if n >= 1 {
            let count = (r[0] as usize).min(16);
            for k in 0..count {
                let rec = &r[1 + k * 12..1 + (k + 1) * 12];
                if 1 + (k + 1) * 12 > n {
                    break;
                }
                let id = (rec[0] as usize) & 15;
                let x = u16::from_le_bytes([rec[4], rec[5]]) as f32;
                let y = u16::from_le_bytes([rec[6], rec[7]]) as f32;
                cur[id] = (true, x, y);
                ids[held] = id;
                held += 1;
            }
        }
        // The contacts present both now and last frame are the ones that moved.
        let mut both = [0usize; 2];
        let mut nb = 0usize;
        for &id in ids[..held].iter() {
            if prev[id].0 && nb < 2 {
                both[nb] = id;
                nb += 1;
            }
        }
        let (ocx, ocy, oz) = (cx, cy, zoom);
        if nb == 1 {
            // One finger: the plane follows it.
            let id = both[0];
            cx -= (cur[id].1 - prev[id].1) / zoom;
            cy -= (cur[id].2 - prev[id].2) / zoom;
        } else if nb == 2 {
            // Two fingers: zoom by how much they spread, about the point between them, so what is under your fingers stays under your fingers.
            let (a, b) = (both[0], both[1]);
            let d0 = dist(prev[a].1, prev[a].2, prev[b].1, prev[b].2);
            let d1 = dist(cur[a].1, cur[a].2, cur[b].1, cur[b].2);
            if d0 > 8.0 {
                let (m0x, m0y) = ((prev[a].1 + prev[b].1) * 0.5, (prev[a].2 + prev[b].2) * 0.5);
                let (m1x, m1y) = ((cur[a].1 + cur[b].1) * 0.5, (cur[a].2 + cur[b].2) * 0.5);
                let (wx, wy) = (cx + (m0x - w * 0.5) / zoom, cy + (m0y - h * 0.5) / zoom);
                zoom = (zoom * d1 / d0).clamp(0.15, 12.0);
                cx = wx - (m1x - w * 0.5) / zoom;
                cy = wy - (m1y - h * 0.5) / zoom;
            }
        }
        if cx != ocx || cy != ocy || zoom != oz {
            camera(grants, cx, cy, zoom);
        }
        prev = cur;
        // A hundred-and-twentieth of a second while a finger is down, a thirtieth at rest.
        sleep_until(now() + if held > 0 { freq() / 120 } else { freq() / 30 });
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
