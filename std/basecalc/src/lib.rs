//! basecalc as a ferros kist: the owner's calculator (its engine is basecalc's own library: bases 2 to 36, complex numbers, adjustable precision, trig, logs, constants, `&` for the last result) behind a scientific keypad whose digit block follows the base.
//! This library is the pad without a screen (the keys, the layout, the model, what the vault keeps), so all of it is tested on the host; the window is src/main.rs, fluor's ferros host, built for ferros alone.

pub mod calc;
pub mod keys;
pub mod layout;
pub mod store;

#[cfg(test)]
mod tests;
