//! ferros's own look, without ferros: the person's language, number base, zone and theme as the platform reads them each frame, the deck's theme colours, and every number and time written thru tatau.

use ferros_lang::{Lang, Msg, Say};
use fluor::theme::{dark, fmt, from_gamma2, vsf};

/// What the platform knows of the person and the moment, read at each frame (the person can change any of it while Kūaha runs).
#[derive(Clone, Copy, Debug)]
pub struct Locale {
    pub lang: Lang,
    pub base: tatau::Base,
    /// Seconds east of UTC that the base shows (ferros_sys `zone`: arabic the legal offset, dozenal and hex the standard one).
    pub zone: i64,
    /// Eagle time now, oscillations.
    pub now: i64,
    /// The deck's theme (ferros_sys preference 5), an index into `THEMES`.
    pub theme: usize,
}

impl Locale {
    /// English, dozenal, UTC, the grey theme: a desktop's first guess and the tests'.
    pub fn plain(now: i64) -> Locale {
        Locale { lang: Lang::En, base: tatau::Base::Dms, zone: 0, now, theme: 0 }
    }

    pub fn t(&self, m: Msg) -> &'static str {
        ferros_lang::text(self.lang, m)
    }

    pub fn say(&self, m: Say) -> String {
        let mut s = String::new();
        let _ = ferros_lang::say(self.lang, m, self.numbers(), &mut s);
        s
    }

    /// Numbers in the person's base, drawn with the Oxanium +glyphs face's dozenal digits (Kūaha loads it: `crate::FACE`).
    pub fn numbers(&self) -> tatau::Fmt {
        tatau::Fmt::new(self.base, tatau::Style::Glyphs)
    }

    pub fn count(&self, n: u64) -> String {
        let mut s = String::new();
        if let Ok(tatau::Out::Nothing) = tatau::count(&mut s, self.numbers(), n, 1) {
            s.push('0');
        }
        s
    }

    /// A size in bytes, as tatau writes sizes (bits everywhere but arabic, which reads bytes with binary prefixes).
    pub fn size(&self, bytes: u64) -> String {
        let mut s = String::new();
        // WHY saturating: a size is the hold's word (a card read from the vault); past 2^61 bytes it is wrong anyway, and a wrapped count would read as small.
        if let Ok(tatau::Out::Nothing) = tatau::size_bits(&mut s, self.numbers(), bytes.saturating_mul(8), 1) {
            s.push('0');
        }
        s
    }

    /// A share of a whole (the clips area used), in the base.
    pub fn share(&self, part: u64, whole: u64) -> String {
        let mut s = String::new();
        let _ = tatau::share(&mut s, self.numbers(), part, whole, 2);
        s
    }

    /// When, from Eagle time: the date and time in the zone, as the deck writes it (payloads/map: Eagle time to the wall clock for the eye only).
    pub fn when(&self, eagle: i64) -> String {
        /// Eagle time's epoch in seconds since 1970 (the map's `EPOCH_UNIX`): for the calendar's arithmetic only, never kept.
        const EPOCH_UNIX: i64 = -14_182_940;
        let ops = vsf::types::eagle_time::OSCILLATIONS_PER_SECOND as i64;
        let secs = eagle.div_euclid(ops) + EPOCH_UNIX + self.zone;
        let ns = (eagle.rem_euclid(ops) as i128 * 1_000_000_000 / ops as i128) as u32;
        let mut s = String::new();
        let _ = tatau::date_time(&mut s, self.numbers(), secs, ns, eagle);
        s
    }

    pub fn colours(&self) -> Colours {
        // WHY the min: the index is the platform's preference byte; one past the table reads as the last theme, as ferros_sys::theme::current does.
        Colours::of(&THEMES[self.theme.min(THEMES.len() - 1)])
    }
}

/// The deck's themes as ferros_sys::theme::THEMES holds them (copied: ferros_sys is the device's no_std crate, never a desktop's), G#RRGGBB codes at gamma 2 as the display takes them.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub plane: u32,
    pub ring: u32,
    pub lit: u32,
    pub mark: u32,
    pub text: u32,
    pub text2: u32,
    pub text3: u32,
}

pub const THEMES: [Theme; 4] = [
    Theme { plane: 0x2A2A2A, ring: 0x8A8A8A, lit: 0xFFA000, mark: 0xFFFFFF, text: 0xF0F0F0, text2: 0x9A9A9A, text3: 0x5C5C5C },
    Theme { plane: 0x222830, ring: 0x7A8CA0, lit: 0x3CC8FF, mark: 0xE8F0F8, text: 0xE8F0F8, text2: 0x8EA0B4, text3: 0x506070 },
    Theme { plane: 0x2C241E, ring: 0xA08060, lit: 0xFF7A30, mark: 0xF4E8DC, text: 0xF4E8DC, text2: 0xB09880, text3: 0x6A5848 },
    Theme { plane: 0x1E0806, ring: 0x802018, lit: 0xFF2810, mark: 0xD04030, text: 0xD04030, text2: 0x902818, text3: 0x501008 },
];

/// A gamma-2 code as fluor's buffer stores it: opaque, α and darkness, in the buffer's working primaries.
pub const fn ink(gamma2: u32) -> u32 {
    dark(fmt(vsf(from_gamma2(gamma2 & 0x00FF_FFFF))))
}

/// The theme's colours as the buffer stores them.
#[derive(Clone, Copy, Debug)]
pub struct Colours {
    pub ground: u32,
    pub plane: u32,
    pub ring: u32,
    pub lit: u32,
    pub text: u32,
    pub text2: u32,
    pub text3: u32,
    /// A removal armed: the WiFi page's forget, armed (the button look's held fill, the grey theme's).
    pub armed: u32,
}

impl Colours {
    fn of(t: &Theme) -> Colours {
        Colours { ground: ink(0), plane: ink(t.plane), ring: ink(t.ring), lit: ink(t.lit), text: ink(t.text), text2: ink(t.text2), text3: ink(t.text3), armed: ink(0x6A160C) }
    }
}

/// Each kind's flavour, G#RRGGBB at gamma 2 (as the radios' flavours are picked): its circle's ring and glyph.
pub fn flavour(kind: &[u8; 32]) -> u32 {
    use ferros_items::kinds::*;
    ink(match *kind {
        PHOTO => 0xE0A040,
        RAW_VIDEO => 0xE04838,
        VIDEO => 0xC050B0,
        DOCUMENT => 0xC8C8C8,
        SCAN => 0x40B8B0,
        LIST => 0x88C048,
        CONTACT => 0x8C9CE8,
        SONG => 0xD878A0,
        GPS_TRACK => 0xB04CC8,
        _ => 0x8A8A8A,
    })
}
