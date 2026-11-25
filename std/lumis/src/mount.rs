//! Lumis's camera mount over Bluetooth (lumis ACCESSORIES.md; BT.md "The client ops"): a session on the `bt` endpoint (the Bluetooth right, slot 9), the accessory protocol on an L2CAP credit-based channel (PSM G#81), and lumis_core's `spin` state between it and the UI: the menu's MOUNT row asks for a pairing and confirms its code, the rate slider's rate goes to the mount when it changes, the mount's battery comes back.
//! Pairing finds a mount in pairing mode (its class byte's top bit), offers, shows the code, and once the person confirmed here and on the mount keeps the pairing in Lumis's vault (`Pairing`); then a hello opens a session. With a pairing kept, Lumis connects to the mount's address whenever it can (a connection waits in the controller until the mount advertises) and says hello. A link or a channel lost, or a frame that does not verify, ends the session: the mount stops on its own (its supervision timeout), and Lumis looks for it again.

use crate::sys;
use lumis_accessory::doc::MESSAGE_MOST;
use lumis_accessory::frame::MAC_LEN;
use lumis_accessory::msg::Message;
use lumis_accessory::{pair, session, Chains, StaticSecret};
use lumis_core::spin::{self, Ask, Mount};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::Duration;

/// The Bluetooth right's slot: a kist's asked rights from slot 7 in the order it declares them (camera 7, items 8, bluetooth 9; KISTS.md "The slots"), and a spawn's `--use camera --items --use bt` alike.
const BT: u64 = 9;
/// The entropy right (slot 6, implicit).
const ENTROPY: u64 = 6;
/// The accessories' channel (ACCESSORIES.md "The link").
const PSM: u16 = 0x81;
/// A message and its X2: 2 KiB and 32 bytes.
const MTU: u16 = (MESSAGE_MOST + MAC_LEN) as u16;
/// Connection parameters asked: 7.5 to 15 ms (1.25 ms units), latency 0, the supervision timeout 1 s (10 ms units; the owner, 2026-11-24: "coast to stop after 1s").
const PARAMS: [u16; 4] = [6, 12, 0, 100];
/// An EVENTS reply's room.
const ROOM: u32 = 1 << 13;
/// How often the session's loop looks at the slider and the menu, when no event comes.
const LOOK: Duration = Duration::from_millis(1 << 5);
/// Where Lumis keeps its side of a pairing in its vault (one small module: when the identity's pairing store reaches kists, it moves there).
const STORE: &str = "mount/pairing";

/// The family's discovery tag (ACCESSORIES.md "Discovery"): the first 23 bytes of BLAKE3("lumis" ‖ "accessory") (with the flags and the class byte, an advertisement's 31 exactly).
fn tag() -> [u8; 23] {
    let mut h = blake3::Hasher::new();
    h.update(b"lumis");
    h.update(b"accessory");
    h.finalize().as_bytes()[..23].try_into().unwrap()
}

fn fresh<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    ferros_sys::entropy(sys::cap(ENTROPY), &mut b);
    b
}

fn eagle_now() -> Option<i64> {
    match ferros_sys::eagle_now() {
        0 => None,
        t => Some(t),
    }
}

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

/// One `bt` call: the reply's bytes, None when refused by the kernel or answered empty.
fn call(m: &[u8], room: usize) -> Option<Vec<u8>> {
    let mut r = vec![0u8; room];
    let n = sys::call_live(BT, m, &mut r);
    (n != sys::ERR && n != 0).then(|| {
        r.truncate(n as usize);
        r
    })
}

fn op(session: u64, op: u8, rest: &[u8]) -> Option<Vec<u8>> {
    let mut m = vec![op];
    m.extend_from_slice(&session.to_le_bytes());
    m.extend_from_slice(rest);
    call(&m, 1 << 6)
}

/// An event from the driver (BT.md "Events").
enum Ev {
    Adv { address_type: u8, address: [u8; 6], data: Vec<u8> },
    Connected { ok: bool, link: u16 },
    Disconnected { link: u16 },
    Opened { link: u16, channel: u16, ok: bool },
    Sdu { link: u16, channel: u16, data: Vec<u8> },
    Closed { link: u16, channel: u16 },
    /// The session is gone (its EVENTS answered empty).
    Gone,
}

/// The events thread: EVENTS held, each reply's events sent on.
fn events(session: u64, to: Sender<Ev>) {
    let mut m = vec![7u8];
    m.extend_from_slice(&session.to_le_bytes());
    m.extend_from_slice(&ROOM.to_le_bytes());
    loop {
        let Some(r) = call(&m, ROOM as usize) else {
            let _ = to.send(Ev::Gone);
            return;
        };
        let mut at = 0;
        // Each event [kind u8][length u16][body]; a reply holds whole events.
        while at + 3 <= r.len() {
            let (kind, len) = (r[at], le16(&r, at + 1) as usize);
            // WHY get: the driver's lengths; a body past the reply is no event (the reply holds whole ones, so this is a driver's bug, said and stopped).
            let Some(e) = r.get(at + 3..at + 3 + len) else {
                println!("lumis: mount: an event past its reply");
                break;
            };
            at += 3 + len;
            let ev = match kind {
                0 if e.len() >= 9 => Ev::Adv { address_type: e[1], address: e[2..8].try_into().unwrap(), data: e[9..].to_vec() },
                1 if e.len() >= 3 => Ev::Connected { ok: e[0] == 0, link: le16(e, 1) },
                2 if e.len() >= 2 => Ev::Disconnected { link: le16(e, 0) },
                3 if e.len() >= 6 => Ev::Opened { link: le16(e, 0), channel: le16(e, 2), ok: le16(e, 4) == 0 },
                4 if e.len() >= 4 => Ev::Sdu { link: le16(e, 0), channel: le16(e, 2), data: e[4..].to_vec() },
                5 if e.len() >= 4 => Ev::Closed { link: le16(e, 0), channel: le16(e, 2) },
                _ => continue,
            };
            if to.send(ev).is_err() {
                return;
            }
        }
    }
}

/// An advertisement from a mount of the family: its class byte (the top bit: in pairing mode), from manufacturer data company G#FFFF, the tag, then the class.
fn family_class(data: &[u8]) -> Option<u8> {
    let tag = tag();
    let mut at = 0;
    while at < data.len() {
        let len = data[at] as usize;
        // WHY get: the advertiser's bytes; a structure past the data is not one.
        let s = data.get(at + 1..at + 1 + len)?;
        if len >= 1 + 2 + 24 && s[0] == 0xFF && s[1] == 0xFF && s[2] == 0xFF && s[3..26] == tag {
            return Some(s[26]);
        }
        if len == 0 {
            return None;
        }
        at += 1 + len;
    }
    None
}

/// Lumis's side of a pairing (Lumis's vault, `STORE`, one VSF file): the mount's address and its type, its static key, Lumis's static key for it (`krx`), and `P` (`ksb`).
#[derive(Clone)]
struct Pairing {
    address: [u8; 6],
    address_type: u8,
    accessory_key: [u8; 32],
    phone_secret: [u8; 32],
    secret: [u8; 32],
}

impl Pairing {
    fn schema() -> vsf::schema::SectionSchema {
        use vsf::schema::TypeConstraint as T;
        vsf::schema::SectionSchema::new("mount").field("address", T::AnyUnsigned).field("address_type", T::AnyUnsigned).field("accessory_key", T::X25519Key).field("phone_secret", T::X25519PrivateKey).field("secret", T::DerivedSecret)
    }

    fn encode(&self) -> Vec<u8> {
        use vsf::schema::IntoVsfType;
        use vsf::types::VsfType;
        let mut a = [0u8; 8];
        a[..6].copy_from_slice(&self.address);
        let fields = vec![
            ("address".to_string(), u64::from_le_bytes(a).into_vsf_type()),
            ("address_type".to_string(), (self.address_type as u64).into_vsf_type()),
            ("accessory_key".to_string(), VsfType::kx(self.accessory_key.to_vec())),
            ("phone_secret".to_string(), VsfType::krx(self.phone_secret.to_vec())),
            ("secret".to_string(), VsfType::ksb(self.secret.to_vec())),
        ];
        let mut b = vsf::vsf_builder::VsfBuilder::new().provenance_only();
        if let Some(t) = eagle_now() {
            b = b.creation_time_oscillations(t);
        }
        b.add_section("mount", fields).build().expect("a VSF file of the pairing's own fields")
    }

    fn decode(doc: &[u8]) -> Option<Pairing> {
        use vsf::types::VsfType;
        let s = vsf::schema::SectionBuilder::parse_document(Self::schema(), doc, None).ok()?;
        let one = |f: &str| s.get(f).ok()?.first().cloned();
        let bytes32 = |v: Option<VsfType>| -> Option<[u8; 32]> {
            match v? {
                VsfType::kx(b) | VsfType::krx(b) | VsfType::ksb(b) => b.as_slice().try_into().ok(),
                _ => None,
            }
        };
        let address = u64::from_vsf(one("address")?)?.to_le_bytes()[..6].try_into().ok()?;
        Some(Pairing { address, address_type: u64::from_vsf(one("address_type")?)? as u8, accessory_key: bytes32(one("accessory_key"))?, phone_secret: bytes32(one("phone_secret"))?, secret: bytes32(one("secret"))? })
    }

    fn load(vault: &kete::FlatStorage) -> Option<Pairing> {
        Pairing::decode(&vault.read_device(STORE).ok().flatten()?)
    }

    fn keep(&self, vault: &kete::FlatStorage) {
        if let Err(e) = vault.write_device(STORE, &self.encode()) {
            println!("lumis: mount: the pairing was not kept ({e:?})");
        }
    }

    fn phone_static(&self) -> StaticSecret {
        StaticSecret::from(self.phone_secret)
    }

    fn phone_key(&self) -> [u8; 32] {
        *x25519_dalek::PublicKey::from(&self.phone_static()).as_bytes()
    }
}

trait FromVsf: Sized {
    fn from_vsf(v: vsf::types::VsfType) -> Option<Self>;
}

impl FromVsf for u64 {
    fn from_vsf(v: vsf::types::VsfType) -> Option<u64> {
        <u64 as vsf::schema::FromVsfType>::from_vsf_type(&v).ok()
    }
}

/// Where the mount thread is.
enum State {
    /// No pairing, nothing asked.
    Idle,
    /// Scanning for a mount in pairing mode.
    Searching,
    /// A connection asked, to pair (`pairing` None) or for a session.
    Connecting { address: [u8; 6], address_type: u8 },
    /// Connected; the channel asked.
    Opening { link: u16, address: [u8; 6], address_type: u8 },
    /// The offer sent; its reply awaited.
    Offered { link: u16, channel: u16, offer: pair::Offer, phone_secret: [u8; 32], address: [u8; 6], address_type: u8 },
    /// The code shown; the person's confirmation awaited (then the mount's).
    Code { link: u16, channel: u16, chains: Chains, paired: pair::Paired, phone_secret: [u8; 32], address: [u8; 6], address_type: u8, confirmed: bool },
    /// The hello sent; the mount's awaited.
    Hello { link: u16, channel: u16, hello: session::Hello },
    /// A session.
    Session { link: u16, channel: u16, chains: Chains, rate: Option<i32> },
}

/// The mount thread, when Lumis holds the Bluetooth right and the `bt` driver answers: `spin`'s row shows from then.
pub fn start(vault: Option<Arc<kete::FlatStorage>>) {
    if sys::grant_count() <= BT {
        return;
    }
    let Some(r) = call(&[6], 1 << 4) else {
        println!("lumis: mount: the bt driver did not answer (no mount)");
        return;
    };
    if r.len() < 9 || r[0] != 0 {
        println!("lumis: mount: the bt driver refused a session (nobody attested?)");
        return;
    }
    let session = u64::from_le_bytes(r[1..9].try_into().unwrap());
    let (tx, rx) = channel();
    if std::thread::Builder::new().name("lumis-mount-events".into()).spawn(move || events(session, tx)).is_err() {
        return;
    }
    let _ = std::thread::Builder::new().name("lumis-mount".into()).spawn(move || run(session, rx, vault));
}

fn run(session: u64, rx: Receiver<Ev>, vault: Option<Arc<kete::FlatStorage>>) {
    let mut pairing = vault.as_deref().and_then(Pairing::load);
    let mut state = State::Idle;
    println!("lumis: mount: {}", if pairing.is_some() { "paired: looking for it" } else { "not paired" });
    loop {
        // With a pairing and nothing under way, connect to it (the controller waits for its advertisements).
        if let (State::Idle, Some(p)) = (&state, &pairing) {
            let mut m = vec![p.address_type];
            m.extend_from_slice(&p.address);
            for v in PARAMS {
                m.extend_from_slice(&v.to_le_bytes());
            }
            match op(session, 10, &m).map(|r| r[0]) {
                Some(0) => state = State::Connecting { address: p.address, address_type: p.address_type },
                other => {
                    println!("lumis: mount: CONNECT answered {other:?}; again in a second");
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                }
            }
        }
        spin::set_mount(match (&state, &pairing) {
            (State::Session { .. }, _) => Mount::Connected,
            (State::Searching, _) | (State::Offered { .. }, _) => Mount::Searching,
            (State::Code { confirmed: false, paired, .. }, _) => Mount::Code(paired.check),
            (State::Code { confirmed: true, .. }, _) => Mount::Confirming,
            (State::Connecting { .. } | State::Opening { .. }, None) => Mount::Searching,
            (_, Some(_)) => Mount::Looking,
            (_, None) => Mount::Unpaired,
        });
        // The menu's asks: a pairing (none kept, nothing under way), a search given up, a code confirmed.
        match spin::take_ask() {
            Some(Ask::Pair) if pairing.is_none() && matches!(state, State::Idle) => {
                if op(session, 8, &[1]).map(|r| r[0]) == Some(0) {
                    state = State::Searching;
                }
            }
            Some(Ask::Cancel) if matches!(state, State::Searching) => {
                op(session, 9, &[]);
                state = State::Idle;
            }
            Some(Ask::Confirm) => {
                if let State::Code { link, channel, chains, phone_secret, confirmed: confirmed @ false, .. } = &mut state {
                    let phone_key = *x25519_dalek::PublicKey::from(&StaticSecret::from(*phone_secret)).as_bytes();
                    let frame = chains.send.mask(&Message::PairConfirm { phone_key }.encode(eagle_now()));
                    if send(session, *link, *channel, &frame) {
                        *confirmed = true;
                    }
                }
            }
            _ => {}
        }
        // The session's rate, when the slider moved it.
        if let State::Session { link, channel, chains, rate } = &mut state {
            let now = spin::rate();
            if *rate != Some(now) {
                let frame = chains.send.mask(&Message::Rate { rate: now as i64, at: None }.encode(eagle_now()));
                if send(session, *link, *channel, &frame) {
                    *rate = Some(now);
                }
            }
        }
        let ev = match rx.recv_timeout(LOOK) {
            Ok(ev) => ev,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        state = step(session, state, ev, &mut pairing, vault.as_deref());
    }
}

/// An SDU sent whole (SEND held until it is in the transport): false when refused (the channel is gone).
fn send(session: u64, link: u16, channel: u16, sdu: &[u8]) -> bool {
    let mut m = link.to_le_bytes().to_vec();
    m.extend_from_slice(&channel.to_le_bytes());
    m.extend_from_slice(sdu);
    op(session, 13, &m).map(|r| r[0]) == Some(0)
}

fn open_channel(session: u64, link: u16) -> bool {
    let mut m = link.to_le_bytes().to_vec();
    m.extend_from_slice(&PSM.to_le_bytes());
    m.extend_from_slice(&MTU.to_le_bytes());
    op(session, 12, &m).map(|r| r[0]) == Some(0)
}

/// The link let go (and with it its channel): back to looking.
fn drop_link(session: u64, link: u16) -> State {
    op(session, 11, &link.to_le_bytes());
    State::Idle
}

fn step(session: u64, state: State, ev: Ev, pairing: &mut Option<Pairing>, vault: Option<&kete::FlatStorage>) -> State {
    // An SDU, a close or a disconnection of a link or channel that is not the one under way (an old one's, late) is not this state's: passed over.
    let (link_now, channel_now) = match &state {
        State::Opening { link, .. } => (Some(*link), None),
        State::Offered { link, channel, .. } | State::Code { link, channel, .. } | State::Hello { link, channel, .. } | State::Session { link, channel, .. } => (Some(*link), Some(*channel)),
        _ => (None, None),
    };
    match &ev {
        Ev::Sdu { link, channel, .. } if (Some(*link), Some(*channel)) != (link_now, channel_now) => return state,
        Ev::Closed { link, channel } if (Some(*link), Some(*channel)) != (link_now, channel_now) => return state,
        Ev::Disconnected { link } if Some(*link) != link_now && !matches!(state, State::Connecting { .. }) => return state,
        _ => {}
    }
    match (state, ev) {
        (_, Ev::Gone) => {
            println!("lumis: mount: the bt session is gone");
            spin::set_mount(Mount::Off);
            std::thread::park();
            State::Idle
        }
        // A mount in pairing mode heard: connect to it.
        (State::Searching, Ev::Adv { address_type, address, data }) if family_class(&data).is_some_and(|c| c & 0x80 != 0) => {
            op(session, 9, &[]);
            let mut m = vec![address_type];
            m.extend_from_slice(&address);
            for v in PARAMS {
                m.extend_from_slice(&v.to_le_bytes());
            }
            if op(session, 10, &m).map(|r| r[0]) == Some(0) {
                State::Connecting { address, address_type }
            } else {
                State::Idle
            }
        }
        (s @ State::Searching, _) => s,
        (State::Connecting { address, address_type }, Ev::Connected { ok: true, link }) => {
            if open_channel(session, link) {
                State::Opening { link, address, address_type }
            } else {
                drop_link(session, link)
            }
        }
        (State::Connecting { .. }, Ev::Connected { ok: false, .. }) => State::Idle,
        // The channel open: pair (no pairing yet), or say hello.
        (State::Opening { link, address, address_type }, Ev::Opened { link: l, channel, ok: true }) if l == link => match pairing {
            None => {
                let phone_secret: [u8; 32] = fresh();
                let offer = pair::offer(phone_secret, fresh(), fresh(), eagle_now());
                if send(session, link, channel, &offer.doc) {
                    State::Offered { link, channel, offer, phone_secret, address, address_type }
                } else {
                    drop_link(session, link)
                }
            }
            Some(p) => {
                let hello = session::hello(&p.phone_key(), fresh(), fresh(), eagle_now());
                if send(session, link, channel, &hello.doc) {
                    State::Hello { link, channel, hello }
                } else {
                    drop_link(session, link)
                }
            }
        },
        (State::Offered { link, channel, offer, phone_secret, address, address_type }, Ev::Sdu { data, .. }) => match offer.accept(&data) {
            Some(paired) => {
                println!("lumis: mount: paired with a mount, its code {}: confirm it on both", spin::code_letters(paired.check));
                State::Code { link, channel, chains: Chains::phone(&paired.secret), paired, phone_secret, address, address_type, confirmed: false }
            }
            None => drop_link(session, link),
        },
        // The mount's pair_done: the pairing kept, then a hello on the same link.
        (State::Code { link, channel, mut chains, paired, phone_secret, address, address_type, confirmed: true }, Ev::Sdu { data, .. }) => {
            match chains.receive.unmask(&data).and_then(|m| Message::decode(&m)) {
                Some(Message::PairDone { accessory_key }) if accessory_key == paired.peer => {
                    let p = Pairing { address, address_type, accessory_key, phone_secret, secret: paired.secret };
                    if let Some(v) = vault {
                        p.keep(v);
                    }
                    println!("lumis: mount: the pairing kept");
                    let hello = session::hello(&p.phone_key(), fresh(), fresh(), eagle_now());
                    *pairing = Some(p);
                    if send(session, link, channel, &hello.doc) {
                        State::Hello { link, channel, hello }
                    } else {
                        drop_link(session, link)
                    }
                }
                _ => drop_link(session, link),
            }
        }
        (State::Hello { link, channel, hello }, Ev::Sdu { data, .. }) => {
            let Some(p) = pairing.as_ref() else { return drop_link(session, link) };
            match hello.accept(&data, &p.phone_static(), &p.accessory_key, &p.secret) {
                Some(mut chains) => {
                    println!("lumis: mount: a session");
                    let sync = chains.send.mask(&Message::Sync { sent: eagle_now().unwrap_or(0), counter: None }.encode(eagle_now()));
                    send(session, link, channel, &sync);
                    State::Session { link, channel, chains, rate: None }
                }
                None => {
                    println!("lumis: mount: the mount did not know this pairing");
                    drop_link(session, link)
                }
            }
        }
        (State::Session { link, channel, mut chains, rate }, Ev::Sdu { data, .. }) => {
            let Some(m) = chains.receive.unmask(&data).and_then(|m| Message::decode(&m)) else {
                println!("lumis: mount: a frame that did not verify: the session ends");
                return drop_link(session, link);
            };
            match m {
                Message::State { battery_mv, .. } => spin::set_battery_mv(battery_mv as i32),
                Message::Sync { sent, counter: Some(counter) } => {
                    if let Some(now) = eagle_now() {
                        let clock = chains.send.mask(&Message::Clock { counter, at: sent + (now - sent) / 2, drift_ppb: 0 }.encode(eagle_now()));
                        send(session, link, channel, &clock);
                    }
                }
                Message::Refused { words, .. } => println!("lumis: mount: refused: {words}"),
                _ => {}
            }
            State::Session { link, channel, chains, rate }
        }
        // The link or the channel gone: look again.
        (_, Ev::Disconnected { .. } | Ev::Closed { .. }) => State::Idle,
        (s, _) => s,
    }
}
