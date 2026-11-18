//! The calculator behind the keys: basecalc's own state and evaluator, the expression being typed, the result shown and the history.
//! Every key is a change of this model and nothing else, so the whole pad is testable without a screen.

use crate::keys::{self, Key, Kind};
use crate::store::{Line, Saved};
use basecalc::{evaluate, num2string, BasecalcState, Span};
use rug::Complex;

/// The most lines the history keeps (and the vault holds); the oldest goes first.
pub const HISTORY: usize = 1 << 8;

/// One evaluated line: what was typed and the settings it was typed under, and what it gave.
#[derive(Clone)]
pub struct Entry {
    pub line: Line,
    pub value: Complex,
    /// The result in the base it was typed in.
    pub spans: Vec<Span>,
}

/// What the result line shows.
#[derive(Clone, PartialEq, Debug)]
pub enum Shown {
    Nothing,
    /// The expression's value as typed so far (its brackets closed), dim.
    Preview(Vec<Span>),
    /// The last result, in the base now in use.
    Value(Vec<Span>),
    /// Why the last `=` gave nothing.
    Error(String),
}

/// What a key changed, for the window.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Change {
    /// The keys moved (a base, the layer, the picker).
    pub relayout: bool,
    /// Something the vault keeps changed: a line evaluated, a setting.
    pub save: bool,
}

pub struct Calc {
    pub state: BasecalcState,
    /// The expression as the engine reads it.
    pub expr: String,
    /// The expression shown is the one just evaluated: the next value starts a new one, the next operator continues from its result.
    pub done: bool,
    /// The last result (the engine's `&`).
    pub value: Option<Complex>,
    pub shown: Shown,
    pub history: Vec<Entry>,
    /// The function rows' second layer is up.
    pub second: bool,
    /// The picker of every base is up.
    pub picking: bool,
}

impl Calc {
    pub fn new() -> Calc {
        Calc { state: BasecalcState::new(), expr: String::new(), done: false, value: None, shown: Shown::Nothing, history: Vec::new(), second: false, picking: false }
    }

    /// A calculator as the vault left it: each line run again in order from a fresh engine, under the settings it was typed with, so `&`, assignments and basecalc's random draws come back as they were; then the settings in use.
    pub fn restore(saved: &Saved) -> Calc {
        let mut c = Calc::new();
        for line in &saved.lines {
            c.settings(line.base, line.digits, line.radians);
            if let Ok((value, spans)) = evaluate(&line.expr, &mut c.state) {
                c.history.push(Entry { line: line.clone(), value: value.clone(), spans });
                c.value = Some(value);
            }
        }
        c.settings(saved.base, saved.digits, saved.radians);
        c.reshow();
        c
    }

    /// What the vault keeps.
    pub fn saved(&self) -> Saved {
        Saved { base: self.state.base, digits: self.state.digits, radians: self.state.radians, lines: self.history.iter().map(|e| e.line.clone()).collect() }
    }

    fn settings(&mut self, base: u8, digits: usize, radians: bool) {
        self.state.base = base;
        self.state.digits = digits;
        self.state.radians = radians;
        self.state.set_precision();
    }

    /// The last result again, in the base and digits now in use.
    fn reshow(&mut self) {
        self.shown = match &self.value {
            Some(v) => Shown::Value(num2string(v, &self.state)),
            None => Shown::Nothing,
        };
    }

    /// The expression's value so far, on a copy of the engine (a preview draws no random number of the real one's).
    fn preview(&mut self) {
        if self.expr.is_empty() {
            self.reshow();
            return;
        }
        let mut copy = self.state.clone();
        self.shown = match evaluate(&keys::closed(&self.expr), &mut copy) {
            Ok((_, spans)) => Shown::Preview(spans),
            // Half an expression is no error yet.
            Err(_) => Shown::Nothing,
        };
    }

    /// A new expression after a result, or the same one carried on.
    fn start_value(&mut self) {
        if self.done {
            self.expr.clear();
            self.done = false;
        }
    }

    /// An operator after a result continues from it (`&`), as one with nothing before it does once there is a result; a minus with nothing before it is a sign.
    fn start_operator(&mut self, minus: bool) {
        if self.done || (self.expr.is_empty() && self.value.is_some() && !minus) {
            self.expr = keys::PREV.text.to_string();
            self.done = false;
        }
    }

    /// `=`: the expression, its brackets closed, evaluated by the engine; kept in the history.
    fn equals(&mut self) -> Change {
        if self.expr.is_empty() || self.done {
            return Change::default();
        }
        let expr = keys::closed(&self.expr);
        match evaluate(&expr, &mut self.state) {
            Ok((value, spans)) => {
                let line = Line { expr: expr.clone(), base: self.state.base, digits: self.state.digits, radians: self.state.radians };
                self.history.push(Entry { line, value: value.clone(), spans: spans.clone() });
                if self.history.len() > HISTORY {
                    self.history.remove(0);
                }
                self.value = Some(value);
                self.shown = Shown::Value(spans);
                self.expr = expr;
                self.done = true;
                Change { relayout: false, save: true }
            }
            Err(why) => {
                self.shown = Shown::Error(why);
                Change::default()
            }
        }
    }

    /// The precision one step along: 1, 2, 3, 4, 6, 8, 12, 16 and on to 2^12, each a power of two or one and a half of one, as a camera's half stops.
    fn step_digits(&mut self, more: bool) -> Change {
        let d = self.state.digits;
        let stops = (0..=12).map(|k| 1usize << k).chain((0..11).map(|k| 3usize << k));
        let next = if more { stops.filter(|&s| s > d).min() } else { stops.filter(|&s| s < d).max() };
        match next {
            Some(n) => {
                self.state.digits = n;
                self.state.set_precision();
                if self.done || self.expr.is_empty() {
                    self.reshow();
                } else {
                    self.preview();
                }
                Change { relayout: false, save: true }
            }
            None => Change::default(),
        }
    }

    /// A base chosen: an expression being typed is finished first in the base it was typed in, then everything shows in the new one (basecalc's `:base`, which works for `&` too).
    fn set_base(&mut self, base: u8) -> Change {
        self.picking = false;
        if !self.done && !self.expr.is_empty() {
            self.equals();
        }
        self.state.base = base;
        self.state.set_precision();
        if self.done || self.expr.is_empty() {
            self.reshow();
        } else {
            self.preview();
        }
        Change { relayout: true, save: true }
    }

    pub fn press(&mut self, key: Key) -> Change {
        let typed = Change::default();
        match key {
            Key::Digit(d) => {
                self.start_value();
                self.expr.push(keys::digit_char(d));
            }
            Key::Write(unit) => match unit.kind {
                Kind::Infix => {
                    self.start_operator(unit == keys::SUB);
                    self.expr.push_str(unit.text);
                }
                Kind::Opens | Kind::Value => {
                    self.start_value();
                    if keys::ends_with_value(&self.expr) {
                        self.expr.push_str(keys::MUL.text);
                    }
                    self.expr.push_str(unit.text);
                }
                Kind::Digit => {
                    self.start_value();
                    self.expr.push_str(unit.text);
                }
                Kind::Closes | Kind::Comma => {
                    self.done = false;
                    self.expr.push_str(unit.text);
                }
            },
            Key::Square => {
                self.start_operator(false);
                self.expr.push_str(keys::POW.text);
                self.expr.push_str(&keys::in_base(2, self.state.base));
            }
            Key::Back => {
                self.done = false;
                let kept = keys::back(&self.expr).len();
                self.expr.truncate(kept);
            }
            Key::Clear => {
                self.expr.clear();
                self.done = false;
                self.shown = Shown::Nothing;
                return typed;
            }
            Key::Equals => return self.equals(),
            Key::Second => {
                self.second = !self.second;
                return Change { relayout: true, save: false };
            }
            Key::Angle => {
                self.state.radians = !self.state.radians;
                if !self.done {
                    self.preview();
                }
                return Change { relayout: false, save: true };
            }
            Key::Fewer => return self.step_digits(false),
            Key::More => return self.step_digits(true),
            Key::Base(b) => return self.set_base(b),
            Key::Bases => {
                self.picking = !self.picking;
                return Change { relayout: true, save: false };
            }
        }
        self.preview();
        typed
    }
}

impl Default for Calc {
    fn default() -> Self {
        Calc::new()
    }
}
