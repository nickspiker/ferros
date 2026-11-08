//! The camera cutout mapped on the glass (owner, 2026-11-08: the glass's pixels are on or off; he marks a solid area that is completely obscured). The display draws one white pixel on black (op 52 mode 3, through the capture payload, about 40 ms a step); the keys move it and space logs where it is.
//!
//! Keys: ← → ↑ ↓ one pixel, [ ] five across; space logs the dot's (x, y) (owner, 2026-11-08: "Strictly log the xy when I press space"); q prints every logged point and leaves (the display released).
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
    let mut points: Vec<(i64, i64)> = Vec::new();
    // Every logged point also goes to a file as it is taken, so a terminal that loses its scrollback loses nothing (2026-11-08: a corner's scan vanished with a click).
    let _ = std::fs::create_dir_all("/mnt/Harbor/tmp/cutout-scan");
    // Named by the first number not yet taken (scan-0, scan-1, …): no clock in it.
    let log_path = (0..).map(|n| format!("/mnt/Harbor/tmp/cutout-scan/scan-{n}.txt")).find(|p| !std::path::Path::new(p).exists()).unwrap();
    let mut log = std::fs::OpenOptions::new().create(true).append(true).open(&log_path).ok();
    println!("points also go to {log_path}");
    let raw = Raw::on();
    let say = |s: String| {
        print!("\r\x1b[2K{s}");
        let _ = std::io::stdout().flush();
    };
    dot(x, y);
    say(format!("dot at ({x}, {y})  points logged 0"));
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
                points.push((x, y));
                print!("\r\x1b[2Klogged {x} {y}\r\n");
                if let Some(f) = log.as_mut() {
                    let _ = writeln!(f, "{x} {y}");
                    let _ = f.flush();
                }
                say(format!("dot at ({x}, {y})  logged ({x}, {y}), points logged {}", points.len()));
                continue;
            }
            [b'q'] | [3] => break,
            _ => continue,
        }
        dot(x, y);
        say(format!("dot at ({x}, {y})  points logged {}", points.len()));
    }
    drop(raw);
    send("3400");
    println!("\nPoints logged (x, y):");
    for (x, y) in &points {
        println!("{x} {y}");
    }
}
