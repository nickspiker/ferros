//! The NFC driver (docs/NFC.md): an EL0 process that runs the ST54L's NFC controller over NCI 2.0 on hsi2c_0 and answers as Photon's pairing tag, the same "dumb tag" Android's PhotonHceService is (photon docs/pairing-v2.md): under Photon's AID, SELECT → 9000, GET DATA → the live pairing secret S ‖ 9000 while a ceremony is live, else 6A82; anything else 6D00.
//!
//! The kernel keeps what a process cannot do behind the `nfc` power capability (kernel nfc.rs): the bus's pin modes and controller from cold, the chip's reset pulse, the PMU clock pad, the motor's reset. Everything else is here, thru MMIO grants: the bus's registers, and the interrupt line's level (gpa7-6, high while the chip holds a message).
//!
//! The chip, as Android brings it up (captured 2026-10-31 from Android's i2c trace and the August bugreport's NFC snoop): after reset it holds a CORE_RESET notification; CORE_RESET (keep config), CORE_INIT (NCI 2.0), ST's PROP_NFC_MODE_SET on (the chip resets into NFC mode and says so again), CORE_INIT again, ST's CORE_CONF_PROP, then the stack's RF set-up. The eSE (NFCEE G#86) is never touched: every listen route here goes to the host. Firmware and RF config are already the image's (version 02 06 17 50, config 76 DA): nothing is flashed.
//!
//! Spawn: ferros-bridge spawn nfc.bin --name nfc --serve nfc --hw hsi2c0 --hw gpio-alive --power nfc
//! Grant slots, in that order: 0 console, 1 the `nfc` endpoint (serve), 2 hsi2c_0, 3 gpio-alive, 4 the power switch.
//!
//! The `nfc` endpoint, first byte the op:
//! - `0` STATE: text (the chip's versions, discovery, counts).
//! - `1` SERVE + S (32 bytes): a ceremony is live; GET DATA answers S until STOP. → [0].
//! - `2` STOP: no ceremony; GET DATA answers 6A82. → [0].
//! - `4` READER: the sponsor's side of the tap (photon PhotonNfc's reader): a device found answering ISO-DEP while discovery polls is asked for Photon's tag (SELECT the AID, GET DATA) and the secret it serves kept. Discovery itself is unchanged. → [0], or [1] refused (nobody has attested). Asked again while on: nothing changes.
//! - `5` READER STOP. → [0].
//! - `6` READ SECRET: the secret read off a tapped device since the last ask (32 bytes, once), else nothing.
//! - `3` READS: activations u32, S served u32, field-on events u32 (little-endian, 12 bytes): a ceremony's caller watches S served move to know the sponsor's phone read it.
//! - `7` MOTOR: the vibration motor's DEVID and REVID (u32 LE each, 0 for a read that failed), and its state on the console.
//! - `8` BUZZ + n (one byte, at most G#26): the motor plays ROM waveform n. → [0], or [1] refused (n too large) or a write failed.
//! - `9` TAG: held until discovery reads an unlock tag (NFC-A, Type 2, a 7- or 10-byte UID), then → its UID; → nothing when let go (op 10, or a fifth caller held). Attest with a tag (AUTH.md): the attest screen holds it while it shows, the settings page while it adds a tag. The screen off stops discovery, so nothing is read in the dark.
//! - `10` TAG STOP: every held TAG answered with nothing. → [0].
//! - `11` PROBE: answered [0] at once, then running discovery stopped and started again, its tag detector afresh (`Nfc::rediscover`); a deactivation refused is discovery found stopped without a word, and is said; stopped discovery is retried at once; a chip that does not answer has its reset pulsed and is brought up again. The attest screen rings it at a touch on its empty entry (a doorbell, never a timer: 2026-11-17, a tag that read nothing at the attest screen after hours, `Nfc::probe`); until 2026-11-24 it sent RF_DISCOVER alone, which running discovery answers G#A0 and which changes nothing.
//! - `12` OPEN: held while the client wants NFC on with the screen off (owner, 2026-11-24: "Screen off should turn off the NFC driver unless an app has it open specifically."); → nothing when let go (op 13, or a fifth holder), and dropped when the client's process ends. Without one held, the screen off stops discovery and the field, and the chip sleeps.
//! - `13` CLOSE: every held OPEN answered with nothing; a client that still wants NFC open holds again. → [0].
//!
//! A chip that stops answering is brought up again from a reset pulse (`Nfc::restart`), not with NCI commands alone: a command left unanswered, a bring-up that failed, and a reset the chip did by itself (a CORE_RESET notification outside the bring-up) all lead there, at the next retry, which a screen coming on, a TAG held or a PROBE makes at once (2026-11-17: after a RAM reload the Pro's chip never came up, its CORE_RESET answered after A#590 ms, past the old wait, and every retry was CORE_RESET again; on 2026-11-14 it answered nothing at a screen on after A#42 minutes dark, and discovery was retried for good without a pulse).
//!
//! The vibration motor (a CS40L26 at G#43) shares this bus, so its driver is here, the bus's one owner (KERNEL-OUT.md decision 8, the kernel's haptics.rs until 2026-11-10): the `nfc` switch pulses its reset with the NFC chip's, and ops 7 and 8 run between NCI exchanges, never during one.
#![no_std]
#![no_main]

use core::fmt::Write;
use ferros_sys::{cap, console, freq, mmio_read, mmio_write, now, recv, reply, sleep_until, NOTHING};

const S_NFC: u64 = 1;
const S_BUS: u64 = 2;
const S_ALIVE: u64 = 3;
const S_POWER: u64 = 4;

const TARGET: u8 = 0x08;
/// gpa7's data word in gpio-alive (the bank at +G#C0): pin 6 is the chip's interrupt.
const GPA7_DAT: u64 = 0xC4;
const IRQ_PIN: u32 = 6;
/// The vibration motor's driver, a Cirrus CS40L26 at G#43 on the same bus (zuma-shusky-haptics.dtsi; Google's google-modules/amplifiers/cs40l26). Its registers are 32-bit, addressed by a 32-bit big-endian word, values 32-bit big-endian (the driver's regmap). It runs its ROM firmware out of reset (the `nfc` switch pulses its reset with the NFC chip's); in the driver's `cirrus,fw-rom-only` mode a ROM waveform is queued by writing its index (G#0180_0000 + n, n at most G#26) to the DSP's virtual mailbox G#13020, after a "prevent hibernate" command there. The tuning (boost to 11 V, 2.5 A peak, f0 145 Hz, ReDC 13 Ω) is the RAM firmware's business and is not applied.
const MOTOR: u8 = 0x43;
const M_DEVID: u32 = 0x0;
const M_REVID: u32 = 0x4;
const M_IRQ1_STATUS: u32 = 0x10004;
const M_IRQ1_EINT_1: u32 = 0x10010;
const M_MBOX: u32 = 0x13020;
const M_PREVENT_HIBER: u32 = 0x0200_0003;
const M_ROM_INDEX: u32 = 0x0180_0000;
const M_ROM_LAST: u8 = 0x26;
/// The ROM firmware's power-state word, by silicon revision (cs40l26.c rom_regs): B2, then A1/B0/B1.
const M_PM_STATE_B2: u32 = 0x0280_1F98;
const M_PM_STATE_A1: u32 = 0x0280_0370;
/// Photon's AID (photon PhotonNfc.AID).
const AID: [u8; 8] = [0xF0, 0x50, 0x48, 0x4F, 0x54, 0x4F, 0x4E, 0x01];

// The HS-I2C controller (kernel hsi2c.rs, i2c-exynos5.c auto mode).
const CTL: u64 = 0x00;
const FIFO_CTL: u64 = 0x04;
const CONF: u64 = 0x40;
/// The controller's configuration words, kept across its software reset (`reset`): TRAILING_CTL, CLK_CTL, CLK_SLOT, TIMEOUT, then the timings HS1-HS3, FS1-FS3 and SLA (i2c-exynos5.c G#08-G#10, G#48, G#54-G#6C), as the kernel's `nfc` switch left them (hsi2c.rs `bring_up`). CONF is written last, apart.
/// TIMEOUT above all: it comes out of reset enabled (bit 31, G#800000FF), and with it on no auto-mode transfer completes; `bring_up` clears it. Measured on the Pro (2026-11-22, b1141): a reset that kept only the timings, TRAILING_CTL and CONF left it G#800000FF and every transfer after failed, the stored nfc too once restarted, since the switch's `ready` does not look at it.
const KEPT: [u64; 11] = [0x08, 0x0C, 0x10, 0x48, 0x54, 0x58, 0x5C, 0x60, 0x64, 0x68, 0x6C];
const INT_ENABLE: u64 = 0x20;
const INT_STATUS: u64 = 0x24;
const FIFO_STATUS: u64 = 0x30;
const TX_DATA: u64 = 0x34;
const RX_DATA: u64 = 0x38;
const AUTO_CONF: u64 = 0x44;
const TRANS_STATUS: u64 = 0x50;
const ADDR: u64 = 0x70;
const CTL_MASTER: u32 = 1 << 3;
const CTL_SW_RST: u32 = 1 << 31;
const CTL_RXCHON: u32 = 1 << 6;
const CTL_TXCHON: u32 = 1 << 7;
const INT_TRANSFER_DONE: u32 = 1 << 7;
const INT_ERRORS: u32 = 0xF << 8;
const INT_NODEV: u32 = 1 << 10;
const FIFO_TX_EMPTY: u32 = 1 << 8;
/// FIFO_STATUS's levels: the transmit FIFO's in bits 6-0, the receive FIFO's in bits 22-16 (i2c-exynos5.c HSI2C_TX_FIFO_LVL_MASK, HSI2C_RX_FIFO_LVL).
const FIFO_LVL: u32 = 0x7F;
const FIFO_RX_LVL_SHIFT: u32 = 16;
const FIFO_RX_EMPTY: u32 = 1 << 24;
const AUTO_READ: u32 = 1 << 16;
const AUTO_STOP_AFTER_TRANS: u32 = 1 << 17;
const AUTO_MASTER_RUN: u32 = 1 << 31;
const TRANS_MASTER_BUSY: u32 = 1 << 17;
const ADDR_MASTER_ID_7: u32 = 7 << 24;
/// Polls per wait, a syscall each: thousands of microseconds, far past a 400 kHz byte.
const WAIT_POLLS: u32 = 1 << 12;
/// The receive FIFO's depth.
const FIFO_DEPTH: u32 = 1 << 6;

const E_BUSY: u32 = 2;
const E_NODEV: u32 = 4;
const E_BUS: u32 = 8;
const E_TIMEOUT: u32 = 0x10;

/// An NCI packet: a 3-byte header and up to 255 payload bytes.
type Packet = [u8; 3 + 255];

struct Line {
    b: [u8; 512],
    n: usize,
}
impl Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let k = s.len().min(self.b.len() - self.n);
        self.b[self.n..self.n + k].copy_from_slice(&s.as_bytes()[..k]);
        self.n += k;
        Ok(())
    }
}
macro_rules! say {
    ($g:expr, $($t:tt)*) => {{
        let mut l = Line { b: [0; 512], n: 0 };
        let _ = write!(l, $($t)*);
        console($g, &l.b[..l.n]);
    }};
}

struct Hex<'a>(&'a [u8]);
impl core::fmt::Display for Hex<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        for b in self.0 {
            write!(f, "{b:02X}")?;
        }
        Ok(())
    }
}

fn sleep_ms(n: u64) {
    sleep_until(now() + freq() / 1000 * n);
}

/// How long a command waits for its response: two seconds, the NCI stack's own command-complete timeout (libnfc-nci NFC_CMD_CMPL_TIMEOUT), which the chip is held to rather than to a shorter wait of ours. Until 2026-11-17 it was 512 ms, and after a RAM reload the Pro's chip answered CORE_RESET (with its notification, reason 2) A#590 ms after the command, so every bring-up failed.
const CMD_WAIT_MS: u64 = 1 << 11;

/// The chip's interrupt line (kernel `nfc-irq`, SPI 42, added after the last grant for a holder of hsi2c0: process.rs `LINES_WITH`, 2026-11-13).
const S_IRQ: u64 = 5;
/// gpa7's words in gpio-alive: its pin functions, and its external-interrupt type, mask and pending words (pinctrl-gs.c zuma_pin_alive: gpa7's EINT offset G#18 in the wakeup block's ECON G#700, EMASK G#900, EPEND G#A00).
const GPA7_CON: u64 = 0xC0;
const GPA7_ECON: u64 = 0x700 + 0x18;
const GPA7_EMASK: u64 = 0x900 + 0x18;
const GPA7_EPEND: u64 = 0xA00 + 0x18;
/// The serving loop's doorbell: counted up by `irq_thread` at each interrupt it takes. The loop's idle wait is on it, ended also by a call (`wait_world_or_call`), a preference changing (the screen: the kernel wakes every word waiter), or discovery's retry. Until 2026-11-13 the loop read the pin every millisecond (every 16 in the dark): ~950 wakes a second on the 8a with the screen on.
/// Controller resets after a failed transfer (`recover`), for STATE; the first `RESET_SAY` are said on the console.
static RESETS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
const RESET_SAY: u32 = 1 << 4;
static RUNG: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// 1 while `irq_thread` waits on the line; 0 once it took an interrupt and left the line masked until the loop has read the chip (as payloads/pcie1).
static ARMED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static mut IRQ_STACK: [u8; 1 << 13] = [0; 1 << 13];

/// The interrupt's thread: armed, it waits on the line with no deadline, then counts `RUNG` up, wakes the loop and disarms itself (the line stays masked until `irq_ack`).
extern "C" fn irq_thread(cap: u64) -> ! {
    use core::sync::atomic::Ordering::{Acquire, Release};
    loop {
        while ARMED.load(Acquire) == 0 {
            ferros_sys::wait_world(&ARMED, 0, 0);
        }
        ferros_sys::irq_wait_world(cap, 0);
        ARMED.store(0, Release);
        RUNG.fetch_add(1, Release);
        ferros_sys::wake(&RUNG, 1);
    }
}

/// The line granted and its thread started: the chip's IRQ pin (gpa7-6) an external interrupt, level high (the chip holds it high while it has a message), its pending bit cleared and unmasked. False on a kernel without the line (before 2026-11-13): the loop reads the pin every millisecond as before.
fn irq_up(grants: u64) -> bool {
    let n = unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + grants + 0x200) as *const u32) } as u64;
    if n <= S_IRQ || ferros_sys::irq_wait(cap(grants, S_IRQ), 1) == ferros_sys::ERR {
        return false;
    }
    let alive = cap(grants, S_ALIVE);
    let rmw = |off: u64, clear: u32, set: u32| mmio_write(alive, off, mmio_read(alive, off) as u32 & !clear | set);
    rmw(GPA7_CON, 0xF << (4 * IRQ_PIN), 0xF << (4 * IRQ_PIN));
    rmw(GPA7_ECON, 0x7 << (4 * IRQ_PIN), 1 << (4 * IRQ_PIN));
    mmio_write(alive, GPA7_EPEND, 1 << IRQ_PIN);
    rmw(GPA7_EMASK, 1 << IRQ_PIN, 0);
    // SAFETY: the thread's stack, given to it alone, once.
    ferros_sys::thread_spawn(irq_thread, unsafe { &mut *(&raw mut IRQ_STACK) }, cap(grants, S_IRQ)) != ferros_sys::ERR
}

struct Nfc {
    grants: u64,
    /// The live pairing secret, while a ceremony is.
    secret: Option<[u8; 32]>,
    /// Data credits on the static RF connection (conn 0), from the activation and the controller's credit notifications.
    credits: u32,
    /// An incoming APDU being reassembled from segments (PBF set).
    apdu: [u8; 512],
    apdu_len: usize,
    activations: u32,
    served: u32,
    fields: u32,
    tags: u32,
    version: [u8; 4],
    config: [u8; 2],
    discovering: bool,
    /// A reader's field on us now (RF_FIELD_INFO_NTF's last word).
    field: bool,
    /// Reader mode (op 4, the sponsor's side of the tap): discovery polls NFC-A only and does not listen, so this device is the reader when two devices touch; a device found answering ISO-DEP is asked for Photon's tag (SELECT, then GET DATA) and its secret kept for the identity service (op 6).
    reading: bool,
    /// Where the exchange with a tapped device stands: 0 none, 1 the SELECT sent, 2 the GET DATA sent.
    rstage: u8,
    read_secret: Option<[u8; 32]>,
    /// Discovery's starts that failed in a row: the first is said, the rest are quiet (`quiet` while one is tried) and spaced twice as far apart each time (`retry_wait`).
    discover_failures: u32,
    quiet: bool,
    /// A CORE_RESET_NTF seen since this was cleared (`init`).
    reset_seen: bool,
    /// Inside `init`, where a reset notification is the bring-up's own; outside it one is the chip resetting by itself, and it is brought up again (`needs_init`).
    in_init: bool,
    /// The chip is to be pulsed out of reset and brought up again at the next retry (`restart`): a bring-up that failed, a command it left unanswered, a reset it did by itself.
    needs_init: bool,
    /// A command `cmd` sent went unanswered (or could not be written) since this was cleared.
    unanswered: bool,
    /// Bring-ups from a reset pulse after the first (`restart`); probes (op 11), and those that found discovery stopped without a word.
    pulses: u32,
    probes: u32,
    found_stopped: u32,
    /// ST's own notifications (G#6F): counted, the last one's first bytes kept for STATE, and each said while `st_said` is under `ST_SAY` in a 64 s report's window (they were dropped unseen until 2026-11-17; one may be the only word of a chip that stopped).
    st_notes: u32,
    st_last: [u8; 8],
    st_said: u32,
    /// When the chip last sent anything (the counter), for STATE.
    heard_at: u64,
    /// The chip's full polls (ST's G#6F05, "active reader starts", the notification ST's HAL watches for a stuck poll): since the bring-up, and since discovery last started (`polls_at_start` the count then, `started_at` the counter).
    /// Between full polls the chip only senses its antenna for a tag's load (its tag detector, measured on the Pro 2026-11-24: two full polls about 480 ms apart as discovery starts, then none until a tag comes near; docs/NFC.md "A tag that did nothing at the attest screen").
    polls: u32,
    polls_at_start: u32,
    started_at: u64,
    /// A tag or a reader activated us since discovery last started, for STATE.
    read_since_start: bool,
    /// A tag in the field, for the console's circle (measure STATE 2): set when one is read, cleared when a full poll ends with none read (the chip's G#6F06), when discovery stops, or at a reset pulse.
    tag_here: bool,
    /// A tag read since the chip's last full poll began (its G#6F05).
    poll_saw_tag: bool,
    /// Discovery started again on a doorbell (`rediscover`), for STATE.
    rediscovers: u32,
    /// Op 9's held calls (attest with a tag, AUTH.md): each a reply token, answered with the UID of the next unlock tag discovery reads (`tag_read`), or with nothing by op 10. At most `HELD`, the oldest giving way.
    held: [[u8; 32]; HELD],
    nheld: usize,
    /// Op 12's held calls: each a client holding NFC open with the screen off (owner, 2026-11-24: "Screen off should turn off the NFC driver unless an app has it open specifically."). Let go by op 13, or dropped when the process behind it ends (`sweep`). At most `HELD`, the oldest giving way.
    opens: [[u8; 32]; HELD],
    nopen: usize,
}

/// How many op 9 calls are held at once: the attest screen's and the settings page's, with room.
const HELD: usize = 1 << 2;
/// ST notifications said per 64 s report window: past it they are only counted, so a chatty firmware cannot fill the console ring.
const ST_SAY: u32 = 1 << 4;
/// Discovery: listening as a card on NFC-A and NFC-B, and between listens polling for tags on NFC-A, NFC-B and NFC-V (each entry a technology and mode, then 1: every discovery period), as Android interleaves them.
const DISCOVER: [u8; 14] = [0x21, 0x03, 0x0B, 0x05, 0x00, 0x01, 0x01, 0x01, 0x06, 0x01, 0x80, 0x01, 0x81, 0x01];
/// RF_DISCOVER's status when discovery runs already (NCI 2.0 DISCOVERY_ALREADY_STARTED).
const ALREADY_STARTED: u8 = 0xA0;

impl Nfc {
    fn rd(&self, off: u64) -> u32 {
        mmio_read(cap(self.grants, S_BUS), off) as u32
    }
    fn wr(&self, off: u64, v: u32) {
        mmio_write(cap(self.grants, S_BUS), off, v);
    }
    fn irq(&self) -> bool {
        mmio_read(cap(self.grants, S_ALIVE), GPA7_DAT) as u32 & 1 << IRQ_PIN != 0
    }

    fn fail(&self, st: u32) -> Option<u32> {
        if st & INT_NODEV != 0 {
            self.wr(INT_STATUS, st);
            return Some(E_NODEV);
        }
        if st & INT_ERRORS != 0 {
            self.wr(INT_STATUS, st);
            return Some(E_BUS);
        }
        None
    }

    fn wait_done(&self) -> Result<(), u32> {
        for _ in 0..WAIT_POLLS {
            let st = self.rd(INT_STATUS);
            if let Some(e) = self.fail(st) {
                return Err(e);
            }
            if st & INT_TRANSFER_DONE != 0 {
                self.wr(INT_STATUS, st);
                return Ok(());
            }
        }
        Err(E_TIMEOUT)
    }

    /// One auto-mode transfer to the NFC chip with STOP: `tx` written, or `rx` filled.
    fn transfer(&self, tx: &[u8], rx: &mut [u8]) -> Result<(), u32> {
        self.xfer(TARGET, tx, rx, false)
    }

    /// One auto-mode transfer to `target` on the bus: `tx` written, or `rx` filled, with STOP after it unless `hold` keeps the bus for the read that follows (the motor's register reads: a 32-bit address, then the value). The RX FIFO (64 deep) is drained as the bytes come, so a read of any NCI packet's length works.
    fn xfer(&self, target: u8, tx: &[u8], rx: &mut [u8], hold: bool) -> Result<(), u32> {
        let r = self.xfer_once(target, tx, rx, hold);
        if let Err(e) = r {
            self.recover(target, e);
        }
        r
    }

    /// A transfer that failed (a NACK, a bus error, a timeout), cleaned up as Linux does (i2c-exynos5.c: `exynos5_i2c_reset` on "ack was not received" at a write or a read, and on a timeout): the controller let finish what it is clocking, then reset, which empties both FIFOs.
    /// WHY: a NACK leaves the unsent bytes in the transmit FIFO, and nothing here took them out (i2c-exynos5.c knows no FIFO flush but the reset; drain_rx empties only the receive side). The next write then sends them ahead of its own, its own stay for the one after, and so on. On the Pro (2026-11-22) the motor NACKed its wake-up once and every later register read returned the register before it, exactly: REVID read DEVID's G#40A260, IRQ1 status REVID's G#A1, EINT1 IRQ1's G#1, the mailbox EINT1's G#80004100, the ROM state the mailbox's G#0; the next op 7 NACKed again and ran two behind (DEVID read the ROM state's G#2), with no receive bytes ever reported dropped. Each NACK added one register of lag, which only a queued 4-byte address explains. The NFC chip's own NACKs while it wakes (`send`'s retries) go thru the same path, so a NACKed packet would have stayed queued too (inferred, not seen).
    fn recover(&self, target: u8, e: u32) {
        for _ in 0..WAIT_POLLS {
            if self.rd(TRANS_STATUS) & TRANS_MASTER_BUSY == 0 {
                break;
            }
        }
        let st = self.rd(FIFO_STATUS);
        self.reset();
        let n = RESETS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if n < RESET_SAY {
            say!(self.grants, "nfc: a transfer to G#{target:02X} failed (G#{e:X}), A#{} bytes unsent in the transmit FIFO and A#{} in the receive FIFO: the controller reset, both emptied\n", st & FIFO_LVL, st >> FIFO_RX_LVL_SHIFT & FIFO_LVL);
        }
    }

    /// The controller's software reset with its configuration kept, `exynos5_i2c_reset`'s order: SW_RST set and cleared in CTL, every word of `KEPT` back (the timeout's among them, off as `bring_up` left it), CTL master, then CONF (auto mode). Whatever reset does to them, they come back as the kernel's `nfc` switch left them. Every transfer writes its own FIFO_CTL, INT_ENABLE, ADDR and AUTO_CONF.
    fn reset(&self) {
        let conf = self.rd(CONF);
        let kept = KEPT.map(|o| self.rd(o));
        let c = self.rd(CTL);
        self.wr(CTL, c | CTL_SW_RST);
        self.wr(CTL, c & !CTL_SW_RST);
        for (&o, v) in KEPT.iter().zip(kept) {
            self.wr(o, v);
        }
        self.wr(CTL, CTL_MASTER);
        self.wr(CONF, conf);
    }

    /// One try of `xfer`: on an error it returns at once, the controller left as the error found it (`xfer` resets it).
    fn xfer_once(&self, target: u8, tx: &[u8], rx: &mut [u8], hold: bool) -> Result<(), u32> {
        let mut idle = false;
        for _ in 0..WAIT_POLLS {
            if self.rd(TRANS_STATUS) & TRANS_MASTER_BUSY == 0 {
                idle = true;
                break;
            }
        }
        if !idle {
            return Err(E_BUSY);
        }
        // Every transfer starts from empty FIFOs. A failed transfer resets the controller (`xfer`, `recover`), which empties both; this is the check that nothing got past it. The read below takes the first bytes the receive FIFO holds, and a write sends the transmit FIFO's first bytes, so either would answer or address the wrong register: on the Pro every motor read returned the value of a read before it (2026-11-10 three behind, 2026-11-22 one, then two).
        let tx_left = self.rd(FIFO_STATUS) & FIFO_LVL;
        if tx_left != 0 {
            say!(self.grants, "nfc: A#{tx_left} bytes were waiting in the transmit FIFO before a transfer to G#{target:02X}: the controller reset\n");
            self.reset();
        }
        let stale = self.drain_rx();
        if stale != 0 {
            say!(self.grants, "nfc: A#{stale} bytes were waiting in the receive FIFO before a transfer to G#{target:02X}: dropped\n");
        }
        let read = !rx.is_empty();
        let len = if read { rx.len() } else { tx.len() } as u32;
        self.wr(FIFO_CTL, if read { 0x0002_0023 } else { 0x0001_0013 });
        self.wr(CTL, CTL_MASTER | if read { CTL_RXCHON } else { CTL_TXCHON });
        self.wr(INT_STATUS, self.rd(INT_STATUS));
        self.wr(ADDR, ADDR_MASTER_ID_7 | (target as u32) << 10);
        self.wr(INT_ENABLE, INT_TRANSFER_DONE | INT_ERRORS);
        let auto = len | if read { AUTO_READ } else { 0 } | if hold { 0 } else { AUTO_STOP_AFTER_TRANS };
        self.wr(AUTO_CONF, auto);
        self.wr(AUTO_CONF, auto | AUTO_MASTER_RUN);
        if read {
            self.read_run(rx)
        } else {
            for &b in tx {
                let mut room = false;
                for _ in 0..WAIT_POLLS {
                    if self.rd(FIFO_STATUS) & 0x7F < 48 {
                        room = true;
                        break;
                    }
                    if let Some(e) = self.fail(self.rd(INT_STATUS)) {
                        return Err(e);
                    }
                }
                if !room {
                    return Err(E_TIMEOUT);
                }
                self.wr(TX_DATA, b as u32);
            }
            self.wait_done()?;
            for _ in 0..WAIT_POLLS {
                if self.rd(FIFO_STATUS) & FIFO_TX_EMPTY != 0 {
                    break;
                }
            }
            Ok(())
        }
    }

    /// A running read's bytes into `rx` as they come, then its TRANSFER_DONE.
    fn read_run(&self, rx: &mut [u8]) -> Result<(), u32> {
        let mut got = 0;
        let mut polls = 0;
        while got < rx.len() {
            if self.rd(FIFO_STATUS) & FIFO_RX_EMPTY == 0 {
                rx[got] = self.rd(RX_DATA) as u8;
                got += 1;
                polls = 0;
                continue;
            }
            if let Some(e) = self.fail(self.rd(INT_STATUS)) {
                return Err(e);
            }
            polls += 1;
            if polls > WAIT_POLLS {
                return Err(E_TIMEOUT);
            }
        }
        self.wait_done()
    }

    /// Whatever the receive FIFO holds, read and dropped; how many bytes. WHY the bound: the FIFO is 64 deep, so a status still saying "not empty" after 64 reads is the controller misreporting, and this stops rather than spins.
    fn drain_rx(&self) -> u32 {
        for n in 0..FIFO_DEPTH {
            if self.rd(FIFO_STATUS) & FIFO_RX_EMPTY != 0 {
                return n;
            }
            self.rd(RX_DATA);
        }
        FIFO_DEPTH
    }

    /// One NCI packet out: the whole packet in one write. The chip NACKs while it is busy waking; the HAL retries, and so does this, every millisecond for 16.
    fn send(&self, p: &[u8]) -> Result<(), u32> {
        let mut r = Err(E_NODEV);
        for _ in 0..16 {
            r = self.transfer(p, &mut []);
            if r != Err(E_NODEV) {
                break;
            }
            sleep_ms(1);
        }
        r
    }

    /// One NCI packet in, if the chip holds one: the 3-byte header, then its payload in a second read (st21nfc.c's two-state read). Leading idle bytes (G#7E) in the header are dropped and made up with more reads; a header all idle is no packet.
    fn read(&self, p: &mut Packet) -> Result<Option<usize>, u32> {
        let mut h = [0u8; 3];
        self.transfer(&[], &mut h)?;
        let idle = h.iter().take_while(|&&b| b == 0x7E).count();
        if idle == 3 {
            return Ok(None);
        }
        if idle > 0 {
            h.copy_within(idle.., 0);
            self.transfer(&[], &mut h[3 - idle..])?;
        }
        p[..3].copy_from_slice(&h);
        let n = h[2] as usize;
        if n > 0 {
            self.transfer(&[], &mut p[3..3 + n])?;
        }
        Ok(Some(3 + n))
    }

    /// Wait up to `ms` for the interrupt line, then read what the chip holds.
    fn next(&mut self, p: &mut Packet, ms: u64) -> Option<usize> {
        let end = now() + freq() / 1000 * ms;
        loop {
            if self.irq() {
                match self.read(p) {
                    Ok(Some(n)) => {
                        self.heard_at = now();
                        return Some(n);
                    }
                    Ok(None) => {}
                    // Fresh out of reset the chip raises its line before it answers its address: tried again next millisecond.
                    Err(E_NODEV) => {}
                    Err(e) => {
                        say!(self.grants, "nfc: read failed G#{e:X}\n");
                        return None;
                    }
                }
            }
            if now() > end {
                return None;
            }
            sleep_ms(1);
        }
    }

    /// A command and its response (MT 2, the same GID and OID); notifications that come first are handled on the way. None, and `unanswered` set, on a write that failed or no response in `CMD_WAIT_MS`.
    fn cmd(&mut self, c: &[u8], p: &mut Packet) -> Option<usize> {
        if let Err(e) = self.send(c) {
            say!(self.grants, "nfc: write {} failed G#{e:X}\n", Hex(c));
            self.unanswered = true;
            return None;
        }
        let end = now() + freq() / 1000 * CMD_WAIT_MS;
        while now() < end {
            let Some(n) = self.next(p, CMD_WAIT_MS) else { break };
            if p[0] >> 5 == 2 && p[0] & 0xF == c[0] & 0xF && p[1] & 0x3F == c[1] & 0x3F {
                return Some(n);
            }
            self.packet(p, n);
        }
        self.unanswered = true;
        if !self.quiet {
            say!(self.grants, "nfc: no response to {}\n", Hex(c));
        }
        None
    }

    /// A command whose response must report status OK (payload byte 0 = 0): its response's length, else a line on the console.
    fn ok(&mut self, c: &[u8], p: &mut Packet) -> Option<usize> {
        let n = self.cmd(c, p)?;
        if n < 4 || p[3] != 0 {
            say!(self.grants, "nfc: {} → {}\n", Hex(c), Hex(&p[..n]));
            return None;
        }
        Some(n)
    }

    /// A packet that is not the awaited response: notifications and data.
    fn packet(&mut self, p: &Packet, n: usize) {
        let mt = p[0] >> 5;
        let pl = &p[3..n];
        match (mt, p[0] & 0xF, p[1] & 0x3F) {
            // Data on the static RF connection: an APDU (segments gathered until PBF clears).
            (0, 0, _) => {
                let k = pl.len().min(self.apdu.len() - self.apdu_len);
                self.apdu[self.apdu_len..self.apdu_len + k].copy_from_slice(&pl[..k]);
                self.apdu_len += k;
                if p[0] & 0x10 == 0 {
                    let mut a = [0u8; 512];
                    let len = self.apdu_len;
                    a[..len].copy_from_slice(&self.apdu[..len]);
                    self.apdu_len = 0;
                    if self.rstage != 0 {
                        self.tag_answer(&a[..len]);
                    } else {
                        self.apdu(&a[..len]);
                    }
                }
            }
            // CORE_RESET_NTF: the chip reset itself (a reason in byte 0: 1 = the host asked, A0 = into NFC mode).
            (3, 0, 0) => {
                self.reset_seen = true;
                if pl.len() >= 2 {
                    say!(self.grants, "nfc: reset notification, reason G#{:02X}, config {}\n", pl[0], if pl[1] == 1 { "kept" } else { "reset" });
                }
                // After the reason, the config status, NCI version and manufacturer id: ST's manufacturer info (its length, then two bytes, the firmware's four, ..., the RF config's two, a mode byte).
                if pl.len() >= 31 {
                    self.version.copy_from_slice(&pl[7..11]);
                    self.config.copy_from_slice(&pl[28..30]);
                }
                // Outside the bring-up nobody asked for it: the chip lost its configuration and its discovery with the reset, so it is brought up again (it was left thinking it discovered, and read nothing after, until 2026-11-17).
                if !self.in_init {
                    say!(self.grants, "nfc: the chip reset by itself: brought up again\n");
                    self.needs_init = true;
                    self.discovering = false;
                }
            }
            // CORE_CONN_CREDITS_NTF: entries of (conn id, credits).
            (3, 0, 6) => {
                for e in pl.get(1..).unwrap_or(&[]).chunks(2) {
                    if e.len() == 2 && e[0] == 0 {
                        self.credits += e[1] as u32;
                    }
                }
            }
            // RF_INTF_ACTIVATED_NTF: a reader selected us; byte 5 is the connection's initial credits.
            (3, 1, 5) => {
                self.read_since_start = true;
                // Byte 3 is the technology and mode: below G#80 we polled and found a tag (or a device answering as one). Its technology parameters follow byte 6 (their length): for NFC-A the SENS_RES, then the UID's length and the UID. Logged, then the tag let go back to discovery (RF_DEACTIVATE, type 3).
                let mode = pl.get(3).copied().unwrap_or(0);
                // Reader mode and an ISO-DEP device (interface G#02): Photon's tag asked for, its SELECT the first APDU.
                if mode < 0x80 && self.reading && pl.get(1) == Some(&0x02) {
                    self.credits = pl.get(5).copied().unwrap_or(1) as u32;
                    self.apdu_len = 0;
                    self.rstage = 1;
                    let mut c = [0u8; 5 + 8 + 1];
                    c[..5].copy_from_slice(&[0x00, 0xA4, 0x04, 0x00, AID.len() as u8]);
                    c[5..13].copy_from_slice(&AID);
                    say!(self.grants, "nfc: a device answers as a tag: asking for Photon's\n");
                    self.data(&c);
                    return;
                }
                if mode < 0x80 {
                    self.tags += 1;
                    self.tag_here = true;
                    self.poll_saw_tag = true;
                    let tl = pl.get(6).copied().unwrap_or(0) as usize;
                    let tp = pl.get(7..7 + tl).unwrap_or(&[]);
                    let uid = if mode == 0 { tp.get(2).and_then(|&l| tp.get(3..3 + l as usize)).unwrap_or(&[]) } else { &[] };
                    let protocol = pl.get(2).copied().unwrap_or(0);
                    // The UID is never logged since it opens the device (attest with a tag, 2026-11-11): its length only, and the SENS_RES that says the tag's kind.
                    say!(self.grants, "nfc: tag: mode G#{mode:02X}, protocol G#{protocol:02X}, interface G#{:02X}, a A#{}-byte UID, SENS_RES {}\n", pl.get(1).copied().unwrap_or(0), uid.len(), Hex(tp.get(..2).unwrap_or(&[])));
                    // An unlock tag: NFC-A (mode 0), Type 2 (protocol G#02, NTAG and its kin), a double or triple size UID, which a tag keeps for life (a 4-byte one may be random at every read, ISO/IEC 14443-3).
                    if mode == 0 && protocol == 0x02 && matches!(uid.len(), 7 | 10) {
                        self.tag_read(uid);
                    }
                    if let Err(e) = self.send(&[0x21, 0x06, 0x01, 0x03]) {
                        say!(self.grants, "nfc: release failed G#{e:X}\n");
                    }
                    return;
                }
                self.activations += 1;
                self.credits = pl.get(5).copied().unwrap_or(1) as u32;
                self.apdu_len = 0;
                say!(self.grants, "nfc: activated: interface G#{:02X}, protocol G#{:02X}, mode G#{:02X}\n", pl.get(1).copied().unwrap_or(0), pl.get(2).copied().unwrap_or(0), pl.get(3).copied().unwrap_or(0));
            }
            // RF_DEACTIVATE_NTF: only idle (type 0) stops discovery and needs it started again; sleep (1, 2: a reader that put the card to sleep) leaves the controller in listen sleep, back to discovery by itself, and discovery (3) is discovery. Treating sleep as stopped re-sent RF_DISCOVER every pass, answered G#A0 (already started), after the first tap to bind (2026-11-04).
            (3, 1, 6) => {
                let kind = pl.first().copied().unwrap_or(0);
                say!(self.grants, "nfc: deactivated, type G#{kind:02X}\n");
                self.discovering = kind != 0;
            }
            // RF_FIELD_INFO_NTF: a reader's field came or went.
            (3, 1, 7) => {
                if pl.first() == Some(&1) {
                    self.fields += 1;
                }
                self.field = pl.first() == Some(&1);
                // Logged as it comes and goes (rare: a reader held to the device): the first thing to see when a tap does nothing is whether the controller felt the reader at all (2026-11-04, tap to bind: no field counted).
                say!(self.grants, "nfc: a reader's field {}\n", if self.field { "on" } else { "off" });
            }
            // The RF_DEACTIVATE a tag's release sent: its answer.
            (2, 1, 6) => {}
            // ST's proprietary notifications (G#6F): not ours to act on, but counted and said (`st_notes`), since a chip that stops reading may say so only here.
            (3, 0xF, _) => {
                self.st_notes += 1;
                // A full poll begins (G#6F05) and ends (G#6F06), each tag read inside one: measured on the Pro (2026-11-24), a tag held to the device is read once a poll, every 62 ms, each read between its poll's 6F05 and 6F06. A poll that ends with no tag read is the tag gone.
                if p[1] & 0x3F == 5 {
                    self.polls += 1;
                    self.poll_saw_tag = false;
                }
                if p[1] & 0x3F == 6 && !self.poll_saw_tag {
                    self.tag_here = false;
                }
                // WHY the min: a notification may be shorter than the 8 bytes kept (a 3-byte header and nothing), and copying 8 from it would panic; a longer one keeps its start. The 64 said below is the console line's room (128 hex digits in its 512 bytes), not a bound (n is at most the packet's 258).
                // Said whole up to 64 bytes since 2026-11-18: the reload's second notification (`6F 02 16 31 20 00 82 06 01 ...`, 25 bytes) was cut at 16, and it is the one a cold boot does not send.
                let k = n.min(self.st_last.len());
                self.st_last = [0; 8];
                self.st_last[..k].copy_from_slice(&p[..k]);
                if self.st_said < ST_SAY {
                    self.st_said += 1;
                    say!(self.grants, "nfc: ST notification {}\n", Hex(&p[..n.min(64)]));
                }
            }
            _ => say!(self.grants, "nfc: {}\n", Hex(&p[..n])),
        }
    }

    /// The tag's two commands (PhotonHceService.processCommandApdu): SELECT by name → 9000 for Photon's AID (6A82 for any other: the controller routes every ISO-DEP AID here), GET DATA → S ‖ 9000 or 6A82, anything else 6D00.
    fn apdu(&mut self, a: &[u8]) {
        let mut r = [0u8; 3 + 34];
        let n = if a.len() >= 4 && a[0] == 0x00 && a[1] == 0xA4 && a[2] == 0x04 {
            let lc = a.get(4).copied().unwrap_or(0) as usize;
            let mine = a.get(5..5 + lc) == Some(&AID[..]);
            r[3..5].copy_from_slice(if mine { &[0x90, 0x00] } else { &[0x6A, 0x82] });
            2
        } else if a.len() >= 2 && a[0] == 0x80 && a[1] == 0xCA {
            match self.secret {
                Some(s) => {
                    r[3..35].copy_from_slice(&s);
                    r[35..37].copy_from_slice(&[0x90, 0x00]);
                    self.served += 1;
                    34
                }
                None => {
                    r[3..5].copy_from_slice(&[0x6A, 0x82]);
                    2
                }
            }
        } else {
            r[3..5].copy_from_slice(&[0x6D, 0x00]);
            2
        };
        r[2] = n as u8;
        // A reply sent with no credit breaks the controller's flow control (NCI 2.0 4.4.4): said, not hidden, and sent anyway, since the reader is waiting on it.
        if self.credits == 0 {
            say!(self.grants, "nfc: replying without a credit\n");
        } else {
            self.credits -= 1;
        }
        if let Err(e) = self.send(&r[..3 + n]) {
            say!(self.grants, "nfc: reply failed G#{e:X}\n");
        }
        say!(self.grants, "nfc: APDU {} → {}\n", Hex(&a[..a.len().min(16)]), if n == 34 { "S" } else { "status" });
    }

    /// Discovery started (`DISCOVER`).
    fn discover(&mut self, p: &mut Packet) -> bool {
        self.quiet = self.discover_failures > 0;
        // G#A0, discovery already started, is discovering too.
        self.discovering = match self.cmd(&DISCOVER, p) {
            Some(n) if n >= 4 && (p[3] == 0 || p[3] == ALREADY_STARTED) => true,
            Some(n) => {
                if !self.quiet {
                    say!(self.grants, "nfc: discover refused: {}\n", Hex(&p[..n]));
                }
                false
            }
            None => false,
        };
        self.quiet = false;
        if self.discovering {
            self.polls_at_start = self.polls;
            self.started_at = now();
            self.read_since_start = false;
            if self.discover_failures > 0 {
                say!(self.grants, "nfc: discovery started again after {} failed tries\n", self.discover_failures);
            }
            self.discover_failures = 0;
        } else {
            if self.discover_failures == 0 {
                say!(self.grants, "nfc: discovery did not start; trying again, each time twice as long after\n");
            }
            self.discover_failures += 1;
        }
        self.discovering
    }

    /// From reset to listening (the module comment's sequence), its reset notifications its own (`in_init`).
    fn init(&mut self, p: &mut Packet) -> bool {
        self.in_init = true;
        let up = self.init_steps(p);
        self.in_init = false;
        self.needs_init = !up;
        up
    }

    /// The chip's reset pulsed again (the `nfc` power switch on: the kernel's pulse, as at the start) and the chip brought up from it: what a chip that stopped answering needs, since NCI commands are what it stopped answering.
    fn restart(&mut self, p: &mut Packet) -> bool {
        self.pulses += 1;
        self.discovering = false;
        self.tag_here = false;
        self.credits = 0;
        self.apdu_len = 0;
        self.rstage = 0;
        if !ferros_sys::power(cap(self.grants, S_POWER), true) {
            say!(self.grants, "nfc: the reset pulse was refused (HSI1 off?)\n");
            self.needs_init = true;
            return false;
        }
        self.init(p)
    }

    fn init_steps(&mut self, p: &mut Packet) -> bool {
        // The reset notification the pulse left.
        if let Some(n) = self.next(p, 64) {
            self.packet(p, n);
        }
        let steps: [&[u8]; 4] = [
            &[0x20, 0x00, 0x01, 0x01],
            &[0x20, 0x01, 0x02, 0x00, 0x00],
            // PROP_NFC_MODE_SET on: the chip resets into NFC mode (a reset notification, reason A0).
            &[0x2F, 0x02, 0x02, 0x02, 0x01],
            &[0x20, 0x01, 0x02, 0x00, 0x00],
        ];
        for (k, c) in steps.iter().enumerate() {
            self.reset_seen = false;
            if self.ok(c, p).is_none() {
                // CORE_RESET whose response never came but whose notification did (reason 1, the host asked): the reset happened. Seen after a restart of this driver (2026-11-08): the notification arrived, the response did not, and every later command was refused as uninitialised.
                if k == 0 && self.reset_seen {
                    say!(self.grants, "nfc: CORE_RESET answered by its notification alone; going on\n");
                    continue;
                }
                return false;
            }
            if k == 2 {
                if let Some(n) = self.next(p, 64) {
                    self.packet(p, n);
                }
            }
        }
        let rf: [&[u8]; 8] = [
            // CORE_CONF_PROP (libnfc-hal-st.conf).
            &[0x20, 0x02, 0x0A, 0x03, 0xA1, 0x01, 0x1E, 0xA2, 0x01, 0x19, 0x80, 0x01, 0x01],
            // The discover map: ISO-DEP and NFC-DEP to their interfaces, as Android maps them.
            &[0x21, 0x00, 0x07, 0x02, 0x04, 0x03, 0x02, 0x05, 0x03, 0x03],
            // NFCC_CONFIG_CONTROL: the controller may manage its RF configuration.
            &[0x20, 0x02, 0x04, 0x01, 0x85, 0x01, 0x01],
            // Power sub-state: screen on, unlocked.
            &[0x20, 0x09, 0x01, 0x00],
            // Listen routing, all to the host: the empty-AID default, the ISO-DEP protocol, technologies A and B.
            &[0x21, 0x01, 0x15, 0x00, 0x04, 0x52, 0x02, 0x00, 0x11, 0x41, 0x03, 0x00, 0x11, 0x04, 0x00, 0x03, 0x00, 0x39, 0x00, 0x00, 0x03, 0x00, 0x39, 0x01],
            // Listen parameters: SDD bit frame 4, platform 0, SEL_INFO ISO-DEP, SENSB_INFO 1, NFC-F protocol 0.
            &[0x20, 0x02, 0x10, 0x05, 0x30, 0x01, 0x04, 0x31, 0x01, 0x00, 0x32, 0x01, 0x20, 0x38, 0x01, 0x01, 0x50, 0x01, 0x00],
            // Field-info notifications on.
            &[0x20, 0x02, 0x04, 0x01, 0x80, 0x01, 0x01],
            // TOTAL_DURATION 500 ms.
            &[0x20, 0x02, 0x05, 0x01, 0x00, 0x02, 0xF4, 0x01],
        ];
        for c in rf {
            if self.ok(c, p).is_none() {
                return false;
            }
        }
        self.discover(p)
    }

    /// One APDU to the tapped device on the static RF connection, as one data packet (every APDU here fits one).
    fn data(&mut self, apdu: &[u8]) {
        let mut r = [0u8; 3 + 32];
        r[2] = apdu.len() as u8;
        r[3..3 + apdu.len()].copy_from_slice(apdu);
        // A packet sent with no credit breaks the controller's flow control (NCI 2.0 4.4.4): said, not hidden, and sent anyway.
        if self.credits == 0 {
            say!(self.grants, "nfc: sending without a credit\n");
        } else {
            self.credits -= 1;
        }
        if let Err(e) = self.send(&r[..3 + apdu.len()]) {
            say!(self.grants, "nfc: send to the tag failed G#{e:X}\n");
        }
    }

    /// The tapped device's answer (reader mode, PhotonNfc's reader): to the SELECT 9000, then GET DATA (80 CA 00 00 00); to that, S (32 bytes) and 9000, kept for the identity service. Anything else ends the exchange. The device is then let go back to discovery (RF_DEACTIVATE, type 3).
    fn tag_answer(&mut self, a: &[u8]) {
        let ok = a.len() >= 2 && a[a.len() - 2..] == [0x90, 0x00];
        match self.rstage {
            1 if ok => {
                self.rstage = 2;
                self.data(&[0x80, 0xCA, 0x00, 0x00, 0x00]);
                return;
            }
            2 if ok && a.len() == 34 => {
                let mut s = [0u8; 32];
                s.copy_from_slice(&a[..32]);
                self.read_secret = Some(s);
                self.tags += 1;
                say!(self.grants, "nfc: a pairing secret read from a tapped device\n");
            }
            1 => say!(self.grants, "nfc: the tapped device is not a Photon joiner ({})\n", Hex(&a[..a.len().min(4)])),
            _ => say!(self.grants, "nfc: the tapped device's GET DATA answer has an unexpected shape ({} bytes)\n", a.len()),
        }
        self.rstage = 0;
        if let Err(e) = self.send(&[0x21, 0x06, 0x01, 0x03]) {
            say!(self.grants, "nfc: release failed G#{e:X}\n");
        }
    }

    /// An unlock tag read: every held op 9 answered with its UID, and none held after. A tag held to the device is read again at each discovery cycle (TOTAL_DURATION, 500 ms), so a caller still waiting hears it again then.
    fn tag_read(&mut self, uid: &[u8]) {
        for t in &self.held[..self.nheld] {
            reply(t, uid);
        }
        if self.nheld > 0 {
            say!(self.grants, "nfc: an unlock tag's UID to A#{} waiting\n", self.nheld);
        }
        self.nheld = 0;
    }

    /// Op 9 held: answered at the next unlock tag (`tag_read`) or by op 10. Full, the oldest gives way (answered with nothing, so its caller asks again if it still wants one).
    fn tag_hold(&mut self, token: [u8; 32]) {
        if self.nheld == HELD {
            reply(&self.held[0], &[]);
            self.held.copy_within(1.., 0);
            self.nheld -= 1;
        }
        self.held[self.nheld] = token;
        self.nheld += 1;
    }

    /// Op 10: every held op 9 let go, answered with nothing.
    fn tag_let_go(&mut self) {
        for t in &self.held[..self.nheld] {
            reply(t, &[]);
        }
        self.nheld = 0;
    }

    /// Op 12 held: NFC kept on with the screen off for as long as it is (`sweep`, op 13). Full, the oldest gives way (answered with nothing, so its caller holds again if it still wants NFC open). The newest holder's end is watched: the kernel sets `CALLER_ENDED` in `RUNG` when it ends, any way, which wakes the loop (`sweep`).
    fn open_hold(&mut self, token: [u8; 32]) {
        if self.nopen == HELD {
            reply(&self.opens[0], &[]);
            self.opens.copy_within(1.., 0);
            self.nopen -= 1;
        }
        let _ = ferros_sys::caller_watch(&token, &RUNG);
        self.opens[self.nopen] = token;
        self.nopen += 1;
    }

    /// Op 13: every held op 12 let go, answered with nothing.
    fn open_let_go(&mut self) {
        for t in &self.opens[..self.nopen] {
            reply(t, &[]);
        }
        self.nopen = 0;
    }

    /// The held calls whose process has ended, dropped: at the kernel's ring of a watched end, and when the screen goes off (the moment an older holder's life decides). The kernel watches one caller a watcher, so an older holder's end is found here, never on a clock.
    fn sweep(&mut self) {
        let mut k = 0;
        while k < self.nopen {
            if ferros_sys::caller_alive(&self.opens[k]) == Some(false) {
                self.opens.copy_within(k + 1.., k);
                self.nopen -= 1;
                say!(self.grants, "nfc: a client that held NFC open ended; A#{} hold it now\n", self.nopen);
            } else {
                k += 1;
            }
        }
    }

    /// Reader mode on or off. Discovery stays as it is (listening and polling, interleaved): a tapped device found while this one polls is asked for Photon's tag. A poll-only discovery (NFC-A alone, `21 03 03 01 00 01`) went unanswered by this controller every time (2026-11-08), so the tap rides the polling discovery already does.
    fn set_reading(&mut self, on: bool) -> bool {
        if self.reading != on {
            self.reading = on;
            self.rstage = 0;
        }
        true
    }

    /// The screen off: discovery stopped (RF_DEACTIVATE to idle) and the controller told (CORE_SET_POWER_SUB_STATE 1, screen off), so neither the field nor the listen runs in the dark; a tap is consent only on a device whose screen is on (AUTH.md), so nothing is lost. On: sub-state 0 (on, unlocked, as the bring-up sets it), and the loop starts discovery again at once. 2026-11-10: NFC drew ~20 mW with the screen off and woke its driver ~115 times a second.
    fn screen(&mut self, dark: bool, p: &mut Packet) {
        if dark {
            self.tag_here = false;
            if self.discovering {
                self.ok(&[0x21, 0x06, 0x01, 0x00], p);
                self.discovering = false;
            }
            self.ok(&[0x20, 0x09, 0x01, 0x01], p);
        } else {
            self.ok(&[0x20, 0x09, 0x01, 0x00], p);
        }
    }

    /// How long to wait before starting discovery again after `discover_failures` failures in a row: a second, doubling to at most 64. WHY the cap: a controller that recovers is found again within a minute, and the doubling has nothing left to save past that. A screen coming on, a TAG held and a PROBE each retry at once, so a person waiting on a tag never waits on this.
    fn retry_wait(&self) -> u64 {
        freq() << self.discover_failures.min(6)
    }

    /// Discovery stopped (RF_DEACTIVATE to idle) and started again, which sets the chip's tag detector afresh: a PROBE's (op 11) way of making sure of the chip.
    /// Between its full polls the chip only senses its antenna, and polls when a tag's load moves it (docs/NFC.md "A tag that did nothing at the attest screen"); RF_DISCOVER alone, the probe until 2026-11-24, is answered G#A0 by running discovery and changes nothing in the chip.
    /// Measured on the Pro (2026-11-24): the detector still woke on the owner's tag after 52 minutes of discovery with the screen on, so nothing restarts discovery on a clock or on a TAG held.
    /// A deactivation refused (G#06, not discovering) is discovery found stopped with no word from the chip, said and counted; no answer is a silent chip, brought up again from a reset pulse.
    fn rediscover(&mut self, why: &str, p: &mut Packet) {
        self.rediscovers += 1;
        let polls = self.polls - self.polls_at_start;
        let ms = (now() - self.started_at) * 1000 / freq();
        self.unanswered = false;
        match self.cmd(&[0x21, 0x06, 0x01, 0x00], p) {
            Some(n) if n >= 4 && p[3] == 0 => {}
            Some(n) => {
                self.found_stopped += 1;
                say!(self.grants, "nfc: {why}: the chip was not discovering ({}), with no word from it (last heard A#{} ms ago)\n", Hex(&p[..n.min(8)]), (now() - self.heard_at) * 1000 / freq());
            }
            None => {}
        }
        self.discovering = false;
        if self.unanswered {
            say!(self.grants, "nfc: {why}: the chip is silent: its reset pulsed and brought up again\n");
            self.needs_init = true;
            return;
        }
        say!(self.grants, "nfc: {why}: discovery started again, the tag detector afresh (A#{polls} full polls in the A#{ms} ms since it last started)\n");
        self.discover(p);
        if self.unanswered {
            self.needs_init = true;
        }
    }

    /// Op 11 (a doorbell, `PROBE` in the module comment): running discovery started again (`rediscover`). Discovery stopped (the dark, a retry pending): nothing here, the caller retries at once.
    /// Until 2026-11-24 it sent RF_DISCOVER alone, which running discovery answers G#A0 (already started) and changes nothing in the chip.
    fn probe(&mut self, p: &mut Packet) {
        self.probes += 1;
        if self.discovering {
            self.rediscover("a probe", p);
        }
    }

    fn state(&self, out: &mut Line) {
        let _ = write!(out, "firmware {}, RF config {}; {}{}; S {}; activations {}, S served {}, fields {}, tags {}; TAG held A#{}, held open A#{}; discovery failures A#{}, reset pulses A#{}, probes A#{} (A#{} found discovery stopped), discovery started again on a doorbell A#{}; full polls A#{} (A#{} in the A#{} ms since discovery started, a tag read since {}); ST notifications A#{} (last {}); chip last heard A#{} ms ago; bus resets after a failed transfer A#{}", Hex(&self.version), Hex(&self.config), if self.discovering { "listening (A, B), polling (A, B, V)" } else { "not listening" }, if self.needs_init { ", to be brought up again" } else { "" }, if self.secret.is_some() { "live" } else { "none" }, self.activations, self.served, self.fields, self.tags, self.nheld, self.nopen, self.discover_failures, self.pulses, self.probes, self.found_stopped, self.rediscovers, self.polls, self.polls - self.polls_at_start, (now() - self.started_at) * 1000 / freq(), if self.read_since_start { "yes" } else { "no" }, self.st_notes, Hex(&self.st_last), (now() - self.heard_at) * 1000 / freq(), RESETS.load(core::sync::atomic::Ordering::Relaxed));
    }

    /// One of the motor's registers: its address written without STOP, then the value read.
    fn motor_read(&self, reg: u32) -> Result<u32, u32> {
        let mut b = [0u8; 4];
        self.motor_retry(|| {
            self.xfer(MOTOR, &reg.to_be_bytes(), &mut [], true)?;
            self.xfer(MOTOR, &[], &mut b, false)
        })?;
        Ok(u32::from_be_bytes(b))
    }
    fn motor_write(&self, reg: u32, v: u32) -> Result<(), u32> {
        let mut w = [0u8; 8];
        w[..4].copy_from_slice(&reg.to_be_bytes());
        w[4..].copy_from_slice(&v.to_be_bytes());
        self.motor_retry(|| self.xfer(MOTOR, &w, &mut [], false))
    }
    /// The ROM firmware hibernates when idle, and a hibernating CS40L26 may NACK the transaction that wakes it (the first op 7 on the Pro, 2026-11-10, read nothing): a NACK is tried again every millisecond for 16, as the NFC chip's writes are, and a retry that was needed is said.
    fn motor_retry(&self, mut f: impl FnMut() -> Result<(), u32>) -> Result<(), u32> {
        let mut r = f();
        let mut tries = 1;
        while r == Err(E_NODEV) && tries < 16 {
            sleep_ms(1);
            r = f();
            tries += 1;
        }
        if tries > 1 {
            say!(self.grants, "nfc: the motor NACKed A#{} times before {}\n", tries - 1, if r.is_ok() { "answering" } else { "giving up" });
        }
        r
    }

    /// The motor's IDs and state on the console (what the kernel's `diag haptic` printed): DEVID, REVID, IRQ1 status and EINT1, the mailbox, the ROM's power state. DEVID and REVID returned (0 for a read that failed).
    fn motor_report(&self) -> (u32, u32) {
        let mut l = Line { b: [0; 512], n: 0 };
        let h = |l: &mut Line, r: Result<u32, u32>| {
            let _ = match r {
                Ok(v) => write!(l, "G#{v:X}"),
                Err(e) => write!(l, "error G#{e:X}"),
            };
        };
        let (id, rev) = (self.motor_read(M_DEVID), self.motor_read(M_REVID));
        let pm = if rev == Ok(0xB2) { M_PM_STATE_B2 } else { M_PM_STATE_A1 };
        let _ = write!(l, "nfc: motor CS40L26: DEVID ");
        h(&mut l, id);
        let _ = write!(l, " (G#40A260 A, G#40A26B B) REVID ");
        h(&mut l, rev);
        let _ = write!(l, "; IRQ1 status ");
        h(&mut l, self.motor_read(M_IRQ1_STATUS));
        let _ = write!(l, " EINT1 ");
        h(&mut l, self.motor_read(M_IRQ1_EINT_1));
        let _ = write!(l, "; mailbox ");
        h(&mut l, self.motor_read(M_MBOX));
        let _ = write!(l, "; ROM power state ");
        h(&mut l, self.motor_read(pm));
        let _ = writeln!(l, " (0 hibernate, 1 shutdown, 2 standby, 3 active)");
        console(self.grants, &l.b[..l.n]);
        (id.unwrap_or(0), rev.unwrap_or(0))
    }

    /// ROM waveform `n` played (the kernel's `diag haptic play`): the ROM firmware kept awake, 2 ms, the waveform's index queued, 20 ms, the state before and after on the console.
    fn motor_play(&self, n: u8) -> bool {
        self.motor_report();
        let a = self.motor_write(M_MBOX, M_PREVENT_HIBER);
        sleep_ms(2);
        let b = self.motor_write(M_MBOX, M_ROM_INDEX + n as u32);
        say!(self.grants, "nfc: motor: prevent hibernate {a:?}, ROM waveform {n} {b:?}\n");
        sleep_ms(20);
        self.motor_report();
        a.is_ok() && b.is_ok()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let mut nfc = Nfc { grants, secret: None, credits: 0, apdu: [0; 512], apdu_len: 0, activations: 0, served: 0, fields: 0, tags: 0, version: [0; 4], config: [0; 2], discovering: false, field: false, reading: false, rstage: 0, read_secret: None, discover_failures: 0, quiet: false, reset_seen: false, in_init: false, needs_init: false, unanswered: false, pulses: 0, probes: 0, found_stopped: 0, st_notes: 0, st_last: [0; 8], st_said: 0, heard_at: now(), polls: 0, polls_at_start: 0, started_at: now(), read_since_start: false, tag_here: false, poll_saw_tag: false, rediscovers: 0, held: [[0; 32]; HELD], nheld: 0, opens: [[0; 32]; HELD], nopen: 0 };
    let mut p: Packet = [0; 258];
    if !ferros_sys::power(cap(grants, S_POWER), true) {
        say!(grants, "nfc: power on failed (HSI1 off?)\n");
        ferros_sys::exit(1);
    }
    // A bring-up that failed is tried again whole, from a reset pulse (`restart`), where discovery would be: a controller that never finished its initialisation refuses discovery (status G#06) forever, and one that stopped answering answers no CORE_RESET either.
    let up = nfc.init(&mut p);
    let mut l = Line { b: [0; 512], n: 0 };
    nfc.state(&mut l);
    say!(grants, "nfc: {}: {}\n", if up { "up" } else { "bring-up failed" }, core::str::from_utf8(&l.b[..l.n]).unwrap_or(""));
    let mut msg = [0u8; 64];
    let mut token = [0u8; 32];
    let mut retry_at = 0u64;
    // The corners' NFC circle (ferros_sys::measure), at each change: up; listening, with the carrier it listens on (NFC's 13.56 MHz); a reader's field on us (connected, while it lasts).
    let mut shown = None;
    // The screen's state, read every pass (a memory read; its change wakes the loop) (`Nfc::screen`).
    let mut dark = false;
    // The chip's interrupt line (`irq_up`), and the loop's wakes said every 64 s or more: passes, and waits.
    let irq = irq_up(grants);
    say!(grants, "nfc: {}\n", if irq { "waiting on the chip's interrupt line, no timer" } else { "no interrupt line granted (a kernel before 2026-11-13): reading its pin every millisecond" });
    let (mut passes, mut waits, mut said_at) = (0u32, 0u32, now());
    loop {
        // The process watchdog's pet, once a pass (ferros_sys::pet; measuring, `diag pets`).
        ferros_sys::pet();
        // The interrupt's count before the chip is read: an interrupt while this pass runs ends its wait at once.
        let seen = RUNG.load(core::sync::atomic::Ordering::Acquire);
        passes += 1;
        if now().wrapping_sub(said_at) >= freq() << 6 {
            say!(grants, "nfc: A#{passes} passes, A#{waits} waits in the last A#{} ms; ST notifications A#{} so far, full polls A#{} (A#{} since discovery started)\n", (now() - said_at) * 1000 / freq(), nfc.st_notes, nfc.polls, nfc.polls - nfc.polls_at_start);
            (passes, waits, said_at) = (0, 0, now());
            nfc.st_said = 0;
        }
        // A watched holder of op 12 ended (`open_hold`): the bit cleared, the holders swept.
        if seen & ferros_sys::CALLER_ENDED != 0 {
            RUNG.fetch_and(!ferros_sys::CALLER_ENDED, core::sync::atomic::Ordering::AcqRel);
            nfc.sweep();
            continue;
        }
        {
            let screen_off = ferros_sys::pref(ferros_sys::PREF_SCREEN_OFF) == 1;
            // An older holder's end is found as the screen goes off, before it keeps the radio on.
            if screen_off && !dark && nfc.nopen > 0 {
                nfc.sweep();
            }
            // The screen off turns NFC off unless a client holds it open (op 12; owner, 2026-11-24).
            let off = screen_off && nfc.nopen == 0;
            if off != dark {
                dark = off;
                retry_at = 0;
                // A chip to be brought up again is told the screen's state by nothing but its bring-up, which waits for the screen on (below).
                if !nfc.needs_init {
                    nfc.unanswered = false;
                    nfc.screen(dark, &mut p);
                    if nfc.unanswered {
                        nfc.needs_init = true;
                        nfc.discovering = false;
                    }
                }
                say!(grants, "nfc: {}: {}\n", if dark { "screen off" } else if screen_off { "screen off, held open by a client" } else { "screen on" }, if dark { "discovery stopped, the field off, the chip in standby" } else if nfc.needs_init { "the chip is brought up again from a reset pulse" } else { "discovery again" });
            }
        }
        // The console's NFC circle (owner, 2026-11-24): STATE 0 not listening, 1 listening (the chip's tag detector; its full polls are not shown apart), 2 a tag in the field or a reader's field on us; HOLDS, the clients holding NFC open with the screen off. Written only when one changes: the kernel rings the link's bell then, and the display draws the circle again.
        let showing = (nfc.discovering, nfc.field || nfc.tag_here, nfc.nopen);
        if shown != Some(showing) {
            shown = Some(showing);
            use ferros_sys::{measure as k, measure_word as w, status, MEASURE_NONE};
            status(0, w(k::STATE, if !nfc.discovering { 0 } else if showing.1 { 2 } else { 1 }));
            status(1, if nfc.discovering { w(k::FREQ, 13_560) } else { MEASURE_NONE });
            status(2, if nfc.nopen > 0 { w(k::HOLDS, nfc.nopen as i64) } else { MEASURE_NONE });
        }
        if nfc.irq() {
            match nfc.read(&mut p) {
                Ok(Some(n)) => {
                    nfc.heard_at = now();
                    nfc.packet(&p, n);
                }
                Ok(None) => {}
                Err(e) => say!(grants, "nfc: read failed G#{e:X}\n"),
            }
            continue;
        }
        // Discovery stopped (a deactivation to idle, a bring-up that failed, a chip that stopped answering or reset by itself): started again, a second later, then twice as long after each failure (`retry_wait`), at once on a doorbell (the screen on, a TAG held, a PROBE). A chip to be brought up again gets its reset pulsed first (`restart`); one that leaves discovery's start unanswered is brought up so next.
        if !nfc.discovering && !dark && now() > retry_at {
            if nfc.needs_init {
                let tries = nfc.discover_failures;
                nfc.quiet = tries > 0;
                let up = nfc.restart(&mut p);
                nfc.quiet = false;
                if up {
                    say!(grants, "nfc: up again from a reset pulse after A#{} failed tries (pulses A#{})\n", tries, nfc.pulses);
                    nfc.discover_failures = 0;
                } else {
                    nfc.discover_failures = tries + 1;
                }
            } else {
                nfc.unanswered = false;
                nfc.discover(&mut p);
                if nfc.unanswered {
                    nfc.needs_init = true;
                }
            }
            retry_at = now() + nfc.retry_wait();
        }
        let n = recv(cap(grants, S_NFC), &mut msg, &mut token, true);
        if n != NOTHING && n != ferros_sys::ERR && n > 0 {
            match msg[0] {
                0 => {
                    let mut l = Line { b: [0; 512], n: 0 };
                    nfc.state(&mut l);
                    reply(&token, &l.b[..l.n]);
                }
                // While nobody has attested no secret is served to a reader (AUTH.md "Radios while locked"; the tap is fgtw's, which only a person starts).
                1 if n == 33 && ferros_sys::facts().0[83] != 2 => {
                    reply(&token, &[1]);
                }
                1 if n == 33 => {
                    let mut s = [0u8; 32];
                    s.copy_from_slice(&msg[1..33]);
                    nfc.secret = Some(s);
                    reply(&token, &[0]);
                }
                2 => {
                    nfc.secret = None;
                    reply(&token, &[0]);
                }
                3 => {
                    let mut r = [0u8; 12];
                    r[..4].copy_from_slice(&nfc.activations.to_le_bytes());
                    r[4..8].copy_from_slice(&nfc.served.to_le_bytes());
                    r[8..].copy_from_slice(&nfc.fields.to_le_bytes());
                    reply(&token, &r);
                }
                // The sponsor's side of the tap (the identity service's add flow): reader mode on (gated like the tag: nothing before a person has attested), off, and the secret read off a tapped device, once.
                4 if ferros_sys::facts().0[83] != 2 => {
                    reply(&token, &[1]);
                }
                4 | 5 => {
                    let on = msg[0] == 4;
                    let was = nfc.reading;
                    let ok = nfc.set_reading(on);
                    if was == on {
                        reply(&token, &[0]);
                        continue;
                    }
                    say!(grants, "nfc: reader mode {}\n", if !ok { "refused" } else if on { "on" } else { "off" });
                    reply(&token, &[!ok as u8]);
                }
                6 => {
                    match nfc.read_secret.take() {
                        Some(s) => reply(&token, &s),
                        None => reply(&token, &[]),
                    };
                }
                // The vibration motor, this bus's other device (KERNEL-OUT.md decision 8: the bus's owner serves its neighbour): its state, and a ROM waveform played.
                7 => {
                    let (id, rev) = nfc.motor_report();
                    let mut r = [0u8; 8];
                    r[..4].copy_from_slice(&id.to_le_bytes());
                    r[4..].copy_from_slice(&rev.to_le_bytes());
                    reply(&token, &r);
                }
                // WHY the refusal: the ROM holds waveforms 0 to G#26 (the driver's limit); a larger index is a caller's mistake, said rather than played as something else.
                8 if n == 2 && msg[1] <= M_ROM_LAST => {
                    let ok = nfc.motor_play(msg[1]);
                    reply(&token, &[!ok as u8]);
                }
                8 => {
                    reply(&token, &[1]);
                }
                // Attest with a tag (AUTH.md): the next unlock tag's UID, held until discovery reads one (no polling by the caller), or let go by op 10. Not gated on a person present: this is the owner's exception to "no NFC action while locked" (2026-11-11), and while locked only the attest screen holds this endpoint (fgtw, its other holder, starts once a person is present), which hands the UID to the kernel and nowhere else.
                // A caller that starts waiting for a tag is a doorbell too: discovery stopped (a failure waiting out `retry_wait`) is tried again at once.
                9 => {
                    nfc.tag_hold(token);
                    if !nfc.discovering {
                        retry_at = 0;
                    }
                }
                10 => {
                    nfc.tag_let_go();
                    reply(&token, &[0]);
                }
                // NFC held open with the screen off (owner, 2026-11-24): the call held until op 13 or the caller's end; the next pass decides the radio.
                12 => {
                    nfc.open_hold(token);
                }
                13 => {
                    nfc.open_let_go();
                    reply(&token, &[0]);
                }
                // The attest screen's doorbell at a touch (`Nfc::probe`): answered first, so the caller never waits on the chip.
                11 => {
                    reply(&token, &[0]);
                    nfc.probe(&mut p);
                    if !nfc.discovering {
                        retry_at = 0;
                    }
                }
                _ => {
                    reply(&token, &[]);
                }
            }
            continue;
        }
        if irq {
            use core::sync::atomic::Ordering::{Acquire, Release};
            // The line its last interrupt left masked re-armed (its pending bit cleared, the line acked, its thread woken), then the pass again before any wait: the chip read once more, so a message from before the clear is taken, one after raises the line again.
            if ARMED.load(Acquire) == 0 {
                mmio_write(cap(grants, S_ALIVE), GPA7_EPEND, 1 << IRQ_PIN);
                ferros_sys::irq_ack(cap(grants, S_IRQ));
                ARMED.store(1, Release);
                ferros_sys::wake(&ARMED, 1);
                continue;
            }
            // Then a wait on what brings work: the chip's interrupt, a call, the screen changing, and discovery's retry when it is stopped with the screen on (`retry_at`); no other deadline.
            waits += 1;
            let until = if !nfc.discovering && !dark { retry_at } else { 0 };
            ferros_sys::wait_world_or_call(&RUNG, seen, until);
            continue;
        }
        // Without the line: a pass a millisecond while discovery runs; while the screen is off nothing comes from the air, and a request (the state, a secret) waits at most this long.
        sleep_ms(if dark { 1 << 4 } else { 1 });
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_report(env!("CARGO_BIN_NAME"), info)
}

// The entry and its image header (ferros_sys::entry!: the kernel sizes the arc from the image and its .bss).
ferros_sys::entry!(payload_main);
