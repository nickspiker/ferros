//! basecalc on ferros as a kist (docs/KISTS.md "basecalc"): the pad (this crate's library) in a window that fills the glass, thru fluor's ferros host; or, built with `--features puck`, the pad as a puck on the deck (puck.rs, docs/PUCKS.md).
//! Its grant slots, as ferros lays out a kist (ferros_kist `slots`): 1 display, 2 touch, 3 surface (a puck's square, for the puck), 4 ira, 5 its vault domain, 6 entropy, then its wants (the puck's `puck`, 7) and the GPU's three and sound out, which it does not use.
//! Built for ferros alone (tools/basecalc-build.sh); on another target the binary only says so, and the pad's tests are `cargo test --lib`.

#[cfg(target_os = "ferros")]
mod cshim;
#[cfg(all(target_os = "ferros", feature = "puck"))]
mod dial;
#[cfg(all(target_os = "ferros", feature = "puck"))]
mod puck;
#[cfg(target_os = "ferros")]
mod sys;
#[cfg(target_os = "ferros")]
// The puck uses the pad and its vault from here, not the window's host.
#[cfg_attr(feature = "puck", allow(dead_code))]
mod window;

#[cfg(all(target_os = "ferros", not(feature = "puck")))]
fn main() {
    window::main()
}

#[cfg(all(target_os = "ferros", feature = "puck"))]
fn main() {
    puck::main()
}

#[cfg(not(target_os = "ferros"))]
fn main() {
    eprintln!("basecalc: the kist runs on ferros (tools/basecalc-build.sh); the pad's tests are `cargo test --lib`");
}
