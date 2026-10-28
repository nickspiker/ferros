//! Hive (a working name): the honeycomb keyboard as an app (honeycomb has the keyboard itself; Claude memories/input_rhe_on_glass.md the design). A tap types into a line above the keys and to the console; enter prints the line; two fingers dragged up or down scroll the extent (digits, symbols) a row per row pitch; map hands the screen back.
//!
//! A window (WINDOWS.md): it draws its own surface, shows it at the app level, and takes a finger only where the display says the point is its own (the corners outside the circle are the system's).
//!
//! Each finger is tracked on its own, so a second finger landing before the first lifts (fast typing) is two taps, not a gesture. A key is lit while a finger is on it and types when that finger lifts without moving past the slop. Two fingers down together that move are a scroll; a finger that moves alone types nothing yet: that is where shape writing goes.
//!
//! Spawned with `ferros-bridge spawn hive.bin --name hive --use display --use touch --dma surface`. Grant slots: 0 console, 1 display (use), 2 touch (use), 3 its surface.
#![no_std]
#![no_main]

use ferros_sys::{call, cap, console, exit, freq, now, sleep_until, window};
use honeycomb::{Event, Key, Keyboard, NONE_KEY};

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
        let base = self.kb.top - size * 3 / 2;
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
    let mut hive = Hive { display, base, w, h, kb: Keyboard::new(w, h), text: [0; 256], len: 0 };
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
    // Each finger on its own: where it landed, the key under it, whether it moved past the slop, whether it is ours at all. The slop is the map's, scaled: a 56th of the screen's width.
    let slop = (w / 56) as i64;
    let mut fingers = [(false, 0i64, 0i64, NONE_KEY, false, false); 16];
    // A two-finger scroll: the view and the fingers' mean height when it began.
    let mut scroll: Option<(usize, i64)> = None;
    loop {
        let n = call(touch, &[1], &mut r) as usize;
        let count = if n >= 1 && n != ferros_sys::ERR as usize { (r[0] as usize).min(16) } else { 0 };
        let mut here = [false; 16];
        let mut dirty = false;
        let (mut sum_y, mut ours) = (0i64, 0i64);
        for k in 0..count {
            if 1 + (k + 1) * 12 > n {
                break;
            }
            let rec = &r[1 + k * 12..1 + (k + 1) * 12];
            let id = (rec[0] as usize) & 15;
            let (x, y) = (u16::from_le_bytes([rec[4], rec[5]]) as i64, u16::from_le_bytes([rec[6], rec[7]]) as i64);
            here[id] = true;
            let f = &mut fingers[id];
            if !f.0 {
                let mine = window::mine(display, base, x as u16, y as u16);
                *f = (true, x, y, if mine { hive.kb.hit(x, y) } else { NONE_KEY }, false, mine);
                if f.3 != NONE_KEY {
                    hive.kb.draw_key(hive.surf(), f.3, true, &mut hive.label());
                    dirty = true;
                }
            } else if !f.4 && (x - f.1).pow(2) + (y - f.2).pow(2) > slop * slop {
                f.4 = true;
                if f.3 != NONE_KEY {
                    hive.kb.draw_key(hive.surf(), f.3, false, &mut hive.label());
                    dirty = true;
                }
            }
            if f.5 {
                sum_y += y;
                ours += 1;
            }
        }
        // Two of our fingers: a scroll from where they stood when the second landed; dragging down shows the rows above.
        if ours == 2 {
            let mean = sum_y / 2;
            match scroll {
                None => scroll = Some((hive.kb.view, mean)),
                Some((v0, y0)) => {
                    if hive.kb.scroll_to(v0 as i64 + (mean - y0) / hive.kb.row_pitch()) {
                        hive.kb.draw(hive.surf(), &mut hive.label());
                        dirty = true;
                    }
                }
            }
        }
        for id in 0..16 {
            let f = fingers[id];
            if f.0 && !here[id] {
                fingers[id].0 = false;
                if f.3 != NONE_KEY {
                    hive.kb.draw_key(hive.surf(), f.3, false, &mut hive.label());
                    dirty = true;
                }
                // A tap: it did not move, and no scroll ran while it was down.
                if f.3 != NONE_KEY && !f.4 && scroll.is_none() {
                    hive.tap(grants, f.3);
                }
            }
        }
        if fingers.iter().all(|f| !f.0) {
            scroll = None;
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
