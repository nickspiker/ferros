//! stdpanic: a std program that dies on purpose, for the death reports (KERNEL.md "A process that dies"): std's panic hook hands the kernel the message and place (the fork's `default_hook` thru ferros_rt `panic_report`), from a thread named `worker`, a few calls deep, and the abort ends the process with them.
//!
//! `ferros-bridge spawn stdpanic.bin --name stdpanic --entropy`, then `ferros-bridge deaths 1`.

use std::thread;

#[inline(never)]
fn sum_to(v: &[u32], end: usize) -> u32 {
    v[..end].iter().sum()
}

#[inline(never)]
fn work(v: &[u32]) -> u32 {
    // Past the slice's end by a number only known at run time.
    let end = std::hint::black_box(v.len() * 6409);
    sum_to(v, end) + 1
}

fn main() {
    println!("stdpanic: dying on purpose in a thread named worker");
    let v: Vec<u32> = (0..1 << 6).collect();
    let h = thread::Builder::new().name("worker".into()).spawn(move || work(&v)).unwrap();
    let r = h.join();
    println!("stdpanic: lived ({})", r.is_ok());
}
