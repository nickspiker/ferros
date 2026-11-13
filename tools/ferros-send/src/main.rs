//! `ferros send <kist.capsule.vsf>`: stow a kist on a ferros device over USB (docs/KISTS.md "Stowing"; owner, 2026-11-11: "how do we install kists over USB? Can we make a standard method so the end user doesn't have to get them from the [catalog]", "Just seems to me that we should just make a proper checked path and start using it", and "the proper term for installing a kist is stow").
//!
//! The device does the checking, as it checks a kist from the directory: every egg of its signature against its developer's keys and the person's trust in them, then the kernel's gate, the sandbox checker and the seal. Nothing is stowed until the person says Stow on the device's screen. This tool carries the capsule in, offers it, follows the device's checks, and lets the USB link go once the offer waits on the person. It holds no development command (the capability it uses, `ferros.kist.offer`, is one a production build answers too).
//!
//! **It never holds the link while the person decides** (owner, 2026-11-12: a stow left waiting on the Pro while the owner was in Mahere held the device's USB interface, and every other host tool was shut out). The claim on the interface is exclusive, and nothing the device does when the person answers reaches a host that holds no claim, so the only waits with no claim and no timer are the person's own: `ferros send` ends at "waiting on the person" (exit 3) with the link released, and `ferros sent <capsule>` asks the device once, at once, how that offer stands (the device keeps an offer's end until another offer replaces it). Meanwhile the device's Deck button is red (docs/NOTIFY.md "System questions"), and one tap on it opens the offer. A `ferros send` of a capsule the device is still asking about carries nothing again: it says so.
//!
//! The device: the first ferros device on USB (VID G#1209, PID G#4665), the one on bus `FERROS_BUS` (lsusb's number), or the one in socket `FERROS_PORT` (sysfs's `1-2.4`) when several are plugged in. Exit 0 stowed, 1 refused or failed, 2 cancelled, 3 waiting on the person (the link let go).
//!
//! The wire (docs/USB.md "Offering a kist", ferros_pt `caps::OFFER`, ferros_kist::offer): parts of 4 MiB (Write), the offer (Exec), then the offer's state read and held by the device until it changes (Read), each answer printed as it comes, until it asks the person or ends.

use ferros_kist::offer;
use ferros_kist::tatau::{Base, Fmt, Style};
use ferros_link::{pt, usb::UsbLink};
use ferros_pt::command::caps::OFFER;
use ferros_pt::Op;
use std::io::Write;
use std::process::exit;

fn usage() -> ! {
    eprintln!("Usage: ferros send <kist.capsule.vsf>");
    eprintln!("       ferros sent <kist.capsule.vsf>");
    eprintln!();
    eprintln!("send: offers a kist to the ferros device on USB and lets the link go once it waits on the person, who stows or cancels it on the device's screen (its Deck button turns red: a tap opens it).");
    eprintln!("sent: how that offer stands now: stowed, refused, cancelled, or still waiting.");
    eprintln!("Exit 0 stowed, 1 refused or failed, 2 cancelled, 3 waiting on the person.");
    eprintln!("FERROS_BUS=<bus number> or FERROS_PORT=<bus>-<port path> picks the device when more than one is plugged in.");
    exit(1);
}

/// The exit codes.
const STOWED: i32 = 0;
const FAILED: i32 = 1;
const CANCELLED: i32 = 2;
const WAITING: i32 = 3;

/// The offer of `address`'s state on the device: [state, why, flags]. The device answers at once when its state is not `seen`, else holds the answer until it changes or its lease (`offer::HOLD_SECONDS`) ends. `u8::MAX` is no state, answered at once.
async fn state(link: &UsbLink, address: &[u8; 32], seen: u8) -> [u8; 3] {
    let mut p = Vec::with_capacity(33);
    p.extend_from_slice(address);
    p.push(seen);
    let r = pt::call(link, &pt::command(OFFER, Op::Read, &p)).await.unwrap_or_else(|e| {
        eprintln!("the link failed: {e}");
        exit(FAILED);
    });
    match r[..] {
        [s, w, f, ..] => [s, w, f],
        _ => {
            eprintln!("the device's answer does not read ({} bytes)", r.len());
            exit(FAILED);
        }
    }
}

/// A state said, and the exit code it ends with (None: not ended, the device still working on it).
fn tell(state: u8, why: u8, flags: u8) -> Option<i32> {
    match state {
        offer::OFFERED => say("waiting for the device's stower to take it"),
        offer::CHECKING => say("the device is checking it: its signature, then its code"),
        offer::ASKING => {
            if flags & offer::NATIVE != 0 {
                say("unchecked code, development build only");
            }
            say("waiting for the person to say Stow or Cancel on the device: its Deck button is red, and a tap on it opens the offer");
            say("the USB link is free meanwhile; `ferros sent <capsule>` says how it ended");
            return Some(WAITING);
        }
        offer::STOWED => {
            say("stowed");
            return Some(STOWED);
        }
        offer::REFUSED => {
            say(&format!("refused: {}", offer::words(why)));
            return Some(FAILED);
        }
        offer::CANCELLED => {
            say("cancelled");
            return Some(CANCELLED);
        }
        offer::GONE => {
            say("refused: the device has no such offer (another replaced it, the device restarted, or the person left)");
            return Some(FAILED);
        }
        _ => {
            say(&format!("refused: the device answered a state this tool does not know ({state})"));
            return Some(FAILED);
        }
    }
    None
}

/// The offer followed from state `seen` until it asks the person or ends: each state said as the device tells it (it holds each read until the state changes, so nothing here waits on a timer of its own). The exit code.
async fn follow(link: &UsbLink, address: &[u8; 32], mut seen: u8) -> i32 {
    loop {
        let [s, why, flags] = state(link, address, seen).await;
        if s != seen {
            seen = s;
            if let Some(code) = tell(s, why, flags) {
                return code;
            }
        }
    }
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

/// A line said and flushed at once, whatever stdout is (a pipe, a file, a terminal): a person or a script watching it sees each stage as it happens.
fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// A command's plain answer: Ok for [0], else the device's words after its status.
fn plain(r: &[u8]) -> Result<(), String> {
    match r.split_first() {
        Some((0, _)) => Ok(()),
        // A part the device's vault did not keep comes back as its status alone (2026-11-12, Photon on the 8a: "not sent: " and nothing after it).
        Some((s, [])) => Err(format!("the device answered status A#{s} and no words (`ferros-bridge diag log` says which step)")),
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
    let (sent, path) = match (args.get(1).map(String::as_str), args.get(2), args.len()) {
        (Some("send"), Some(p), 3) => (false, p.clone()),
        (Some("sent"), Some(p), 3) => (true, p.clone()),
        _ => usage(),
    };
    let capsule = std::fs::read(&path).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        exit(FAILED);
    });
    // What it says it is, read from its own first bytes before anything is sent (the device checks it all again, and more).
    let decl = ferros_kist::declaration(&capsule).unwrap_or_else(|e| {
        eprintln!("{path}: not a kist ({e})");
        exit(FAILED);
    });
    let mut what = decl.name.clone();
    if let Ok(Some(v)) = ferros_kist::version_of(&capsule) {
        what.push(' ');
        let _ = ferros_kist::write_version(&mut what, fmt(), &v.0);
    }
    let address = *blake3::hash(&capsule).as_bytes();
    let len = capsule.len() as u64;
    let link = UsbLink::open().await.unwrap_or_else(|e| {
        eprintln!("{e}");
        exit(FAILED);
    });

    // How the device has this capsule's offer now, answered at once: `sent` says it and ends; `send` carries nothing again for an offer the device still has under way.
    let [s, why, flags] = state(&link, &address, u8::MAX).await;
    if sent {
        say(&format!("{what}: the device's offer of it is {}", offer::state_name(s)));
        let code = tell(s, why, flags).unwrap_or(WAITING);
        drop(link);
        exit(code);
    }
    if !offer::finished(s) {
        say(&format!("{what} is offered already ({}): nothing is carried again", offer::state_name(s)));
        let code = match tell(s, why, flags) {
            Some(code) => code,
            None => follow(&link, &address, s).await,
        };
        drop(link);
        exit(code);
    }
    say(&format!("offering {what} ({}) to the device", size(len)));

    // The parts, in order, each kept by the device before the next goes; each said as the device keeps it (a part is a few seconds: the device writes it to its vault before it answers).
    let ps = parts(&address, &capsule);
    let n = ps.len();
    for (k, (at, p)) in ps.into_iter().enumerate() {
        let part_len = p.len() as u64 - 41;
        let r = pt::call_held(&link, &pt::command(OFFER, Op::Write, &p)).await.unwrap_or_else(|e| {
            eprintln!("the link failed: {e}");
            exit(FAILED);
        });
        if let Err(w) = plain(&r) {
            eprintln!("not sent: {w}");
            if r.first() == Some(&2) {
                eprintln!("(another kist waits on the person: the device's Deck button is red, and a tap on it opens that offer; Stow or Cancel it there first)");
            }
            exit(FAILED);
        }
        say(&format!("  part {} of {n} kept: {} of {}", k + 1, size(at + part_len), size(len)));
    }

    let mut p = Vec::with_capacity(40);
    p.extend_from_slice(&address);
    p.extend_from_slice(&len.to_le_bytes());
    let r = pt::call(&link, &pt::command(OFFER, Op::Exec, &p)).await.unwrap_or_else(|e| {
        eprintln!("the link failed: {e}");
        exit(FAILED);
    });
    if let Err(w) = plain(&r) {
        eprintln!("not offered: {w}");
        exit(FAILED);
    }
    say("offered");

    // Followed while the device works on it (its stower taking it, its checks), then the link let go as soon as it asks the person, or ends.
    let code = follow(&link, &address, u8::MAX).await;
    drop(link);
    exit(code);
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
        assert!(plain(&[4]).is_err_and(|w| w.contains("A#4")));
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
        // The link is let go at the person's question: asking ends this tool's part (waiting), the device's own work does not.
        assert_eq!(tell(offer::ASKING, 0, 0), Some(WAITING));
        assert_eq!(tell(offer::OFFERED, 0, 0), None);
        assert_eq!(tell(offer::CHECKING, 0, 0), None);
        assert_eq!(tell(offer::STOWED, 0, 0), Some(STOWED));
        assert_eq!(tell(offer::CANCELLED, 0, 0), Some(CANCELLED));
        assert_eq!(tell(offer::REFUSED, 1, 0), Some(FAILED));
        assert_eq!(tell(offer::GONE, 0, 0), Some(FAILED));
        for w in 1..=offer::why::NATIVE_HERE {
            assert_ne!(offer::words(w), "something went wrong", "reason {w}");
            assert!(!offer::words(w).contains('\u{2014}'));
        }
    }
}
