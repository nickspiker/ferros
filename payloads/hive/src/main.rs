//! Hive (a working name): the honeycomb keyboard as an app (honeycomb has the keyboard itself; Claude memories/input_rhe_on_glass.md the design). A tap types into a line above the keys and to the console; one finger sliding through a word's letters types the word (shape writing, honeycomb::shape, over rhe's English list), with three alternatives in a strip above the keys, a tap on one swapping it in; enter prints the line; two fingers dragged up or down scroll the extent (digits, symbols) a row per row pitch; map hands the screen back.
//!
//! A window (WINDOWS.md): it draws its own surface, shows it at the app level, and takes a finger only where the display says the point is its own (the corners outside the circle are the system's).
//!
//! Each finger is tracked on its own, so a second finger landing before the first lifts (fast typing) is two taps, not a gesture. A key is lit while a finger is on it and types when that finger lifts without moving past the slop. Two fingers down together that move are a scroll; a finger that moves alone types nothing yet: that is where shape writing goes.
//!
//! Spawned with `ferros-bridge spawn hive.bin --name hive --use display --use touch --dma surface`. Grant slots: 0 console, 1 display (use), 2 touch (use), 3 its surface.
#![no_std]
#![no_main]

use ferros_sys::{call, cap, console, exit, freq, now, sleep_until, window};
use honeycomb::{Event, Key, Keyboard, Lexicon, Touch, Touches};

/// The shape-writing lexicon, built from rhe's list (build.rs).
static LEXICON: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/lexicon.bin"));

const S_DISPLAY: u64 = 1;
const S_TOUCH: u64 = 2;
const S_SURF: u64 = 3;

struct Hive {
    display: u64,
    base: u64,
    w: usize,
    h: usize,
    kb: Keyboard,
    text: [u8; 256],
    len: usize,
    /// The last traced word: where it starts in the text and how long it is, and the alternatives offered for it (lexicon ranks, u32::MAX for none).
    word: Option<(usize, usize)>,
    cur: u32,
    alts: [u32; 3],
}

impl Hive {
    fn surf(&self) -> &'static mut [u64] {
        unsafe { core::slice::from_raw_parts_mut(self.base as *mut u64, self.w * self.h) }
    }

    fn label(&self) -> impl FnMut(i64, i64, u16, u16, &str) + '_ {
        move |x, y, px, v, t| window::text(self.display, self.base, x as i16, y as i16, px, v, t.as_bytes())
    }

    /// The typed line's band above the keys: cleared, then the line's tail and a cursor.
    fn line(&self) {
        let s = self.surf();
        let w = self.w;
        let size = self.w / 21;
        let base = self.strip().0 - size;
        s[(base - size * 3 / 2) * w..(base + size / 2) * w].fill(0);
        // The tail that fits: a whole number of UTF-8 characters, about 30 of them.
        let mut from = self.len;
        let mut n = 0;
        while from > 0 && n < 30 {
            from -= 1;
            if self.text[from] & 0xC0 != 0x80 {
                n += 1;
            }
        }
        let mut t = [0u8; 160];
        let k = self.len - from;
        t[..k].copy_from_slice(&self.text[from..self.len]);
        t[k] = b'_';
        window::text(self.display, self.base, (w / 28) as i16, base as i16, size as u16, honeycomb::WHITE as u16, &t[..k + 1]);
    }

    /// The strip of alternatives between the typed line and the keys: three slots across.
    fn strip(&self) -> (usize, usize) {
        let h = self.w / 10;
        (self.kb.top - h, self.kb.top)
    }

    fn draw_strip(&self) {
        let (y0, y1) = self.strip();
        let s = self.surf();
        s[y0 * self.w..y1 * self.w].fill(0);
        let lex = Lexicon(LEXICON);
        let size = self.w / 28;
        for (k, &r) in self.alts.iter().enumerate() {
            let Some(word) = (r != u32::MAX).then(|| lex.word(r)).flatten() else { continue };
            let cx = self.w * (2 * k + 1) / 6;
            let x = cx as i64 - word.len() as i64 * size as i64 * 28 / 100;
            window::text(self.display, self.base, x as i16, ((y0 + y1) / 2 + size * 35 / 100) as i16, size as u16, 9000, word);
        }
    }

    /// A trace decoded: the best word typed (a space before it after another word, a capital if shift is on), the next three offered in the strip.
    fn traced(&mut self, trace: &[(i64, i64)]) {
        let lex = Lexicon(LEXICON);
        let mut best = [(0u32, 0i64); 4];
        self.kb.decode(trace, lex, &mut best);
        if best[0].0 == u32::MAX {
            return;
        }
        let word = lex.word(best[0].0).unwrap_or(b"");
        if self.len > 0 && self.text[self.len - 1] != b' ' {
            self.push(' ');
        }
        let start = self.len;
        for (i, &c) in word.iter().enumerate() {
            let c = c as char;
            self.push(if i == 0 && self.kb.shift { c.to_ascii_uppercase() } else { c });
        }
        if self.kb.shift {
            self.kb.shift = false;
        }
        self.word = Some((start, self.len - start));
        self.cur = best[0].0;
        self.alts = [best[1].0, best[2].0, best[3].0];
        self.draw_strip();
        self.line();
    }

    /// A tap above the keys: on an alternative, it replaces the last traced word.
    fn tap_at(&mut self, x: i64, y: i64) {
        let (y0, y1) = self.strip();
        if (y as usize) < y0 || (y as usize) >= y1 {
            return;
        }
        // The touch server's x is a panel pixel, under the width: 0, 1 or 2.
        let slot = x as usize * 3 / self.w;
        let (Some((start, n)), r) = (self.word, self.alts[slot]) else { return };
        if r == u32::MAX || start + n != self.len {
            return;
        }
        let lex = Lexicon(LEXICON);
        let Some(word) = lex.word(r) else { return };
        let cap = self.text[start].is_ascii_uppercase();
        self.len = start;
        for (i, &c) in word.iter().enumerate() {
            self.push(if i == 0 && cap { (c as char).to_ascii_uppercase() } else { c as char });
        }
        // The replaced word takes the alternative's place in the strip.
        self.alts[slot] = self.cur;
        self.cur = r;
        self.word = Some((start, self.len - start));
        self.draw_strip();
        self.line();
    }

    fn all(&self) {
        let s = self.surf();
        s.fill(0);
        self.kb.draw(s, &mut self.label());
        let size = (self.w / 37) as u16;
        window::text(self.display, self.base, (self.w / 28) as i16, (self.h / 8) as i16, size, 5000, b"hive: the honeycomb keyboard (two fingers: scroll; page on the symbol row)");
        self.line();
    }

    fn show(&self) -> bool {
        window::show(self.display, self.base, 0, 0, self.w as u16, self.h as u16, window::APP)
    }

    fn push(&mut self, c: char) {
        let mut b = [0u8; 4];
        let e = c.encode_utf8(&mut b).as_bytes();
        while self.len + e.len() > self.text.len() {
            // Full: the oldest character goes.
            let first = (1..self.len).find(|&i| self.text[i] & 0xC0 != 0x80).unwrap_or(self.len);
            self.text.copy_within(first..self.len, 0);
            self.len -= first;
        }
        self.text[self.len..self.len + e.len()].copy_from_slice(e);
        self.len += e.len();
    }

    /// A tap on `key`: typed, or the keyboard changed.
    fn tap(&mut self, grants: u64, key: Key) {
        match self.kb.tap(key) {
            Some(Event::Char(c)) => self.push(c),
            Some(Event::Back) => {
                while self.len > 0 {
                    self.len -= 1;
                    if self.text[self.len] & 0xC0 != 0x80 {
                        break;
                    }
                }
            }
            Some(Event::Enter) => {
                let mut m = [0u8; 300];
                let head = b"hive: entered: ";
                m[..head.len()].copy_from_slice(head);
                m[head.len()..head.len() + self.len].copy_from_slice(&self.text[..self.len]);
                m[head.len() + self.len] = b'\n';
                console(grants, &m[..head.len() + self.len + 1]);
                self.len = 0;
            }
            Some(Event::Leave) => {
                window::done(self.display, self.base);
                console(grants, b"hive: back to the map\n");
                exit(0);
            }
            Some(Event::Redraw) => self.kb.draw(self.surf(), &mut self.label()),
            None => {}
        }
        self.line();
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let (display, touch) = (cap(grants, S_DISPLAY), cap(grants, S_TOUCH));
    let mut r = [0u8; 256];
    if call(display, &[5], &mut r) < 4 {
        console(grants, b"hive: no display server\n");
        exit(1);
    }
    let (w, h) = (u16::from_le_bytes([r[0], r[1]]) as usize, u16::from_le_bytes([r[2], r[3]]) as usize);
    let base = ferros_sys::dma(cap(grants, S_SURF));
    if base == 0 || base == u64::MAX || w * h * 8 > 32 << 20 {
        console(grants, b"hive: no surface\n");
        exit(2);
    }
    let mut kb = Keyboard::new(w, h);
    // The device's layout (SETTINGS.md); a value from a newer build falls back to the default.
    let l = ferros_sys::pref(ferros_sys::PREF_LAYOUT) as usize;
    kb.layout = if l < honeycomb::LAYOUTS { l } else { 0 };
    let mut hive = Hive { display, base, w, h, kb, text: [0; 256], len: 0, word: None, cur: u32::MAX, alts: [u32::MAX; 3] };
    let t0 = now();
    hive.all();
    let drawn = ferros_sys::us_since(t0);
    if !hive.show() {
        console(grants, b"hive: the display refused the window\n");
        exit(3);
    }
    {
        let mut l = Line::new();
        let _ = core::fmt::Write::write_fmt(&mut l, format_args!("hive: on the panel ({w}x{h}; drawn in {drawn} us)\n"));
        console(grants, l.as_bytes());
    }
    let mut touches = Touches::new(w);
    let mut held = [(0u8, 0i64, 0i64); 16];
    loop {
        let n = call(touch, &[1], &mut r) as usize;
        let count = if n >= 1 && n != ferros_sys::ERR as usize { (r[0] as usize).min(16) } else { 0 };
        let mut k = 0;
        while k < count && 1 + (k + 1) * 12 <= n {
            let rec = &r[1 + k * 12..1 + (k + 1) * 12];
            held[k] = (rec[0], u16::from_le_bytes([rec[4], rec[5]]) as i64, u16::from_le_bytes([rec[6], rec[7]]) as i64);
            k += 1;
        }
        // The poll's changes first (it holds the keyboard), then drawn.
        let mut outs = [Touch::Scrolled; 48];
        let mut no = 0;
        touches.poll(&mut hive.kb, &held[..k], &mut |x, y| window::mine(display, base, x as u16, y as u16), &mut |t| {
            outs[no] = t;
            no += 1;
        });
        // The device's layout changed (the attest screen's switch, `diag pref layout`): the keys follow.
        let l = ferros_sys::pref(ferros_sys::PREF_LAYOUT) as usize;
        let relaid = l != hive.kb.layout && l < honeycomb::LAYOUTS;
        if relaid {
            hive.kb.layout = l;
            hive.kb.draw(hive.surf(), &mut hive.label());
        }
        let dirty = no > 0 || relaid;
        for &t in &outs[..no] {
            match t {
                Touch::Light(key) => hive.kb.draw_key(hive.surf(), key, true, &mut hive.label()),
                Touch::Unlight(key) => hive.kb.draw_key(hive.surf(), key, false, &mut hive.label()),
                Touch::Scrolled => hive.kb.draw(hive.surf(), &mut hive.label()),
                Touch::Tap(key) => {
                    // A tapped character ends the traced word's chance to be swapped.
                    hive.word = None;
                    hive.tap(grants, key);
                }
                Touch::Trail(x, y) => honeycomb::dot(hive.surf(), w, x, y, (w / 96) as i64, honeycomb::WHITE / 2),
                Touch::Traced => {
                    let mut tr = [(0i64, 0i64); 256];
                    let t = touches.trace();
                    tr[..t.len()].copy_from_slice(t);
                    let n = t.len();
                    hive.traced(&tr[..n]);
                    // The trail off the keys.
                    hive.kb.draw(hive.surf(), &mut hive.label());
                }
                Touch::TapAt(x, y) => hive.tap_at(x, y),
            }
        }
        if dirty {
            hive.show();
        }
        sleep_until(now() + freq() / 240);
    }
}

/// A console line in a fixed buffer.
struct Line {
    b: [u8; 160],
    n: usize,
}

impl Line {
    fn new() -> Line {
        Line { b: [0; 160], n: 0 }
    }
    fn as_bytes(&self) -> &[u8] {
        &self.b[..self.n]
    }
}

impl core::fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let k = s.len().min(self.b.len() - self.n);
        self.b[self.n..self.n + k].copy_from_slice(&s.as_bytes()[..k]);
        self.n += k;
        Ok(())
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
