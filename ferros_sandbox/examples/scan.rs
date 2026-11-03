//! Scan a flat payload image (objcopy -O binary of its .text) and tally what the checker refuses: `cargo run -p ferros_sandbox --example scan -- <image.bin>...`.

use ferros_sandbox::{check, Refusal};

fn main() {
    for path in std::env::args().skip(1) {
        let code = std::fs::read(&path).expect("read image");
        let mut tally: Vec<(Refusal, usize, u32, usize)> = Vec::new();
        let mut total = 0;
        // The image header (a pad word at 4, "ferrosRT" at 8, five words to 56) is data the entry branches over; check_image steps over it, and so does this tally.
        let header = code.len() >= 56 && &code[8..16] == b"ferrosRT";
        for (k, w) in code.chunks_exact(4).enumerate() {
            if header && (1..14).contains(&k) {
                continue;
            }
            let insn = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
            total += 1;
            if let Err(why) = check(insn) {
                match tally.iter_mut().find(|t| t.0 == why) {
                    Some(t) => t.1 += 1,
                    None => tally.push((why, 1, insn, 4 * k)),
                }
            }
        }
        let bad: usize = tally.iter().map(|t| t.1).sum();
        println!("{path}: {total} words, {bad} refused");
        tally.sort_by(|a, b| b.1.cmp(&a.1));
        for (why, n, first, at) in tally {
            println!("  {n:6}  {why:?}  (first: {first:08x} at +{at:#x})");
        }
    }
}
