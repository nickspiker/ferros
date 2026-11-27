//! The cost of an entry against the old flash log's line, on the host (`cargo run --release -p ferros_ledger --features ring --example bench`).
//! The device's own measure is `bridge diag "log bench"` (flashlog.rs), which times the same steps with the UFS writes.
//!
//! Old line: `format!("b<boot> t<ms> <text>\n")` copied into the tail block; two 4 KiB writes (tail block, header).
//! Entry: the seal key made (the device derives it from the ira per entry), the payload sealed, the document built and verified for its hash; then written A, read back, written B.

use std::time::Instant;

use ferros_ledger::category::{self, Class};
use ferros_ledger::entry::{self, Payload, Place, SealKey};
use ferros_ledger::ring::{Blocks, Ring, BLOCK};

/// Lines as the kernel writes them (flashlog callers, 2026-11-17).
const LINES: [&str; 8] = [
    "camera: on in A#412 ms, module A#2, S2MPUs G#1F (1 fenced)",
    "pd: gating g2d after the jump",
    "lock held long: A#3 holds of 2^-7 s or more since the last line, the last t8123 for A#9812 us: syscall 13 op 2 thread 4",
    "kvault open ok in 15 ms: manifestus gen A#4211, A#812 values",
    "usb: tcpc role G#A cc G#14 power G#D usb switch G#9",
    "heartbeat: up 3600 s, commands from the USB process 812, slices C4-C8 [12, 9, 3, 0, 0]",
    "boot cold, flash log init true",
    "identity unlocked",
];

struct Mem {
    blocks: Vec<[u8; BLOCK]>,
    reads: u64,
    writes: u64,
}

impl Blocks for Mem {
    fn read(&mut self, block: u32, out: &mut [u8; BLOCK]) -> bool {
        self.reads += 1;
        out.copy_from_slice(&self.blocks[block as usize]);
        true
    }
    fn write(&mut self, block: u32, data: &[u8; BLOCK]) -> bool {
        self.writes += 1;
        self.blocks[block as usize] = *data;
        true
    }
}

fn main() {
    const N: usize = 1 << 13;
    let key = [7u8; 32];

    // The old line: format and copy into a block buffer.
    let mut tail = [0u8; BLOCK];
    let mut used = 0usize;
    let mut old_bytes = 0usize;
    let t = Instant::now();
    for k in 0..N {
        let line = format!("b{} t{} {}\n", 931, 1000 + k * 7, LINES[k % LINES.len()]);
        for &c in line.as_bytes() {
            if used == BLOCK {
                used = 0;
            }
            tail[used] = c;
            used += 1;
        }
        old_bytes += line.len();
        std::hint::black_box(&tail);
    }
    let old_ns = t.elapsed().as_nanos() as f64 / N as f64;

    // The entry: key, seal, build, hash.
    let mut docs = Vec::with_capacity(N);
    let mut prev = entry::genesis();
    let t = Instant::now();
    for k in 0..N {
        let sk = SealKey::new(&key);
        let place = Place { seq: k as u64, boot: 931, prev, time: Some(1 << 60) };
        let d = entry::build(&place, Class::System, &sk, &Payload::new(category::TEXT, "kernel", LINES[k % LINES.len()])).unwrap();
        prev = entry::hash_of_built(&d).unwrap();
        docs.push(d);
    }
    let new_ns = t.elapsed().as_nanos() as f64 / N as f64;
    let new_bytes: usize = docs.iter().map(|d| d.len()).sum();

    // Reading back: verify and open.
    let t = Instant::now();
    for d in &docs {
        let s = entry::parse(d).unwrap();
        std::hint::black_box(entry::open(&s, &SealKey::new(&key)).unwrap());
    }
    let open_ns = t.elapsed().as_nanos() as f64 / N as f64;

    // The ring on a RAM flash: the I/O an entry costs.
    let half = 1u32 << 10;
    let mut m = Mem { blocks: vec![[0u8; BLOCK]; 2 * half as usize], reads: 0, writes: 0 };
    let (mut r, _) = Ring::open(&mut m, half);
    let t = Instant::now();
    for k in 0..N {
        r.append(&mut m, 931, Some(1 << 60), Class::System, &SealKey::new(&key), &Payload::new(category::TEXT, "kernel", LINES[k % LINES.len()])).unwrap();
    }
    let ring_ns = t.elapsed().as_nanos() as f64 / N as f64;
    let (w, rd) = (m.writes, m.reads);
    let t = Instant::now();
    let (o, _) = Ring::open(&mut m, half);
    let open_ring_us = t.elapsed().as_micros();
    let open_reads = m.reads - rd;

    println!("per entry, A#{N} entries, host (release):");
    println!("  old line: A#{:.0} ns to format and copy, A#{:.1} bytes, 2 block writes", old_ns, old_bytes as f64 / N as f64);
    println!("  entry:    A#{:.0} ns to key, seal, build and hash, A#{:.1} bytes", new_ns, new_bytes as f64 / N as f64);
    println!("  read:     A#{:.0} ns to verify and open", open_ns);
    println!("  ring:     A#{:.0} ns an append with RAM I/O; A#{:.2} block writes and A#{:.2} block reads an entry", ring_ns, w as f64 / N as f64, rd as f64 / N as f64);
    println!("  open:     A#{open_ring_us} us, A#{open_reads} block reads to find the head of a ring of A#{} blocks a copy (next entry A#{})", half, o.next_seq());
    println!("  capacity: about A#{:.0} entries a 64 MiB copy (A#{} entries a 4 KiB block)", (1u64 << 26) as f64 / (new_bytes as f64 / N as f64), BLOCK * N / new_bytes);
}
