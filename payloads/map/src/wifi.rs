//! The WiFi page's picker (docs/WIFI.md "The picker"; owner, 2026-11-18, on the Pro out and about: "there is no ssid connect option or anything"): the networks in range with their measures as numbers (never bars), the networks remembered, a network picked, its passphrase typed on the keyboard, the join, and a remembered network forgotten.
//! It is the WiFi link's page, under its switch: the page the radios corner's WiFi circle opens (dragged toward the centre, WINDOWS.md "The corners": "Top right, the radios: connections, WiFi join") and the one Settings and Connections reach.
//! The list comes from a thread holding the driver's NETS (payloads/pcie1 op 12) while the page shows, answered as networks are found; forgetting and joining a remembered network go thru a second thread, so the page never waits on the driver.
//! The passphrase is typed on the attest screen's keyboard (ahau), asked for thru the identity service's words (fgtw op 15 [4, name]) as the People and Fleet pages ask for theirs, and ahau hands it straight to the driver: it never passes thru the map.

use super::*;
use core::sync::atomic::{AtomicU32, AtomicU64};

/// Grant slot 10: use wlan (init's slot 44, since 2026-11-18): NETS, FORGET and JOIN KEPT on the WiFi driver. The kernel's torch is the slot after it (S_TORCH).
pub const S_WLAN: u64 = 10;

/// Whether this map was handed `wlan`: its grants page counts 12 slots or more (the console, init's ten, the kernel's torch after them). An init from before hands nine, the torch at 10, and the page then says there is no list.
pub fn granted(grants: u64) -> bool {
    // SAFETY: the grants page is this process's, its slot count at G#200 (as every driver reads it).
    unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + grants + 0x200) as *const u32) > 11 }
}

/// NETS's reply at most: its 5-byte head and the driver's 4 KiB list (pcie1 `NETS_MAX`).
const LIST: usize = (1 << 12) + 8;
/// Networks in range the page lists, and remembered ones not in range: sixteen each at most (the page's tap rows are 64 in all, and its knobs come after).
const SHOWN: usize = 1 << 4;

/// The page showing (0 while it does not, else which showing this is: SHOWS counts them), for the watch thread.
static OPEN: AtomicU64 = AtomicU64::new(0);
static SHOWS: AtomicU64 = AtomicU64::new(0);
/// The driver's list as the watch thread last had it (NETS's reply after its head), under a sequence count (odd while it is written); length 0: no answer from a driver with NETS.
static SEQ: AtomicU32 = AtomicU32::new(0);
static mut COPY: ([u8; LIST], usize) = ([0; LIST], 0);
/// A request for the driver (FORGET, JOIN KEPT): its bytes, and which request this is (counted); the driver's reply byte comes back in DONE as the count << 8 | the byte (255: no driver answered).
static ASK: AtomicU64 = AtomicU64::new(0);
static mut ASK_REQ: ([u8; 2 + 32], usize) = ([0; 2 + 32], 0);
static DONE: AtomicU64 = AtomicU64::new(0);

/// The NETS watch, on a thread of its own: asleep while the page does not show; while it shows, NETS held at the driver until the list changes (a network found, a scan begun or ended, one kept or forgotten), then published, and asked again with the count it gave. A NETS held with nothing new starts the driver's next scan, so it scans only while this page shows.
/// A driver not serving `wlan` (restarted, or switched off): asked again after a doubling wait, from 1/64 s up to a second, as the NFC page's add does, while the page shows: no doorbell rings when a driver starts.
pub extern "C" fn watch(grants: u64) -> ! {
    use core::sync::atomic::Ordering;
    let svc = cap(grants, S_WLAN);
    let mut count = u32::MAX;
    let mut opened = 0u64;
    let mut absent = freq() >> 6;
    let mut out = [0u8; LIST];
    loop {
        let w = OPEN.load(Ordering::Acquire);
        if w == 0 || !granted(grants) {
            ferros_sys::wait_world(&OPEN, w, 0);
            continue;
        }
        // A new showing: u32::MAX is never a count the driver gives, so its first NETS is answered at once with the list as it stands.
        if w != opened {
            opened = w;
            count = u32::MAX;
        }
        let mut m = [12u8, 0, 0, 0, 0];
        m[1..].copy_from_slice(&count.to_le_bytes());
        let n = ferros_sys::call_live(svc, &m, &mut out);
        // No driver serving, or one from before NETS (a reply of no bytes): nothing published but the absence, and asked again after the wait.
        if n == ferros_sys::ERR || n == ferros_sys::NOTHING || n < 7 {
            publish(&[]);
            sleep_until(now() + absent);
            if absent < freq() {
                absent <<= 1;
            }
            continue;
        }
        absent = freq() >> 6;
        count = u32::from_le_bytes([out[0], out[1], out[2], out[3]]);
        // The page went (or showed again) while the call was held: this list is not that showing's.
        if OPEN.load(Ordering::Acquire) != w {
            continue;
        }
        // WHY the min: the reply's length is the driver's word; the list is kept only as far as the buffer took it.
        let k = (n as usize).min(out.len());
        publish(&out[5..k]);
    }
}

fn publish(b: &[u8]) {
    use core::sync::atomic::Ordering;
    SEQ.fetch_add(1, Ordering::AcqRel);
    unsafe {
        let c = &mut *(&raw mut COPY);
        c.0[..b.len()].copy_from_slice(b);
        c.1 = b.len();
    }
    SEQ.fetch_add(1, Ordering::AcqRel);
}

/// The requests' thread: asleep on ASK; each request called on the driver (CALL_LIVE: a driver that is not serving fails at once) and its reply byte left in DONE.
pub extern "C" fn asker(grants: u64) -> ! {
    use core::sync::atomic::Ordering;
    let svc = cap(grants, S_WLAN);
    let mut done = 0u64;
    loop {
        let v = ASK.load(Ordering::Acquire);
        if v == done {
            ferros_sys::wait_world(&ASK, v, 0);
            continue;
        }
        done = v;
        let (q, n) = unsafe { *(&raw const ASK_REQ) };
        let mut r = [0u8; 4];
        let k = if granted(grants) { ferros_sys::call_live(svc, &q[..n], &mut r) } else { ferros_sys::ERR };
        let byte = if k == ferros_sys::ERR || k == ferros_sys::NOTHING || k < 1 { 255 } else { r[0] as u64 };
        DONE.store(v << 8 | byte, Ordering::Release);
    }
}

/// A request handed to `asker`: [op, name length, name].
fn ask(op: u8, name: &Name) -> u64 {
    use core::sync::atomic::Ordering;
    unsafe {
        let q = &mut *(&raw mut ASK_REQ);
        q.0[0] = op;
        q.0[1] = name.n as u8;
        q.0[2..2 + name.n].copy_from_slice(&name.b[..name.n]);
        q.1 = 2 + name.n;
    }
    let w = ASK.fetch_add(1, Ordering::AcqRel) + 1;
    ferros_sys::wake(&ASK, w);
    w
}

/// A network's name: its SSID's octets (an SSID is octets, not text).
#[derive(Clone, Copy, PartialEq)]
pub struct Name {
    b: [u8; 32],
    n: usize,
}
impl Name {
    fn of(s: &[u8]) -> Name {
        let mut b = [0u8; 32];
        b[..s.len()].copy_from_slice(s);
        Name { b, n: s.len() }
    }
    fn bytes(&self) -> &[u8] {
        &self.b[..self.n]
    }
    /// The name for the page: as text when its octets are UTF-8 with no control characters, else as hexadecimal (the honest form of a name that is not text, as the link's detail writes it).
    fn line(&self) -> Line {
        let mut l = Line::new();
        match core::str::from_utf8(self.bytes()) {
            Ok(t) if !t.chars().any(char::is_control) => {
                l.s(t);
            }
            _ => {
                for b in self.bytes() {
                    let _ = write!(l, "{b:02X}");
                }
            }
        }
        l
    }
}

/// A tap on the picker: a network in range (its row), a remembered one not in range (its row), or the picked network's buttons.
#[derive(Clone, Copy, PartialEq)]
pub enum Act {
    Pick(u8),
    Kept(u8),
    /// Its passphrase on the keyboard.
    Type,
    /// A remembered one joined now (JOIN KEPT).
    Join,
    /// Forgotten (two taps).
    Forget,
}

/// What the page says of the last thing done.
#[derive(Clone, Copy, PartialEq)]
enum Said {
    Nothing,
    /// The keyboard asked for this network's passphrase.
    Typing(Name),
    /// A join under way.
    Joining(Name),
    Joined(Name),
    /// A join that did not take: the driver's outcome (1 out of reach, 2 refused, 3 timed out; 0 another network was joined instead).
    Failed(Name, i64),
    Forgot(Name),
    /// A request the driver refused, in words.
    Refused(Msg),
}

/// The picker's state, the main loop's: the list as last taken, the network picked, the forget armed, what the page says.
pub struct Picker {
    list: [u8; LIST],
    len: usize,
    seen: u32,
    pick: Option<Name>,
    armed: Option<Name>,
    said: Said,
    shown: bool,
    done_seen: u64,
    /// The request asked last (its count, its op, the network), for its reply.
    asked: (u64, u8, Name),
}

impl Picker {
    pub fn new() -> Picker {
        Picker { list: [0; LIST], len: 0, seen: 0, pick: None, armed: None, said: Said::Nothing, shown: false, done_seen: 0, asked: (0, 0, Name::of(&[])) }
    }
}

/// A network in range, as NETS gives it (pcie1 `nets_reply`).
#[derive(Clone, Copy)]
struct Net {
    name: Name,
    rssi: i8,
    flags: u8,
    noise: i8,
    aps: u16,
    freq: u16,
    width: u16,
    /// 0 not remembered, 1 the person's, 2 the device's.
    kept: u8,
}

/// A network remembered, as NETS gives it: whose (0 the person's, 1 the device's), whether it is the one joined now, its name, when this device last joined it.
#[derive(Clone, Copy)]
struct Kept {
    class: u8,
    now: bool,
    name: Name,
    joined: Option<i64>,
}

/// The list read: whether a scan runs, the networks in range (strongest first) and the remembered ones. None for a list that is not whole.
/// WHY every length check: the bytes are another process's (the driver's); one that runs short ends the list rather than indexing past it.
fn parse(b: &[u8]) -> Option<(bool, Vec<Net>, Vec<Kept>)> {
    let scanning = *b.first()? != 0;
    let count = *b.get(1)? as usize;
    let mut at = 2;
    let mut nets = Vec::with_capacity(count);
    for _ in 0..count {
        let n = *b.get(at)? as usize;
        let e = b.get(at + 1..at + 1 + n + 10)?;
        if n > 32 {
            return None;
        }
        let u16_at = |k: usize| u16::from_le_bytes([e[n + k], e[n + k + 1]]);
        nets.push(Net { name: Name::of(&e[..n]), rssi: e[n] as i8, flags: e[n + 1], noise: e[n + 2] as i8, aps: u16_at(3), freq: u16_at(5), width: u16_at(7), kept: e[n + 9] });
        at += 1 + n + 10;
    }
    let count = *b.get(at)? as usize;
    at += 1;
    let mut kept = Vec::with_capacity(count);
    for _ in 0..count {
        let h = b.get(at..at + 3)?;
        let n = h[2] as usize;
        let e = b.get(at + 3..at + 3 + n + 8)?;
        if n > 32 {
            return None;
        }
        let when = i64::from_le_bytes(e[n..n + 8].try_into().ok()?);
        kept.push(Kept { class: h[0], now: h[1] != 0, name: Name::of(&e[..n]), joined: (when != i64::MIN).then_some(when) });
        at += 3 + n + 8;
    }
    Some((scanning, nets, kept))
}

/// WiFi switched off by its status circle (preference 15, bit 1 in the facts' order).
fn switched_off() -> bool {
    ferros_sys::pref(ferros_sys::PREF_RADIOS_OFF) >> 1 & 1 == 1
}

/// The main loop's turn, once a pass: the watch started or stopped as the page shows or goes, the list taken when the watch moved, a request's reply taken. True when the page needs drawing again.
pub fn pass(map: &mut Map, page_shown: bool) -> bool {
    use core::sync::atomic::Ordering;
    // WiFi switched off (its status circle, preference 15 bit 1): no driver runs, so nothing is watched; the page says it is off.
    let page_shown = page_shown && !switched_off();
    let mut dirty = false;
    let p = &mut map.wifi;
    if page_shown != p.shown {
        p.shown = page_shown;
        let v = if page_shown { SHOWS.fetch_add(1, Ordering::AcqRel) + 1 } else { 0 };
        OPEN.store(v, Ordering::Release);
        ferros_sys::wake(&OPEN, 1);
        // A forget armed is let go with the page; the pick and what the page said stay for its next showing.
        p.armed = None;
        dirty = true;
    }
    let s0 = SEQ.load(Ordering::Acquire);
    if s0 != p.seen && s0 & 1 == 0 {
        let (b, n) = unsafe { *(&raw const COPY) };
        if SEQ.load(Ordering::Acquire) == s0 {
            p.seen = s0;
            if (n, &b[..n]) != (p.len, &p.list[..p.len]) {
                p.list[..n].copy_from_slice(&b[..n]);
                p.len = n;
                dirty = true;
            }
        }
    }
    let d = DONE.load(Ordering::Acquire);
    if d != p.done_seen {
        p.done_seen = d;
        let (count, op, name) = p.asked;
        if d >> 8 == count {
            let r = d & 0xFF;
            p.said = match (op, r) {
                (13, 0) => Said::Joining(name),
                (13, 1) => Said::Refused(Msg::NotRemembered),
                (11, 0) => {
                    if p.pick == Some(name) {
                        p.pick = None;
                    }
                    Said::Forgot(name)
                }
                (11, 1) => Said::Refused(Msg::NotRemembered),
                (11, 2) => Said::Refused(Msg::CouldNotForget),
                _ => Said::Refused(Msg::WifiNotAnswering),
            };
            dirty = true;
        }
    }
    dirty
}

impl Map {
    /// A tap on the picker.
    pub(super) fn wifi_tap(&mut self, act: Act) {
        let p = &self.wifi;
        let parsed = parse(&p.list[..p.len]);
        let (nets, kept) = parsed.map(|(_, n, k)| (n, k)).unwrap_or_default();
        let away: Vec<&Kept> = kept.iter().filter(|k| !nets.iter().any(|n| n.name == k.name)).collect();
        let p = &mut self.wifi;
        let picked = match act {
            Act::Pick(i) => nets.get(i as usize).map(|n| n.name),
            Act::Kept(i) => away.get(i as usize).map(|k| k.name),
            _ => None,
        };
        match act {
            // A row: that network picked (its facts and buttons under it), or let go when it was.
            Act::Pick(_) | Act::Kept(_) => {
                p.pick = if picked == p.pick { None } else { picked };
                p.armed = None;
            }
            // Its passphrase asked of the keyboard (the identity service's words, kind 4); the keyboard hands it to the driver.
            Act::Type => {
                if let Some(name) = p.pick {
                    let mut m = [0u8; 3 + 32];
                    m[0] = 15;
                    m[1] = 4;
                    m[2] = name.n as u8;
                    m[3..3 + name.n].copy_from_slice(name.bytes());
                    people_ask(&m[..3 + name.n]);
                    p.said = Said::Typing(name);
                    p.armed = None;
                }
            }
            Act::Join => {
                if let Some(name) = p.pick {
                    let w = ask(13, &name);
                    p.asked = (w, 13, name);
                    p.said = Said::Joining(name);
                    p.armed = None;
                }
            }
            // Two taps: the first arms it, the second forgets it.
            Act::Forget => {
                if let Some(name) = p.pick {
                    if p.armed == Some(name) {
                        let w = ask(11, &name);
                        p.asked = (w, 11, name);
                        p.armed = None;
                    } else {
                        p.armed = Some(name);
                    }
                }
            }
        }
    }

    /// How a join the page started goes, read from the WiFi link's facts and its detail (the network joined): STATE 2 with this network's name joined, the driver's JOIN measure 1 to 3 failed, another network's name joined instead failed. Read as the page draws, which its link's bell brings.
    fn wifi_follow(&mut self) {
        let name = match self.wifi.said {
            Said::Typing(n) | Said::Joining(n) => n,
            _ => return,
        };
        let f = ferros_sys::facts();
        // The WiFi link's measures: facts 41 to 48 (33 + 8 × link 1).
        let wifi = |kind: u16| f.0[41..49].iter().filter_map(|&m| ferros_sys::measure_of(m)).find(|m| m.0 == kind).map(|m| m.1);
        let (state, join) = (wifi(ferros_sys::measure::STATE), wifi(ferros_sys::measure::JOIN));
        let typing = matches!(self.wifi.said, Said::Typing(_));
        if typing && join == Some(0) {
            self.wifi.said = Said::Joining(name);
            return;
        }
        if let (false, Some(o @ 1..=3)) = (typing, join) {
            self.wifi.said = Said::Failed(name, o);
            return;
        }
        if state == Some(2) {
            match self.wifi_joined() {
                Some(j) if j == name => self.wifi.said = Said::Joined(name),
                Some(_) if !typing => self.wifi.said = Said::Failed(name, 0),
                _ => {}
            }
        }
    }

    /// The network joined now, from the WiFi link's detail (its `ssid`, read with the Settings capability).
    fn wifi_joined(&self) -> Option<Name> {
        let mut doc = [0u8; 1 << 10];
        let n = ferros_sys::detail(cap(self.grants, S_SETTINGS), ferros_sys::control::WIFI, &mut doc)?;
        // WHY the bound: the kernel returns the document's whole length, which may be past the buffer; a document not held whole is not read.
        if n == 0 || n > doc.len() {
            return None;
        }
        let fields = ferros_detail::parse(ferros_sys::control::WIFI, &doc[..n])?;
        fields.iter().find(|(f, _, _)| *f == "ssid").and_then(|(_, _, v)| match v {
            ferros_detail::vsf::VsfType::nw(s) => Some(Name::of(s.as_bytes())),
            _ => None,
        })
    }

    /// The picker's rows on the WiFi page, from `y` on: the networks in range, each a row (its name, and its signal or "connected"), the one picked with its measures and buttons under it; then the networks remembered and not in range; what the page says of the last thing done at the top. In the page's own look (`draw_page`'s rows, facts, buttons and headings).
    pub(super) fn wifi_rows(&mut self, b: &mut Batch, y: &mut i64, cx: i64, hit: bool) {
        // Switched off: the switch and the state above say so, and there is nothing to pick from.
        if switched_off() {
            return;
        }
        let uq = self.unit();
        let uu = |n: i64, d: i64| n * uq / (d << 8);
        let half = self.w / 2;
        let (small, note) = (uu(5, 4), uu(7, 8));
        let f = ferros_sys::numbers(Style::Glyphs);
        // Only the rows in the page's band (and a row past either edge) are sent: a frame's ops go in one call of at most 4 KiB (the display's batch, `Batch`), whose ops past it are dropped, the camera's among them, and sixteen networks with one's measures open would take most of it. A row out of the band is laid out (its height counts, as the page scrolls) and never drawn or tapped.
        let (_, band_top, band_bottom) = self.bands();
        let seen = move |y: i64| y > band_top - uu(3, 1) && y < band_bottom + uu(3, 1);
        let header = |b: &mut Batch, y: &mut i64, t: &str| {
            *y += uu(1, 2);
            if seen(*y) {
                b.text(cx, *y, note as u16, dim(), 1, t);
            }
            *y += uu(2, 1);
        };
        // A line of words, centred, wrapped at spaces to the page's width by the typeface's average advance (five eighths of its size), as `draw_page`'s rows are.
        let words = |b: &mut Batch, y: &mut i64, colour: u32, t: &str| {
            let fits = ((2 * half - uu(2, 1)) * 8 / (small * 5)) as usize;
            let mut rest = t;
            loop {
                let cut = if rest.len() <= fits {
                    rest.len()
                } else {
                    let mut k = fits;
                    while !rest.is_char_boundary(k) {
                        k -= 1;
                    }
                    rest[..k].rfind(' ').map_or(k, |k| k + 1)
                };
                // WHY: a line that takes nothing (fewer than one character fits) would leave `rest` as it was and never end the loop. PROOF: every other case cuts at least one byte. A line takes at least its first character.
                let cut = if cut == 0 { rest.chars().next().map_or(rest.len(), char::len_utf8) } else { cut };
                if seen(*y) {
                    b.text(cx, *y, small as u16, colour, 1, rest[..cut].trim_end());
                }
                rest = rest[cut..].trim_start();
                if rest.is_empty() {
                    break;
                }
                *y += uu(3, 2);
            }
            *y += uu(3, 1);
        };
        // A fact: its name right of the centre's left (white when it is the network picked), its value left of the centre's right; tappable across the page when `what` is given.
        let fact = |m: &mut Map, b: &mut Batch, y: &mut i64, name: &str, lit: bool, value: &str, what: Option<Act>| {
            let y0 = *y;
            *y += uu(2, 1) + uu(1, 4);
            if !seen(y0) {
                return;
            }
            b.text(cx - uu(1, 1), y0, small as u16, if lit { white() } else { grey() }, 2, name);
            b.text(cx + uu(1, 1), y0, small as u16, white(), 0, value);
            if let (true, Some(a)) = (hit, what) {
                m.push_row(cx - half, cx + half, y0 - uu(2, 1), *y - uu(1, 4), Choice::Wifi(a));
            }
        };
        let button = |m: &mut Map, b: &mut Batch, y: &mut i64, t: &str, armed: bool, a: Act| {
            if !seen(*y) {
                *y += uu(3, 1);
                return;
            }
            let [fill, held, tl, br] = BUTTON_LOOK[ferros_sys::theme::index()];
            let down = armed || m.pressed == Some(Choice::Wifi(a));
            b.button(cx, *y - uu(1, 2), uu(16, 1), uu(2, 1), uu(5, 4), [if down { held } else { fill }, tl, br, white()], t);
            if hit {
                m.push_row(cx - half, cx + half, *y - uu(2, 1), *y + uu(1, 1), Choice::Wifi(a));
            }
            *y += uu(3, 1);
        };
        let measure = |kind: u16, v: i64| {
            let mut l = Line::new();
            let what = measure_words(&mut l, f, kind, v);
            (what, l)
        };
        header(b, y, tr(Msg::NetworksInRange));
        if !granted(self.grants) {
            words(b, y, white(), tr(Msg::NoNetworkList));
            return;
        }
        // The join followed only on the page drawn live (the layout passes and a neighbour's slide draw it too, and need not read the facts again).
        if hit {
            self.wifi_follow();
        }
        let said = match self.wifi.said {
            Said::Nothing => None,
            Said::Typing(n) => Some(say(Say::TypeOnKeyboard(n.line().as_str()))),
            Said::Joining(n) => Some(say(Say::JoiningNet(n.line().as_str()))),
            Said::Joined(n) => Some(say(Say::JoinedNet(n.line().as_str()))),
            Said::Failed(n, 1) => Some(say(Say::OutOfReach(n.line().as_str()))),
            Said::Failed(n, 2) => Some(say(Say::RefusedPassphrase(n.line().as_str()))),
            Said::Failed(n, _) => Some(say(Say::DidNotAnswer(n.line().as_str()))),
            Said::Forgot(n) => Some(say(Say::Forgotten(n.line().as_str()))),
            Said::Refused(m) => {
                let mut l = Line::new();
                l.s(tr(m));
                Some(l)
            }
        };
        if let Some(l) = said {
            words(b, y, white(), l.as_str());
        }
        let Some((scanning, nets, kept)) = parse(&self.wifi.list[..self.wifi.len]) else {
            words(b, y, white(), tr(if self.wifi.len == 0 { Msg::WifiStarting } else { Msg::NoNetworkList }));
            return;
        };
        if nets.is_empty() {
            words(b, y, white(), tr(if scanning { Msg::LookingForNetworks } else { Msg::NoNetworksFound }));
        }
        let (pick, armed) = (self.wifi.pick, self.wifi.armed);
        // The picked network's buttons: joined already none; WPA2-PSK and remembered, join and a new passphrase; WPA2-PSK, its passphrase; anything else, why not. A remembered one can be forgotten (two taps).
        let buttons = |m: &mut Map, b: &mut Batch, y: &mut i64, name: Name, joinable: bool, now: bool, remembered: bool| {
            if joinable && !now && remembered {
                button(m, b, y, tr(Msg::Join), false, Act::Join);
                button(m, b, y, tr(Msg::NewPassphrase), false, Act::Type);
            } else if joinable && !now {
                button(m, b, y, tr(Msg::TypePassphrase), false, Act::Type);
            } else if !joinable {
                words(b, y, dim(), tr(Msg::JoinsWpa2Only));
            }
            if remembered {
                let armed = armed == Some(name);
                button(m, b, y, tr(if armed { Msg::TapAgainForget } else { Msg::Forget }), armed, Act::Forget);
            }
        };
        // Whose list keeps it, and when this device last joined it.
        let kept_facts = |m: &mut Map, b: &mut Batch, y: &mut i64, k: &Kept| {
            words(b, y, grey(), tr(if k.class == 0 { Msg::KeptForYou } else { Msg::KeptForDevice }));
            let mut when = Line::new();
            match k.joined {
                // Eagle time to the wall clock in the zone, for the eye only (tatau writes it in the device's base); the epoch itself is a network kept before 2026-11-18, joined at a time not kept.
                Some(t) if t != 0 => {
                    const EPOCH_UNIX: i64 = -14_182_940;
                    let ops = ferros_kist::vsf::OSCILLATIONS_PER_SECOND as i64;
                    let secs = t.div_euclid(ops) + EPOCH_UNIX + ferros_sys::zone();
                    let ns = (t.rem_euclid(ops) as i128 * 1_000_000_000 / ops as i128) as u32;
                    let _ = tatau::date_time(&mut when, f, secs, ns, t);
                }
                Some(_) => {
                    when.s("-");
                }
                None => {
                    when.s(tr(Msg::NotJoinedYet));
                }
            }
            fact(m, b, y, tr(Msg::LastJoined), false, when.as_str(), None);
        };
        for (i, n) in nets.iter().take(SHOWN).enumerate() {
            let k = kept.iter().find(|k| k.name == n.name);
            let now = k.is_some_and(|k| k.now);
            let mut v = Line::new();
            if now {
                v.s(tr(Msg::Connected));
            } else {
                v = measure(ferros_sys::measure::RSSI, n.rssi as i64 * 1000).1;
                if n.kept != 0 {
                    v.s(", ");
                    v.s(tr(Msg::KeptTag));
                }
            }
            let lit = pick == Some(n.name);
            fact(self, b, y, n.name.line().as_str(), lit, v.as_str(), Some(Act::Pick(i as u8)));
            if !lit {
                continue;
            }
            // The picked network's measures (owner, 2026-11-03: many dimensions as numbers, never bars), each in the device's base.
            let mut facts: [Option<(&str, Line)>; 4] = [None, None, None, None];
            facts[0] = Some(measure(ferros_sys::measure::RSSI, n.rssi as i64 * 1000));
            if n.noise != 0 {
                facts[1] = Some(measure(ferros_sys::measure::SNR, (n.rssi as i64 - n.noise as i64) * 1000));
            }
            if n.freq != 0 {
                facts[2] = Some(measure(ferros_sys::measure::FREQ, n.freq as i64 * 1000));
            }
            if n.width != 0 {
                facts[3] = Some(measure(ferros_sys::measure::BANDWIDTH, n.width as i64 * 1000));
            }
            for (what, l) in facts.iter().flatten() {
                fact(self, b, y, what, false, l.as_str(), None);
            }
            let mut aps = Line::new();
            if let Ok(tatau::Out::Nothing) = tatau::count(&mut aps, f, n.aps as u64, 1) {
                aps.s("0");
            }
            fact(self, b, y, tr(Msg::AccessPoints), false, aps.as_str(), None);
            // The bands it answered on (flags bits 5, 3, 6: 2.4, 5 and 6 GHz).
            let mut bands = Line::new();
            for (bit, band) in [(5, "2.4 GHz"), (3, "5 GHz"), (6, "6 GHz")] {
                if n.flags >> bit & 1 != 0 {
                    if bands.n > 0 {
                        bands.s(", ");
                    }
                    bands.s(band);
                }
            }
            if bands.n > 0 {
                fact(self, b, y, tr(Msg::Band), false, bands.as_str(), None);
            }
            let joinable = n.flags & 1 != 0;
            let security = if joinable { Msg::Wpa2Personal } else if n.flags & 2 != 0 { Msg::SecurityOpen } else { Msg::SecurityOther };
            fact(self, b, y, tr(Msg::Security), false, tr(security), None);
            if let Some(k) = k {
                kept_facts(self, b, y, k);
            }
            buttons(self, b, y, n.name, joinable, now, k.is_some());
        }
        // The remembered networks the last scan did not see: each its row, picked to forget it.
        header(b, y, tr(Msg::NetworksRemembered));
        let away: Vec<&Kept> = kept.iter().filter(|k| !nets.iter().any(|n| n.name == k.name)).collect();
        if kept.is_empty() {
            words(b, y, white(), tr(Msg::NoneYet));
        }
        for (i, k) in away.iter().take(SHOWN).enumerate() {
            let lit = pick == Some(k.name);
            fact(self, b, y, k.name.line().as_str(), lit, tr(Msg::NotSeenLastScan), Some(Act::Kept(i as u8)));
            if lit {
                kept_facts(self, b, y, k);
                let armed = armed == Some(k.name);
                button(self, b, y, tr(if armed { Msg::TapAgainForget } else { Msg::Forget }), armed, Act::Forget);
            }
        }
        words(b, y, dim(), tr(Msg::NetworksKeptNote));
    }
}
