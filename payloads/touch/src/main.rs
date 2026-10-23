//! The touch server: an EL0 process that reads the ST FTM5 touch controller over the HSI0 USI2 SPI port and serves contacts on the `touch` endpoint.
//!
//! The kernel brings the controller up before this process is spawned — rails over ACPM, reset, the scan-mode command (`touch::up`, docs/oracle/TOUCH.md) — so nothing here needs a privileged path. From then on the port is driven exactly as the kernel drove it: CPU mode, byte transfers, every transfer inside the 64-byte FIFO, through one MMIO capability. The controller's interrupt line is read as a GPIO *level* rather than taken as an interrupt, because EL0 has no interrupt capability on ferros today and every EL0 driver polls (payloads/pcie1, the WiFi driver). A level read costs one syscall and the queue holds events until drained, so polling loses nothing but latency.
//!
//! Spawn: ferros-bridge spawn touch.bin 00 --name touch --serve touch --hw usi2-spi --hw gpio-gpn0
//! Grant slots, in that order: 0 console, 1 the `touch` endpoint (serve), 2 the SPI port, 3 the GPIO bank with the interrupt line.
//!
//! The `touch` endpoint. One op per call, first byte the op, and a contact record is 12 bytes:
//! `id u8`, `phase u8` (0 enter, 1 move, 2 leave), `kind u8` (the controller's touch type — finger, glove, stylus, palm), `pressure u8` (A#0-255, the controller's own scale, passed through unscaled), `x u16 LE`, `y u16 LE`, `major u16 LE`, `minor u16 LE`. Coordinates are panel pixels, the same space the `display` endpoint takes and fluor hit-tests in, so nothing converts anywhere.
//!
//! - `0` CHANGES: every contact event seen since the last CHANGES call — a count byte, then that many records, oldest first. This is what an event loop wants.
//! - `1` HELD: the contacts on the glass *now* — a count byte, then that many records, phase always move. This is what "how hard is this control being pressed" wants, and it exists because the controller reports a contact entering, then only when something about it changes, then leaving: **a finger held perfectly still produces no events at all**. A delta-only endpoint would make every client rebuild this table, and get it wrong the same way the decay screen did before the owner caught it.
//! - `2` INFO: `x_max u16 LE`, `y_max u16 LE`, `slots u8` — the panel's largest coordinates and how many contacts the table holds.

#![no_std]
#![no_main]

use ferros_sys::{cap, console, delay_us, freq, mmio_read, mmio_write, now, recv, reply, NOTHING};

const S_CONSOLE: u64 = 0;
const S_TOUCH: u64 = 1;
const S_SPI: u64 = 2;
const S_GPIO: u64 = 3;
/// The controller's interrupt line (kernel `touch-int`, gpn0-0): the loop waits on it instead of polling the pin.
const S_IRQ: u64 = 4;

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
const EVT_SIZE: usize = 8;
/// Event identifiers are the HIGH NIBBLE of byte zero alone: the low nibble is payload, carrying the top two bits of the major axis.
const EVT_ENTER: u8 = 0x1;
const EVT_MOTION: u8 = 0x2;
const EVT_LEAVE: u8 = 0x3;

const SLOTS: usize = 16;
/// How many events are remembered between two CHANGES calls. A client polling every frame sees far fewer; a client that stops asking loses the oldest.
const CHANGES: usize = 64;
const REC: usize = 12;

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
    /// gpn0-0 as a level-low external interrupt, unmasked, pending cleared: the GIC line (S_IRQ) then fires while the controller holds INT low.
    fn irq_setup(&self) {
        let g = cap(self.grants, S_GPIO);
        mmio_write(g, GPIO_CON, (mmio_read(g, GPIO_CON) as u32 & !0xF) | 0xF);
        mmio_write(g, EINT_CON, mmio_read(g, EINT_CON) as u32 & !0xF);
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

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _input: u64, _input_len: u64) -> ! {
    let p = Port { grants };
    console(grants, b"touch: serving the touch endpoint\n");
    // What is on the glass, and what has happened since the client last asked.
    let mut held = [Contact::default(); SLOTS];
    let mut down = 0u16;
    let mut changes = [Contact::default(); CHANGES];
    let (mut head, mut count) = (0usize, 0usize);
    let mut msg = [0u8; 64];
    let mut out = [0u8; 1 + REC * SLOTS];
    let mut token = [0u8; 32];
    let tick = freq() / 128;
    p.irq_setup();
    loop {
        // The controller first, so a client's call is answered from a table that is already current.
        if p.irq() {
            let mut batch = [[0u8; EVT_SIZE]; 32];
            let n = p.drain(&mut batch);
            for e in batch[..n].iter() {
                let id = ((e[1] >> 4) & 15) as usize;
                let phase = match e[0] >> 4 {
                    EVT_ENTER => 0,
                    EVT_MOTION => 1,
                    EVT_LEAVE => 2,
                    // Status, controller-ready and error events carry no contact.
                    _ => continue,
                };
                let c = decode(e, phase);
                if phase == 2 {
                    down &= !(1 << id);
                } else {
                    held[id] = c;
                    down |= 1 << id;
                }
                changes[(head + count) % CHANGES] = c;
                if count < CHANGES {
                    count += 1;
                } else {
                    head = (head + 1) % CHANGES;
                }
            }
        }
        // Then any client waiting. `recv` polls, so the loop never blocks and the table keeps moving.
        let n = recv(cap(grants, S_TOUCH), &mut msg, &mut token, true);
        if n != NOTHING && n != 0 {
            let len = match msg[0] {
                0 => {
                    let k = count.min(SLOTS);
                    out[0] = k as u8;
                    for i in 0..k {
                        changes[(head + i) % CHANGES].write(&mut out[1 + i * REC..1 + (i + 1) * REC]);
                    }
                    head = (head + k) % CHANGES;
                    count -= k;
                    1 + k * REC
                }
                1 => {
                    let mut k = 0;
                    for id in 0..SLOTS {
                        if down >> id & 1 == 1 {
                            let mut c = held[id];
                            c.phase = 1;
                            c.write(&mut out[1 + k * REC..1 + (k + 1) * REC]);
                            k += 1;
                        }
                    }
                    out[0] = k as u8;
                    1 + k * REC
                }
                2 => {
                    out[0..2].copy_from_slice(&1343u16.to_le_bytes());
                    out[2..4].copy_from_slice(&2991u16.to_le_bytes());
                    out[4] = SLOTS as u8;
                    5
                }
                _ => {
                    out[0] = 0;
                    1
                }
            };
            reply(&token, &out[..len]);
        }
        // Until the controller raises its line, a client calls (the kernel ends the wait early for the endpoint's poller), or the tick — whichever first. The line is masked from firing to `irq_done`; the drain above has emptied the FIFO by then, so INT is high again.
        ferros_sys::irq_wait(cap(grants, S_IRQ), now() + tick);
        p.irq_done();
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
