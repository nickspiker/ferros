//! The panel's own brightness (DBV) slid live, over a one-pixel white crosshair on black, for looking at the emission's duty (owner, 2026-11-08: "slider for brightness and gimme a 1px white crosshair on black so I can look for duty cycle"). The display draws the crosshair raw (op 52 mode 7, through the capture payload) and takes each DBV as it comes (op 6); nothing else is drawn until the tool leaves.
//!
//! Keys: ← → one DBV step, [ ] sixteen, ↓ ↑ two hundred fifty-six; h high brightness mode on and off (WRCTRLD G#53 = G#E0 / G#20, op 9); q leaves: the DBV the display server set at start back (akita's 3175, normal mode) and the display released.
//!
//! Usage: tools/dbv-slider/target/release/dbv-slider [x y] [dbv]   (the crosshair's column and row; default akita's middle, 539 1199; the DBV to start at, default 3175)
use std::io::{Read, Write};
use std::process::Command;

const REPO: &str = "/mnt/Harbor/Code/ferros";
/// akita's top of normal mode (payloads/display `AK3B_DBV_TOP`), where the display server holds it.
const DBV_HOME: i64 = 3175;

fn send(msg: &str) {
    let _ = Command::new(format!("{REPO}/target/debug/ferros-bridge"))
        .args(["spawn", &format!("{REPO}/target/el0/aarch64-unknown-none/release/capture.bin"), msg, "--use", "display"])
        .output();
}

fn le16(v: i64) -> String {
    let n = v as u16;
    format!("{:02X}{:02X}", n & 0xFF, n >> 8)
}

fn dbv(v: i64) {
    send(&format!("06{}", le16(v)));
}

fn hbm(on: bool) {
    send(if on { "0953E0" } else { "095320" });
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
    let nums: Vec<i64> = std::env::args().skip(1).filter_map(|s| s.parse().ok()).collect();
    let (x, y) = if nums.len() >= 2 { (nums[0], nums[1]) } else { (539, 1199) };
    let mut v = if nums.len() >= 3 { nums[2] } else { DBV_HOME };
    let mut high = false;
    send(&format!("3407{}{}", le16(x), le16(y)));
    dbv(v);
    let raw = Raw::on();
    let say = |v: i64, high: bool| {
        print!("\r\x1b[2KDBV {v} (G#{v:03X}){}", if high { "  high brightness mode" } else { "" });
        let _ = std::io::stdout().flush();
    };
    say(v, high);
    let mut stdin = std::io::stdin();
    let mut b = [0u8; 3];
    loop {
        let n = stdin.read(&mut b).unwrap_or(0);
        if n == 0 {
            break;
        }
        let was = v;
        match &b[..n] {
            [27, 91, 67] => v += 1,
            [27, 91, 68] => v -= 1,
            [b']'] => v += 1 << 4,
            [b'['] => v -= 1 << 4,
            [27, 91, 65] => v += 1 << 8,
            [27, 91, 66] => v -= 1 << 8,
            [b'h'] => {
                high = !high;
                hbm(high);
            }
            [b'q'] | [3] => break,
            _ => continue,
        }
        // WHY the clamp: the register is twelve bits; a key past either end stays at the end rather than wrapping to the other.
        v = v.clamp(0, (1 << 12) - 1);
        if v != was {
            dbv(v);
        }
        say(v, high);
    }
    drop(raw);
    if high {
        hbm(false);
    }
    dbv(DBV_HOME);
    send("3400");
    println!("\nDBV back to {DBV_HOME}, normal mode, display released");
}
