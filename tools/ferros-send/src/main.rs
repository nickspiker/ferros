//! `ferros send <kist.capsule.vsf>`: stow a kist on a ferros device over USB (docs/KISTS.md "Stowing"; owner, 2026-11-11: "how do we install kists over USB? Can we make a standard method so the end user doesn't have to get them from the [catalog]", "Just seems to me that we should just make a proper checked path and start using it", and "the proper term for installing a kist is stow").
//!
//! The device does the checking, as it checks a kist from the directory: every egg of its signature against its developer's keys and the person's trust in them, then the kernel's gate, the sandbox checker and the seal. Nothing is stowed until the person says Stow on the device's screen. This tool carries the capsule in, offers it, and prints how it ended: `stowed`, `refused: <why>` or `cancelled`. It holds no development command (the capability it uses, `ferros.kist.offer`, is one a production build answers too).
//!
//! The device: the first ferros device on USB (VID G#1209, PID G#4665), or the one on bus `FERROS_BUS` (lsusb's number) when several are plugged in. Exit 0 stowed, 1 refused or failed, 2 cancelled.
//!
//! The wire (docs/USB.md "Offering a kist", ferros_pt `caps::OFFER`, ferros_kist::offer): parts of 4 MiB (Write), the offer (Exec), then the offer's state read and held by the device until it changes (Read), each answer printed as it comes.

use ferros_kist::offer;
use ferros_kist::tatau::{Base, Fmt, Style};
use ferros_link::{pt, usb::UsbLink};
use ferros_pt::command::caps::OFFER;
use ferros_pt::Op;
use std::io::Write;
use std::process::exit;

fn usage() -> ! {
    eprintln!("Usage: ferros send <kist.capsule.vsf>");
    eprintln!();
    eprintln!("Offers a kist to the ferros device on USB; the person stows or cancels it on the device's screen.");
    eprintln!("FERROS_BUS=<bus number> picks the device when more than one is plugged in.");
    exit(1);
}

/// Numbers as a terminal reads them: decimal, words for units.
fn fmt() -> Fmt {
    Fmt::new(Base::Arabic, Style::Words)
}

fn size(bytes: u64) -> String {
    let mut s = String::new();
    let _ = ferros_kist::tatau::size_bits(&mut s, fmt(), bytes << 3, 1);
    s
}

/// A command's plain answer: Ok for [0], else the device's words after its status.
fn plain(r: &[u8]) -> Result<(), String> {
    match r.split_first() {
        Some((0, _)) => Ok(()),
        Some((_, words)) => Err(String::from_utf8_lossy(words).into_owned()),
        None => Err("the device gave no answer".into()),
    }
}

/// The capsule's Write parts, in order, each with the offset it starts at: [address 32][offset u64 LE][more u8][bytes], `more` 0 on the last alone (the device offers nothing it has not had whole).
fn parts(address: &[u8; 32], capsule: &[u8]) -> Vec<(u64, Vec<u8>)> {
    let n = capsule.len().div_ceil(offer::PART);
    capsule
        .chunks(offer::PART)
        .enumerate()
        .map(|(k, part)| {
            let at = (k * offer::PART) as u64;
            let mut p = Vec::with_capacity(41 + part.len());
            p.extend_from_slice(address);
            p.extend_from_slice(&at.to_le_bytes());
            p.push((k + 1 < n) as u8);
            p.extend_from_slice(part);
            (at, p)
        })
        .collect()
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = match (args.get(1).map(String::as_str), args.get(2), args.len()) {
        (Some("send"), Some(p), 3) => p.clone(),
        _ => usage(),
    };
    let capsule = std::fs::read(&path).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        exit(1);
    });
    // What it says it is, read from its own first bytes before anything is sent (the device checks it all again, and more).
    let decl = ferros_kist::declaration(&capsule).unwrap_or_else(|e| {
        eprintln!("{path}: not a kist ({e})");
        exit(1);
    });
    let mut what = decl.name.clone();
    if let Ok(Some(v)) = ferros_kist::version_of(&capsule) {
        what.push(' ');
        let _ = ferros_kist::write_version(&mut what, fmt(), &v.0);
    }
    let address = *blake3::hash(&capsule).as_bytes();
    let len = capsule.len() as u64;
    let link = UsbLink::open().unwrap_or_else(|e| {
        eprintln!("{e}");
        exit(1);
    });
    println!("offering {what} ({}) to the device", size(len));

    // The parts, in order, each kept by the device before the next goes.
    for (at, p) in parts(&address, &capsule) {
        let part_len = p.len() as u64 - 41;
        let r = pt::call(&link, &pt::command(OFFER, Op::Write, &p)).await.unwrap_or_else(|e| {
            eprintln!("\nthe link failed: {e}");
            exit(1);
        });
        if let Err(w) = plain(&r) {
            eprintln!("\nnot sent: {w}");
            exit(1);
        }
        eprint!("\r  {} of {} sent", size(at + part_len), size(len));
        let _ = std::io::stderr().flush();
    }
    eprintln!();

    let mut p = Vec::with_capacity(40);
    p.extend_from_slice(&address);
    p.extend_from_slice(&len.to_le_bytes());
    let r = pt::call(&link, &pt::command(OFFER, Op::Exec, &p)).await.unwrap_or_else(|e| {
        eprintln!("the link failed: {e}");
        exit(1);
    });
    if let Err(w) = plain(&r) {
        eprintln!("not offered: {w}");
        exit(1);
    }

    // Its state, read again each time it changes: the device holds each read until it does (or for a few seconds, then answers it unchanged), so nothing here waits on a timer of its own.
    let mut seen = u8::MAX;
    loop {
        let mut p = Vec::with_capacity(33);
        p.extend_from_slice(&address);
        p.push(seen);
        let r = pt::call(&link, &pt::command(OFFER, Op::Read, &p)).await.unwrap_or_else(|e| {
            eprintln!("the link failed: {e}");
            exit(1);
        });
        let [state, why, flags] = match r[..] {
            [s, w, f, ..] => [s, w, f],
            _ => {
                eprintln!("the device's answer does not read ({} bytes)", r.len());
                exit(1);
            }
        };
        if state != seen {
            seen = state;
            match state {
                offer::OFFERED => println!("offered; waiting for the device's installer"),
                offer::CHECKING => println!("the device is checking it: its signature, then its code"),
                offer::ASKING => {
                    if flags & offer::NATIVE != 0 {
                        println!("unchecked code, development build only");
                    }
                    println!("on the device's screen: Stow or Cancel");
                }
                offer::STOWED => {
                    println!("stowed");
                    exit(0);
                }
                offer::REFUSED => {
                    println!("refused: {}", offer::words(why));
                    exit(1);
                }
                offer::CANCELLED => {
                    println!("cancelled");
                    exit(2);
                }
                offer::GONE => {
                    println!("refused: the offer is gone (the device restarted, or the person left)");
                    exit(1);
                }
                _ => {
                    println!("refused: the device answered a state this tool does not know ({state})");
                    exit(1);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each part is read by the kernel as [address 32][offset u64 LE][more u8][bytes] (service.rs, the `ferros.kist.offer` Write): the offsets run on without a gap, and only the last says no more.
    #[test]
    fn parts_frame_as_the_kernel_reads_them() {
        let address = [7u8; 32];
        for len in [1, offer::PART - 1, offer::PART, offer::PART + 1, 3 * offer::PART] {
            let capsule: Vec<u8> = (0..len).map(|i| i as u8).collect();
            let ps = parts(&address, &capsule);
            assert_eq!(ps.len(), len.div_ceil(offer::PART), "{len}");
            let mut next = 0u64;
            let mut whole = Vec::new();
            for (k, (at, p)) in ps.iter().enumerate() {
                assert_eq!(&p[..32], &address);
                assert_eq!(u64::from_le_bytes(p[32..40].try_into().unwrap()), *at);
                assert_eq!(*at, next);
                assert_eq!(p[40], (k + 1 < ps.len()) as u8);
                assert!(p.len() > 41 && p.len() - 41 <= offer::PART);
                next += (p.len() - 41) as u64;
                whole.extend_from_slice(&p[41..]);
            }
            assert_eq!(whole, capsule);
        }
    }

    #[test]
    fn plain_answers() {
        assert_eq!(plain(&[0]), Ok(()));
        assert_eq!(plain(&[1, b'n', b'o']), Err("no".into()));
        assert!(plain(&[]).is_err());
    }

    /// Every state the device answers either ends the wait or is one this tool says, and every reason has words.
    #[test]
    fn states_and_reasons() {
        for s in [offer::STOWED, offer::REFUSED, offer::CANCELLED, offer::GONE] {
            assert!(offer::finished(s));
        }
        for s in [offer::OFFERED, offer::CHECKING, offer::ASKING] {
            assert!(!offer::finished(s));
        }
        for w in 1..=offer::why::NATIVE_HERE {
            assert_ne!(offer::words(w), "something went wrong", "reason {w}");
            assert!(!offer::words(w).contains('\u{2014}'));
        }
    }
}
