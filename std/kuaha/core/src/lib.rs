//! Kūaha (Māori for doorway; `kuaha` in code): the kist that shows the person what is on their device and lets them act on it (owner, 2026-11-20: "need a way to see what's on this device").
//!
//! - **The hold's items** (docs/STORE.md, docs/HOLD-API.md): found by kind, time, maker and words, never folders; each a card with its kind, title, maker, time and size in a circle of its kind; opened to its facts, removed with two taps (gone at once), and handed to the kist that opens it (greyed, naming that kist, until the hand-off is built).
//! - **The whole device**: the hold's totals by kind, the RAW clips area (used, free, the largest free stretch, its clips) and what each stowed kist keeps for itself.
//! - **The look**: the deck's theme, circles, every word in the person's language (ferros_lang) and every number in their base (tatau).
//!
//! This crate has no platform: the hold is a `HoldSource` (the kist's `items` grant on ferros, the hold's server in tests, the person's devices over their links on a desktop later) and the window is whoever calls `Kuaha::draw` and feeds it touches.

pub mod kinds;
pub mod look;
pub mod model;
pub mod source;
pub mod ui;

pub use look::Locale;
pub use model::{serve, Answer, Ask, Model, Prefs, View};
pub use source::{HoldSource, Link, LinkKind, Over, Problem, Usage};
pub use ui::{panel_circle, Circle, Kuaha, FACE};
