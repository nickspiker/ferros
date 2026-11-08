//! The camera cutout mapped row by row on the glass (owner, 2026-11-08: "right arrow until it disappears then go to the next line and repeat"; the glass's pixels are on or off). The display draws one white pixel on black (op 52 mode 3, through the capture payload, about 40 ms a step); the keys move it, and the person marks where it disappears and where it comes back on each row.
//!
//! Keys: ← → one pixel, [ ] five; ↑ ↓ a row; space marks (first the first hidden pixel, then the first visible one after it); enter keeps the row and goes down one, starting a few pixels left of the last left edge; u forgets the row's marks; q prints the map and leaves (the display released).
//!
//! Usage: tools/cutout-scan/target/release/cutout-scan [x y]   (the start, panel pixels; default above akita's cutout)
use std::io::{Read, Write};
use std::process::Command;

const REPO: &str = "/mnt/Harbor/Code/ferros";

fn send(msg: &str) {
    let _ = Command::new(format!("{REPO}/target/debug/ferros-bridge"))
        .args(["spawn", &format!("{REPO}/target/el0/aarch64-unknown-none/release/capture.bin"), msg, "--use", "display"])
        .output();
}

fn u16hex(v: i64) -> String {
    let n = (v * 16) as u16;
    format!("{:02X}{:02X}", n & 0xFF, n >> 8)
}

fn dot(x: i64, y: i64) {
    send(&format!("3403{}{}", u16hex(x), u16hex(y)));
}

/// The terminal in raw mode for the keys, put back on the way out.
struct Raw(libc::termios);
impl Raw {
    fn on() -> Raw {
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            libc::tcgetattr(0, &mut t);
            let old = t;
            libc::cfmakeraw(&mut t);
            libc::tcsetattr(0, libc::TCSANOW, &t);
            Raw(old)
        }
    }
}
impl Drop for Raw {
    fn drop(&mut self) {
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &self.0) };
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (mut x, mut y): (i64, i64) = if a.len() >= 3 { (a[1].parse().unwrap(), a[2].parse().unwrap()) } else { (518, 48) };
    // Rows kept: (row, first hidden column, last hidden column).
    let mut rows: Vec<(i64, i64, i64)> = Vec::new();
    let mut marks: Vec<i64> = Vec::new();
    let raw = Raw::on();
    let say = |s: String| {
        print!("\r\x1b[2K{s}");
        let _ = std::io::stdout().flush();
    };
    dot(x, y);
    say(format!("dot at ({x}, {y})  marks {marks:?}  rows kept {}", rows.len()));
    let mut stdin = std::io::stdin();
    let mut b = [0u8; 3];
    loop {
        let n = stdin.read(&mut b).unwrap_or(0);
        if n == 0 {
            break;
        }
        match &b[..n] {
            [27, 91, 67] => x += 1,
            [27, 91, 68] => x -= 1,
            [27, 91, 65] => y -= 1,
            [27, 91, 66] => y += 1,
            [b']'] => x += 5,
            [b'['] => x -= 5,
            [b' '] => {
                if marks.len() < 2 {
                    marks.push(x);
                }
            }
            [b'u'] => marks.clear(),
            [b'\r'] | [b'\n'] => {
                if marks.len() == 2 {
                    rows.push((y, marks[0], marks[1] - 1));
                }
                let restart = marks.first().copied().unwrap_or(x) - 4;
                marks.clear();
                y += 1;
                x = restart;
            }
            [b'q'] | [3] => break,
            _ => continue,
        }
        dot(x, y);
        say(format!("dot at ({x}, {y})  marks {marks:?}  rows kept {}", rows.len()));
    }
    drop(raw);
    send("3400");
    println!("\nThe cutout, row by row (row: first hidden column ..= last hidden column):");
    for (r, l, rr) in &rows {
        println!("{r}: {l}..={rr}");
    }
}
