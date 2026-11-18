//! basecalc on ferros as a kist (docs/KISTS.md "basecalc"): the pad (this crate's library) in a window that fills the glass, thru fluor's ferros host.
//! Its grant slots, as ferros lays out a kist that asks for nothing (ferros_kist `slots`): 1 display, 2 touch, 3 surface, 4 ira, 5 its vault domain, 6 entropy, then the GPU's three and sound out, which it does not use.
//! Built for ferros alone (tools/basecalc-build.sh); on another target the binary only says so, and the pad's tests are `cargo test --lib`.

#[cfg(target_os = "ferros")]
mod cshim;
#[cfg(target_os = "ferros")]
mod sys;
#[cfg(target_os = "ferros")]
mod window;

#[cfg(target_os = "ferros")]
fn main() {
    window::main()
}

#[cfg(not(target_os = "ferros"))]
fn main() {
    eprintln!("basecalc: the kist runs on ferros (tools/basecalc-build.sh); the pad's tests are `cargo test --lib`");
}
