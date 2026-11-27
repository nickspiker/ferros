//! ferros's one typeface as 16-bit coverage, turned at any angle: the display's glyph engine (payloads/display/src/glyphs.rs, its source kept there and compiled here too, 2026-11-18), so a puck draws its words in the face the display draws its own (docs/PUCKS.md).
//!
//! Composition is the caller's: `Glyphs::cover` hands each pixel's coverage to a closure, which puts it under what is there (front to back), so text drawn first stays on top.
#![no_std]

extern crate alloc;

// The engine's source stays the display's (payloads/display/src/glyphs.rs), compiled here as well, so the display and the pucks share one.
#[path = "../../../payloads/display/src/glyphs.rs"]
mod engine;
pub use engine::Glyphs;

/// Oxanium (OFL, assets/fonts), the owner's choice for ferros's face, in its +glyphs cut, which adds tatau's twelve dozenal digit glyphs at G#10-G#1B, so numbers draw in the device base as glyphs (SETTINGS.md).
pub static FONT: &[u8] = include_bytes!("../../../assets/fonts/Oxanium-Regular+glyphs.ttf");
