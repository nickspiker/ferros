//! A line's numbers, typed (2026-11-22, owner: "So the debug printies really bug me. I'd like those to honor the base too."): a line the glass may show is said once, words and numbers apart, to a `Say`, and each surface writes the numbers its own way.
//!
//! - **The person's** (`Person`, the glass): the device's base, tatau's process-wide (SETTINGS.md "The number base"), the dozenal digits as glyphs, written when the line is drawn, so a line kept since before a base change reads in the new one.
//! - **The developer's** (`Dev`, the console ring, the flash log, the host): `A#` decimal and `G#` hexadecimal (CLAUDE.md), unchanged by the base.
//!
//! Text carries only human words (AGENT.md): a number is never put in a line's text for a reader to find again, it travels as a `Num`, its kind deciding how each base writes it.

use core::fmt::{self, Write};

/// One number in a line, by what it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Num {
    /// An amount of things (blocks, bytes when the kind is not a size): tatau's count, DMS its doublings.
    Count(u64),
    /// A size in bytes: tatau's size, counted in bits wherever the base is not arabic.
    Bytes(u64),
    /// A duration in nanoseconds: tatau's duration (DLS in seconds, DMS the hydrogen line's doublings).
    Ns(u64),
    /// A position or a name, not an amount (a core, a slot): its digits in the base, never doublings.
    /// The developer's form is `A#` decimal.
    Index(u64),
    /// A source line or column: digits in the base as `Index`; the developer's form plain decimal, as `file:line:column` that editors open.
    Line(u64),
    /// A pattern of bits (a register, an address, a code): digits in the base as `Index`; the developer's form `G#` hexadecimal.
    Bits(u64),
    /// A whole 64-bit register as `Bits`, the developer's form padded to its 16 hexadecimal digits.
    Word(u64),
}

impl Num {
    /// The developer's form: `A#` decimal for amounts and positions, `G#` hexadecimal for bits, a duration as whole seconds or milliseconds with its unit.
    pub fn dev(self, w: &mut impl Write) -> fmt::Result {
        match self {
            Num::Count(n) | Num::Index(n) => write!(w, "A#{n}"),
            Num::Bytes(n) => write!(w, "A#{n} bytes"),
            Num::Ns(ns) if ns % 1_000_000_000 == 0 => write!(w, "A#{} s", ns / 1_000_000_000),
            Num::Ns(ns) => write!(w, "A#{} ms", ns / 1_000_000),
            Num::Line(n) => write!(w, "{n}"),
            Num::Bits(n) => write!(w, "G#{n:X}"),
            Num::Word(n) => write!(w, "G#{n:016X}"),
        }
    }

    /// The person's form in `f` (`Fmt::current(Style::Glyphs)` on the glass): tatau's count, size and duration for amounts, the digits of the base for positions and bits.
    /// Zero in DMS has no doublings and is written 0, as the display's console writes it.
    pub fn person(self, w: &mut impl Write, f: tatau::Fmt) -> fmt::Result {
        let out = match self {
            Num::Count(n) => tatau::count(w, f, n, 1)?,
            Num::Bytes(n) => tatau::size_bits(w, f, n << 3, 1)?,
            Num::Ns(ns) => tatau::duration_ns(w, f, ns, 1)?,
            Num::Index(n) | Num::Line(n) | Num::Bits(n) | Num::Word(n) => {
                return match f.base {
                    tatau::Base::Arabic => write!(w, "{n}"),
                    tatau::Base::Hex => write!(w, "{n:X}"),
                    tatau::Base::Dms | tatau::Base::Dls => tatau::dozenal_whole(w, f.style, n),
                };
            }
        };
        if out == tatau::Out::Nothing {
            w.write_str("0")?;
        }
        Ok(())
    }
}

/// Where a line is said: its words as they are, its numbers typed.
pub trait Say {
    fn s(&mut self, words: &str);
    fn n(&mut self, n: Num);
}

/// The developer's copy of a line: words and `Num::dev` into any text.
pub struct Dev<'a, W: Write>(pub &'a mut W);

impl<W: Write> Say for Dev<'_, W> {
    fn s(&mut self, words: &str) {
        let _ = self.0.write_str(words);
    }
    fn n(&mut self, n: Num) {
        let _ = n.dev(self.0);
    }
}

/// The person's copy of a line: words and `Num::person` in `f` into any text.
pub struct Person<'a, W: Write>(pub &'a mut W, pub tatau::Fmt);

impl<W: Write> Say for Person<'_, W> {
    fn s(&mut self, words: &str) {
        let _ = self.0.write_str(words);
    }
    fn n(&mut self, n: Num) {
        let _ = n.person(self.0, self.1);
    }
}
