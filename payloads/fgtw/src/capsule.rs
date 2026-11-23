//! The fleet's capsule source (docs/IDENTITY-SPACE.md "Capsules", source 2; owner, 2026-11-10: "BLE, then LAN, then WAN, then hopefully we shouldn't need to but fgtw.org"): a device's stower asks for a kist's capsule by its BLAKE3, and the person's other devices answer from their own `capsules.`, a piece at a time.
//! Pure: no syscall, no clock, no frame, so the bookkeeping is tested on the host (the tests below). main.rs carries it out (`on_capsule_want`, ops 38, 39 and 40).
//! Two halves:
//! - Asking (op 38): the stower's call held while a `capsule_want` is out to the members heard, ended by a piece from one of them, by every one of them out (a `capsule_none`, or its frame given up on), or by the asked devices' time (`ASK_US`, the space's fan-out ask's rule: a sibling whose fgtw predates `capsule_want` takes the frame and never answers, and nothing else would end the call).
//! - Answering (ops 39 and 40): a member's `capsule_want` handed to this device's stower, which holds op 39 for it and hands the piece back with op 40; fgtw signs it and sends it to that member. The stower reads its own `capsules.` (init's grant page has no room for a `capsules.` grant to fgtw, and fgtw needs none: the stower holds the scope).
//! Trust: none in the bytes. The asker's stower takes them through Intake, the capsule's address, its eggs, the person's pin and the kernel's check, as it does the directory's.

use alloc::vec::Vec;
use photon_core::pt::Addr;

/// The asked devices' time to answer a piece: the space's fan-out ask's (space.rs `ASK_US`), about eight seconds.
pub const ASK_US: u64 = 1 << 23;

/// Op 36's reply when no member has the piece (none heard, every one said none or was given up on, or their time ran out): [NOBODY]. A piece is [PIECE][the whole length u64 LE][the bytes].
pub const PIECE: u8 = 0;
pub const NOBODY: u8 = 1;

/// One op 38 under way.
struct Ask {
    tok: [u8; 32],
    capsule: [u8; 32],
    offset: u64,
    /// The `capsule_want`'s BLAKE3, which Photon Transport's `Sent` names it by.
    frame: [u8; 32],
    /// The devices asked (key, address) and whether each is out: answered none, or the frame to it given up on.
    asked: Vec<([u8; 32], Addr, bool)>,
    until_us: u64,
}

/// A member's `capsule_want`, waiting for this device's stower (op 39) or with it (`handed`) until op 40 hands its piece back.
pub struct Want {
    pub id: u32,
    pub sender: [u8; 32],
    pub from: Addr,
    pub capsule: [u8; 32],
    pub offset: u64,
    handed: bool,
}

/// What a member's `capsule_want` comes to (`Capsules::want`).
pub enum Wanted {
    /// No stower answers here (none has held op 39 since fgtw started, a stower from before the fleet source): a `capsule_none` at once, so the asker goes on to its next source.
    None,
    /// Queued behind the one the stower has: handed at its next op 39.
    Queued,
    /// The stower's held op 39 answered now: its token and [id u32 LE][capsule 32][offset u64 LE].
    Ring([u8; 32], Vec<u8>),
}

#[derive(Default)]
pub struct Capsules {
    asks: Vec<Ask>,
    /// The member that served the last piece (the capsule, its key, its address): the next piece of that capsule is asked of it alone, not of the whole fleet each time.
    server: Option<([u8; 32], [u8; 32], Addr)>,
    wants: Vec<Want>,
    next: u32,
    /// The stower's op 39, held while no want waits.
    held: Option<[u8; 32]>,
    /// A stower has held op 39 since fgtw started.
    answering: bool,
}

impl Capsules {
    /// Who op 38 asks for `capsule` from `offset`, of the members heard: the one that served the piece before it, while it is heard, else every one. Empty: nobody to ask, and the call is answered NOBODY at once.
    pub fn targets(&self, capsule: &[u8; 32], offset: u64, heard: &[([u8; 32], Addr)]) -> Vec<([u8; 32], Addr)> {
        if offset > 0 {
            if let Some((c, k, _)) = &self.server {
                if c == capsule {
                    if let Some(h) = heard.iter().find(|h| h.0 == *k) {
                        return alloc::vec![*h];
                    }
                }
            }
        }
        heard.to_vec()
    }

    /// Op 36's call held: the `capsule_want` (its BLAKE3 `frame`) went to `to`.
    pub fn ask(&mut self, tok: [u8; 32], capsule: [u8; 32], offset: u64, frame: [u8; 32], to: &[([u8; 32], Addr)], now_us: u64) {
        let asked = to.iter().map(|(k, a)| (*k, *a, false)).collect();
        self.asks.push(Ask { tok, capsule, offset, frame, asked, until_us: now_us + ASK_US });
    }

    /// A member's `capsule_piece`: the ask it answers ended, with its token and op 38's reply. None for a piece no ask here wants: another capsule or offset, a device that was not asked, an empty piece or one past its own whole length.
    pub fn piece(&mut self, sender: &[u8; 32], capsule: &[u8; 32], offset: u64, total: u64, bytes: &[u8]) -> Option<([u8; 32], Vec<u8>)> {
        // WHY checked_add: `offset` and `total` are another device's words off the network; a sum past u64 is no piece of anything, not a wrap to a small end.
        let end = offset.checked_add(bytes.len() as u64)?;
        if bytes.is_empty() || end > total {
            return None;
        }
        let (at, addr) = self.asks.iter().enumerate().find_map(|(at, a)| if a.capsule == *capsule && a.offset == offset { a.asked.iter().find(|d| d.0 == *sender).map(|d| (at, d.1)) } else { None })?;
        let a = self.asks.swap_remove(at);
        self.server = Some((*capsule, *sender, addr));
        let mut r = Vec::with_capacity(9 + bytes.len());
        r.push(PIECE);
        r.extend_from_slice(&total.to_le_bytes());
        r.extend_from_slice(bytes);
        Some((a.tok, r))
    }

    /// A member's `capsule_none`: that device out of every ask for the capsule; the tokens of the asks now ended (every device asked is out), each to be answered NOBODY.
    pub fn none(&mut self, sender: &[u8; 32], capsule: &[u8; 32]) -> Vec<[u8; 32]> {
        if self.server.as_ref().is_some_and(|(c, k, _)| c == capsule && k == sender) {
            self.server = None;
        }
        for a in self.asks.iter_mut().filter(|a| a.capsule == *capsule) {
            for d in a.asked.iter_mut().filter(|d| d.0 == *sender) {
                d.2 = true;
            }
        }
        self.ended(None)
    }

    /// A frame's delivery (Photon Transport's `Sent`): a `capsule_want` given up on counts the device it was for out.
    pub fn sent(&mut self, to: &Addr, hash: &[u8; 32], ok: bool) -> Vec<[u8; 32]> {
        if ok {
            return Vec::new();
        }
        for a in self.asks.iter_mut().filter(|a| a.frame == *hash) {
            for d in a.asked.iter_mut().filter(|d| d.1 == *to) {
                d.2 = true;
            }
        }
        self.ended(None)
    }

    /// The asks whose time is out at `now_us`, or whose devices are all out: their tokens, each to be answered NOBODY.
    pub fn step(&mut self, now_us: u64) -> Vec<[u8; 32]> {
        self.ended(Some(now_us))
    }

    /// The asks every asked device is out of, and with `now_us` those whose time is out: taken, their tokens.
    fn ended(&mut self, now_us: Option<u64>) -> Vec<[u8; 32]> {
        let mut out = Vec::new();
        let mut k = 0;
        while k < self.asks.len() {
            let a = &self.asks[k];
            if a.asked.iter().all(|d| d.2) || now_us.is_some_and(|t| t >= a.until_us) {
                out.push(self.asks.swap_remove(k).tok);
            } else {
                k += 1;
            }
        }
        out
    }

    /// The earliest asked devices' time (`idle_until`), None while nothing is asked.
    pub fn due(&self) -> Option<u64> {
        self.asks.iter().map(|a| a.until_us).min()
    }

    /// A member's `capsule_want` (its handle proof and membership checked by the caller).
    pub fn want(&mut self, sender: [u8; 32], from: Addr, capsule: [u8; 32], offset: u64) -> Wanted {
        if !self.answering {
            return Wanted::None;
        }
        // The same ask again (its frame sent twice, or asked again after its time): the one queued answers it.
        if let Some(w) = self.wants.iter_mut().find(|w| w.sender == sender && w.capsule == capsule && w.offset == offset) {
            w.from = from;
            return Wanted::Queued;
        }
        let id = self.next;
        // WHY wrapping: an id names a want that lives while one piece is read and sent; after 2^32 wants the id comes round to one long gone, never a want still here in practice, and a wrap is not an overflow to stop on.
        self.next = self.next.wrapping_add(1);
        self.wants.push(Want { id, sender, from, capsule, offset, handed: false });
        match self.held.take() {
            Some(tok) => Wanted::Ring(tok, self.hand_next().unwrap()),
            None => Wanted::Queued,
        }
    }

    /// The stower's op 39: the next want at once, or the call held until one comes. A call held before it (a stower that started again) is handed back with nothing, its token returned with it.
    pub fn hold(&mut self, tok: [u8; 32]) -> (Option<Vec<u8>>, Option<[u8; 32]>) {
        self.answering = true;
        if let Some(r) = self.hand_next() {
            return (Some(r), None);
        }
        (None, self.held.replace(tok))
    }

    /// The first want not handed yet, handed: op 39's reply.
    fn hand_next(&mut self) -> Option<Vec<u8>> {
        let w = self.wants.iter_mut().find(|w| !w.handed)?;
        w.handed = true;
        let mut r = Vec::with_capacity(44);
        r.extend_from_slice(&w.id.to_le_bytes());
        r.extend_from_slice(&w.capsule);
        r.extend_from_slice(&w.offset.to_le_bytes());
        Some(r)
    }

    /// The stower's op 40 for want `id`: that want, taken; None for an id not handed (gone already, or never).
    pub fn take(&mut self, id: u32) -> Option<Want> {
        let at = self.wants.iter().position(|w| w.id == id && w.handed)?;
        Some(self.wants.swap_remove(at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(last: u8) -> Addr {
        let mut ip = [0u8; 16];
        ip[..4].copy_from_slice(&[192, 168, 1, last]);
        Addr { fam: 4, ip, port: 4383 }
    }

    const CAP: [u8; 32] = [7; 32];

    #[test]
    fn a_piece_from_an_asked_member_answers_the_ask() {
        let mut c = Capsules::default();
        let heard = [([1; 32], addr(1)), ([2; 32], addr(2))];
        let to = c.targets(&CAP, 0, &heard);
        assert_eq!(to.len(), 2);
        c.ask([9; 32], CAP, 0, [5; 32], &to, 0);
        // Not asked, another offset, past its total, empty: none of them answers.
        assert!(c.piece(&[3; 32], &CAP, 0, 10, b"abc").is_none());
        assert!(c.piece(&[1; 32], &CAP, 4, 10, b"abc").is_none());
        assert!(c.piece(&[1; 32], &CAP, 0, 2, b"abc").is_none());
        assert!(c.piece(&[1; 32], &CAP, 0, 10, b"").is_none());
        assert!(c.piece(&[1; 32], &CAP, u64::MAX, u64::MAX, b"abc").is_none());
        let (tok, r) = c.piece(&[2; 32], &CAP, 0, 10, b"abc").unwrap();
        assert_eq!(tok, [9; 32]);
        assert_eq!(r[0], PIECE);
        assert_eq!(u64::from_le_bytes(r[1..9].try_into().unwrap()), 10);
        assert_eq!(&r[9..], b"abc");
        assert_eq!(c.due(), None);
        // The next piece asked of the member that served this one, alone.
        assert_eq!(c.targets(&CAP, 3, &heard), alloc::vec![([2; 32], addr(2))]);
        // Another capsule, or the first piece, asks every one.
        assert_eq!(c.targets(&[8; 32], 3, &heard).len(), 2);
        assert_eq!(c.targets(&CAP, 0, &heard).len(), 2);
    }

    #[test]
    fn every_member_out_answers_nobody_without_waiting() {
        let mut c = Capsules::default();
        let to = [([1; 32], addr(1)), ([2; 32], addr(2))];
        c.ask([9; 32], CAP, 0, [5; 32], &to, 0);
        assert!(c.none(&[1; 32], &CAP).is_empty());
        // A delivery that went, and a frame that is not this ask's, change nothing.
        assert!(c.sent(&addr(2), &[5; 32], true).is_empty());
        assert!(c.sent(&addr(2), &[6; 32], false).is_empty());
        assert_eq!(c.sent(&addr(2), &[5; 32], false), alloc::vec![[9; 32]]);
        assert_eq!(c.due(), None);
    }

    #[test]
    fn a_silent_member_answers_nobody_at_its_time() {
        let mut c = Capsules::default();
        c.ask([9; 32], CAP, 0, [5; 32], &[([1; 32], addr(1))], 100);
        assert_eq!(c.due(), Some(100 + ASK_US));
        assert!(c.step(100 + ASK_US - 1).is_empty());
        assert_eq!(c.step(100 + ASK_US), alloc::vec![[9; 32]]);
    }

    #[test]
    fn a_none_from_the_server_asks_the_fleet_again() {
        let mut c = Capsules::default();
        let heard = [([1; 32], addr(1)), ([2; 32], addr(2))];
        c.ask([9; 32], CAP, 0, [5; 32], &heard, 0);
        c.piece(&[1; 32], &CAP, 0, 10, b"abc").unwrap();
        let to = c.targets(&CAP, 3, &heard);
        assert_eq!(to.len(), 1);
        c.ask([9; 32], CAP, 3, [6; 32], &to, 0);
        assert_eq!(c.none(&[1; 32], &CAP), alloc::vec![[9; 32]]);
        assert_eq!(c.targets(&CAP, 3, &heard).len(), 2);
    }

    #[test]
    fn wants_wait_for_the_stower_and_are_handed_once() {
        let mut c = Capsules::default();
        // No stower has held op 39: a none at once.
        assert!(matches!(c.want([1; 32], addr(1), CAP, 0), Wanted::None));
        // The stower holds op 39 with nothing waiting: held.
        assert_eq!(c.hold([4; 32]), (None, None));
        let Wanted::Ring(tok, r) = c.want([1; 32], addr(1), CAP, 0) else { panic!("the held call is answered") };
        assert_eq!(tok, [4; 32]);
        assert_eq!(r.len(), 44);
        let id = u32::from_le_bytes(r[..4].try_into().unwrap());
        assert_eq!(&r[4..36], &CAP);
        assert_eq!(u64::from_le_bytes(r[36..44].try_into().unwrap()), 0);
        // The same want again while the stower has it: no second hand-off.
        assert!(matches!(c.want([1; 32], addr(1), CAP, 0), Wanted::Queued));
        // Another queued while the stower is busy, handed at its next op 39.
        assert!(matches!(c.want([2; 32], addr(2), CAP, 0), Wanted::Queued));
        let w = c.take(id).unwrap();
        assert_eq!((w.sender, w.from, w.offset), ([1; 32], addr(1), 0));
        assert!(c.take(id).is_none());
        let (now, old) = c.hold([4; 32]);
        let r = now.unwrap();
        assert!(old.is_none());
        let id = u32::from_le_bytes(r[..4].try_into().unwrap());
        // Not handed twice: the stower's next op 39 is held.
        assert_eq!(c.hold([4; 32]), (None, None));
        assert_eq!(c.take(id).unwrap().sender, [2; 32]);
        // A second held call (a stower that started again) hands the first back.
        assert_eq!(c.hold([3; 32]), (None, Some([4; 32])));
    }
}
