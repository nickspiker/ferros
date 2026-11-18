//! The keys: what each one writes into the expression (basecalc's own syntax, the text the engine reads) and what it shows.
//! The expression is kept as the engine's text alone; what the display shows, and what the delete key takes back, are both read back out of it by [`units`], so the two can never disagree.

/// A key's text unit: what the engine reads, what the person sees, and how it sits between values.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Unit {
    /// The engine's text (basecalc syntax: `#sin(`, `@pi`, `*`).
    pub text: &'static str,
    /// What the key and the expression show.
    pub label: &'static str,
    /// What kind of thing it is, for the display's colour and for the implied product.
    pub kind: Kind,
}

/// What a unit is in an expression.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// An operator between two values (`+`, `×`, `mod`).
    Infix,
    /// Starts a value: a function's name and its open bracket, an open bracket, a complex number's open bracket.
    Opens,
    /// A whole value: a constant, the previous result, i.
    Value,
    /// Ends a value: a close bracket.
    Closes,
    /// Inside a value: a digit or the radix point.
    Digit,
    /// The separator inside a complex number.
    Comma,
}

const fn u(text: &'static str, label: &'static str, kind: Kind) -> Unit {
    Unit { text, label, kind }
}

pub const ADD: Unit = u("+", "+", Kind::Infix);
pub const SUB: Unit = u("-", "\u{2212}", Kind::Infix);
pub const MUL: Unit = u("*", "\u{d7}", Kind::Infix);
pub const DIV: Unit = u("/", "\u{f7}", Kind::Infix);
pub const POW: Unit = u("^", "x\u{207f}", Kind::Infix);
pub const MOD: Unit = u("%", "mod", Kind::Infix);
/// `x$b`: the logarithm of x in base b (basecalc's `$`).
pub const LOGB: Unit = u("$", "log\u{2099}", Kind::Infix);
pub const OPEN: Unit = u("(", "(", Kind::Opens);
pub const CLOSE: Unit = u(")", ")", Kind::Closes);
pub const POINT: Unit = u(".", ".", Kind::Digit);
/// The previous result.
pub const PREV: Unit = u("&", "&", Kind::Value);
/// i, as basecalc writes it: the complex number [0, 1].
pub const I: Unit = u("[0,1]", "i", Kind::Value);
pub const CPLX_OPEN: Unit = u("[", "[", Kind::Opens);
pub const CPLX_COMMA: Unit = u(",", ",", Kind::Comma);
pub const CPLX_CLOSE: Unit = u("]", "]", Kind::Closes);
/// The reciprocal: to the power minus one (`1` is a digit in every base).
pub const RECIP: Unit = u("^-1", "1/x", Kind::Infix);

pub const SQRT: Unit = u("#sqrt(", "\u{221a}", Kind::Opens);
pub const ABS: Unit = u("#abs(", "|x|", Kind::Opens);
pub const LN: Unit = u("#ln(", "ln", Kind::Opens);
pub const LOG: Unit = u("#log(", "log", Kind::Opens);
pub const SIN: Unit = u("#sin(", "sin", Kind::Opens);
pub const COS: Unit = u("#cos(", "cos", Kind::Opens);
pub const TAN: Unit = u("#tan(", "tan", Kind::Opens);
pub const ASIN: Unit = u("#asin(", "asin", Kind::Opens);
pub const ACOS: Unit = u("#acos(", "acos", Kind::Opens);
pub const ATAN: Unit = u("#atan(", "atan", Kind::Opens);
pub const CEIL: Unit = u("#ceil(", "ceil", Kind::Opens);
pub const FLOOR: Unit = u("#floor(", "floor", Kind::Opens);
pub const ROUND: Unit = u("#round(", "round", Kind::Opens);
pub const INT: Unit = u("#int(", "int", Kind::Opens);
pub const FRAC: Unit = u("#frac(", "frac", Kind::Opens);
pub const RE: Unit = u("#re(", "Re", Kind::Opens);
pub const IM: Unit = u("#im(", "Im", Kind::Opens);
pub const ARG: Unit = u("#angle(", "arg", Kind::Opens);
pub const SIGN: Unit = u("#sign(", "sgn", Kind::Opens);
pub const ERF: Unit = u("#erf(", "erf", Kind::Opens);

pub const PI: Unit = u("@pi", "\u{3c0}", Kind::Value);
pub const E: Unit = u("@e", "e", Kind::Value);
pub const PHI: Unit = u("@phi", "\u{3c6}", Kind::Value);
pub const GAMMA: Unit = u("@gamma", "\u{3b3}", Kind::Value);
/// Catalan's constant, G as it is usually written.
pub const CATALAN: Unit = u("@catalan", "G", Kind::Value);
pub const RAND: Unit = u("@rand", "rand", Kind::Value);
pub const GRAND: Unit = u("@grand", "grand", Kind::Value);

/// Every unit the keys write; where one text begins another (`^` and `^-1`, `[` and `[0,1]`) [`units`] takes the longest.
/// The square writes `^` and then two in the base's digits, which [`units`] reads back as those units like any typed text.
pub const UNITS: &[Unit] = &[
    RECIP, I, ADD, SUB, MUL, DIV, POW, MOD, LOGB, OPEN, CLOSE, POINT, PREV, CPLX_OPEN, CPLX_COMMA, CPLX_CLOSE, SQRT, ABS, LN, LOG, SIN, COS, TAN, ASIN, ACOS, ATAN, CEIL, FLOOR, ROUND, INT, FRAC, RE, IM, ARG, SIGN, ERF, PI, E, PHI, GAMMA, CATALAN, RAND, GRAND,
];

/// What a key does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    /// A digit of the base, 0 to 35.
    Digit(u8),
    /// A unit written into the expression.
    Write(Unit),
    /// The square: to the power two, two written in the base's own digits.
    Square,
    /// Takes the last unit back.
    Back,
    /// Clears the expression, then the result.
    Clear,
    /// Evaluates.
    Equals,
    /// The function rows' other layer.
    Second,
    /// Radians or degrees.
    Angle,
    /// Fewer digits shown (and computed).
    Fewer,
    /// More digits.
    More,
    /// One of the quick bases.
    Base(u8),
    /// Opens or closes the picker of every base.
    Bases,
}

impl Key {
    /// The key's face.
    pub fn label(self) -> String {
        match self {
            Key::Digit(d) => digit_char(d).to_string(),
            Key::Write(unit) => unit.label.to_string(),
            Key::Square => "x\u{b2}".to_string(),
            Key::Back => "\u{232b}".to_string(),
            Key::Clear => "AC".to_string(),
            Key::Equals => "=".to_string(),
            Key::Second => "2nd".to_string(),
            Key::Angle => String::new(),
            Key::Fewer => "\u{2212}".to_string(),
            Key::More => "+".to_string(),
            Key::Base(b) => base_label(b),
            Key::Bases => "\u{2026}".to_string(),
        }
    }
}

/// A digit's character as basecalc writes it: 0 to 9, then A to Z (the dozenal digits are A and B).
pub fn digit_char(d: u8) -> char {
    (if d < 10 { b'0' + d } else { b'A' + d - 10 }) as char
}

/// A count written in `base`, most significant digit first.
pub fn in_base(mut n: usize, base: u8) -> String {
    let b = base as usize;
    let mut out = Vec::new();
    loop {
        out.push(digit_char((n % b) as u8));
        n /= b;
        if n == 0 {
            break;
        }
    }
    out.iter().rev().collect()
}

/// A base as ferros writes one: the base in base 36, then `#` (G# hex, A# decimal, 2# binary, 10# base 36; Claude memories/feedback_number_format.md).
pub fn base_label(base: u8) -> String {
    format!("{}#", in_base(base as usize, 36))
}

/// The function rows: two layers of three rows of six (the second layer is the inverses, the parts and the rarer constants).
pub fn function_rows(second: bool) -> [[Key; 6]; 3] {
    use Key::{Square, Write as W};
    if second {
        [[W(ASIN), W(ACOS), W(ATAN), W(RE), W(IM), W(ARG)], [W(CEIL), W(FLOOR), W(ROUND), W(INT), W(FRAC), W(SIGN)], [W(PHI), W(GAMMA), W(CATALAN), W(RAND), W(GRAND), W(ERF)]]
    } else {
        [[W(SIN), W(COS), W(TAN), W(LN), W(LOG), W(SQRT)], [W(POW), Square, W(RECIP), W(ABS), W(MOD), W(LOGB)], [W(PI), W(E), W(I), W(CPLX_OPEN), W(CPLX_COMMA), W(CPLX_CLOSE)]]
    }
}

/// The operator columns beside the digits, top to bottom.
pub const OPERATORS: [[Key; 2]; 5] = [
    [Key::Write(OPEN), Key::Write(CLOSE)],
    [Key::Write(MUL), Key::Write(DIV)],
    [Key::Write(ADD), Key::Write(SUB)],
    [Key::Write(POINT), Key::Write(PREV)],
    [Key::Back, Key::Clear],
];

/// The settings: the layer key, the angle unit and the precision's two keys, in the settings row's cells 0, 1, 2 and 4 (the precision itself shows in cell 3).
/// The hook for "settings on the circle's back" (a proposed convention, not built: a turn of the circle shows a kist's settings, another turns it back): these are what would move there, and this list is the one place that says which keys they are.
pub const SETTINGS: [(usize, Key); 4] = [(0, Key::Second), (1, Key::Angle), (2, Key::Fewer), (4, Key::More)];

/// The quick bases along the top: binary, octal, decimal, dozenal, hex and base 36; every other base is in the picker.
pub const QUICK_BASES: [u8; 6] = [2, 8, 10, 12, 16, 36];

/// A piece of an expression: its text and the unit it is (a digit's unit is made on the spot).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Piece<'a> {
    pub text: &'a str,
    pub label: &'a str,
    pub kind: Kind,
}

/// The expression cut into the units the keys wrote, left to right: a known unit wherever one starts, else one character as a digit.
pub fn units(expr: &str) -> Vec<Piece<'_>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < expr.len() {
        let rest = &expr[at..];
        let found = UNITS.iter().filter(|u| rest.starts_with(u.text)).max_by_key(|u| u.text.len());
        let piece = match found {
            Some(u) => Piece { text: u.text, label: u.label, kind: u.kind },
            None => {
                let n = rest.chars().next().map_or(1, char::len_utf8);
                Piece { text: &rest[..n], label: &rest[..n], kind: Kind::Digit }
            }
        };
        at += piece.text.len();
        out.push(piece);
    }
    out
}

/// The expression without its last unit (the delete key).
pub fn back(expr: &str) -> &str {
    match units(expr).last() {
        Some(p) => &expr[..expr.len() - p.text.len()],
        None => expr,
    }
}

/// Whether the expression ends with a whole value, so a value written next to it means a product (`2π` is `2*@pi`; basecalc reads no implied product).
pub fn ends_with_value(expr: &str) -> bool {
    matches!(units(expr).last(), Some(Piece { kind: Kind::Value | Kind::Closes | Kind::Digit, .. }))
}

/// The expression with its open brackets closed, as `=` and the preview evaluate it: round brackets and complex ones, innermost first.
pub fn closed(expr: &str) -> String {
    let mut out = expr.to_string();
    let mut open = Vec::new();
    for p in units(expr) {
        if p.text == "[" {
            open.push(']');
        } else if p.text.ends_with('(') {
            open.push(')');
        } else if p.text == ")" || p.text == "]" {
            // A close with nothing open is the person's own; the engine says so.
            open.pop();
        }
    }
    while let Some(c) = open.pop() {
        out.push(c);
    }
    out
}
