//! oracle-decode: captures taken under Android decoded into what they carry, the Rust of the Python decoders ferros was brought up with (owner, 2026-11-21: no Python), same inputs, same output.
//!
//! - `oracle-decode spi < capture.log`: an hv register-oracle capture of an Exynos (s3c64xx family) SPI controller as SPI transactions, `T0001 W 87 | R 00 ...` (tools/oracle-decode-spi.py; docs/oracle/TOUCH.md).
//! - `oracle-decode modem-ring [--data-head] < capture.log`: the hv's modem boot-channel trace (FERROS_HV_MODEM_RING) as the boot protocol, data runs summarised (tools/oracle-decode-modem-ring.py; docs/oracle/MODEM-BOOT-RECON.md).
//! - `oracle-decode usf <capture.txt> [--raw] [--only tx|rx] [--skip-streams]`: an ftrace of the usf_tx / usf_rx kprobes as the AoC sensor service's messages, each FlatBuffer walked without a schema (tools/usf-decode.py; docs/SENSORS.md "Capturing USF").
//! - `oracle-decode dsi <root>`: every `<root>/<state>/dsi-trace.txt` (an ftrace of the panel driver's DSI traffic) as `<root>/<state>/dsi-commands.txt`, a command list in ms from the first event (oracle/display/husky-hk3/tools/decode.py; that directory's README).

mod dsi;
mod modem;
mod spi;
mod usf;

use std::process::exit;

fn die(s: &str) -> ! {
    eprintln!("oracle-decode: {s}");
    exit(2)
}

/// A file as text, bad bytes replaced (as Python's `errors="replace"`).
fn read_text(path: &str) -> String {
    String::from_utf8_lossy(&std::fs::read(path).unwrap_or_else(|e| die(&format!("{path}: {e}")))).into_owned()
}

/// Standard input as text, bad bytes replaced.
fn stdin_text() -> String {
    let mut b = Vec::new();
    std::io::Read::read_to_end(&mut std::io::stdin(), &mut b).unwrap_or_else(|e| die(&format!("stdin: {e}")));
    String::from_utf8_lossy(&b).into_owned()
}

/// Bytes as lowercase hex, `sep` between them.
fn hex(b: &[u8], sep: &str) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(sep)
}

/// A run of `n` hex digits (either case) at the start of `s`, exactly `n` when `n` is given, else as many as there are (at least one); its value and the rest.
fn hex_run(s: &str, n: Option<usize>) -> Option<(u64, &str)> {
    let len = s.bytes().take_while(u8::is_ascii_hexdigit).count();
    let take = match n {
        Some(n) if len >= n => n,
        None if len > 0 => len,
        _ => return None,
    };
    Some((u64::from_str_radix(&s[..take], 16).ok()?, &s[take..]))
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        die("spi | modem-ring | usf | dsi (see the source's header)");
    }
    let cmd = args.remove(0);
    match cmd.as_str() {
        "spi" => spi::run(),
        "modem-ring" => modem::run(&args),
        "usf" => usf::run(&args),
        "dsi" => dsi::run(&args),
        _ => die(&format!("unknown command {cmd}")),
    }
}
