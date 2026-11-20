//! Where the hold is reached from: the one interface Kūaha's model and UI talk to.
//! On ferros the source is the kist's own `items` grant (`Over<Sys>`, std/kuaha); in tests it is the hold's server over a RAM vault; on a desktop later it is the person's devices reached over their links (USB, the LAN, the internet), each hold the same interface.
//! Nothing here names a platform: a transport is ferros_items's `Transport`, and a link is only a kind, a name and a measured speed.

use ferros_items::kist::{Hold, Transport};
use ferros_items::{Card, Error, Find, Found, Proposed};

/// Why a call came back empty-handed, as the person is told it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    /// The `items` right is an empty slot: the person said no (KISTS.md "No lying": a denied right is an empty slot, never fake data).
    NoRight,
    /// The right is held and every call is refused: a kernel or vault from before the hold.
    Silent,
    /// This call refused: a clip still recording, an item not the caller's to change.
    Refused,
    /// Gone: removed meanwhile, or a handle from before a lock.
    Gone,
    /// An answer that does not read, or anything else.
    Broken,
}

/// How a source reaches a hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkKind {
    /// The hold of the device Kūaha runs on: no link at all.
    Local,
    /// A cable to a plugged-in device: shown as USB, with its speed, and taken for big moves whenever present.
    Usb,
    /// The same local network as the device.
    Lan,
    /// The person's device reached across the internet.
    Internet,
}

/// One way a source reaches its hold, with what it measured; a desktop shows the one a copy is using.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub kind: LinkKind,
    /// A name a person reads (the device's, the network's); never translated.
    pub name: String,
    /// Measured, not promised: bits a second over the last transfer, None until one has run.
    pub bits_per_second: Option<u64>,
    /// Whether calls go this way now.
    pub in_use: bool,
}

/// The hold's items by kind: how many and their bytes.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct KindTotal {
    pub kind: [u8; 32],
    pub items: u64,
    pub bytes: u64,
}

/// Who made items, as a find narrows by them (ferros_items `maker_tag`), with the name the cards carry.
#[derive(Clone, Debug, PartialEq)]
pub struct MakerTotal {
    pub tag: [u8; 32],
    pub name: String,
    pub items: u64,
}

/// Everything in the hold this caller may open, counted: per kind and per maker.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Totals {
    pub kinds: Vec<KindTotal>,
    pub makers: Vec<MakerTotal>,
    pub items: u64,
    pub bytes: u64,
}

/// A record of the RAW clips area: one single-copy extent.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipRecord {
    /// Whose it is: the kist's name, or the person's item's title when the hold names one.
    pub owner: String,
    pub blocks: u64,
    /// The bytes written (a sealed clip's length).
    pub bytes: u64,
    /// Sealed by its recorder or by the vault's recoverer; false while it records.
    pub sealed: bool,
}

/// The clips area's blocks are 4 KiB (manifestus `extent`, RAW-VIDEO.md).
pub const CLIP_BLOCK: u64 = 1 << 12;

/// The RAW clips area (single-copy extents, RAW-VIDEO.md): its blocks, those taken, and the longest free stretch, which bounds the longest clip that can start now.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipsArea {
    pub blocks: u64,
    pub taken: u64,
    pub largest_free: u64,
    /// The area's records, when the source lists them (the device's usage call does not: each kist's clips are counted on its own row).
    pub clips: Vec<ClipRecord>,
}

/// A stowed kist and what it keeps for itself: its own domain's values and bytes (KISTS.md "The unstow sheet", summed from their heads), None until sized, and its clips in the clips area (Lumis's, recorded into its domain until its move into the hold).
#[derive(Clone, Debug, PartialEq)]
pub struct KistStore {
    pub developer: [u8; 32],
    pub option: [u8; 32],
    pub name: String,
    pub values: Option<u64>,
    pub bytes: Option<u64>,
    pub clip_bytes: u64,
}

/// The whole device (the hold's usage call, OP_USAGE): every item by kind whoever made it, the clips area, and each stowed kist.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Usage {
    pub kinds: Vec<KindTotal>,
    pub clips: Option<ClipsArea>,
    pub kists: Vec<KistStore>,
}

/// A kist by its identity: its developer key and option.
pub type KistId = ([u8; 32], [u8; 32]);

/// A hold, wherever it is.
/// Every call blocks until its answer: the platform runs them off the drawing thread (`crate::Kuaha::take_asks`, `serve`).
pub trait HoldSource {
    /// A page of matching items, each with this caller's handle.
    fn find(&mut self, f: &Find) -> Result<Found, Problem>;
    fn card(&mut self, handle: &[u8; 32]) -> Result<Card, Problem>;
    /// Body bytes from `offset`: how many came and the body's size.
    fn read(&mut self, handle: &[u8; 32], offset: u64, buf: &mut [u8]) -> Result<(usize, u64), Problem>;
    /// Gone at once (no bin, STORE.md §7.7).
    fn remove(&mut self, handle: &[u8; 32]) -> Result<(), Problem>;
    /// A new item with its whole body: a desktop's copy from the computer. The kist on ferros declares no kind it makes, so its hold refuses it.
    fn keep(&mut self, p: &Proposed, body: &[u8]) -> Result<([u8; 32], Card), Problem>;
    /// The whole device at once, its kists named but not yet sized (sizing one is slow: Photon's 2412 values took most of a second on the Pro); None from a hold that does not count it.
    fn usage(&mut self) -> Result<Option<Usage>, Problem>;
    /// These kists' own storage, sized: each comes back with its values and bytes.
    fn size_kists(&mut self, kists: &[KistId]) -> Result<Vec<KistStore>, Problem>;
    /// How this source reaches its hold, fastest first.
    fn links(&self) -> Vec<Link>;

    /// Every item counted by kind and maker: pages of a find over everything, so it needs nothing past the calls every hold answers.
    fn totals(&mut self) -> Result<Totals, Problem> {
        let mut t = Totals::default();
        let mut skip = 0;
        loop {
            let f = Find { skip, count: ferros_items::FIND_MAX as u64, ..Default::default() };
            let page = self.find(&f)?;
            for (_, c) in &page.items {
                t.items += 1;
                t.bytes += c.size;
                match t.kinds.iter_mut().find(|k| k.kind == c.kind) {
                    Some(k) => {
                        k.items += 1;
                        k.bytes += c.size;
                    }
                    None => t.kinds.push(KindTotal { kind: c.kind, items: 1, bytes: c.size }),
                }
                let tag = c.kist.tag();
                match t.makers.iter_mut().find(|m| m.tag == tag) {
                    Some(m) => m.items += 1,
                    None => t.makers.push(MakerTotal { tag, name: c.kist.name.clone(), items: 1 }),
                }
            }
            skip += page.items.len() as u64;
            if page.items.is_empty() || skip >= page.total {
                break;
            }
        }
        // The project's kinds in their own order (ferros_items `kinds::KINDS`), then any other by its count.
        let rank = |k: &[u8; 32]| ferros_items::kinds::KINDS.iter().position(|(_, id)| id == k).unwrap_or(usize::MAX);
        t.kinds.sort_by(|a, b| rank(&a.kind).cmp(&rank(&b.kind)).then(b.items.cmp(&a.items)));
        t.makers.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(t)
    }
}

/// The hold over any of ferros_items's transports: the device's syscall (`kist::Sys`), or the server's straight call in a test.
pub struct Over<T: Transport> {
    pub hold: Hold<T>,
    /// Whether the `items` grant is held (an empty slot means the person said no): tells a refusal of the right from a hold that does not answer.
    pub right: bool,
}

impl<T: Transport> Over<T> {
    pub fn new(t: T, clock: fn() -> i64, right: bool) -> Over<T> {
        Over { hold: Hold::new(t, clock), right }
    }

    /// A refusal of a call every hold answers to its own caller: the right is off, or the hold is not there.
    fn whole(&self, e: Error) -> Problem {
        match e {
            Error::Refused if !self.right => Problem::NoRight,
            Error::Refused => Problem::Silent,
            e => item(e),
        }
    }
}

fn kist_store(k: ferros_items::wire::KistUse) -> KistStore {
    KistStore { developer: k.developer, option: k.option, name: k.name, values: k.values, bytes: k.bytes, clip_bytes: k.clip_blocks * CLIP_BLOCK }
}

/// A refusal about one item.
fn item(e: Error) -> Problem {
    match e {
        Error::Refused => Problem::Refused,
        Error::NotFound => Problem::Gone,
        _ => Problem::Broken,
    }
}

impl<T: Transport> HoldSource for Over<T> {
    fn find(&mut self, f: &Find) -> Result<Found, Problem> {
        self.hold.find(f).map_err(|e| self.whole(e))
    }
    fn card(&mut self, handle: &[u8; 32]) -> Result<Card, Problem> {
        self.hold.card(handle).map_err(item)
    }
    fn read(&mut self, handle: &[u8; 32], offset: u64, buf: &mut [u8]) -> Result<(usize, u64), Problem> {
        self.hold.read(handle, offset, buf).map_err(item)
    }
    fn remove(&mut self, handle: &[u8; 32]) -> Result<(), Problem> {
        self.hold.remove(handle).map_err(item)
    }
    fn keep(&mut self, p: &Proposed, body: &[u8]) -> Result<([u8; 32], Card), Problem> {
        self.hold.keep(p, body).map_err(item)
    }
    fn usage(&mut self) -> Result<Option<Usage>, Problem> {
        let u = self.hold.usage_of(&[]).map_err(|e| self.whole(e))?;
        Ok(Some(Usage {
            kinds: u.kinds.into_iter().map(|k| KindTotal { kind: k.kind, items: k.items, bytes: k.bytes }).collect(),
            clips: u.clips.map(|c| ClipsArea { blocks: c.blocks, taken: c.taken, largest_free: c.largest_free, clips: Vec::new() }),
            kists: u.kists.into_iter().map(kist_store).collect(),
        }))
    }
    fn size_kists(&mut self, kists: &[KistId]) -> Result<Vec<KistStore>, Problem> {
        let mut out = Vec::new();
        for batch in kists.chunks(ferros_items::wire::USAGE_KISTS) {
            let u = self.hold.usage_of(batch).map_err(|e| self.whole(e))?;
            out.extend(u.kists.into_iter().filter(|k| k.bytes.is_some() && batch.contains(&(k.developer, k.option))).map(kist_store));
        }
        Ok(out)
    }
    fn links(&self) -> Vec<Link> {
        vec![Link { kind: LinkKind::Local, name: String::new(), bits_per_second: None, in_use: true }]
    }
}
