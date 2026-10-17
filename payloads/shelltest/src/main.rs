//! A client of the `display` endpoint: draws SHELL.md's home screen (status line, the people, the foot) through the display server and presents it, then keeps the status clock ticking once a second so the screen reads as alive. The shell itself will do this with live rows; this proves the endpoint.
//!
//! Spawn after `display`: ferros-bridge spawn shelltest.bin 00 --name shelltest --use display   (input 01: draw once and exit)
//! Grant slots: 0 console, 1 display (use).

#![no_std]
#![no_main]

use ferros_sys::{call, cap, console};

const S_DISPLAY: u64 = 1;

struct Client {
    grants: u64,
    info: [u8; 12],
}

impl Client {
    fn op(&self, msg: &[u8]) -> u8 {
        let mut r = [0u8; 16];
        let n = call(cap(self.grants, S_DISPLAY), msg, &mut r);
        if n == ferros_sys::ERR || n == 0 { 0xFF } else { r[0] }
    }
    fn begin(&self) -> u8 {
        self.op(&[1])
    }
    fn text(&self, x: u16, y: u16, px: u16, colour: u32, align: u8, s: &str) -> u8 {
        let mut m = [0u8; 256];
        m[0] = 2;
        m[1..3].copy_from_slice(&x.to_le_bytes());
        m[3..5].copy_from_slice(&y.to_le_bytes());
        m[5..7].copy_from_slice(&px.to_le_bytes());
        m[7..11].copy_from_slice(&colour.to_le_bytes());
        m[11] = align;
        let n = s.len().min(m.len() - 12);
        m[12..12 + n].copy_from_slice(&s.as_bytes()[..n]);
        self.op(&m[..12 + n])
    }
    fn rect(&self, x: u16, y: u16, w: u16, h: u16, colour: u32) -> u8 {
        let mut m = [0u8; 13];
        m[0] = 3;
        for (i, v) in [x, y, w, h].iter().enumerate() {
            m[1 + 2 * i..3 + 2 * i].copy_from_slice(&v.to_le_bytes());
        }
        m[9..13].copy_from_slice(&colour.to_le_bytes());
        self.op(&m)
    }
    fn present(&self) -> u8 {
        self.op(&[4])
    }
    fn info(&mut self) -> bool {
        let mut r = [0u8; 16];
        let n = call(cap(self.grants, S_DISPLAY), &[5], &mut r);
        if n >= 12 {
            self.info.copy_from_slice(&r[..12]);
            true
        } else {
            false
        }
    }
}

const WHITE: u32 = 0xF0F0F0;
const GREY: u32 = 0x9A9A9A;
const DIM: u32 = 0x5C5C5C;
const RULE: u32 = 0x282828;
const UNREAD: u32 = 0xFF00FF;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
/// hh:mm:ss of the server's uptime — the only clock there is until a time source exists.
fn uptime(buf: &mut [u8; 8], secs: u32) -> &str {
    let (hh, mm, ss) = ((secs / 3600) % 100, (secs / 60) % 60, secs % 60);
    let two = |v: u32, o: usize, b: &mut [u8; 8]| {
        b[o] = b'0' + (v / 10) as u8;
        b[o + 1] = b'0' + (v % 10) as u8;
    };
    two(hh, 0, buf);
    buf[2] = b':';
    two(mm, 3, buf);
    buf[5] = b':';
    two(ss, 6, buf);
    core::str::from_utf8(buf).unwrap_or("--:--:--")
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let mut sel = [0u8; 1];
    let sel = if input_len > 0 && ferros_sys::input(0, &mut sel) == 1 { sel[0] } else { 0 };
    let once = sel == 1;
    let mut c = Client { grants, info: [0; 12] };
    if !c.info() {
        console(grants, b"shelltest: no display server\n");
        ferros_sys::exit(1);
    }
    // Input 02: the highlight test card instead of the home screen, then exit.
    if sel == 2 {
        let r = c.op(&[11]);
        console(grants, if r == 0 { b"shelltest: test card up\n" } else { b"shelltest: test card failed\n" });
        ferros_sys::exit(0);
    }
    let w = u16::from_le_bytes([c.info[0], c.info[1]]);
    let h = u16::from_le_bytes([c.info[2], c.info[3]]);
    let span = w as f32;
    let margin = (span * 0.04) as u16;
    let mut clock = [0u8; 8];
    let mut first = true;
    loop {
    let secs = u32::from_le_bytes([c.info[8], c.info[9], c.info[10], c.info[11]]);
    let mut bad = 0u32;
    bad += c.begin() as u32;
    // Status line.
    bad += c.text(margin, (span * 0.05) as u16, (span * 0.021) as u16, GREY, 0, uptime(&mut clock, secs)) as u32;
    bad += c.text(w - margin, (span * 0.05) as u16, (span * 0.021) as u16, DIM, 2, "USB \u{2022} 71% \u{2022} slot b") as u32;
    bad += c.rect(margin, (span * 0.08) as u16, w - 2 * margin, 0, RULE) as u32;
    // The people, most recent first; an unread mark beside the ones who want you.
    let rows: [(&str, &str, &str, bool); 4] = [("Ada", "the ferry was foggy, photos tonight", "2 m", true), ("Lin", "beam later?", "40 m", true), ("Mum", "call when you land", "3 h", false), ("Tomas", "sent the plot", "yesterday", false)];
    let row_h = span * 0.11;
    for (i, (who, last, when, unread)) in rows.iter().enumerate() {
        let y0 = span * 0.12 + i as f32 * row_h;
        bad += c.text(margin + (span * 0.03) as u16, (y0 + span * 0.045) as u16, (span * 0.028) as u16, WHITE, 0, who) as u32;
        bad += c.text(margin + (span * 0.03) as u16, (y0 + span * 0.08) as u16, (span * 0.021) as u16, GREY, 0, last) as u32;
        bad += c.text(w - margin, (y0 + span * 0.045) as u16, (span * 0.019) as u16, DIM, 2, when) as u32;
        if *unread {
            bad += c.rect(margin, (y0 + span * 0.03) as u16, (span * 0.012) as u16, (span * 0.012) as u16, UNREAD) as u32;
        }
        bad += c.rect(margin, (y0 + row_h - span * 0.005) as u16, w - 2 * margin, 0, RULE) as u32;
    }
    // The foot.
    bad += c.rect(margin, h - (span * 0.09) as u16, w - 2 * margin, 0, RULE) as u32;
    bad += c.text(margin, h - (span * 0.05) as u16, (span * 0.019) as u16, DIM, 0, "apps") as u32;
    bad += c.text(w - margin, h - (span * 0.05) as u16, (span * 0.019) as u16, GREY, 2, "slot b \u{2022} ferros") as u32;
    let status = c.present();
    if first {
        first = false;
        let mut line = *b"shelltest: home screen drawn, bad ops 0, present status 0\n";
        let n = line.len();
        line[n - 20] = b'0' + (bad % 10) as u8;
        line[n - 2] = b'0' + status.min(9);
        console(grants, &line);
        // The panel through the server: 140 nits (DBV 1353), then the DBV read back (DCS 52) — the display server's own DSIM path, verified by the panel.
        let b = c.op(&[6, (1353u16 & 0xFF) as u8, (1353u16 >> 8) as u8]);
        let mut r = [0u8; 16];
        let k = call(cap(grants, S_DISPLAY), &[10, 0x52, 2], &mut r);
        let mut line = *b"shelltest: brightness op status 0, DBV readback xx xx\n";
        let n = line.len();
        line[n - 22] = b'0' + b.min(9);
        let hex = b"0123456789ABCDEF";
        if k >= 3 && r[0] == 0 {
            line[n - 6] = hex[(r[1] >> 4) as usize];
            line[n - 5] = hex[(r[1] & 15) as usize];
            line[n - 3] = hex[(r[2] >> 4) as usize];
            line[n - 2] = hex[(r[2] & 15) as usize];
        }
        console(grants, &line);
        if once {
            ferros_sys::exit(0);
        }
    }
    // Once a second: the server's clock moves on, and so does ours.
    ferros_sys::sleep_until(ferros_sys::now() + ferros_sys::freq());
    if !c.info() {
        ferros_sys::exit(2);
    }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
