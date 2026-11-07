//! akita's touch controller (docs/BOARDS.md): a Goodix "nottingham", the Berlin family, on husky's SPI port, reset line and rails, so only the protocol is its own. Read from Google's driver (goodix_brl_spi.c, goodix_brl_hw.c, goodix_ts_core.h; /mnt/Harbor/ferros-ref/touch-goodix): the controller is a memory map with 32-bit big-endian addresses. A read is `F1`, the address, three dummy bytes, then the data; a write is `F0`, the address, the data. Every transfer stays inside the 64-byte FIFO, as the FTM5's do: a longer read goes as several, each at its own address.
//!
//! Unlike the FTM5 it keeps no event queue: on each scan with something to say it pulls INT low for a moment (a falling edge, the tree's `irq-flags = 2`) and leaves one frame at its touch-data address, holding every contact on the glass now. The host reads it, then writes a zero there (`brl_after_event_handler`) to hand the buffer back. Entering and leaving are what changes between two frames.

use crate::{Contact, Port, Table};
use ferros_sys::{console, delay_us};

const WRITE: u8 = 0xF0;
const READ: u8 = 0xF1;
/// The boot option word: eight bytes of G#AA written there read back the same once the controller answers (`brl_dev_confirm`).
const BOOTOPTION: u32 = 0x1_0000;
/// The firmware's version block (`struct goodix_fw_version`, 28 bytes, packed, with its own checksum).
const FW_VERSION: u32 = 0x1_0014;
const VERSION_LEN: usize = 28;
/// The firmware's description of itself (`struct goodix_ic_info`): its length first, then the addresses of everything else.
const IC_INFO: u32 = 0x1_0070;
const IC_INFO_MAX: usize = 1024;
/// A read's payload per transfer: the FIFO less the 8-byte prefix.
const CHUNK: usize = 56;
/// A touch frame's head (`IRQ_EVENT_HEAD_LEN`): the event type in the high nibble of byte 0, the contact count in the low nibble of byte 2, its checksum in bytes 6-7.
const HEAD: usize = 8;
/// The touch event bit of the type nibble (`GOODIX_TOUCH_EVENT >> 4`).
const TYPE_TOUCH: u8 = 0x8;
/// At most this many contacts in one frame (`GOODIX_MAX_TOUCH`).
const MAX_TOUCH: usize = 10;
/// The largest point record the driver knows (`BYTES_PER_POINT`).
const POINT_MAX: usize = 14;

/// What the firmware said about itself that the event loop needs.
pub struct Goodix {
    /// Where a touch frame is left.
    touch_data: u32,
    /// One contact's record, from the firmware (8 or 14 bytes).
    point_len: usize,
    /// The contacts on the glass at the last frame, a bit per id.
    down: u16,
}

/// Read `out.len()` bytes from `addr`, a transfer per CHUNK.
fn read(p: &Port, addr: u32, out: &mut [u8]) -> bool {
    let mut at = 0;
    while at < out.len() {
        let n = (out.len() - at).min(CHUNK);
        let a = (addr + at as u32).to_be_bytes();
        if !p.transact(&[READ, a[0], a[1], a[2], a[3], 0xFF, 0xFF, 0xFF], &mut out[at..at + n]) {
            return false;
        }
        at += n;
    }
    true
}

/// Write `data` (at most the FIFO less the 5-byte prefix) to `addr`.
fn write(p: &Port, addr: u32, data: &[u8]) -> bool {
    let mut b = [0u8; 64];
    b[0] = WRITE;
    b[1..5].copy_from_slice(&addr.to_be_bytes());
    b[5..5 + data.len()].copy_from_slice(data);
    p.transact(&b[..5 + data.len()], &mut [])
}

/// `checksum_cmp`'s byte mode: the bytes before the last two, summed to 16 bits, equal those two (little-endian); and the block is not all zeroes or all ones (`is_risk_data`: what an absent or unready controller reads as).
fn checksum_ok(b: &[u8]) -> bool {
    let n = b.len();
    if n < 2 || b.iter().all(|&x| x == 0) || b.iter().all(|&x| x == 0xFF) {
        return false;
    }
    let sum = b[..n - 2].iter().fold(0u32, |s, &x| s + x as u32) & 0xFFFF;
    sum == u16::from_le_bytes([b[n - 2], b[n - 1]]) as u32
}

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}
fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Bring the controller up, `brl_power_on`'s order: the `touch` power switch (the port, IOVDD, 3 ms, AVDD, 15 ms, reset high), 4 ms, the device confirm, the 100 ms the firmware takes to start (`GOODIX_NORMAL_RESET_DELAY_MS_DEFAULT`); then its version and its own description, from which the touch-data address. Each step goes to the console.
pub fn bring_up(p: &Port, grants: u64, power: u64) -> Option<Goodix> {
    if !ferros_sys::power(power, true) {
        console(grants, b"touch: the power switch failed (port clock gated, or a rail)\n");
        return None;
    }
    delay_us(4000);
    let mut answered = false;
    for _ in 0..3 {
        let mut back = [0u8; 8];
        if write(p, BOOTOPTION, &[0xAA; 8]) && read(p, BOOTOPTION, &mut back) && back == [0xAA; 8] {
            answered = true;
            break;
        }
        delay_us(5000);
    }
    say!(grants, "touch: Goodix device confirm {}\n", if answered { "answered" } else { "no answer" });
    delay_us(100_000);
    let mut v = [0u8; VERSION_LEN];
    if read(p, FW_VERSION, &mut v) && checksum_ok(&v) {
        // rom_pid[6], rom_vid[3], 1, patch_pid[8], patch_vid[4], 1, sensor_id, 2 reserved, checksum.
        let pid = &v[10..18];
        let printable = pid.iter().all(|&c| c == 0 || (0x20..0x7F).contains(&c));
        let n = pid.iter().position(|&c| c == 0).unwrap_or(8);
        say!(grants, "touch: firmware {} G#{:02X}{:02X}{:02X}{:02X}, sensor G#{:02X}\n", if printable { core::str::from_utf8(&pid[..n]).unwrap_or("?") } else { "?" }, v[18], v[19], v[20], v[21], v[23]);
    } else {
        say!(grants, "touch: no valid firmware version (G#{:02X}{:02X}{:02X}{:02X}…)\n", v[0], v[1], v[2], v[3]);
        return None;
    }
    let mut info = [0u8; IC_INFO_MAX];
    let mut len = [0u8; 2];
    if !read(p, IC_INFO, &mut len) {
        return None;
    }
    let n = le16(&len, 0) as usize;
    // WHY: the length is the controller's word, and it sizes the read into a fixed buffer; the driver refuses the same (`length >= GOODIX_IC_INFO_MAX_LEN`).
    if n < 2 || n >= IC_INFO_MAX || !read(p, IC_INFO, &mut info[..n]) || !checksum_ok(&info[..n]) {
        say!(grants, "touch: no valid IC info (length A#{n})\n");
        return None;
    }
    // `convert_ic_info` over the packed layout: length (2), version (16), features (10), then drv, sen, button, force, and five counted lists of u16 (scan rates, mutual, self tx, self rx and stylus frequencies, at most 8 each), then the misc block.
    let mut o = 2 + 16 + 10;
    let (drv, sen) = (info[o], info[o + 1]);
    o += 4;
    for _ in 0..5 {
        let k = info[o] as usize;
        // WHY: each count is the controller's word and steps the offset; the driver refuses more than 8 (MAX_SCAN_RATE_NUM, MAX_SCAN_FREQ_NUM, MAX_FREQ_NUM_STYLUS).
        if k > 8 {
            say!(grants, "touch: IC info list of A#{k} (at most 8)\n");
            return None;
        }
        o += 1 + 2 * k;
    }
    // WHY: the misc block's fields used below end 52 bytes in; a short description would read past what the controller sent.
    if o + 52 > n {
        say!(grants, "touch: IC info too short (A#{n}) for its misc block at A#{o}\n");
        return None;
    }
    let touch_data = le32(&info, o + 44);
    let point_len = le16(&info, o + 50) as usize;
    say!(grants, "touch: A#{drv} drive x A#{sen} sense lines, touch data at G#{touch_data:X}, A#{point_len}-byte points\n");
    // WHY: point_len sizes the frame read; the records the driver knows are 8 or 14 bytes.
    if touch_data == 0 || point_len < 8 || point_len > POINT_MAX {
        return None;
    }
    Some(Goodix { touch_data, point_len, down: 0 })
}

impl Goodix {
    /// Read the frame the controller left, turn it into enter / move / leave against the last one, and hand the buffer back.
    pub fn frame(&mut self, p: &Port, t: &mut Table) {
        let mut h = [0u8; HEAD];
        if !read(p, self.touch_data, &mut h) || h[0] == 0 {
            return;
        }
        let mut now = 0u16;
        let mut seen = [Contact::default(); MAX_TOUCH];
        let mut good = checksum_ok(&h) && (h[0] >> 4) & TYPE_TOUCH != 0;
        let touches = (h[2] & 0x0F) as usize;
        // WHY: the count is the controller's word and sizes the read below; the driver ignores ids past GOODIX_MAX_TOUCH the same way.
        if good && touches > MAX_TOUCH {
            good = false;
        }
        if good && touches > 0 {
            let n = touches * self.point_len + 2;
            let mut d = [0u8; MAX_TOUCH * POINT_MAX + 2];
            good = read(p, self.touch_data + HEAD as u32, &mut d[..n]) && checksum_ok(&d[..n]);
            if good {
                for r in d[..n - 2].chunks_exact(self.point_len) {
                    let id = (r[0] >> 4) as usize;
                    if id >= MAX_TOUCH {
                        continue;
                    }
                    let wide = self.point_len > 8;
                    seen[id] = Contact {
                        id: id as u8,
                        phase: 1,
                        kind: r[0] & 0x0F,
                        pressure: if wide { r[8] } else { 0 },
                        x: le16(r, 2),
                        y: le16(r, 4),
                        major: if wide { le16(r, 9) } else { le16(r, 6) },
                        minor: if wide { le16(r, 11) } else { le16(r, 6) },
                    };
                    now |= 1 << id;
                }
            }
        }
        // Hand the buffer back whatever it held, as the driver does after every event.
        write(p, self.touch_data, &[0]);
        if !good {
            return;
        }
        for id in 0..MAX_TOUCH {
            let (was, is) = (self.down >> id & 1 == 1, now >> id & 1 == 1);
            if is {
                let mut c = seen[id];
                c.phase = if was { 1 } else { 0 };
                t.push(c);
            } else if was {
                t.leave(id);
            }
        }
        self.down = now;
    }
}
