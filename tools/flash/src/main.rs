//! ferros-flash CLI, over images of a disk's two ends (lib.rs). It never touches a device: tools/pixel-carve.sh reads the ends off the phone and writes back the blocks this prints.
//!   ferros-flash show  <head> <tail> <tail-lba> [block]
//!   ferros-flash carve <head> <tail> <tail-lba> <userdata-GiB> <out-dir> [block]
//! `carve` writes <out-dir>/head.bin and tail.bin (the images with the new table), reads them back as a disk would, and prints the block ranges to write as `write <first-lba> <blocks>` lines, the primary's first.

use ferros_flash::Gpt;
use std::process::exit;

fn die(msg: impl core::fmt::Display) -> ! {
    eprintln!("ferros-flash: {msg}");
    exit(1)
}

fn read(path: &str) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| die(format!("{path}: {e}")))
}

fn num(s: &str) -> u64 {
    s.parse().unwrap_or_else(|_| die(format!("not a number: {s}")))
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("show") if a.len() >= 5 => {
            let block = a.get(5).map_or(4096, |s| num(s) as usize);
            let g = Gpt::read(block, &read(&a[2]), &read(&a[3]), num(&a[4])).unwrap_or_else(|e| die(e));
            print!("{}", g.table());
        }
        Some("carve") if a.len() >= 7 => {
            let block = a.get(7).map_or(4096, |s| num(s) as usize);
            let (mut head, mut tail, tail_lba) = (read(&a[2]), read(&a[3]), num(&a[4]));
            let mut g = Gpt::read(block, &head, &tail, tail_lba).unwrap_or_else(|e| die(e));
            g.carve(num(&a[5]) << 30).unwrap_or_else(|e| die(e));
            g.apply(&mut head, &mut tail, tail_lba);
            // Read back from the images as the bootloader will: both CRCs, both copies the same.
            let back = Gpt::read(block, &head, &tail, tail_lba).unwrap_or_else(|e| die(format!("the new table does not read back: {e}")));
            let out = &a[6];
            std::fs::create_dir_all(out).unwrap_or_else(|e| die(e));
            std::fs::write(format!("{out}/head.bin"), &head).unwrap_or_else(|e| die(e));
            std::fs::write(format!("{out}/tail.bin"), &tail).unwrap_or_else(|e| die(e));
            eprint!("{}", back.table());
            for w in g.writes() {
                println!("write {} {}", w.lba, w.bytes.len() / block);
            }
        }
        _ => die("usage: show <head> <tail> <tail-lba> [block] | carve <head> <tail> <tail-lba> <userdata-GiB> <out-dir> [block]"),
    }
}
