//! The panel's own brightness (DBV) and refresh rate slid live, over a white square about a centimetre across on black (a photodiode's patch; owner, 2026-11-08: "ideally on screen with a 1cm-ish square I can put this thing of white I can watch it and get you limits") or a one-pixel white crosshair (`--cross`; "gimme a 1px white crosshair on black so I can look for duty cycle"). The display draws it raw (op 52 mode 6 spans or mode 7, through the capture payload) and takes each DBV (op 6) and rate (op 7) as it comes; nothing else is drawn until the tool leaves. Needs a development display (a release build refuses op 52).
//!
//! Keys: ← → one DBV step, [ ] sixteen, ↓ ↑ two hundred fifty-six; h high brightness mode on and off (WRCTRLD G#53 = G#E0 / G#20, op 9); 1 120 Hz, 6 60 Hz (akita: HS60), 5 akita's NS60; q leaves.
//! On leaving: husky's display and map restarted (the server sets its own brightness and rate at start); akita's DBV back to 3175 and 60 Hz, the display released (a restart breaks akita's picture, docs/AK3B.md).
//!
//! Usage: tools/dbv-slider/target/release/dbv-slider [--cross] [size] [dbv]   (the square's side in pixels, default a centimetre: 192 on the Pro at 489 ppi, 169 on the 8a at 430; the DBV to start at, default the top of normal mode)
use std::io::{Read, Write};
use std::process::Command;

const REPO: &str = "/mnt/Harbor/Code/ferros";

fn bridge(args: &[&str]) -> String {
    Command::new(format!("{REPO}/target/debug/ferros-bridge")).args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

fn send(msg: &str) {
    bridge(&["spawn", &format!("{REPO}/target/el0/aarch64-unknown-none/release/capture.bin"), msg, "--use", "display"]);
}

fn le16(v: i64) -> String {
    let n = v as u16;
    format!("{:02X}{:02X}", n & 0xFF, n >> 8)
}

/// The device on USB: (akita?, width, height, a centimetre in pixels, the top of normal mode's DBV).
fn device() -> (bool, i64, i64, i64, i64) {
    let d = bridge(&["diag"]);
    if d.contains("Pixel 8a") { (true, 1080, 2400, 169, 3175) } else { (false, 1344, 2992, 192, 3307) }
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
    let cross = std::env::args().any(|a| a == "--cross");
    let nums: Vec<i64> = std::env::args().skip(1).filter_map(|s| s.parse().ok()).collect();
    let (akita, w, h, cm, top) = device();
    let side = nums.first().copied().unwrap_or(cm);
    let mut v = nums.get(1).copied().unwrap_or(top);
    let (cx, cy) = (w / 2, h / 2);
    if cross {
        send(&format!("3407{}{}", le16(cx), le16(cy)));
    } else {
        // The square as one white span a row (op 52 mode 6: [row][first][last] each u16).
        let (x0, y0) = (cx - side / 2, cy - side / 2);
        let mut m = String::from("3406");
        for y in y0..y0 + side {
            m += &format!("{}{}{}", le16(y), le16(x0), le16(x0 + side - 1));
        }
        send(&m);
    }
    let dbv = |v: i64| send(&format!("06{}", le16(v)));
    let rate = |fps: i64, byte: &str| {
        if akita { send(&format!("07{:02X}{byte}", fps)) } else { send(&format!("07{:02X}", fps)) }
    };
    dbv(v);
    let mut high = false;
    let mut hz = "as the server left it";
    let raw = Raw::on();
    let say = |v: i64, high: bool, hz: &str| {
        print!("\r\x1b[2K{}  DBV {v} (G#{v:03X}){}  {hz}", if akita { "8a" } else { "Pro" }, if high { "  high brightness mode" } else { "" });
        let _ = std::io::stdout().flush();
    };
    say(v, high, hz);
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
                send(if high { "0953E0" } else { "095320" });
            }
            [b'1'] => {
                rate(120, "00");
                hz = "120 Hz";
            }
            [b'6'] => {
                rate(60, "08");
                hz = if akita { "60 Hz (HS60)" } else { "60 Hz" };
            }
            [b'5'] if akita => {
                rate(60, "18");
                hz = "60 Hz (NS60)";
            }
            [b'q'] | [3] => break,
            _ => continue,
        }
        // WHY the clamp: the register is twelve bits; a key past either end stays at the end rather than wrapping to the other.
        v = v.clamp(0, (1 << 12) - 1);
        if v != was {
            dbv(v);
        }
        say(v, high, hz);
    }
    drop(raw);
    if akita {
        if high {
            send("095320");
        }
        rate(60, "08");
        dbv(top);
        send("3400");
        println!("\nDBV back to {top}, normal mode, 60 Hz, display released");
    } else {
        bridge(&["diag", "restart display map"]);
        println!("\ndisplay and map restarted: the server's own brightness and rate");
    }
}
