//! heapgrow: the owner's test of docs/NO-CEILINGS.md step 3 (2026-11-23): "if I open notepad and it requests 1mb of memory and it's happy, then I go and open a video because I want to search thru the frame metadata and it malloc's 4GB of RAM, it better work."
//! Built with no `--defsym __ferros_heap` (no build.rs): its ceiling is what the device holds (crates/rt/ferros.ld, ferros_sys::HEAP_DEVICE).
//! `ferros-bridge spawn heapgrow.bin --name heapgrow`; every line goes to the console:
//! 0. 1 MiB held and written (the notepad).
//! 1. 4 GiB asked for with `try_reserve_exact` and every byte written (the video): its time, or the refusal as the device gave it.
//! 2. That freed, then 256 MiB at a time until the device says no: how much it gave, then the refusal, said, with the process still running.
//! 3. All freed, and 4 GiB asked for once more: the memory given back is the process's to ask for again.
//! 4. 64 TiB asked for plainly (no try): the out-of-memory line (what was asked, what the device has free), then the process ends.
//! On the 8a (a pool of 5.3 GiB, the system's own processes in it) step 1 may be refused: the line says so, and that is the clear refusal the rule asks for.

use std::time::Instant;

const MIB: usize = 1 << 20;
/// The video's ask: 4 GiB.
const VIDEO: usize = 1 << 32;
/// What step 2 takes at a time: 256 MiB.
const PIECE: usize = 1 << 28;

/// A buffer of `len` bytes, every byte written with `fill`, or the refusal's words.
fn take(len: usize, fill: u8) -> Result<Vec<u8>, String> {
    let mut v: Vec<u8> = Vec::new();
    v.try_reserve_exact(len).map_err(|e| format!("{e}"))?;
    v.resize(len, fill);
    Ok(v)
}

fn main() {
    let t0 = Instant::now();
    let note = match take(MIB, 1) {
        Ok(v) => v,
        Err(e) => {
            println!("heapgrow: 1 MiB refused: {e}");
            return;
        }
    };
    println!("heapgrow: 0. 1 MiB held");
    let t = Instant::now();
    match take(VIDEO, 2) {
        Ok(v) => {
            let sum: u64 = v.iter().step_by(1 << 21).map(|&b| b as u64).sum();
            println!("heapgrow: 1. 4 GiB given and written in {} ms (a byte a frame read back: {})", t.elapsed().as_millis(), sum >> 1);
        }
        Err(e) => println!("heapgrow: 1. 4 GiB refused in {} ms: {e}", t.elapsed().as_millis()),
    }
    let t = Instant::now();
    let mut held: Vec<Vec<u8>> = Vec::new();
    let why = loop {
        match take(PIECE, 3) {
            Ok(v) => held.push(v),
            Err(e) => break e,
        }
    };
    println!("heapgrow: 2. {} MiB given in pieces of 256 MiB in {} ms, then refused: {why}", held.len() * (PIECE / MIB), t.elapsed().as_millis());
    drop(held);
    match take(VIDEO, 4) {
        Ok(_) => println!("heapgrow: 3. 4 GiB given again after the rest was freed"),
        Err(e) => println!("heapgrow: 3. 4 GiB refused again: {e}"),
    }
    assert!(note.iter().all(|&b| b == 1), "the notepad's megabyte changed");
    println!("heapgrow: done in {} ms, the notepad's megabyte as written", t0.elapsed().as_millis());
    // 4. A plain allocation past the device, as most code allocates (no try): the process ends, and the line before its end says what was asked and what the device had free (the fork's out-of-memory hook, ferros_rt `out_of_memory`, the kernel's ARC_FREE).
    println!("heapgrow: 4. 64 TiB asked plainly; the next line is the out-of-memory report and the process ends");
    // Behind black_box and touched: an allocation nothing reads is one LLVM removes (the Pro, 2026-11-23: "given?! 70368744177664" with no allocation made).
    let mut past: Vec<u8> = Vec::with_capacity(std::hint::black_box(1 << 46));
    past.push(1);
    std::hint::black_box(&past);
    println!("heapgrow: 4. given?! {}", past.capacity());
}
