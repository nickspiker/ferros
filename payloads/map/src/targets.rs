//! The deck's drop targets (owner, 2026-11-18, his design with ferros-52's, accepted): while a kist's circle or a puck is dragged on the deck, three targets grow for the length of the drag (Back and Deck cannot be pressed mid-drag anyway).
//!
//! - **About**, out of the bottom-left corner: the item's card on the Deck page (its permissions, version, storage, its own settings); the settings circle's is the About page.
//! - **Unstow**, out of the bottom-right corner, never at once: the card with its unstow asked, a kist's sheet or a puck's two taps (as WiFi's forget). Not for the settings circle.
//! - **Stop**, down from the top centre, for an item that runs: ended at once by init, no confirm (`Map::stop`; owner, 2026-11-18: ferros apps keep every change as it happens, so there is nothing to shut down, and the camera's driver closes its stream as its holder's process ends).
//!
//! Dropped on a target, the item and everything it pushed go back exactly where they stood before the drag (owner: "previous deck arrangement should restore after the target is dropped"), then the target acts.
//! The targets grow and shrink on shown frames (`spring`, C¹, no timer), drawn and hit by one rule: a disc about its corner (or the top's centre) of radius `level` × a quarter of the span, the one under the finger lit.
//! The helm's own marks fade as the targets grow (display op 54, corner 10), so nothing is drawn over them.

use super::*;

/// The targets, by what they do.
pub const ABOUT: u8 = 1;
pub const UNSTOW: u8 = 2;
pub const STOP: u8 = 3;

/// A target's radius whole: the span over 2^SHARE (a quarter).
const SHARE: u32 = 2;

/// The deck as it stood when a drag began: the circle dragged, every circle's place and whether it was arranged (the deck's circles in their order then), and the gather's scatter (a hand move forgets it).
pub struct Before {
    pub i: usize,
    places: Vec<(deck::At, bool)>,
    scatter: Option<(Vec<(i64, i64)>, Cam)>,
}

#[derive(Default)]
pub struct Drop {
    /// The drag going on, and the deck before it.
    pub held: Option<Before>,
    /// How far the targets have grown (0 to 1) and their speed a frame; where they go (1 while a drag goes on); the spring's start: the offset from the goal and the speed when the goal last changed, and the frames since.
    level: f64,
    goal: f64,
    seg: (f64, f64, i32),
    /// The target under the finger, 0 none.
    lit: u8,
    /// Which targets show for this drag.
    unstow: bool,
    stop: bool,
    /// The panel's frame count last stepped from.
    frame_seen: Option<u32>,
    /// The helm's marks' level last sent (op 54 corner 10).
    hide_sent: u16,
}

impl Drop {
    /// Whether the targets are still growing or shrinking: the loop steps them a shown frame at a time until they rest.
    pub fn moving(&self) -> bool {
        self.level != self.goal || self.seg.1 != 0.
    }
}

impl Map {
    /// A one-finger drag of circle `i` begins on the deck: the deck as it stands kept, and the targets on their way out. Nothing for a drag with an app in front or a circle open.
    pub fn drop_begin(&mut self, i: usize) {
        if self.view != View::Map || self.app_front {
            return;
        }
        let item = self.kist(i).map(|k| (k.capsule, k.is_puck()));
        self.drop.unstow = item.is_some();
        self.drop.stop = item.is_some_and(|(c, puck)| if puck { self.pucks_up.contains(&c) && self.live.as_ref().is_none_or(|l| l.contains(&c)) } else { self.kist_state(&c).is_some_and(|s| s.0 == Msg::Running) });
        self.drop.held = Some(Before { i, places: self.deck.circles.iter().map(|c| (c.at, c.arranged)).collect(), scatter: self.scatter.clone() });
        self.drop.lit = 0;
        self.drop_goal(1.);
    }

    /// The drag ended without a drop, or became something else (a second finger): the targets go, the deck as the drag left it.
    pub fn drop_cancel(&mut self) {
        if self.drop.held.take().is_some() {
            self.drop.lit = 0;
            self.drop_goal(0.);
        }
    }

    /// The spring toward `goal` from where the targets are now, at their speed now.
    fn drop_goal(&mut self, goal: f64) {
        let (x, v) = spring(self.drop.seg.0, self.drop.seg.1, self.drop.seg.2);
        let level = self.drop.goal + x;
        self.drop.goal = goal;
        self.drop.seg = (level - goal, v, 0);
        self.drop.level = level;
        self.drop.frame_seen = None;
    }

    /// One shown frame of the targets' spring. True while they still move.
    pub fn drop_step(&mut self, frame_count: u32) -> bool {
        if !self.drop.moving() {
            return false;
        }
        let steps = match self.drop.frame_seen {
            Some(f) => frame_count.wrapping_sub(f) as i32,
            None => 1,
        };
        self.drop.frame_seen = Some(frame_count);
        if steps <= 0 {
            return true;
        }
        let t = self.drop.seg.2 + steps;
        let (x, v) = spring(self.drop.seg.0, self.drop.seg.1, t);
        // At rest once under a 256th of the way and of a 256th a frame: nothing more would show.
        if x.abs() < 1. / (1 << 8) as f64 && v.abs() < 1. / (1 << 8) as f64 {
            self.drop.level = self.drop.goal;
            self.drop.seg = (0., 0., 0);
        } else {
            self.drop.level = self.drop.goal + x;
            self.drop.seg.2 = t;
        }
        // The helm's marks fade as the targets grow (display op 54, corner 10). WHY the bounds: the spring may overshoot a little past 0 or 1; the level sent is the field's range.
        let hide = (self.drop.level.clamp(0., 1.) * 65535.) as u16;
        if hide != self.drop.hide_sent {
            self.drop.hide_sent = hide;
            let l = hide.to_le_bytes();
            let mut r = [0u8; 1];
            call(self.display, &[54, 10, l[0], l[1]], &mut r);
        }
        true
    }

    /// The targets' centres on the glass (pixels) and their radius whole.
    fn drop_places(&self) -> ([(u8, f64, f64); 3], f64) {
        let (w, h) = (self.w as f64, self.h as f64);
        let span = 2. * w * h / (w + h);
        ([(ABOUT, 0., h), (UNSTOW, w, h), (STOP, w * 0.5, 0.)], span / (1 << SHARE) as f64)
    }

    /// Which targets this drag shows.
    fn drop_shows(&self, what: u8) -> bool {
        match what {
            ABOUT => true,
            UNSTOW => self.drop.unstow,
            _ => self.drop.stop,
        }
    }

    /// The finger at (x, y) on the glass during the drag: the target under it lit, by the rule the targets are drawn with. True when that changed.
    pub fn drop_over(&mut self, x: i64, y: i64) -> bool {
        if self.drop.held.is_none() {
            return false;
        }
        let (places, rt) = self.drop_places();
        // WHY the floor: a drag that begins while the targets are still shrinking from the last starts from the spring's small overshoot below 0, and a negative radius squared would light a target the glass does not show.
        let r = rt * self.drop.level.max(0.);
        let lit = places.iter().find(|&&(what, cx, cy)| self.drop_shows(what) && (x as f64 - cx) * (x as f64 - cx) + (y as f64 - cy) * (y as f64 - cy) < r * r).map_or(0, |p| p.0);
        let changed = lit != self.drop.lit;
        self.drop.lit = lit;
        changed
    }

    /// The fingers lifted. Dropped on a target: the deck back as it stood before the drag, and the target acts; what was said, for the console. None when the drag ended anywhere else (the deck as the drag left it).
    pub fn drop_lift(&mut self) -> Option<&'static str> {
        let lit = self.drop.lit;
        let before = self.drop.held.take()?;
        self.drop.lit = 0;
        self.drop_goal(0.);
        if lit == 0 {
            return None;
        }
        // Every circle where it stood (owner: "previous deck arrangement should restore after the target is dropped"): the dragged one and every one it pushed.
        // WHY take: a circle added or taken while the drag went on (a stow, an unstow from another device) leaves the two lists apart; only the circles both hold go back.
        for (i, &(at, arranged)) in before.places.iter().enumerate().take(self.deck.n()) {
            self.deck.circles[i].arranged = arranged;
            if self.deck.circles[i].at != at {
                self.deck.circles[i].at = at;
                self.send_move(i);
            }
        }
        self.scatter = before.scatter;
        let i = before.i;
        Some(match lit {
            ABOUT => self.about(i),
            UNSTOW => self.unstow_ask(i),
            _ => match i.checked_sub(1) {
                Some(k) => self.stop(k),
                None => "map: stop: the settings circle never stops",
            },
        })
    }

    /// Kist k's card on the Deck page, at its top (a row tapped, the deck's About and Unstow); a kist that wants the person's people has their choice read for it (`people_thread`).
    pub fn card_open(&mut self, k: usize) {
        self.card = self.kists.kists.get(k).map(|kist| kist.capsule);
        self.scroll = 0;
        if let Some(kist) = self.kists.kists.get(k).filter(|kist| kist.wants.iter().any(|w| w.right == ferros_kist::PEOPLE)) {
            let mut m = [27u8; 65];
            m[1..33].copy_from_slice(&kist.developer);
            m[33..].copy_from_slice(&kist.option);
            people_ask(&m);
        }
    }

    /// Kist k's Unstow sheet, in its card's place, at its top, at the design's defaults; what it kept for itself counted by the stower meanwhile (stower.rs `size_ask`, drawn when it comes).
    pub fn unstow_sheet(&mut self, k: usize) {
        let Some(kist) = self.kists.kists.get(k) else { return };
        self.sheet = Some(Sheet { capsule: kist.capsule, keep_data: false, keep_made: true, keep_answers: true, here_only: false });
        if self.sheet_size.is_none_or(|s| s.0 != kist.capsule) {
            self.sheet_size = None;
        }
        stower::size_ask(&kist.capsule, &kist.developer, &kist.option);
        self.scroll = 0;
    }

    /// Circle i's page: a kist's or a puck's card on the Deck page; the settings circle's, the About page.
    pub fn about(&mut self, i: usize) -> &'static str {
        let Some(k) = i.checked_sub(1).filter(|&k| k < self.kists.kists.len()) else {
            self.show_page("about");
            return "map: about: the settings";
        };
        self.card_open(k);
        self.sheet = None;
        self.unstow_armed = None;
        self.show_page("deck");
        "map: about: its card"
    }

    /// Circle i's unstow asked, never done at once: its card on the Deck page with a kist's sheet open (its choices, then its Unstow), or a puck's remove armed (its second tap removes it).
    pub fn unstow_ask(&mut self, i: usize) -> &'static str {
        let Some(k) = i.checked_sub(1).filter(|&k| k < self.kists.kists.len()) else { return "map: unstow: the settings circle stays" };
        self.card_open(k);
        if self.kists.kists[k].is_puck() {
            self.sheet = None;
            self.unstow_armed = Some(k as u8);
        } else {
            self.unstow_sheet(k);
        }
        self.show_page("deck");
        "map: unstow: asked on its card"
    }

    /// The person's Stop on kist `k` (its card, its row on the Deck page, the deck's Stop target): one path for all of them, init's OP_STOP at once, one shot dead. A puck stopped stays down (`pucks_held`) until its circle is tapped, its circle showing its name alone meanwhile (the display draws a puck's name while it hands no square; owner: "Stopped puck shows name only").
    pub fn stop(&mut self, k: usize) -> &'static str {
        let Some((capsule, puck)) = self.kists.kists.get(k).map(|kist| (kist.capsule, kist.is_puck())) else { return "map: stop: no such kist" };
        if puck && !self.pucks_held.contains(&capsule) {
            self.pucks_held.push(capsule);
        }
        match (self.stop_now(&capsule), puck) {
            (true, true) => "map: stop: the puck ended, its name alone until tapped",
            (true, false) => "map: stop: ended",
            (false, _) => "map: stop: init did not stop it",
        }
    }

    /// Init's OP_STOP on `capsule`: true when it was running and is ended, or was not running.
    fn stop_now(&self, capsule: &[u8; 32]) -> bool {
        let mut m = [0u8; 33];
        m[0] = ferros_kist::launch::OP_STOP;
        m[1..].copy_from_slice(capsule);
        let mut r = [0u8; 1];
        let n = call(cap(self.grants, S_LAUNCH), &m, &mut r);
        n != ferros_sys::ERR && n >= 1 && (r[0] == ferros_kist::launch::STOPPED || r[0] == ferros_kist::launch::NOT_RUNNING)
    }

    /// The targets into the deck's frame, front to back: each one's word, then its disc, then its edge; nothing at rest.
    pub fn drop_draw(&self, b: &mut Batch) {
        if self.drop.level <= 0. {
            return;
        }
        let (places, rt) = self.drop_places();
        let level = self.drop.level;
        let th = ferros_sys::theme::current();
        // WHY the bounds: the spring overshoots a little past 1; the opacity is the field's range.
        let alpha = (level.clamp(0., 1.) * 255.) as u8;
        for &(what, cx, cy) in &places {
            if !self.drop_shows(what) {
                continue;
            }
            let r = rt * level;
            let lit = self.drop.lit == what;
            // The word at the middle of what shows of the disc: a corner's on its diagonal, the top's straight down, sized by the disc.
            let (wx, wy) = match what {
                ABOUT => (cx + r * 0.42, cy - r * 0.42),
                UNSTOW => (cx - r * 0.42, cy - r * 0.42),
                _ => (cx, cy + r * 0.62),
            };
            let word = tr(match what {
                ABOUT => Msg::PageAbout,
                UNSTOW => Msg::Unstow,
                _ => Msg::Stop,
            });
            let px = (rt * (1. / (1 << 3) as f64)) as u16;
            b.turned(wx as i64, wy as i64, px, if lit { th.plane } else { th.text }, alpha, 1 << 14, 0, word);
            b.disc(cx as i64, cy as i64, r as i64, if lit { th.lit } else { th.plane });
            b.disc(cx as i64, cy as i64, r as i64 + 2, if lit { th.text } else { th.ring });
        }
    }
}
