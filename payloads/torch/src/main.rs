//! The torch (docs/TORCH.md; owner, 2026-11-11: "would be nice to have the torch", then "can we vary the brightness of it at all?"): the rear flash LED lit in torch mode, at a level the quick settings' ring turns. The LED's driver is a TI LM3644 at G#63 on hsi2c_14 (husky and akita alike, from their camera device trees); its torch mode only, never flash or strobe.
//!
//! The kernel keeps what a process may not do behind the `torch` power switch (kernel torch.rs): the bus's pins, clock source and controller, and the chip's hardware enable (HWEN, gpp8-2). Switched off, HWEN is low and the chip is in shutdown, every register back at its default: dark whatever was written here. Here, thru the `hsi2c14` grant: the chip's registers, only while the switch is on (the camera PMIC shares the bus, and the kernel's camera tools refuse while the switch is on).
//!
//! Spawn (by hand; the kernel also starts it itself when a call on `torch` finds no server, process.rs ON_DEMAND):
//! ferros-bridge spawn torch.bin --name torch --serve torch --hw hsi2c14 --power torch
//! Grant slots, in that order: 0 console, 1 the `torch` endpoint (serve), 2 hsi2c_14, 3 the power switch.
//!
//! The `torch` endpoint, first byte the op:
//! - `0` SET + level (u8): 0 off, 1-255 dimmest to brightest (`LEVELS`). → [the level now]: the one asked, or 0 when it could not be lit (the console says why).
//! - `1` LEVEL. → [the level now].
//! - `2` REPORT (development): → [level, torch code, device ID, enable register, flags 1, flags 2], the chip's four read now (reading the flags clears them); zeros for those while dark (the chip is in shutdown and does not answer).
//!
//! Dark and nothing waiting, it ends (one shot dead, no cleanup: the switch is off already): a torch that is off holds no arc. The kernel starts another at the next call.
#![no_std]
#![no_main]

use core::fmt::Write;
use ferros_sys::{cap, console, mmio_read, mmio_write, recv, reply, ERR, NOTHING};

const S_TORCH: u64 = 1;
const S_BUS: u64 = 2;
const S_POWER: u64 = 3;

/// The LM3644 (zuma-husky-camera.dtsi `flash-lm3644`: hsi2c_14, G#63, 8-bit registers and values) and its registers (TI's LM3644 datasheet, register map).
const TARGET: u8 = 0x63;
/// Enable: bit 7 TX pin enable (1 out of reset), 6 strobe type, 5 strobe enable, 4 TORCH/TEMP pin enable, 3-2 mode (00 standby, 01 IR, 10 torch, 11 flash), 1 LED2 enable, 0 LED1 enable.
const ENABLE: u8 = 0x01;
/// LED1 torch brightness: bit 7 LED2's torch current set to LED1's, 6-0 the code.
const LED1_TORCH: u8 = 0x05;
const FLAGS1: u8 = 0x0A;
const FLAGS2: u8 = 0x0B;
const DEVICE_ID: u8 = 0x0C;
/// Standby, the reset value: TX pin enable kept, everything else off.
const STANDBY: u8 = 0x80;
/// Torch mode on LED1 alone, the TX pin enable kept as reset leaves it; strobe, the TORCH/TEMP pin and LED2 off, so nothing but this register lights it and it can only be torch (never mode 11, flash).
const TORCH_LED1: u8 = 0x80 | 0b10 << 2 | 1;

/// The brightest torch code allowed: the chip's torch current is (code × 1.4 mA) + 0.977 mA on the LM3644 (G#7F = 179 mA) and twice that on the LM3644TT (G#7F = 357.6 mA), and which part this is cannot be told from the source. So the cap is the worst case held to Android's own torch: the husky and akita power profiles (`camera.flashlight`, power_profile.xml) put the torch at 240.47 mA from the battery, and on the TT LED1 alone at code 85 draws 1.954 + 85 × 2.8 = 239.95 mA. On the plain part code 85 is 119.98 mA, half of that. Derived, not picked: a measured Android torch (the hv oracle on hsi2c_14, or the gauge's current) is what would move it.
const ANDROID_TORCH_UA: u32 = 240_470;
const TT_BASE_UA: u32 = 1_954;
const TT_STEP_UA: u32 = 2_800;
const TOP: u8 = ((ANDROID_TORCH_UA - TT_BASE_UA) / TT_STEP_UA) as u8;

/// Level 1..255 to torch code, so equal turns of the ring look like equal steps (owner, 2026-11-11): light goes with current and the eye with its logarithm, so the current is geometric in the level, I(L) = I(0) × (I(TOP) / I(0))^((L - 1) / 254), each rounded to the chip's step. The current is linear in the code with an offset, I = step × (code + c0), c0 = 0.977 / 1.4 (the same on both parts), so this is u(L) = c0 × r^(L - 1) in steps, r the 254th root of (TOP + c0) / c0, and the code is u - c0 rounded. Computed at build time: no float at run time. The chip's lowest steps are big ratios (code 0 to 1 is 2.4×, 1 to 2 is 1.6×), so the curve holds code 0 for levels 1-29, 1 for 30-61, 2 for 62-81; 79 distinct currents in all, 7 codes skipped near the top where one level is more than one step (docs/TORCH.md).
const LEVELS: [u8; 255] = levels();
const fn levels() -> [u8; 255] {
    let c0 = 977. / 1400.;
    let ratio = (TOP as f64 + c0) / c0;
    // r by bisection on [1, 2]: r^254 against the ratio, 64 halvings (past f64's precision).
    let (mut lo, mut hi) = (1.0f64, 2.0f64);
    let mut i = 0;
    while i < 64 {
        let mid = (lo + hi) / 2.;
        let mut p = 1.0f64;
        let mut k = 0;
        while k < 254 {
            p *= mid;
            k += 1;
        }
        if p < ratio {
            lo = mid;
        } else {
            hi = mid;
        }
        i += 1;
    }
    let mut t = [0u8; 255];
    let mut u = c0;
    let mut l = 0;
    while l < 255 {
        t[l] = (u - c0 + 0.5) as u8;
        u *= lo;
        l += 1;
    }
    t
}

// The HS-I2C controller (kernel hsi2c.rs, i2c-exynos5.c auto mode), as payloads/nfc drives hsi2c_0.
const CTL: u64 = 0x00;
const FIFO_CTL: u64 = 0x04;
const INT_ENABLE: u64 = 0x20;
const INT_STATUS: u64 = 0x24;
const FIFO_STATUS: u64 = 0x30;
const TX_DATA: u64 = 0x34;
const RX_DATA: u64 = 0x38;
const AUTO_CONF: u64 = 0x44;
const TRANS_STATUS: u64 = 0x50;
const ADDR: u64 = 0x70;
const CTL_MASTER: u32 = 1 << 3;
const CTL_RXCHON: u32 = 1 << 6;
const CTL_TXCHON: u32 = 1 << 7;
const INT_TRANSFER_DONE: u32 = 1 << 7;
const INT_ERRORS: u32 = 0xF << 8;
const INT_NODEV: u32 = 1 << 10;
const FIFO_TX_EMPTY: u32 = 1 << 8;
const FIFO_RX_EMPTY: u32 = 1 << 24;
const AUTO_READ: u32 = 1 << 16;
const AUTO_STOP_AFTER_TRANS: u32 = 1 << 17;
const AUTO_MASTER_RUN: u32 = 1 << 31;
const TRANS_MASTER_BUSY: u32 = 1 << 17;
const ADDR_MASTER_ID_7: u32 = 7 << 24;
/// Polls per wait, a syscall each: thousands of microseconds, far past a byte at 192 kHz.
const WAIT_POLLS: u32 = 1 << 12;
/// The receive FIFO's depth.
const FIFO_DEPTH: u32 = 1 << 6;

const E_BUSY: u32 = 2;
const E_NODEV: u32 = 4;
const E_BUS: u32 = 8;
const E_TIMEOUT: u32 = 0x10;

struct Line {
    b: [u8; 256],
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
        let mut l = Line { b: [0; 256], n: 0 };
        let _ = write!(l, $($t)*);
        console($g, &l.b[..l.n]);
    }};
}

struct Torch {
    grants: u64,
    /// The level now (0 dark).
    level: u8,
    /// The switch on: HWEN high, the bus ours.
    lit: bool,
    /// The device ID read at the first light, said once.
    said_id: bool,
}

impl Torch {
    fn rd(&self, off: u64) -> u32 {
        mmio_read(cap(self.grants, S_BUS), off) as u32
    }
    fn wr(&self, off: u64, v: u32) {
        mmio_write(cap(self.grants, S_BUS), off, v);
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

    /// Whatever the receive FIFO holds, read and dropped (bytes left there would answer the next read: payloads/nfc, 2026-11-10). WHY the bound: the FIFO is 64 deep, so a status still saying "not empty" after 64 reads is the controller misreporting, and this stops rather than spins.
    fn drain_rx(&self) {
        for _ in 0..FIFO_DEPTH {
            if self.rd(FIFO_STATUS) & FIFO_RX_EMPTY != 0 {
                return;
            }
            self.rd(RX_DATA);
        }
    }

    /// One auto-mode transfer to the chip: `tx` written, or `rx` filled, with STOP after it unless `hold` keeps the bus for the read that follows.
    fn xfer(&self, tx: &[u8], rx: &mut [u8], hold: bool) -> Result<(), u32> {
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
        self.drain_rx();
        let read = !rx.is_empty();
        let len = if read { rx.len() } else { tx.len() } as u32;
        self.wr(FIFO_CTL, if read { 0x0002_0023 } else { 0x0001_0013 });
        self.wr(CTL, CTL_MASTER | if read { CTL_RXCHON } else { CTL_TXCHON });
        self.wr(INT_STATUS, self.rd(INT_STATUS));
        self.wr(ADDR, ADDR_MASTER_ID_7 | (TARGET as u32) << 10);
        self.wr(INT_ENABLE, INT_TRANSFER_DONE | INT_ERRORS);
        let auto = len | if read { AUTO_READ } else { 0 } | if hold { 0 } else { AUTO_STOP_AFTER_TRANS };
        self.wr(AUTO_CONF, auto);
        self.wr(AUTO_CONF, auto | AUTO_MASTER_RUN);
        if read {
            self.wait_done()?;
            for b in rx.iter_mut() {
                if self.rd(FIFO_STATUS) & FIFO_RX_EMPTY != 0 {
                    return Err(E_TIMEOUT);
                }
                *b = self.rd(RX_DATA) as u8;
            }
        } else {
            // Two bytes at most here: the 64-deep FIFO takes them without waiting.
            for &b in tx {
                self.wr(TX_DATA, b as u32);
            }
            self.wait_done()?;
            for _ in 0..WAIT_POLLS {
                if self.rd(FIFO_STATUS) & FIFO_TX_EMPTY != 0 {
                    break;
                }
            }
        }
        Ok(())
    }

    fn write(&self, reg: u8, v: u8) -> Result<(), u32> {
        self.xfer(&[reg, v], &mut [], false)
    }

    fn read(&self, reg: u8) -> Result<u8, u32> {
        self.xfer(&[reg], &mut [], true)?;
        let mut v = [0u8; 1];
        self.xfer(&[], &mut v, false)?;
        Ok(v[0])
    }

    /// Dark: standby asked (a courtesy; it may fail with the chip already gone), then the switch off, HWEN low, which is what makes it dark.
    fn dark(&mut self) {
        if self.lit {
            let _ = self.write(ENABLE, STANDBY);
            if !ferros_sys::power(cap(self.grants, S_POWER), false) {
                say!(self.grants, "torch: the switch refused off\n");
            }
            self.lit = false;
        }
        self.level = 0;
    }

    /// The torch to `level`: its current set before torch mode is enabled, so it never lights at the reset code first; a level change while lit only moves the current. Any failure leaves it dark.
    fn set(&mut self, level: u8) {
        if level == self.level {
            return;
        }
        if level == 0 {
            return self.dark();
        }
        if !self.lit {
            if !ferros_sys::power(cap(self.grants, S_POWER), true) {
                say!(self.grants, "torch: the switch refused on (hsi2c_14 not ready)\n");
                return;
            }
            self.lit = true;
            match self.read(DEVICE_ID) {
                Ok(id) if !self.said_id => {
                    self.said_id = true;
                    say!(self.grants, "torch: LM3644 at G#63 answers, device ID G#{id:02X}\n");
                }
                Ok(_) => {}
                Err(e) => {
                    say!(self.grants, "torch: no answer from G#63 (G#{e:X})\n");
                    return self.dark();
                }
            }
        }
        let code = LEVELS[level as usize - 1];
        let r = self.write(LED1_TORCH, code).and_then(|_| if self.level == 0 { self.write(ENABLE, TORCH_LED1) } else { Ok(()) });
        if let Err(e) = r {
            say!(self.grants, "torch: a write failed (G#{e:X}); dark\n");
            return self.dark();
        }
        self.level = level;
        // A fault the chip latched (short, thermal shutdown, undervoltage, overvoltage) shows here; reading clears them.
        let (f1, f2) = (self.read(FLAGS1).unwrap_or(0), self.read(FLAGS2).unwrap_or(0));
        if f1 | f2 != 0 {
            say!(self.grants, "torch: level A#{level} (code A#{code}): flags G#{f1:02X} G#{f2:02X}\n");
        }
    }

    fn report(&self) -> [u8; 6] {
        let code = if self.level == 0 { 0 } else { LEVELS[self.level as usize - 1] };
        if !self.lit {
            return [self.level, code, 0, 0, 0, 0];
        }
        let r = |reg| self.read(reg).unwrap_or(0);
        [self.level, code, r(DEVICE_ID), r(ENABLE), r(FLAGS1), r(FLAGS2)]
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let mut t = Torch { grants, level: 0, lit: false, said_id: false };
    let mut msg = [0u8; 64];
    let mut token = [0u8; 32];
    let mut poll = false;
    loop {
        let n = recv(cap(grants, S_TORCH), &mut msg, &mut token, poll);
        // Dark and nothing waiting: end. A call that comes after this look is left queued, and the kernel starts another torch for it (process.rs `fail_orphaned_calls`).
        if n == NOTHING {
            ferros_sys::exit(0);
        }
        if n == ERR || n == 0 {
            continue;
        }
        match msg[0] {
            0 if n == 2 => {
                t.set(msg[1]);
                reply(&token, &[t.level]);
            }
            1 => {
                reply(&token, &[t.level]);
            }
            2 => {
                reply(&token, &t.report());
            }
            _ => {
                reply(&token, &[]);
            }
        }
        poll = t.level == 0;
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}

// The entry and its image header (ferros_sys::entry!: the kernel sizes the arc from the image and its .bss).
ferros_sys::entry!(payload_main);
