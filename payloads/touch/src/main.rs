//! The touch server: an EL0 process that reads the ST FTM5 touch controller over the HSI0 USI2 SPI port and serves contacts on the `touch` endpoint.
//!
//! It brings the controller up itself (KERNEL-OUT.md move 1): the `touch` power switch does what only the kernel may — the port's clocks, the rails over ACPM, the reset (`fts_chip_powercycle`'s order) — and then this process waits for the controller-ready event, applies the GPIO6 fix-up, and sets scan mode, the probe's order (docs/oracle/TOUCH.md). From then on the port is driven exactly as the kernel drove it: CPU mode, byte transfers, every transfer inside the 64-byte FIFO, through one MMIO capability. The controller's interrupt line is read as a GPIO *level* rather than taken as an interrupt, because EL0 has no interrupt capability on ferros today and every EL0 driver polls (payloads/pcie1, the WiFi driver). A level read costs one syscall and the queue holds events until drained, so polling loses nothing but latency.
//!
//! Spawn: ferros-bridge spawn touch.bin 00 --name touch --serve touch --hw usi2-spi --hw gpio-gpn0 --irq touch-int --power touch
//! Grant slots, in that order: 0 console, 1 the `touch` endpoint (serve), 2 the SPI port, 3 the GPIO bank with the interrupt line, 4 the interrupt, 5 the `touch` power switch.
//!
//! The `touch` endpoint. One op per call, first byte the op, and a contact record is 12 bytes:
//! `id u8`, `phase u8` (0 enter, 1 move, 2 leave), `kind u8` (the controller's touch type — finger, glove, stylus, palm), `pressure u8` (A#0-255, the controller's own scale, passed through unscaled), `x u16 LE`, `y u16 LE`, `major u16 LE`, `minor u16 LE`. Coordinates are panel pixels, the same space the `display` endpoint takes and fluor hit-tests in, so nothing converts anywhere.
//!
//! - `0` CHANGES: every contact event seen since the last CHANGES call — a count byte, then that many records, oldest first. This is what an event loop wants.
//! - `1` HELD: the contacts on the glass *now* — a count byte, then that many records, phase always move. This is what "how hard is this control being pressed" wants, and it exists because the controller reports a contact entering, then only when something about it changes, then leaving: **a finger held perfectly still produces no events at all**. A delta-only endpoint would make every client rebuild this table, and get it wrong the same way the decay screen did before the owner caught it.
//! - `2` INFO: `x_max u16 LE`, `y_max u16 LE`, `slots u8` — the panel's largest coordinates and how many contacts the table holds.
//! - `3` INJECT, development builds only (docs/INPUT-INJECT.md, payloads/inject): `count u8`, then that many records as above with the id a finger number (below `ferros_inject::FINGERS`) → `status u8` (0 taken, 1 refused), `injected u16 LE`. The records go into the table in order exactly as the controller's events do, so CHANGES and HELD serve them to every consumer alike; finger f takes the slot 15 − f. A batch is taken whole or not at all: refused when a finger enters while its slot is held (by a real finger or itself) or moves or leaves while it is not an injected contact. A real contact landing on an injected slot takes it over, and the injector's next record for it is refused.
//! - `4` INJECTED, development builds only: `injected u16 LE` (bit s: the contact in slot s is injected), `taken u32 LE` (injected records taken since this process started): the one place an injected contact differs from a real one, for diagnostics.
//! A production build (FERROS_PRODUCTION set at build time, as the kernel's and the display's) has neither op: they answer as unknown ops do.

#![no_std]
#![no_main]

use ferros_sys::{cap, console, delay_us, freq, mmio_read, mmio_write, now, recv, reply, NOTHING};

const S_CONSOLE: u64 = 0;
const S_TOUCH: u64 = 1;
const S_SPI: u64 = 2;
const S_GPIO: u64 = 3;
/// The controller's interrupt line (kernel `touch-int`, gpn0-0): the loop waits on it instead of polling the pin.
const S_IRQ: u64 = 4;
/// The `touch` power switch: clocks, rails and reset (the kernel's touch.rs `power`).
const S_POWER: u64 = 5;

// The s3c64xx SPI registers, offsets from the port's base (ferros_kernel/src/spi.rs).
const CH_CFG: u64 = 0x00;
const MODE_CFG: u64 = 0x08;
const SLAVE_SEL: u64 = 0x0C;
const STATUS: u64 = 0x14;
const TX_DATA: u64 = 0x18;
const RX_DATA: u64 = 0x1C;
const PACKET_CNT: u64 = 0x20;
const FB_CLK: u64 = 0x2C;
const CH_TX_ON: u32 = 1 << 0;
const CH_RX_ON: u32 = 1 << 1;
const CH_SW_RST: u32 = 1 << 5;
const PACKET_CNT_EN: u32 = 1 << 16;
/// Byte transfers, every channel's threshold at its maximum: one transfer per command, no DMA.
const MODE_BYTE: u32 = 0x1FF8_0000;
/// The interrupt line is gpn0 pin 0; a GPIO bank's data register is at +4, and the line is active low.
const GPIO_DAT: u64 = 0x04;
const GPIO_CON: u64 = 0x00;
/// The bank's external-interrupt registers (pinctrl-exynos.h, relative to the GPIO_CUSTOM_ALIVE block the grant covers): control (a nibble per pin: 0 = low level), mask and pending.
const EINT_CON: u64 = 0x700;
const EINT_MASK: u64 = 0x900;
const EINT_PEND: u64 = 0xA00;

/// FTM5 commands (docs/oracle/TOUCH.md).
const FIFO_READ: u8 = 0x87;
/// Hardware-register read and write (32-bit address, big-endian; a read returns one dummy byte first) and the scan-mode command (ftsHardware.h, SPI variant).
const HW_REG_R: u8 = 0xFB;
const HW_REG_W: u8 = 0xFA;
const SCAN_MODE: u8 = 0xA0;
const EVT_SIZE: usize = 8;
/// Event identifiers are the HIGH NIBBLE of byte zero alone: the low nibble is payload, carrying the top two bits of the major axis.
const EVT_CONTROLLER_READY: u8 = 0x0;
const EVT_ENTER: u8 = 0x1;
const EVT_MOTION: u8 = 0x2;
const EVT_LEAVE: u8 = 0x3;

const SLOTS: usize = 16;
/// How many events are remembered between two CHANGES calls. A client polling every frame sees far fewer; a client that stops asking loses the oldest.
const CHANGES: usize = 64;
const REC: usize = 12;
/// A development build (FERROS_PRODUCTION unset): the injection ops exist (payloads/display's `DEV`).
const DEV: bool = option_env!("FERROS_PRODUCTION").is_none();
/// Fingers an injector may hold (ferros_inject::FINGERS): finger f is slot 15 − f.
const INJECT_FINGERS: usize = 1 << 3;

struct Port {
    grants: u64,
}

impl Port {
    fn rd(&self, off: u64) -> u32 {
        mmio_read(cap(self.grants, S_SPI), off) as u32
    }
    fn wr(&self, off: u64, v: u32) {
        mmio_write(cap(self.grants, S_SPI), off, v);
    }
    /// True while the controller is asserting its interrupt line.
    fn irq(&self) -> bool {
        mmio_read(cap(self.grants, S_GPIO), GPIO_DAT) as u32 & 1 == 0
    }
    /// gpn0-0 as an external interrupt, unmasked, pending cleared: level-low for the FTM5 (the GIC line fires while the controller holds INT low), a falling edge for akita's Goodix (it pulses INT low once a frame: the tree's `irq-flags = 2`; the edge latches in the bank's pending bit until `irq_done`).
    fn irq_setup(&self, edge: bool) {
        let g = cap(self.grants, S_GPIO);
        mmio_write(g, GPIO_CON, (mmio_read(g, GPIO_CON) as u32 & !0xF) | 0xF);
        mmio_write(g, EINT_CON, (mmio_read(g, EINT_CON) as u32 & !0xF) | if edge { 2 } else { 0 });
        mmio_write(g, EINT_PEND, 1);
        mmio_write(g, EINT_MASK, mmio_read(g, EINT_MASK) as u32 & !1);
    }
    /// The line handled: the bank's pending bit cleared, the GIC line unmasked.
    fn irq_done(&self) {
        mmio_write(cap(self.grants, S_GPIO), EINT_PEND, 1);
        ferros_sys::irq_ack(cap(self.grants, S_IRQ));
    }
    fn flush(&self) {
        self.wr(CH_CFG, self.rd(CH_CFG) & !(CH_TX_ON | CH_RX_ON));
        self.wr(PACKET_CNT, 0);
        self.wr(CH_CFG, self.rd(CH_CFG) | CH_SW_RST);
        let t = now() + freq() / 1000;
        while (self.rd(STATUS) >> 6) & 0x1FF != 0 || (self.rd(STATUS) >> 15) & 0x1FF != 0 {
            if now() > t {
                break;
            }
        }
        self.wr(CH_CFG, self.rd(CH_CFG) & !CH_SW_RST);
    }
    /// One direction of one transfer. Never longer than the 64-byte FIFO, which is what keeps this DMA-free.
    fn segment(&self, tx: Option<&[u8]>, rx: Option<&mut [u8]>, len: usize) -> bool {
        if len == 0 || len > 64 {
            return false;
        }
        self.wr(PACKET_CNT, 0);
        self.wr(PACKET_CNT, len as u32);
        self.wr(PACKET_CNT, len as u32 | PACKET_CNT_EN);
        self.wr(MODE_CFG, MODE_BYTE);
        let ch = self.rd(CH_CFG) & !(CH_TX_ON | CH_RX_ON | 0xC);
        if let Some(t) = tx {
            self.wr(CH_CFG, ch | CH_RX_ON | CH_TX_ON);
            for &x in t {
                self.wr(TX_DATA, x as u32);
            }
        } else {
            self.wr(CH_CFG, ch | CH_RX_ON);
        }
        let t = now() + freq() / 100;
        while ((self.rd(STATUS) >> 15) & 0x1FF) < len as u32 {
            if now() > t {
                self.flush();
                return false;
            }
        }
        if let Some(out) = rx {
            for o in out.iter_mut().take(len) {
                *o = self.rd(RX_DATA) as u8;
            }
        }
        self.flush();
        true
    }
    /// A command, then its reply. The controller's select is the controller's own (`Cs::Native`), so no GPIO is touched here.
    fn transact(&self, cmd: &[u8], out: &mut [u8]) -> bool {
        self.wr(FB_CLK, 0);
        self.wr(SLAVE_SEL, 0);
        let n = cmd.len();
        let mut ok = self.segment(Some(cmd), None, n);
        if ok && !out.is_empty() {
            let n = out.len();
            ok = self.segment(None, Some(out), n);
        }
        delay_us(10);
        self.wr(SLAVE_SEL, 1);
        ok
    }
    /// Drain the event queue as the vendor interrupt handler does: one read for the first event, whose byte seven carries in its low five bits how many are still queued, then bursts of up to seven more (seven events and the dummy byte fill one 64-byte segment).
    /// Reading one event at a time races the queue cursor — the same event returns with its tail zeroed, which decodes as a contact at the top of the screen at zero pressure.
    fn drain(&self, buf: &mut [[u8; EVT_SIZE]; 32]) -> usize {
        let mut b = [0u8; 1 + EVT_SIZE * 7];
        if !self.transact(&[FIFO_READ], &mut b[..1 + EVT_SIZE]) {
            return 0;
        }
        if b[1] == 0x00 || b[1] == 0xFF {
            return 0;
        }
        buf[0].copy_from_slice(&b[1..1 + EVT_SIZE]);
        let mut n = 1;
        let mut queued = (buf[0][7] & 0x1F) as usize;
        while queued > 0 && n < buf.len() {
            let k = queued.min(7).min(buf.len() - n);
            if !self.transact(&[FIFO_READ], &mut b[..1 + EVT_SIZE * k]) {
                return n;
            }
            for i in 0..k {
                let e = &b[1 + i * EVT_SIZE..1 + (i + 1) * EVT_SIZE];
                if e[0] == 0x00 || e[0] == 0xFF {
                    return n;
                }
                buf[n].copy_from_slice(e);
                n += 1;
            }
            queued -= k;
        }
        n
    }
}

/// One contact, as the endpoint serves it.
#[derive(Clone, Copy, Default)]
struct Contact {
    id: u8,
    phase: u8,
    kind: u8,
    pressure: u8,
    x: u16,
    y: u16,
    major: u16,
    minor: u16,
}

impl Contact {
    fn write(&self, out: &mut [u8]) {
        out[0] = self.id;
        out[1] = self.phase;
        out[2] = self.kind;
        out[3] = self.pressure;
        out[4..6].copy_from_slice(&self.x.to_le_bytes());
        out[6..8].copy_from_slice(&self.y.to_le_bytes());
        out[8..10].copy_from_slice(&self.major.to_le_bytes());
        out[10..12].copy_from_slice(&self.minor.to_le_bytes());
    }
}

/// An event's contact fields, assembled as the vendor driver assembles them (`AREA_SCALE` is 16).
fn decode(e: &[u8; EVT_SIZE], phase: u8) -> Contact {
    Contact {
        id: (e[1] >> 4) & 15,
        phase,
        kind: e[1] & 0x0F,
        pressure: e[5],
        x: (((e[3] as u16) & 0xF) << 8) | e[2] as u16,
        y: ((e[4] as u16) << 4) | ((e[3] as u16) >> 4),
        major: ((((e[0] as u16 & 0x0C) << 2) | ((e[6] as u16 & 0xF0) >> 4)) * 16).min(4095),
        minor: ((((e[7] as u16 & 0xC0) >> 2) | (e[6] as u16 & 0x0F)) * 16).min(4095),
    }
}

/// What is on the glass, and what has happened since the client last asked: the endpoint's state, whichever controller fills it.
struct Table {
    held: [Contact; SLOTS],
    down: u16,
    changes: [Contact; CHANGES],
    head: usize,
    count: usize,
    /// Bit s: the contact in slot s came from an injector (op 3), not the controller. Diagnostics only (op 4): nothing served to a consumer carries it.
    injected: u16,
    /// Injected records taken since the process started.
    taken: u32,
}

impl Table {
    /// One contact event from the controller. A real finger in a slot an injector held takes the slot over.
    fn push(&mut self, c: Contact) {
        self.injected &= !(1 << c.id);
        self.record(c);
    }
    /// Op 3: an injector's batch, `count` records at `recs` with finger numbers for ids, taken whole or refused whole (see the module's op list). Returns whether it was taken.
    fn inject(&mut self, recs: &[u8], count: usize) -> bool {
        // The order first, on copies: nothing is applied unless every record is in order.
        let (mut down, mut injected) = (self.down, self.injected);
        for r in recs.chunks_exact(REC).take(count) {
            // WHY: the finger number is the caller's; a finger past INJECT_FINGERS has no slot (15 − f would leave the table's top half, and underflow past 15).
            if r[0] as usize >= INJECT_FINGERS {
                return false;
            }
            let bit = 1u16 << (SLOTS - 1 - r[0] as usize);
            match r[1] {
                0 if down & bit == 0 => {
                    down |= bit;
                    injected |= bit;
                }
                1 if injected & bit != 0 => {}
                2 if injected & bit != 0 => {
                    down &= !bit;
                    injected &= !bit;
                }
                _ => return false,
            }
        }
        for r in recs.chunks_exact(REC).take(count) {
            let w = |at: usize| u16::from_le_bytes([r[at], r[at + 1]]);
            let c = Contact { id: (SLOTS - 1 - r[0] as usize) as u8, phase: r[1], kind: r[2], pressure: r[3], x: w(4), y: w(6), major: w(8), minor: w(10) };
            self.record(c);
            self.taken = self.taken.wrapping_add(1);
        }
        self.injected = injected;
        true
    }
    /// One contact event, whoever made it: the held table updated, the event remembered (the oldest dropped once CHANGES are waiting).
    fn record(&mut self, c: Contact) {
        let id = c.id as usize;
        if c.phase == 2 {
            self.down &= !(1 << id);
        } else {
            self.held[id] = c;
            self.down |= 1 << id;
        }
        self.changes[(self.head + self.count) % CHANGES] = c;
        if self.count < CHANGES {
            self.count += 1;
        } else {
            self.head = (self.head + 1) % CHANGES;
        }
    }
    /// The contact `id` has left, where it was last seen.
    fn leave(&mut self, id: usize) {
        let mut c = self.held[id];
        c.phase = 2;
        self.push(c);
    }
}

/// One line to the console, formatted without a heap.
struct Line {
    b: [u8; 160],
    n: usize,
}
impl core::fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let k = s.len().min(self.b.len() - self.n);
        self.b[self.n..self.n + k].copy_from_slice(&s.as_bytes()[..k]);
        self.n += k;
        Ok(())
    }
}
macro_rules! say {
    ($g:expr, $($t:tt)*) => {{
        let mut l = $crate::Line { b: [0; 160], n: 0 };
        let _ = core::fmt::Write::write_fmt(&mut l, format_args!($($t)*));
        console($g, &l.b[..l.n]);
    }};
}

mod goodix;

/// Bring the controller up from cold, as the kernel's `touch::up` did until KERNEL-OUT.md move 1: the `touch` power switch (the port's clocks, reset low, DVDD, 1 ms, AVDD, 15 ms, reset high), then the probe's order from the oracle capture — the controller-ready event (up to 2 s), the GPIO6 fix-up (`FTS_GPIO6_UNUSED`: bit 7 of hardware register G#2000_00DE), the chip ID, and scan mode active with every sensing bit on (`A0 00 FF`, after the two-byte zero write to G#2000_0000 the controller needs before a mode change). Each step goes to the console; a step that fails is said and the rest still run, as the kernel's did.
fn bring_up(p: &Port, grants: u64) -> bool {
    if !ferros_sys::power(cap(grants, S_POWER), true) {
        console(grants, b"touch: the power switch failed (port clock gated, or a rail)\n");
        return false;
    }
    let t0 = now();
    let mut ready = false;
    while now().wrapping_sub(t0) < 2 * freq() {
        let mut b = [0u8; 1 + EVT_SIZE];
        if !p.transact(&[FIFO_READ], &mut b) {
            console(grants, b"touch: SPI transfer failed waiting for the controller\n");
            return false;
        }
        // An empty FIFO reads as all zeroes or all ones; zero shares its high nibble with controller-ready, so nothing-pending is ruled out first.
        if b[1] == 0x00 || b[1] == 0xFF {
            delay_us(4000);
        } else if b[1] >> 4 == EVT_CONTROLLER_READY {
            ready = true;
            break;
        }
    }
    let ms = now().wrapping_sub(t0) * 1000 / freq();
    if !ready {
        say!(grants, "touch: no controller-ready event in 2 s\n");
        return false;
    }
    say!(grants, "touch: controller ready after A#{ms} ms\n");
    let mut g = [0u8; 2];
    if p.transact(&[HW_REG_R, 0x20, 0x00, 0x00, 0xDE], &mut g) && p.transact(&[HW_REG_W, 0x20, 0x00, 0x00, 0xDE, g[1] | 0x80], &mut []) {
        say!(grants, "touch: GPIO6 register G#{:02X} -> G#{:02X}\n", g[1], g[1] | 0x80);
    }
    let mut id = [0u8; 9];
    if p.transact(&[HW_REG_R, 0x20, 0x00, 0x00, 0x00], &mut id) {
        say!(grants, "touch: chip ID G#{:02X}{:02X}, fw G#{:02X}{:02X}\n", id[1], id[2], id[6], id[5]);
    }
    let scanning = p.transact(&[HW_REG_W, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00], &mut []) && p.transact(&[SCAN_MODE, 0x00, 0xFF], &mut []);
    say!(grants, "touch: scan mode {}\n", if scanning { "set" } else { "failed" });
    true
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _input: u64, _input_len: u64) -> ! {
    // The bring-up's console lines format through tables of absolute addresses: apply the payload's relocations first.
    unsafe { ferros_sys::relocate() };
    let p = Port { grants };
    // akita carries a Goodix where husky has the FTM5 (fact 84; goodix.rs): the same port, reset and rails, its own protocol.
    let akita = ferros_sys::facts().board() == Some(2);
    let mut gdx = None;
    // Up to three tries, the controller switched fully off between them: after the jump from Android (the controller still in Android's session) the first bring-up found no controller-ready event, and a restart of this process brought it up (2026-11-01).
    for attempt in 0..3 {
        let up = if akita {
            gdx = goodix::bring_up(&p, grants, cap(grants, S_POWER));
            gdx.is_some()
        } else {
            bring_up(&p, grants)
        };
        if up {
            break;
        }
        if attempt < 2 {
            console(grants, b"touch: powering the controller off and trying again\n");
            ferros_sys::power(cap(grants, S_POWER), false);
            delay_us(64_000);
        }
    }
    console(grants, b"touch: serving the touch endpoint\n");
    let mut t = Table { held: [Contact::default(); SLOTS], down: 0, changes: [Contact::default(); CHANGES], head: 0, count: 0, injected: 0, taken: 0 };
    // Room for an injection batch: the op, the count and a record per finger an injector may hold.
    let mut msg = [0u8; 2 + REC * INJECT_FINGERS];
    let mut out = [0u8; 1 + REC * SLOTS];
    let mut token = [0u8; 32];
    let tick = freq() / 128;
    p.irq_setup(akita);
    let mut screen_off = false;
    // akita: the line fired since the last pass (an edge, gone by the time the pin could be read).
    let mut fired = false;
    loop {
        // The process watchdog's pet, once a pass (ferros_sys::pet; measuring, `diag pets`).
        ferros_sys::pet();
        // Screen off, sensing off (owner, 2026-10-25: lean power): the controller stops scanning while the screen is dark and starts again when it comes back, as the vendor driver's mode handler does on screen down/up — scan mode active with every sensing bit clear (`A0 00 00`) or set (`A0 00 FF`), each after the two-byte zero write to G#2000_0000 the controller needs before a mode change. The FTM5's commands only: akita's Goodix sleeps by a command through its command address (`brl_suspend`), not written yet, and keeps scanning.
        let off = ferros_sys::pref(ferros_sys::PREF_SCREEN_OFF) == 1;
        if off != screen_off && !akita {
            p.transact(&[0xFA, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00], &mut []);
            p.transact(&[0xA0, 0x00, if off { 0x00 } else { 0xFF }], &mut []);
        }
        screen_off = off;
        // The controller first, so a client's call is answered from a table that is already current.
        if let Some(g) = gdx.as_mut() {
            if fired || p.irq() {
                g.frame(&p, &mut t);
            }
        } else if p.irq() {
            let mut batch = [[0u8; EVT_SIZE]; 32];
            let n = p.drain(&mut batch);
            for e in batch[..n].iter() {
                let phase = match e[0] >> 4 {
                    EVT_ENTER => 0,
                    EVT_MOTION => 1,
                    EVT_LEAVE => 2,
                    // Status, controller-ready and error events carry no contact.
                    _ => continue,
                };
                t.push(decode(e, phase));
            }
        }
        // Then any client waiting. `recv` polls, so the loop never blocks and the table keeps moving.
        let n = recv(cap(grants, S_TOUCH), &mut msg, &mut token, true);
        if n != NOTHING && n != 0 {
            let len = match msg[0] {
                0 => {
                    let k = t.count.min(SLOTS);
                    out[0] = k as u8;
                    for i in 0..k {
                        t.changes[(t.head + i) % CHANGES].write(&mut out[1 + i * REC..1 + (i + 1) * REC]);
                    }
                    t.head = (t.head + k) % CHANGES;
                    t.count -= k;
                    1 + k * REC
                }
                1 => {
                    let mut k = 0;
                    for id in 0..SLOTS {
                        if t.down >> id & 1 == 1 {
                            let mut c = t.held[id];
                            c.phase = 1;
                            c.write(&mut out[1 + k * REC..1 + (k + 1) * REC]);
                            k += 1;
                        }
                    }
                    out[0] = k as u8;
                    1 + k * REC
                }
                2 => {
                    // The panel's largest coordinates: husky's HK3 1344 × 2992, akita's AK3B 1080 × 2400 (its tree's panel-max-x/y).
                    let (xm, ym): (u16, u16) = if akita { (1079, 2399) } else { (1343, 2991) };
                    out[0..2].copy_from_slice(&xm.to_le_bytes());
                    out[2..4].copy_from_slice(&ym.to_le_bytes());
                    out[4] = SLOTS as u8;
                    5
                }
                3 if DEV => {
                    let count = msg[1] as usize;
                    // WHY: the count is the caller's; records it names past the bytes it sent were never sent (the kernel cuts a message to this buffer, so a count past INJECT_FINGERS lands here too).
                    let taken = 2 + count * REC <= n as usize && t.inject(&msg[2..2 + count * REC], count);
                    out[0] = if taken { 0 } else { 1 };
                    out[1..3].copy_from_slice(&t.injected.to_le_bytes());
                    3
                }
                4 if DEV => {
                    out[0..2].copy_from_slice(&t.injected.to_le_bytes());
                    out[2..6].copy_from_slice(&t.taken.to_le_bytes());
                    6
                }
                _ => {
                    out[0] = 0;
                    1
                }
            };
            reply(&token, &out[..len]);
        }
        // Until the controller raises its line, a client calls (the kernel ends the wait early for the endpoint's poller), or the tick — whichever first. The line is masked from firing to `irq_done`; the drain above has emptied the FIFO by then, so INT is high again.
        // A finger is the world: the watchdog's clock paused while waiting for one (ferros_sys::irq_wait_world).
        fired = ferros_sys::irq_wait_world(cap(grants, S_IRQ), now() + tick) == 0;
        p.irq_done();
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
