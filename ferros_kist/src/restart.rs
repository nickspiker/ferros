//! Restarts (owner, 2026-11-21, after the deck panicked on the Pro and nothing started it again: "keyboard, speaker, whatever you think needs to be on the restart list. Not sure if there's a proper crashloop detect too but your call there.").
//!
//! - **The kernel starts a kept process again at its death** (a panic or a fault), at once, from its capsule (docs/KERNEL.md "Restarts").
//! - **A crash loop stops it:** the `LOOP_DEATHS`th death within `WINDOW_S` seconds of the first of them is not answered; it is logged, and the person gets a note that says so. Timestamps compared at the death, never a timer. A restart by hand (`diag restart`) clears the count.
//! - **The person sees it:** a note from the system's restarts source (`note::RESTARTS`), one a process (`notice_key`), its words a line (`notice_words`).
use alloc::string::String;

/// Deaths that stop the restarts when they all fall within `WINDOW_S` seconds: 2^2.
pub const LOOP_DEATHS: usize = 1 << 2;
/// The window, seconds: 2^5.
pub const WINDOW_S: u64 = 1 << 5;

/// One process's last deaths (counter ticks), a ring of `LOOP_DEATHS`, and whether its restarts stopped.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Deaths {
    at: [u64; LOOP_DEATHS],
    /// Deaths recorded since the last clear; the next is written at `n & (LOOP_DEATHS - 1)`.
    n: u64,
    pub stopped: bool,
}

impl Deaths {
    pub const fn new() -> Self {
        Deaths { at: [0; LOOP_DEATHS], n: 0, stopped: false }
    }

    /// A death at `now` (counter ticks, `freq` a second). True when it is a crash loop: the `LOOP_DEATHS`th death within `WINDOW_S` seconds of the first of them; `stopped` is set then, and stays until `clear`.
    /// After the write, slot `n & (LOOP_DEATHS - 1)` holds the oldest of the last `LOOP_DEATHS` (the ring has gone round once by then).
    pub fn died(&mut self, now: u64, freq: u64) -> bool {
        self.at[(self.n & (LOOP_DEATHS as u64 - 1)) as usize] = now;
        self.n += 1;
        let oldest = self.at[(self.n & (LOOP_DEATHS as u64 - 1)) as usize];
        if self.n >= LOOP_DEATHS as u64 && now - oldest <= WINDOW_S * freq {
            self.stopped = true;
        }
        self.stopped
    }

    /// The count and the stop cleared (a restart by hand).
    pub fn clear(&mut self) {
        *self = Deaths::new();
    }

    /// Deaths recorded since the last clear.
    pub fn count(&self) -> u64 {
        self.n
    }
}

impl Default for Deaths {
    fn default() -> Self {
        Deaths::new()
    }
}

/// How a death ended, for the note's words: a panic, a fault, or a plain exit (the vault, which is started again whenever it ends having served).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum End {
    Panic = 1,
    Fault = 2,
    Exit = 3,
}

impl End {
    /// From its byte (`End as u8`); None for any other.
    pub fn from_u8(b: u8) -> Option<End> {
        match b {
            1 => Some(End::Panic),
            2 => Some(End::Fault),
            3 => Some(End::Exit),
            _ => None,
        }
    }

    /// The words for it on the console and in the flash log.
    pub fn word(self) -> &'static str {
        match self {
            End::Panic => "a panic",
            End::Fault => "a fault",
            End::Exit => "its exit",
        }
    }
}

/// What the person calls a process the kernel starts again: its job, never its process name.
pub fn person_name(process: &[u8]) -> &'static str {
    match process {
        b"display" => "The display",
        b"touch" => "Touch",
        b"ahau" => "The keyboard",
        b"map" => "The deck",
        b"fgtw" => "The identity service",
        b"usb" => "USB",
        b"ip" => "The network",
        b"nfc" => "NFC",
        b"gpu" => "The GPU",
        b"kists" => "The stower",
        b"init" => "init",
        b"vault" => "The vault",
        b"audio" => "The speaker",
        b"torch" => "The torch",
        b"camera" => "The camera",
        b"icons" => "The icon decoder",
        _ => "A system process",
    }
}

/// The note's words (ASCII, a short line: they are drawn on one line of the about page, and ferros_kist::note holds words to `WORDS_MAX`): restarted after a panic, a fault or its exit, or, `stopped`, not restarted any more.
pub fn notice_words(process: &[u8], end: End, stopped: bool) -> String {
    let mut s = String::from(person_name(process));
    if stopped {
        s.push_str(" kept crashing: not restarted");
    } else {
        s.push_str(" restarted after ");
        s.push_str(match end {
            End::Exit => "it ended",
            e => e.word(),
        });
    }
    s
}

/// The note's key: one note a process, revised by its next restart.
pub fn notice_key(process: &[u8]) -> [u8; 32] {
    *blake3::Hasher::new_derive_key("ferros 2026-11-21 restart notice").update(process).finalize().as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    const F: u64 = 1 << 20;

    #[test]
    fn three_deaths_restart_the_fourth_in_the_window_stops() {
        let mut d = Deaths::new();
        assert!(!d.died(0, F));
        assert!(!d.died(F, F));
        assert!(!d.died(2 * F, F));
        assert!(d.died(3 * F, F));
        assert!(d.stopped);
        // Stopped stays stopped until cleared.
        assert!(d.died(1 << 40, F));
    }

    #[test]
    fn deaths_spread_past_the_window_never_stop() {
        let mut d = Deaths::new();
        let step = (WINDOW_S * F) / 3 + 1;
        for i in 0..64 {
            assert!(!d.died(i * step, F), "death {i}");
        }
    }

    #[test]
    fn the_window_is_inclusive_and_slides() {
        let mut d = Deaths::new();
        // Three quick deaths, a long gap, then three more: the oldest of the last four is the third quick one, far back.
        for t in [0, 1, 2] {
            assert!(!d.died(t, F));
        }
        let later = 1 << 30;
        assert!(!d.died(later, F));
        assert!(!d.died(later + 1, F));
        assert!(!d.died(later + 2, F));
        // The fourth within the window of the first of the last four (later, exactly WINDOW_S after it).
        assert!(d.died(later + WINDOW_S * F, F));
    }

    #[test]
    fn clear_starts_again() {
        let mut d = Deaths::new();
        for t in 0..4 {
            d.died(t, F);
        }
        assert!(d.stopped);
        d.clear();
        assert_eq!(d.count(), 0);
        assert!(!d.died(5, F));
    }

    #[test]
    fn words_are_a_line_of_ascii() {
        for p in [b"display".as_slice(), b"map", b"ahau", b"fgtw", b"vault", b"audio", b"nobody"] {
            for end in [End::Panic, End::Fault, End::Exit] {
                for stopped in [false, true] {
                    let w = notice_words(p, end, stopped);
                    assert!(w.is_ascii() && w.len() <= crate::note::WORDS_MAX, "{w}");
                }
            }
        }
        assert_eq!(notice_words(b"map", End::Panic, false), "The deck restarted after a panic");
    }

    #[test]
    fn keys_differ_by_process() {
        assert_ne!(notice_key(b"map"), notice_key(b"display"));
    }
}
