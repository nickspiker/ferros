//! The two speaker amplifiers (Cirrus CS35L41 on spi_19; docs/AUDIO.md, payloads/amp for their reads alone): brought from reset to playing the AoC's TDM0 in bypass, no DSP firmware, and back to off.
//!
//! Every step from Google's driver (`private/google-modules/amplifiers/cs35l41` in /mnt/Harbor/husky-kernel), written `c:N` for cs35l41.c, `t:N` for cs35l41-tables.c, `h:N` for cs35l41.h, and the husky device tree (`zuma-shusky-audio.dtsi`, /mnt/Harbor/ferros-ref/kdev-shusky/dts), written `dts:N`. The AoC's side of the link (TDM0: DSP_A, the codecs clock consumers, 4 slots of 32 bits at 48 kHz, 16-bit samples) from /mnt/Harbor/ferros-ref: gs101-audio.dtsi:678-692 and aoc_alsa_card.c:245-246, :553-632.
//!
//! What Linux does that this does not: it loads the amplifiers' own DSP firmware (`cs35l41-dsp1-spk-prot.wmfw` and its tuning, dmesg:7357-7395), the speaker protection, and runs the DAC from the DSP. Here the DAC takes the serial port directly (DAC_PCM1_SRC, the bypass the chip resets to: t:74), so nothing limits the speaker's excursion or temperature but the level: the service's volume, its default two octaves down, and the owner's ears at the first test (AUDIO.md "Verification").

use crate::{say, S_GPN, S_POWER, S_SPI};
use ferros_spi::{Cs, Regs, Spi};
use ferros_sys::{cap, freq, mmio_read, mmio_write, now, sleep_until};

// Registers (h:24-134, h:318-330).
const DEVID: u32 = 0x0;
const REVID: u32 = 0x4;
const OTPID: u32 = 0x10;
const TEST_KEY_CTL: u32 = 0x40;
const OTP_MEM0: u32 = 0x400;
const PWR_CTRL1: u32 = 0x2014;
const PWR_CTRL2: u32 = 0x2018;
const PLL_CLK_CTRL: u32 = 0x2C04;
const GLOBAL_CLK_CTRL: u32 = 0x2C0C;
const TST_FS_MON0: u32 = 0x2D10;
const BSTCVRT_PEAK_CUR: u32 = 0x3808;
const BSTCVRT_COEFF: u32 = 0x3810;
const BSTCVRT_SLOPE_LBST: u32 = 0x3814;
const SP_ENABLES: u32 = 0x4800;
const SP_FORMAT: u32 = 0x4808;
const SP_RX_WL: u32 = 0x4840;
const DAC_PCM1_SRC: u32 = 0x4C00;
const IRQ1_STATUS1: u32 = 0x1_0010;
const IRQ1_STATUS3: u32 = 0x1_0018;
const IRQ1_STATUS4: u32 = 0x1_001C;
const DSP1_CCM_CORE_CTRL: u32 = 0x02BC_1000;

// Bits (h:667-698).
const GLOBAL_EN: u32 = 1;
const AMP_EN: u32 = 1;
const PDN_DONE: u32 = 1 << 23;
const PUP_DONE: u32 = 1 << 24;
const PLL_LOCK: u32 = 1 << 1;
const OTP_BOOT_DONE: u32 = 1 << 1;
/// IRQ1_STATUS1's error bits (h:684-689): the amplifier or the boost shorted, over temperature, the boost's over and under voltage.
const ERRORS: u32 = 1 << 31 | 1 << 8 | 1 << 17 | 1 << 6 | 1 << 7;
/// ASP RX1 and RX2 enables (c:2451-2456: SP_ENABLES bits 16 and 17).
const ASP_RX1_EN: u32 = 1 << 16;
const ASP_RX2_EN: u32 = 1 << 17;
/// DAC_PCM1_SRC's sources (t:74-82: ASPRX1 is 8, ASPRX2 9).
const SRC_ASPRX1: u32 = 0x08;
const SRC_ASPRX2: u32 = 0x09;

/// gpn1's data word in gpio-gpn0 (bank +G#20): pin 0 is the right amplifier's select, active low (dts:144-153, dts:164-165). gpp1's in gpio-gpp0: pin 1 the left's reset (dts:113-120, dts:181). As payloads/amp.
const GPN1_DAT: u64 = 0x24;
const RIGHT_CS: u32 = 0;

/// The B2 errata patch, written once out of reset (c:3675-3686, chosen for REVID G#B2 at c:4154-4483's probe; both husky amplifiers are B2, AUDIO.md).
const ERRATA_B2: [(u32, u32); 10] = [
    (0x40, 0x5555),
    (0x40, 0xAAAA),
    (0x4100, 0),
    (0x4310, 0),
    (0x4400, 0),
    (0x381C, 0x51),
    (0x02BC_20E0, 0),
    (0x02BC_2020, 0),
    (0x40, 0xCCCC),
    (0x40, 0x3333),
];
/// Around GLOBAL_EN on and off (c:2168-2182).
const PUP_PATCH: [(u32, u32); 5] = [(0x40, 0x55), (0x40, 0xAA), (0x2084, 0x002F_1AA0), (0x40, 0xCC), (0x40, 0x33)];
const PDN_PATCH: [(u32, u32); 5] = [(0x40, 0x55), (0x40, 0xAA), (0x2084, 0x002F_1AA3), (0x40, 0xCC), (0x40, 0x33)];

/// The OTP's trims (t:725-827 `otp_map_1`, t:829-931 `otp_map_2`): register, shift, size, in the order the packed bits come. The two maps differ in one element only (`OTP_DIFFERS`); OTPID picks the map (t:933-970: 1 and 8 the first, 2, 3 and 6 the second), the bits start at word 2, bit 16 (every map's offsets). The C arrays are declared 100 long (h:546) with 99 written: the hundredth is all zeros, a field of no bits and no register, which unpacks nothing and is left out here.
const OTP_MAP: [(u32, u8, u8); 99] = [
    (0x2030, 0, 4), (0x2030, 7, 1), (0x208C, 24, 6), (0x2090, 14, 4), (0x2090, 10, 4), (0x300C, 11, 4), (0x394C, 23, 2),
    (0x3950, 0, 7), (0x3950, 8, 7), (0x3950, 16, 8), (0x3950, 24, 8),
    (0x3954, 0, 7), (0x3954, 8, 7), (0x3954, 16, 8), (0x3954, 24, 8),
    (0x3958, 0, 7), (0x3958, 8, 7), (0x3958, 16, 8), (0x3958, 24, 8),
    (0x395C, 0, 7), (0x395C, 8, 7), (0x395C, 16, 8), (0x395C, 24, 8),
    (0x416C, 0, 8), (0x4160, 0, 7), (0x416C, 8, 8), (0x4160, 16, 10), (0x416C, 16, 12), (0x416C, 28, 1),
    (0x4170, 0, 6), (0x4170, 6, 1), (0x4170, 8, 6), (0x4170, 14, 1), (0x4170, 16, 9),
    (0x4360, 0, 5), (0x4360, 6, 9),
    (0x4448, 0, 8), (0x4448, 8, 8), (0x4448, 16, 8), (0x4448, 24, 8), (0x444C, 0, 3),
    (0x6E30, 0, 5), (0x6E30, 8, 5), (0x6E30, 16, 5), (0x6E30, 24, 5),
    (0x6E34, 0, 5), (0x6E34, 8, 5), (0x6E34, 16, 5), (0x6E34, 24, 5),
    (0x6E38, 0, 5), (0x6E38, 8, 5), (0x6E38, 16, 5), (0x6E38, 24, 5),
    (0x6E3C, 0, 5), (0x6E3C, 8, 5), (0x6E3C, 16, 5), (0x6E3C, 24, 5),
    (0x6E40, 0, 5), (0x6E40, 8, 5), (0x6E40, 16, 5), (0x6E40, 24, 5),
    (0x6E44, 0, 5),
    (0x6E48, 0, 10), (0x6E48, 10, 10), (0x6E48, 20, 10),
    (0x6E4C, 0, 10), (0x6E4C, 10, 10), (0x6E4C, 20, 10),
    (0x6E50, 0, 10), (0x6E50, 10, 10), (0x6E50, 20, 10),
    (0x6E54, 0, 10), (0x6E54, 10, 10), (0x6E54, 20, 10),
    (0x6E58, 0, 10), (0x6E58, 10, 10), (0x6E58, 20, 10),
    (0x6E5C, 0, 10), (0x6E5C, 10, 10), (0x6E5C, 20, 10),
    (0x6E60, 0, 10), (0x6E60, 10, 10), (0x6E60, 20, 10),
    (0x6E64, 0, 10),
    (0x7418, 7, 5), (0x741C, 0, 5), (0x741C, 11, 4), (0x741C, 19, 4),
    (0x7434, 17, 1), (0x7434, 18, 7),
    (0x7068, 0, 9), (0x410C, 7, 1), (0x400C, 0, 7),
    // `OTP_DIFFERS`: map 1's "extra bit" (no register), map 2's VMON_POL.
    (0x0000, 0, 1),
    (0x1_7040, 0, 8), (0x1_7040, 8, 8), (0x1_7040, 16, 8), (0x1_7040, 24, 8), (0x1_7044, 0, 24),
];
const OTP_DIFFERS: usize = 93;
const OTP_VMON_POL: (u32, u8, u8) = (0x4000, 11, 1);
/// Words of OTP (h:545) and where the packed trims start in them.
const OTP_WORDS: usize = 32;
const OTP_WORD: usize = 2;
const OTP_BIT: u32 = 16;

/// The boost from the device tree (dts:186-188: 1 µH, 4 µF, 3500 mA peak) thru `cs35l41_boost_config` (c:2917-3011): inductor code 0, capacitor range 0, so K1 and K2 G#24 and the slope G#75 (c:189-204), which are the chip's own defaults (t:56-57); the peak current is the one that changes: (3500 - 1600) / 50 + G#10 = G#36.
const BST_K1: u32 = 0x24;
const BST_K2: u32 = 0x24;
const BST_SLOPE: u32 = 0x75;
const BST_LBST: u32 = 0;
const BST_IPK: u32 = (3500 - 1600) / 50 + 0x10;

/// The serial port as the AoC drives TDM0 (gs101-audio.dtsi:678-692 DSP_A; aoc_alsa_card.c:245-246, 4 slots of S32 carrying S16 stereo at 48 kHz): SP_FORMAT's RX and TX slot widths 32 (bits 24-31, 16-23), format DSP_A 0 (bits 8-10, c:2614-2617), SCLK and LRCLK from the AoC (clock_mode 0, c:2603-2605), no inversions (NB_NF, c:2641-2644). The received word is 16 bits (SP_RX_WL, c:2741-2747; the BE's S16_LE, card.c:245). AUDIO.md had G#2020_0000 and 16 from the same reading; Android's own values on the oracle are the check (AUDIO.md "Verification").
const SP_FORMAT_TDM: u32 = 32 << 24 | 32 << 16;
const RX_WL: u32 = 16;
/// PLL from SCLK at 6.144 MHz (dts:64-88: the card's sysclk is BCLK; c:2816-2878 then c:105-118: REFCLK code G#28), closed loop, enabled: G#510 (AUDIO.md).
const PLL_SCLK_6M144: u32 = 0x28 << 5 | 1 << 4;
/// 48 kHz (c:2694-2705: code 3).
const GLOBAL_FS_48K: u32 = 0x03;
/// The FS monitor's windows for a 6.144 MHz SCLK (c:2880-2915 with c:145-187: 16 and 24): fs1 | fs2 << 12.
const FS_MON_6M144: u32 = 16 | 24 << 12;

/// Polls for a hardware handshake, a millisecond apart (out of reset, power down): Linux's own waits (c:4224-4240 OTP boot; c:2321-2331 PDN_DONE, 100 of them).
const POLLS: u32 = 1 << 7;

/// The port thru its hardware grant.
pub struct Grant(u64);
impl Regs for Grant {
    fn rd(&self, off: usize) -> u32 {
        mmio_read(self.0, off as u64) as u32
    }
    fn wr(&self, off: usize, v: u32) {
        mmio_write(self.0, off as u64, v);
    }
    fn now(&self) -> u64 {
        now()
    }
    fn freq(&self) -> u64 {
        freq()
    }
}

fn sleep_ms(ms: u64) {
    sleep_until(now() + freq() * ms / 1000);
}

/// The right amplifier's select, gpn1-0, active low: a data bit of the granted bank (its mode is the `amp` switch's).
fn right_select(sel: bool) {
    let c = cap(crate::grants(), S_GPN);
    let v = mmio_read(c, GPN1_DAT) as u32;
    mmio_write(c, GPN1_DAT, if sel { v & !(1 << RIGHT_CS) } else { v | 1 << RIGHT_CS });
}

pub struct Amps {
    spi: Spi<Grant>,
}

pub const NAMES: [&str; 2] = ["left", "right"];

impl Amps {
    pub fn new() -> Self {
        Amps { spi: Spi { regs: Grant(cap(crate::grants(), S_SPI)), mode: 0, fb: 0 } }
    }

    fn cs(amp: usize) -> Cs<'static> {
        if amp == 0 { Cs::Native } else { Cs::Pin(&right_select) }
    }

    /// One register: a 32-bit address with bit 31 set, 16 bits of padding, then the value, all big-endian (payloads/amp; AUDIO.md).
    pub fn read(&self, amp: usize, reg: u32) -> Result<u32, &'static str> {
        let r = (reg | 0x8000_0000).to_be_bytes();
        let mut v = [0u8; 4];
        self.spi.transact(Self::cs(amp), &[r[0], r[1], r[2], r[3], 0, 0], &mut v)?;
        Ok(u32::from_be_bytes(v))
    }

    /// One register written: the address with bit 31 clear, the padding, the value.
    pub fn write(&self, amp: usize, reg: u32, val: u32) -> Result<(), &'static str> {
        let r = reg.to_be_bytes();
        let v = val.to_be_bytes();
        self.spi.transact(Self::cs(amp), &[r[0], r[1], r[2], r[3], 0, 0, v[0], v[1], v[2], v[3]], &mut [])
    }

    /// A field changed, the rest kept (regmap_update_bits).
    fn update(&self, amp: usize, reg: u32, mask: u32, val: u32) -> Result<(), &'static str> {
        let v = self.read(amp, reg)?;
        self.write(amp, reg, (v & !mask) | (val & mask))
    }

    fn seq(&self, amp: usize, s: &[(u32, u32)]) -> Result<(), &'static str> {
        for &(r, v) in s {
            self.write(amp, r, v)?;
        }
        Ok(())
    }

    /// The switch on (both resets pulsed, the port's clock at about 3 MHz: kernel amp.rs, spi.rs `halved`), then each one brought up as Linux's probe and the card's hw_params do, everything but the enable.
    pub fn up(&self) -> Result<(), &'static str> {
        if !ferros_sys::power(cap(crate::grants(), S_POWER), true) {
            return Err("the amp switch refused (the port's clocks, buck A, or HSI1 off)");
        }
        for amp in 0..2 {
            self.bring(amp).map_err(|e| {
                say!("audio: {} amplifier: {e}", NAMES[amp]);
                e
            })?;
        }
        Ok(())
    }

    fn bring(&self, amp: usize) -> Result<(), &'static str> {
        // OTP boot done (c:4224-4240).
        let mut booted = false;
        for _ in 0..POLLS {
            if self.read(amp, IRQ1_STATUS4)? & OTP_BOOT_DONE != 0 {
                booted = true;
                break;
            }
            sleep_ms(1);
        }
        if !booted {
            return Err("no OTP_BOOT_DONE");
        }
        let (id, rev) = (self.read(amp, DEVID)?, self.read(amp, REVID)?);
        if id != 0x35A40 && id != 0x35B40 {
            return Err("not a CS35L41");
        }
        // WHY: the errata patch below is B2's (c:4339-4364 picks by revision); another revision would take another patch.
        if rev & 0xFF != 0xB2 {
            return Err("not revision B2: the errata patch here is B2's");
        }
        self.seq(amp, &ERRATA_B2)?;
        self.otp(amp)?;
        // The DSP held off (c:4401): the DAC takes the port directly.
        self.write(amp, DSP1_CCM_CORE_CTRL, 0)?;
        // Boost (c:2917-3011).
        self.update(amp, BSTCVRT_COEFF, 0xFFFF, BST_K1 | BST_K2 << 8)?;
        self.update(amp, BSTCVRT_SLOPE_LBST, 0xFF03, BST_SLOPE << 8 | BST_LBST)?;
        self.update(amp, BSTCVRT_PEAK_CUR, 0x7F, BST_IPK)?;
        // The serial port (c:2594-2686, c:2704-2774) and its clock (c:2816-2915): the PLL opened, its reference set and source chosen, closed, enabled, as c:2856-2876 steps it.
        self.update(amp, SP_FORMAT, 0xFFFF_0000 | 0x0700 | 0x10 | 0x01 | 0x40 | 0x04, SP_FORMAT_TDM)?;
        self.update(amp, SP_RX_WL, 0x3F, RX_WL)?;
        self.update(amp, GLOBAL_CLK_CTRL, 0x1F, GLOBAL_FS_48K)?;
        self.update(amp, PLL_CLK_CTRL, 1 << 11, 1 << 11)?;
        self.update(amp, PLL_CLK_CTRL, 0x7E0, 0x28 << 5)?;
        self.update(amp, PLL_CLK_CTRL, 1 << 4, 0)?;
        self.update(amp, PLL_CLK_CTRL, 0x07, 0)?;
        self.update(amp, PLL_CLK_CTRL, 1 << 11, 0)?;
        self.update(amp, PLL_CLK_CTRL, 1 << 4, 1 << 4)?;
        self.write(amp, TST_FS_MON0, FS_MON_6M144)?;
        // The channel: Linux leaves both DACs on ASPRX1 and lets the DSP firmware pick the right's slot (its tuning file is `R-…`, dmesg:7395); with no DSP, the right amplifier takes ASPRX2 (slot 1, SP_FRAME_RX_SLOT's reset value t:71) itself, or both would play the left.
        let (src, en) = if amp == 0 { (SRC_ASPRX1, ASP_RX1_EN) } else { (SRC_ASPRX2, ASP_RX2_EN) };
        self.write(amp, DAC_PCM1_SRC, src)?;
        self.update(amp, SP_ENABLES, ASP_RX1_EN | ASP_RX2_EN, en)?;
        // The amplifier's own enable, DAPM's "Main AMP" widget (c:2469-2471: PWR_CTRL2 bit 0); GLOBAL_EN comes with the sound (`on`).
        self.update(amp, PWR_CTRL2, AMP_EN, AMP_EN)?;
        let pll = self.read(amp, PLL_CLK_CTRL)?;
        let p2 = self.read(amp, PWR_CTRL2)?;
        say!("audio: {} amplifier: ready (DEVID G#{id:X} REVID G#{rev:X}, PLL_CLK_CTRL G#{pll:X}, PWR_CTRL2 G#{p2:X})", NAMES[amp]);
        // The steps above must leave the PLL as AUDIO.md read it from Linux's: a write that did not take is a broken link, said here rather than heard.
        if pll & 0xFFF != PLL_SCLK_6M144 {
            return Err("PLL_CLK_CTRL did not take G#510");
        }
        Ok(())
    }

    /// The trims unpacked from the OTP into their registers (c:1856-1988), the OTP read at the port's halved clock (h:740: 4 MHz or less).
    fn otp(&self, amp: usize) -> Result<(), &'static str> {
        let second = match self.read(amp, OTPID)? & 0xF {
            1 | 8 => false,
            2 | 3 | 6 => true,
            _ => return Err("an OTP map this driver does not know"),
        };
        let mut mem = [0u32; OTP_WORDS];
        for (k, w) in mem.iter_mut().enumerate() {
            *w = self.read(amp, OTP_MEM0 + 4 * k as u32)?;
        }
        self.write(amp, TEST_KEY_CTL, 0x55)?;
        self.write(amp, TEST_KEY_CTL, 0xAA)?;
        let (mut word, mut bit) = (OTP_WORD, OTP_BIT);
        for (i, &e) in OTP_MAP.iter().enumerate() {
            let (reg, shift, size) = if i == OTP_DIFFERS && second { OTP_VMON_POL } else { e };
            let size = size as u32;
            // The field's bits from this word, and the next word's low bits when it straddles (c:1931-1949).
            let val = if bit + size > 32 {
                let hi_bits = bit + size - 32;
                let v = mem[word] >> bit | (mem[word + 1] & ((1u32 << hi_bits) - 1)) << (32 - bit);
                word += 1;
                bit = hi_bits;
                v
            } else {
                let v = (mem[word] >> bit) & (((1u64 << size) - 1) as u32);
                bit += size;
                v
            };
            if bit == 32 {
                bit = 0;
                word += 1;
            }
            if reg != 0 {
                let mask = ((((1u64 << size) - 1) as u32)) << shift;
                self.update(amp, reg, mask, val << shift)?;
            }
        }
        self.write(amp, TEST_KEY_CTL, 0xCC)?;
        self.write(amp, TEST_KEY_CTL, 0x33)?;
        Ok(())
    }

    /// Both playing: the power-up patch, GLOBAL_EN, a millisecond (c:2279-2299); then each one's PLL lock, power-up done and errors read and said. Sound out needs the AoC's SCLK running already (AUDIO.md: the PLL has no source of its own).
    pub fn on(&self) -> Result<(), &'static str> {
        for amp in 0..2 {
            self.seq(amp, &PUP_PATCH)?;
            self.update(amp, PWR_CTRL1, GLOBAL_EN, GLOBAL_EN)?;
        }
        sleep_ms(1);
        let mut ok = true;
        for amp in 0..2 {
            let s1 = self.read(amp, IRQ1_STATUS1)?;
            let s3 = self.read(amp, IRQ1_STATUS3)?;
            say!("audio: {} amplifier: on, IRQ1_STATUS1 G#{s1:X} (PUP_DONE {}, errors G#{:X}), IRQ1_STATUS3 G#{s3:X} (PLL lock {})", NAMES[amp], (s1 & PUP_DONE != 0) as u8, s1 & ERRORS, (s3 & PLL_LOCK != 0) as u8);
            ok &= s1 & ERRORS == 0;
        }
        if ok { Ok(()) } else { Err("an amplifier reports an error") }
    }

    /// Both quiet: GLOBAL_EN off, PDN_DONE awaited and cleared, the power-down patch (c:2305-2346). The switch goes off when this process ends (the kernel's, for a driver that ends), holding both in reset.
    pub fn off(&self) {
        for amp in 0..2 {
            let _ = self.update(amp, PWR_CTRL1, GLOBAL_EN, 0);
        }
        for amp in 0..2 {
            let mut done = false;
            for _ in 0..POLLS {
                if self.read(amp, IRQ1_STATUS1).map_or(false, |s| s & PDN_DONE != 0) {
                    done = true;
                    break;
                }
                sleep_ms(1);
            }
            let _ = self.write(amp, IRQ1_STATUS1, PDN_DONE);
            let _ = self.seq(amp, &PDN_PATCH);
            if !done {
                say!("audio: {} amplifier: no PDN_DONE", NAMES[amp]);
            }
        }
    }
}
