//! Notes on the deck (docs/NOTIFY.md; owner, 2026-11-12: "I think we try it with the in view, it can notify me, out of view, silent. Screen's big enough, we'll see.").
//!
//! - **The list** is the kernel's (SYS_NOTIFY, ferros_sys::notify): read whenever its bell rings (`WORD`), or when the earliest note's deadline passes (the kernel drops it as the list is read), never on a timer of the map's own.
//! - **The glow:** a kist circle with notes waiting glows for you (400 nm), its reach by how many (`Map::waiting`, display op 57).
//! - **The resting view** (`Map::rest`) is where the person last settled the deck; a circle whose disc meets it is in view (`in_view`). A pan that does not settle changes nothing.
//! - **Personal**, in view: one tap of the motor as it arrives (`quick::buzz`), by the sound mode (preference 39: not in silent) and not under do not disturb; one a kist until the person has looked at it (`tapped`).
//! - **The Deck button** (display op 66; owner, 2026-11-12: "if anything on the deck has changed state, specifically 400nm notifications, we change the deck circle in the helm", "the deck circle shows the latest item icon preview and shows the violet ring"): while anything in view waits, the latest one's source's icon and the count in view.
//! - **Live** takes the screen (`take_over`): the app in front behind, the source's circle open on Answer and Decline, until the person answers or declines or the kist withdraws it (the caller ended).
//! - **Seen:** a kist brought to the front (its circle tapped, a flip, an answer) has its notes cleared (owner, 2026-11-12: "when tapping on Photon it clears it and resumes regular 500nm"); a tap that opened it at a note tells it which (display op 67, `EV_OPEN_AT`).
use super::*;
use core::sync::atomic::AtomicU64;

/// The notes' bell as the kernel writes it here (SYS_NOTIFY op 4).
pub static WORD: AtomicU64 = AtomicU64::new(0);

/// The motor's ROM waveform for a personal note's tap, and for a live note's arrival (the CS40L26's ROM; docs/NFC.md "The vibration motor": waveform 1 is the one played in its bring-up).
const TAP: u8 = 1;
const RING: u8 = 1;
/// The most notes the map reads at once: sixteen kists' worth of the kernel's per-kist count.
const MAX_NOTES: usize = 1 << 8;

/// A note as the kernel lists it (ferros_sys::notify::REC).
#[derive(Clone, Copy)]
pub struct Rec {
    pub source: [u8; 32],
    pub number: u8,
    pub kind: u8,
    pub count: u32,
    pub seq: u64,
    pub deadline: i64,
}

/// The list as the kernel has it now; None without the Settings capability (or a kernel without notes).
fn read(settings: u64) -> Option<Vec<Rec>> {
    let mut buf = alloc::vec![0u8; MAX_NOTES * ferros_sys::notify::REC];
    let n = ferros_sys::notify::list(settings, &mut buf)?;
    // WHY the `take`: the kernel writes whole records, as many as fit; a longer list is cut there, and only the first `n` that landed are read.
    Some(
        buf.chunks_exact(ferros_sys::notify::REC)
            .take(n)
            .map(|r| Rec {
                source: r[..32].try_into().unwrap(),
                number: r[32],
                kind: r[33],
                count: u32::from_le_bytes(r[36..40].try_into().unwrap()),
                seq: u64::from_le_bytes(r[40..48].try_into().unwrap()),
                deadline: i64::from_le_bytes(r[48..56].try_into().unwrap()),
            })
            .collect(),
    )
}

impl Map {
    /// Whether circle `i`'s disc meets the deck's resting view (`rest`): the screen's rectangle as the camera stood when the person last settled the deck.
    pub fn in_view(&self, i: usize) -> bool {
        let (sx, sy) = self.rest.screen(self.w, self.h, self.xs[i], self.ys[i]);
        let sr = (self.rs[i] * self.rest.z) >> 16;
        // The screen's nearest point to the circle's centre: the centre itself held to the rectangle (the nearest point's definition, not a guard).
        let (nx, ny) = (sx.clamp(0, self.w), sy.clamp(0, self.h));
        (sx - nx) * (sx - nx) + (sy - ny) * (sy - ny) <= sr * sr
    }

    /// The notes' bell rang (or a deadline passed): the list read, the glows and the Deck button follow it, a personal note new in view taps once, a live one takes the screen.
    pub fn notes_rang(&mut self) {
        let settings = cap(self.grants, S_SETTINGS);
        let Some(list) = read(settings) else { return };
        let read_before = self.notes_read;
        self.notes = list;
        // The earliest deadline, so the main loop's rest ends there (an alarm, never a poll): Eagle oscillations to counter ticks from now.
        let eagle = ferros_detail::eagle_now();
        self.notes_due = self.notes.iter().filter(|n| n.deadline != 0).map(|n| n.deadline).min().map(|d| {
            let osc = ferros_kist::vsf::OSCILLATIONS_PER_SECOND as i128;
            // WHY the floor: a deadline already past is due now (the next pass reads again and the kernel drops it); a negative distance would wrap the counter's u64 to a time far ahead.
            let ahead = (d as i128 - eagle as i128).max(0);
            now() + (ahead * freq() as i128 / osc) as u64
        });
        // Each kist's waiting: the sum of its notes' counts. WHY the cap: a count is the kist's own word (up to 2^32), the display's field is 16 bits (op 57), and the glow's reach is already near its limit long before (ferros_glow `waiting_reach`, a log).
        self.waiting.clear();
        for n in &self.notes {
            match self.waiting.iter_mut().find(|w| w.0 == n.source) {
                Some(w) => w.1 = (w.1 as u64 + n.count as u64).min(u16::MAX as u64) as u16,
                None => self.waiting.push((n.source, n.count.min(u16::MAX as u32) as u16)),
            }
        }
        // New since the last ring: a personal one in view taps once a kist, a live one takes the screen.
        let mut buzz = false;
        let mut live = None;
        for n in self.notes.clone().iter().filter(|n| n.seq > read_before) {
            self.notes_read = self.notes_read.max(n.seq);
            let circle = self.circle_of(&n.source);
            let mut l = Line::new();
            let _ = write!(l, "map: notes: a {} note from G#{:02X}{:02X}{:02X}{:02X} (note {}, count {}), {}\n", kind_word(n.kind), n.source[0], n.source[1], n.source[2], n.source[3], n.number, n.count, match circle {
                None => "no circle on the deck",
                Some(i) if self.in_view(i) => "in view",
                Some(_) => "out of view: silent",
            });
            console(self.grants, l.as_str().as_bytes());
            match n.kind {
                ferros_kist::note::LIVE => live = Some((n.source, n.number)),
                ferros_kist::note::PERSONAL if circle.is_some_and(|i| self.in_view(i)) && !self.tapped.contains(&n.source) => {
                    self.tapped.push(n.source);
                    buzz = true;
                }
                _ => {}
            }
        }
        if buzz && ferros_sys::pref(ferros_sys::PREF_SOUND) != ferros_sys::SILENT && !ferros_sys::dnd::on() {
            quick::buzz(TAP);
        }
        // A live note taken over the screen and gone (withdrawn: the caller ended; or past its deadline): its page closes.
        if let Some((src, num)) = self.live_note {
            if !self.notes.iter().any(|n| n.source == src && n.number == num) {
                self.live_note = None;
                self.close_live();
            }
        }
        if let Some((src, num)) = live {
            self.take_over(src, num);
        }
        self.notes_view();
        if self.send_states(false) && !self.app_front {
            self.draw();
        }
    }

    /// The Deck button as the deck's summary (display op 66): the latest note in view's source's icon (a bundled icon only; the ring alone otherwise) and how many wait in view. Sent when it changed; called as the list or the resting view changes.
    pub fn notes_view(&mut self) {
        let mut count = 0u64;
        let mut latest: Option<(u64, usize)> = None;
        for n in &self.notes {
            let Some(i) = self.circle_of(&n.source) else { continue };
            if !self.in_view(i) {
                continue;
            }
            count += n.count as u64;
            if latest.is_none_or(|l| n.seq > l.0) {
                latest = Some((n.seq, i));
            }
        }
        // A bundled icon (1-5, the display's ICN bundle); a widget's or none: 0, the ring alone.
        let icon = latest.map_or(0, |(_, i)| self.spec(i).icon);
        let icon = if icon >= 0x80 { 0 } else { icon };
        // WHY the cap: as the circles' counts (`notes_rang`), the op's field is 16 bits.
        let s = (icon, count.min(u16::MAX as u64) as u16);
        if s != self.helm_sent {
            self.helm_sent = s;
            if !ferros_sys::window::helm_note(self.display, s.0, s.1) {
                console(self.grants, b"map: notes: the display did not take the Deck button's note\n");
            }
        }
    }

    /// The latest note waiting from `capsule` (by when it was posted), if any: where a tap on its circle opens it.
    pub fn latest_note(&self, capsule: &[u8; 32]) -> Option<u8> {
        self.notes.iter().filter(|n| n.source == *capsule).max_by_key(|n| n.seq).map(|n| n.number)
    }

    /// `capsule`'s kist came to the front (`launch`): the person has looked, so its notes are cleared (`opening`: the note the tap opened it at, kept for the kist to read where to land). Its window already in front (`raised`): told at once (display op 67); else when it shows (`open_pending`).
    pub fn notes_front(&mut self, capsule: [u8; 32], raised: bool) {
        let settings = cap(self.grants, S_SETTINGS);
        let opening = self.opening.take();
        if let Some(n) = opening {
            ferros_sys::notify::seen(settings, &capsule, n, true);
        }
        if self.notes.iter().any(|n| n.source == capsule) {
            ferros_sys::notify::seen(settings, &capsule, ferros_sys::notify::ALL, false);
        }
        self.tapped.retain(|t| *t != capsule);
        if let Some(n) = opening {
            if raised {
                self.open_at(capsule, n);
            } else {
                self.open_pending = Some((capsule, n));
            }
        }
    }

    /// The kist in front told which note the person opened (display op 67, `EV_OPEN_AT`), if it is still `capsule`'s.
    pub fn open_at(&mut self, capsule: [u8; 32], n: u8) {
        if self.front_kist != Some(capsule) {
            return;
        }
        let said = match ferros_sys::window::open_at(self.display, n) {
            Some(_) => "map: notes: the kist in front opened at its note\n",
            None => "map: notes: no window in front to open at its note\n",
        };
        console(self.grants, said.as_bytes());
    }

    /// A live note takes the screen (docs/NOTIFY.md "The live takeover"): the app in front behind, the source's circle opened on its page (`live_page`, Answer and Decline), one ring of the motor. Modal: a tap outside the circle does not close it; Deck does, and leaves the note waiting.
    fn take_over(&mut self, source: [u8; 32], number: u8) {
        let Some(i) = self.circle_of(&source) else {
            console(self.grants, b"map: notes: a live note from a kist with no circle on the deck\n");
            return;
        };
        self.live_note = Some((source, number));
        self.live_words = self.note_words(&source, number);
        if self.app_front {
            self.send_behind(false);
        }
        if ferros_sys::pref(ferros_sys::PREF_SOUND) != ferros_sys::SILENT {
            quick::buzz(RING);
        }
        if self.view == View::Map {
            self.back = Some(self.cam);
        }
        let to = self.at(i);
        self.view = View::Open(i);
        self.leaving = false;
        self.fly(to);
        self.draw();
        console(self.grants, b"map: notes: a live note took the screen\n");
    }

    /// The live page closes (the note answered, declined or gone): back to the deck as it was.
    fn close_live(&mut self) {
        if matches!(self.view, View::Open(i) if i > 0) {
            let to = self.back.take().unwrap_or_else(|| self.deck_fit());
            self.leaving = true;
            self.fly(to);
            self.draw();
        }
    }

    /// The person's answer on the live page: Answer opens the kist at the note (its circle's tap, `launch`, the note the one opened), Decline lets the note go.
    pub fn live_answer(&mut self, answer: bool) {
        let Some((source, number)) = self.live_note.take() else { return };
        if answer {
            if let Some(i) = self.circle_of(&source) {
                // The deck as it was before the takeover, behind the kist.
                self.view = View::Map;
                if let Some(b) = self.back.take() {
                    self.cam = b;
                }
                self.opening = Some(number);
                self.launch(i);
                if !self.app_front {
                    self.draw();
                }
                return;
            }
        }
        ferros_sys::notify::seen(cap(self.grants, S_SETTINGS), &source, number, false);
        self.close_live();
    }

    /// A note's words (ASCII, at most ferros_kist::note::WORDS_MAX bytes), read from the kernel.
    fn note_words(&self, source: &[u8; 32], number: u8) -> Option<alloc::string::String> {
        let mut buf = alloc::vec![0u8; ferros_kist::note::MAX];
        let n = ferros_sys::notify::read(cap(self.grants, S_SETTINGS), source, number, &mut buf)?;
        // WHY the bound: the length is the kernel's answer; a document past the buffer was not written.
        if n > buf.len() {
            return None;
        }
        ferros_kist::note::parse(&buf[..n])?.words
    }

    /// The live page on its kist's open circle: its name, the note's words (or "for you, now"), Answer and Decline. Whether it drew (the circle is the live note's).
    pub fn live_page(&mut self, b: &mut Batch, i: usize, region: bool) -> bool {
        let Some(k) = self.kist(i).cloned() else { return false };
        if self.live_note.is_none_or(|l| l.0 != k.capsule) {
            return false;
        }
        let (cx, cy, r, uq) = self.page_frame();
        let uu = |n: i64, d: i64| n * uq / (d << 8);
        let half = self.w / 2;
        let (title_y, top, bottom) = self.bands();
        b.text(cx, title_y, uu(2, 1) as u16, white(), 1, &k.name);
        let y = (top + bottom) / 2;
        let words = self.live_words.clone();
        b.text(cx, y - uu(3, 1), uu(5, 4) as u16, white(), 1, words.as_deref().unwrap_or(tr(Msg::ForYouNow)));
        let [bf, bh, tl, br] = BUTTON_LOOK[ferros_sys::theme::index()];
        let live = self.target.is_none();
        for (dy, c, m) in [(uu(1, 1), Choice::LiveAnswer, Msg::Answer), (uu(4, 1), Choice::LiveDecline, Msg::Decline)] {
            b.button(cx, y + dy - uu(1, 2), uu(16, 1), uu(2, 1), uu(3, 2), [if self.pressed == Some(c) { bh } else { bf }, tl, br, white()], tr(m));
            if live {
                self.push_row(cx - half, cx + half, y + dy - uu(2, 1), y + dy + uu(1, 1), c);
            }
        }
        if !region {
            b.disc(cx, cy, r, fill());
        }
        true
    }

    /// Whether the live page is what the open circle shows (a tap outside it does not close it).
    pub fn live_showing(&self) -> bool {
        matches!((self.view, self.live_note), (View::Open(i), Some((src, _))) if self.kist(i).is_some_and(|k| k.capsule == src))
    }
}

/// A kind's word for the console.
fn kind_word(kind: u8) -> &'static str {
    match kind {
        ferros_kist::note::STATUS => "status",
        ferros_kist::note::PERSONAL => "personal",
        ferros_kist::note::LIVE => "live",
        _ => "?",
    }
}
