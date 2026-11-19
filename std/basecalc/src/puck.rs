//! basecalc as a puck (docs/PUCKS.md; owner, 2026-11-18: "basecalc calculator! Well, a nice GUI puck version anyway"): the pad (window.rs's `Pad`: the same keys, layout, display and vault) drawn into the puck's square at the size its circle has on the deck, upright (the display turns it with the circle), and every key pressed by a tap that passes thru to it from the deck (owner: "a simple tap passes thru so we can make our dozenal calculator work too"; one finger dragging moves the circle, two size and turn it, both the deck's).
//! Built with `--features puck` (tools/basecalc-kist.sh), its capsule declaring `puck`: its square is in its surface's slot (3), its link to the display the first asked slot (7).

use crate::sys;
use crate::window::{open_vault, restore, writer, Pad};
use basecalc_kist::layout::Circle;
use fluor::paint::HIT_NONE;
use fluor::text::TextRenderer;
use std::sync::{Arc, Condvar, Mutex};

/// The square's slot (its surface's) and the `puck` endpoint (the first asked want).
const SQUARE: u64 = 3;
const PUCK: u64 = 7;
/// The calls (ferros_puck `call`).
const WATCH: u8 = 1;
const SHOW: u8 = 2;
const OK: u8 = 0;
const TAPS: usize = 1 << 3;
/// The most pixels a side of a square has (ferros_puck `SIDE_MAX`).
const SIDE_MAX: u16 = (1 << 11) - 1;

/// What the display last said: the view's count, on the glass, its diameter; and the taps not yet taken (Q14 of the radius, the face's frame).
#[derive(Default)]
struct Heard {
    count: u32,
    shown: bool,
    d: u16,
    taps: Vec<(i16, i16)>,
}

/// The watch: held on the display until the circle's view moves or a tap comes, its answer handed to the main loop.
fn watcher(heard: Arc<(Mutex<Heard>, Condvar)>) {
    let mut seen = 0u32;
    loop {
        let mut m = [0u8; 6];
        m[0] = WATCH;
        m[1..5].copy_from_slice(&seen.to_le_bytes());
        let mut r = [0u8; 14 + 4 * TAPS];
        let n = sys::call(PUCK, &m, &mut r);
        // No display that knows this puck: nothing will ever be shown; asking again would only spin.
        if n == sys::ERR || n < 14 || r[0] != 0 {
            println!("basecalc: the display does not answer the puck's watch");
            return;
        }
        seen = u32::from_le_bytes(r[1..5].try_into().unwrap());
        let (lock, bell) = &*heard;
        let mut h = lock.lock().unwrap();
        h.count = seen;
        h.shown = r[5] != 0;
        h.d = u16::from_le_bytes([r[7], r[8]]);
        // WHY the bound: the count is the display's word and the reply holds at most `TAPS`.
        let k = (r[13] as usize).min(TAPS).min((n as usize - 14) / 4);
        for j in 0..k {
            let at = 14 + 4 * j;
            h.taps.push((i16::from_le_bytes([r[at], r[at + 1]]), i16::from_le_bytes([r[at + 2], r[at + 3]])));
        }
        bell.notify_all();
    }
}

/// Where the next square of `bytes` goes: clear of the one the display has (ferros_puck::rt's rule).
fn next_offset(shown: Option<(u64, u64)>, bytes: u64) -> u64 {
    match shown {
        Some((off, len)) if off < bytes => (off + len + 63) & !63,
        _ => 0,
    }
}

pub fn main() {
    std::panic::set_hook(Box::new(|info| {
        let at = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
        println!("basecalc: PANIC at {at}: {info}");
    }));
    println!("basecalc: basecalc {} on ferros as a puck, {} grants", env!("CARGO_PKG_VERSION"), sys::grant_count());
    let vault = open_vault();
    let calc = vault.as_deref().map(restore).unwrap_or_default();
    let mut pad = Pad::new(calc, vault.map(writer));
    let mut text = TextRenderer::new();
    let mut damage = fluor::canvas::Damage::new();
    let heard = Arc::new((Mutex::new(Heard::default()), Condvar::new()));
    {
        let heard = heard.clone();
        if let Err(e) = std::thread::Builder::new().name("basecalc-watch".into()).spawn(move || watcher(heard)) {
            println!("basecalc: no watch thread ({e}); nothing will be shown");
        }
    }
    let base = sys::dma(SQUARE);
    if base == sys::ERR {
        println!("basecalc: no square (a capsule without `puck`, or a kernel from before 2026-11-18)");
        return;
    }
    let (mut held, mut shown): (u64, Option<(u64, u64)>) = (0, None);
    let mut seen = 0u32;
    loop {
        let (count, on, d, taps) = {
            let (lock, bell) = &*heard;
            let mut h = lock.lock().unwrap();
            while h.count == seen && h.taps.is_empty() {
                h = bell.wait(h).unwrap();
            }
            (h.count, h.shown, h.d, std::mem::take(&mut h.taps))
        };
        seen = count;
        // WHY the bound: the side is the display's word; past `SIDE_MAX` two squares would not fit the slot.
        let d = d.min(SIDE_MAX) as usize;
        if !on || d == 0 {
            continue;
        }
        // Each tap on the keys as they were last drawn: its point in the square from the face's frame (Q14 of the radius from the centre), the key the hit map holds there.
        let side = pad.size.0;
        let mut pressed = false;
        for (tx, ty) in taps {
            if side == 0 {
                break;
            }
            let r = side as f32 * 0.5;
            let (x, y) = (r + tx as f32 / 16384. * r, r + ty as f32 / 16384. * r);
            // WHY the bounds: a tap on the circle's rim lands up to a pixel past the square the keys were drawn in.
            if x < 0. || y < 0. || x >= side as f32 || y >= side as f32 {
                continue;
            }
            let id = pad.hits[y as usize * side + x as usize];
            if id == HIT_NONE {
                continue;
            }
            let p = pad.drawn[id as usize - 1];
            let change = pad.calc.press(p.key);
            if change.relayout {
                pad.place();
            }
            if change.save {
                pad.save();
            }
            pressed = true;
        }
        // Drawn again only for a new size or a key pressed: a turn alone is the display's to show (the square is upright and it turns it with the circle).
        if !pressed && shown.is_some() && side == d {
            continue;
        }
        // The pad in the circle: the circle fills the square, and nothing of it is the system's.
        if pad.size != (d, d) {
            pad.circle = Some(Circle { cx: d as f32 * 0.5, cy: d as f32 * 0.5, r: d as f32 * 0.5, top: false, bottom: false });
        }
        let bytes = (d * d * 8) as u64;
        let off = next_offset(shown, bytes);
        if off + bytes > held {
            match sys::puck_size(SQUARE, off + bytes) {
                Some(n) => held = n,
                None => {
                    println!("basecalc: no room for a {d} pixel square");
                    continue;
                }
            }
        }
        // SAFETY: [base + off, base + off + bytes) lies inside the frames this puck's slot holds (`held`, the kernel's answer), which nothing else writes; the display reads only the square it was last handed, which `next_offset` keeps clear of.
        let px = unsafe { std::slice::from_raw_parts_mut((base + off) as *mut u64, d * d) };
        px.fill(0);
        pad.draw_all(px, d, d, &mut text, HIT_NONE, &mut damage);
        // Upright: the display turns it with the circle, and the taps come back in the same frame.
        let mut m = [0u8; 11];
        m[0] = SHOW;
        m[1..5].copy_from_slice(&(off as u32).to_le_bytes());
        m[5..7].copy_from_slice(&(d as u16).to_le_bytes());
        m[7..9].copy_from_slice(&(1i16 << 14).to_le_bytes());
        let mut r = [0u8; 1];
        let n = sys::call(PUCK, &m, &mut r);
        if n != sys::ERR && n > 0 && r[0] == OK {
            shown = Some((off, bytes));
            // The slot back down once the display holds a square at its start and the slot is more than four times what two of this size need.
            if off == 0 && held > 8 * bytes {
                if let Some(n) = sys::puck_size(SQUARE, 2 * bytes) {
                    held = n;
                }
            }
        }
    }
}
