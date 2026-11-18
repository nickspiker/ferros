//! The flash ring (LEDGER.md "The flash ring"): entries packed into 4 KiB blocks, two copies of every block, wrapping when full.
//!
//! The region is `2 × half` blocks: copy A is blocks `0..half`, its mirror B is `half..2 × half`, block `k` of the ring at `k` and `half + k`.
//! A block holds complete entry documents back to back from its first byte; what follows the last is unwritten (zeros) and is never parsed as anything.
//! There is no header, no index and no fixed-width field: the head is found from the entries themselves.
//!
//! Writing (write-verify-then-mirror): an entry goes into the head block's copy in RAM, the whole block is written to A, read back and compared, then written to B; only then does `append` return.
//! An entry that does not fit in what is left of the block starts the next block (the oldest block's entries are then gone: that is the wrap).
//!
//! Finding the head at open: blocks are written in ring order, so the first entry's sequence number rises from block 0 up to the head block and drops after it (the older lap).
//! A binary search on "this block's first entry is at least block 0's" finds the head block in about log2(half) block reads; the chain then decides: from the last entry of that block, forward for as long as the next block's first entry names it as `prev`.
//! A block whose copies both hold something that is not an entry falls back to reading every block.
//!
//! A torn write: a write cut off leaves at most the block being written damaged in one copy (A is written whole before B is touched).
//! Entries are checked one by one against their provenance hash, A first, B where A fails, so every entry either copy holds whole is kept; what neither holds whole is named, never passed over in silence.
//! At the end of the head block that is the torn last write (`Why::Torn`, skipped, and overwritten by the next entry); anywhere else it is damage (`Why::Corrupt`).

use alloc::vec;
use alloc::vec::Vec;

use crate::category::Class;
use crate::entry::{self, Error, Hash, Payload, Place, SealKey, Stored};

/// The block size: UFS's logical block.
pub const BLOCK: usize = 4096;

/// The flash under the ring: whole blocks, numbered from the region's start.
pub trait Blocks {
    /// Read block `block` into `out`; false if the read failed.
    fn read(&mut self, block: u32, out: &mut [u8; BLOCK]) -> bool;
    /// Write `data` to block `block`; false if the write failed.
    fn write(&mut self, block: u32, data: &[u8; BLOCK]) -> bool;
}

/// What a read of the ring found that was not a clean entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Why {
    /// The last write before the head was cut off: skipped, and the next entry goes where it was.
    Torn,
    /// Neither copy holds an entry here, and something other than unwritten space is there: damage, named by where it is and what failed.
    Corrupt(Error),
    /// One copy of an entry did not verify; the other did, and was read.
    OneCopy { bad: u8, error: Error },
    /// A copy holds older entries than the other (a write cut off between A and B): the newer copy was read.
    MirrorBehind,
    /// The entry before this one is not the one it names (`prev`): an entry is missing or was changed.
    ChainBreak { seq: u64 },
    /// The sequence jumps: entries between are missing.
    SeqGap { expected: u64, found: u64 },
    /// The flash refused to read a copy of this block.
    Unreadable { copy: u8 },
    /// An entry that verifies but whose seal does not open under the key this device holds for its class (found by the reader that opens, not by the ring).
    Shut(Error),
}

/// A finding, where it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub block: u32,
    /// The byte in the block.
    pub at: usize,
    pub why: Why,
}

/// What a walk of the ring gives, oldest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Entry { block: u32, at: usize, doc: Vec<u8>, stored: Stored },
    Finding(Finding),
}

/// The copy names in findings: 0 the primary (A), 1 the mirror (B).
pub const PRIMARY: u8 = 0;
pub const MIRROR: u8 = 1;

/// One block read and checked: its entries in order, what was wrong, and where its written part ends.
struct Block {
    entries: Vec<(usize, Vec<u8>, Stored)>,
    findings: Vec<Finding>,
    /// Nothing at all is written in either copy.
    empty: bool,
    /// An entry cut off at the end of what is written (a torn write if this is the head block).
    unfinished: Option<usize>,
}

impl Block {
    fn first_seq(&self) -> Option<u64> {
        self.entries.first().map(|e| e.2.place.seq)
    }
}

/// Whether nothing is written from `at` on.
fn blank(b: &[u8], at: usize) -> bool {
    b[at..].iter().all(|&x| x == 0)
}

/// The entry document that starts at `at` in `b`, verified: its bytes and its open fields.
fn entry_at(b: &[u8], at: usize) -> Result<(Vec<u8>, Stored), Error> {
    let len = entry::doc_len(&b[at..])?;
    let doc = b[at..at + len].to_vec();
    let s = entry::parse(&doc)?;
    Ok((doc, s))
}

/// Read block `k` (both copies) and check it, entry by entry, up to `limit` entries (1 where only the block's first entry matters: the head's search).
fn load<D: Blocks>(dev: &mut D, half: u32, k: u32, limit: usize) -> Block {
    let mut a = [0u8; BLOCK];
    let mut b = [0u8; BLOCK];
    let mut out = Block { entries: Vec::new(), findings: Vec::new(), empty: false, unfinished: None };
    let ra = dev.read(k, &mut a);
    let rb = dev.read(half + k, &mut b);
    if !ra {
        out.findings.push(Finding { block: k, at: 0, why: Why::Unreadable { copy: PRIMARY } });
    }
    if !rb {
        out.findings.push(Finding { block: k, at: 0, why: Why::Unreadable { copy: MIRROR } });
    }
    let copies: [Option<&[u8; BLOCK]>; 2] = [ra.then_some(&a), rb.then_some(&b)];
    out.empty = copies.iter().all(|c| c.is_none_or(|c| blank(&c[..], 0)));
    if out.empty {
        return out;
    }
    let mut at = 0usize;
    // The entry before, within this block: the one the next must follow.
    let mut last: Option<(u64, Hash)> = None;
    while at < BLOCK && out.entries.len() < limit {
        let mut tried: [Option<Result<(Vec<u8>, Stored), Error>>; 2] = [None, None];
        let try_copy = |c: usize| copies[c].and_then(|copy| (!blank(&copy[..], at)).then(|| entry_at(&copy[..], at)));
        // A copy fits here if it verifies and, past the block's first entry, follows the entry before.
        let fits = |r: &Option<Result<(Vec<u8>, Stored), Error>>| match (r, last) {
            (Some(Ok((_, s))), Some((seq, h))) => s.place.seq == seq + 1 && s.place.prev == h,
            (Some(Ok(_)), None) => true,
            _ => false,
        };
        tried[0] = try_copy(0);
        // The mirror is parsed only when it differs from the primary's entry: the same bytes verify the same.
        let same = match (&tried[0], copies[1]) {
            (Some(Ok((doc, _))), Some(m)) => at + doc.len() <= BLOCK && m[at..at + doc.len()] == doc[..],
            _ => false,
        };
        if !same {
            tried[1] = try_copy(1);
        }
        let pick = if fits(&tried[0]) {
            Some(0)
        } else if fits(&tried[1]) {
            Some(1)
        } else {
            None
        };
        match pick {
            Some(c) => {
                let (doc, s) = tried[c].take().unwrap().unwrap();
                match &tried[1 - c] {
                    _ if same => {}
                    Some(Err(e)) => out.findings.push(Finding { block: k, at, why: Why::OneCopy { bad: (1 - c) as u8, error: e.clone() } }),
                    Some(Ok((_, o))) if o.place.seq < s.place.seq => out.findings.push(Finding { block: k, at, why: Why::MirrorBehind }),
                    // The mirror readable and blank where the primary holds an entry: the write stopped between the two.
                    None if c == 0 && copies[1].is_some() => out.findings.push(Finding { block: k, at, why: Why::MirrorBehind }),
                    _ => {}
                }
                last = Some((s.place.seq, s.hash));
                let len = doc.len();
                out.entries.push((at, doc, s));
                at += len;
            }
            None => {
                let written = tried.iter().filter(|t| t.is_some()).count();
                if written == 0 {
                    // Both copies are blank from here: the block's written part ends.
                    break;
                }
                // Something is here and neither copy holds a whole entry that follows the one before.
                let error = tried.iter().flatten().find_map(|t| t.as_ref().err().cloned()).unwrap_or(Error::Schema("an entry that does not follow the one before".into()));
                out.findings.push(Finding { block: k, at, why: Why::Corrupt(error) });
                match resync(&copies, at) {
                    Some(next) => {
                        at = next;
                        last = None;
                    }
                    None => {
                        // Nothing whole after it.
                        // Unfinished, the shape a cut-off write leaves, when a readable copy is blank here: A is written whole before B, so a write cut off leaves the entry damaged in one copy and absent from the other.
                        if written < copies.iter().filter(|c| c.is_some()).count() {
                            out.unfinished = Some(at);
                        }
                        break;
                    }
                }
            }
        }
    }
    out
}

/// After damage at `at`, where the next whole entry starts in either copy: the damaged one's own length if its header still reads, else the next document magic that starts a verified entry.
fn resync(copies: &[Option<&[u8; BLOCK]>; 2], at: usize) -> Option<usize> {
    let magic = "RÅ<".as_bytes();
    for copy in copies.iter().flatten() {
        if let Ok(len) = entry::doc_len(&copy[at..]) {
            if at + len < BLOCK && entry_at(&copy[..], at + len).is_ok() {
                return Some(at + len);
            }
        }
    }
    for p in at + 1..BLOCK - magic.len() {
        for copy in copies.iter().flatten() {
            if &copy[p..p + magic.len()] == magic && entry_at(&copy[..], p).is_ok() {
                return Some(p);
            }
        }
    }
    None
}

/// The ring as the writer holds it: where the head is and the head block's bytes.
pub struct Ring {
    half: u32,
    /// The head block (the one being filled); 0 for an empty ring.
    block: u32,
    /// The head block's bytes as written (both copies once `append` returns).
    tail: Vec<u8>,
    /// How much of `tail` is entries.
    used: usize,
    /// The next entry's sequence number and the hash it chains to.
    next_seq: u64,
    last: Hash,
    /// The boot the last entry carries (None: an empty ring).
    last_boot: Option<u64>,
    empty: bool,
}

/// Why an entry was not appended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppendError {
    Build(Error),
    /// A write failed, or copy A did not read back as written (the mirror is untouched).
    Write,
}

impl Ring {
    /// Open the ring on `dev` (`2 × half` blocks): find the head by the chain, and what was wrong on the way.
    pub fn open<D: Blocks>(dev: &mut D, half: u32) -> (Ring, Vec<Finding>) {
        let mut findings = Vec::new();
        let b0 = load(dev, half, 0, 1);
        let empty = Ring { half, block: 0, tail: vec![0u8; BLOCK], used: 0, next_seq: 0, last: entry::genesis(), last_boot: None, empty: true };
        if b0.empty {
            // Blocks are written in order from block 0: an empty block 0 is a ring never written.
            return (empty, findings);
        }
        let head = match b0.first_seq() {
            Some(s0) => search(dev, half, s0),
            None => None,
        };
        let head = match head {
            Some(h) => h,
            None => match scan(dev, half) {
                Some(h) => h,
                None => {
                    findings.extend(b0.findings);
                    return (empty, findings);
                }
            },
        };
        // The chain decides: forward from the candidate for as long as the next block's first entry follows the last.
        let mut k = head;
        let mut blk = load(dev, half, k, usize::MAX);
        loop {
            let Some(last) = blk.entries.last() else { break };
            let n = (k + 1) % half;
            if n == head {
                break;
            }
            let next = load(dev, half, n, usize::MAX);
            match next.entries.first() {
                Some((_, _, s)) if s.place.prev == last.2.hash && s.place.seq == last.2.place.seq + 1 => {
                    k = n;
                    blk = next;
                }
                _ => break,
            }
        }
        for f in blk.findings.iter() {
            let mut f = f.clone();
            if Some(f.at) == blk.unfinished {
                f.why = Why::Torn;
            }
            findings.push(f);
        }
        findings.extend(torn_after(dev, half, k));
        let Some((at, doc, s)) = blk.entries.last() else {
            return (empty, findings);
        };
        let mut tail = vec![0u8; BLOCK];
        for (a, d, _) in blk.entries.iter() {
            tail[*a..*a + d.len()].copy_from_slice(d);
        }
        let ring = Ring { half, block: k, tail, used: at + doc.len(), next_seq: s.place.seq + 1, last: s.hash, last_boot: Some(s.place.boot), empty: false };
        (ring, findings)
    }

    /// The boot the newest entry carries; None for an empty ring.
    pub fn last_boot(&self) -> Option<u64> {
        self.last_boot
    }

    /// The next entry's sequence number.
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// The newest entry's provenance hash (the genesis hash for an empty ring).
    pub fn head_hash(&self) -> Hash {
        self.last
    }

    /// The head block.
    pub fn head_block(&self) -> u32 {
        self.block
    }

    /// The blocks in one copy.
    pub fn half(&self) -> u32 {
        self.half
    }

    pub fn is_empty(&self) -> bool {
        self.empty
    }

    /// Seal `payload` under `key` as the next entry and write it (write, verify, mirror) before returning.
    /// Words too long for one block go as several entries, each with its part of the words, in order.
    /// Returns the provenance hash of the (last) entry written.
    pub fn append<D: Blocks>(&mut self, dev: &mut D, boot: u64, time: Option<i64>, class: Class, key: &SealKey, payload: &Payload) -> Result<Hash, AppendError> {
        let place = Place { seq: self.next_seq, boot, prev: self.last, time };
        let doc = entry::build(&place, class, key, payload).map_err(AppendError::Build)?;
        if doc.len() > BLOCK {
            // Split the words at a character boundary near the middle and write the halves in order.
            let w = payload.words.as_str();
            let mut cut = w.len() / 2;
            while !w.is_char_boundary(cut) {
                cut -= 1;
            }
            if cut == 0 {
                return Err(AppendError::Build(Error::Build("one character's entry is larger than a block".into())));
            }
            let (x, y) = w.split_at(cut);
            let first = Payload { category: payload.category.clone(), writer: payload.writer.clone(), words: x.into(), refused: 0 };
            let second = Payload { category: payload.category.clone(), writer: payload.writer.clone(), words: y.into(), refused: 0 };
            self.append(dev, boot, time, class, key, &first)?;
            return self.append(dev, boot, time, class, key, &second);
        }
        let hash = entry::hash_of_built(&doc).map_err(AppendError::Build)?;
        self.write(dev, &doc)?;
        self.next_seq += 1;
        self.last = hash;
        self.last_boot = Some(boot);
        self.empty = false;
        Ok(hash)
    }

    /// Put one entry document into the head block (the next block when it does not fit) and write the block: A, read back and compare, then B.
    fn write<D: Blocks>(&mut self, dev: &mut D, doc: &[u8]) -> Result<(), AppendError> {
        let (block, used, mut img) = if self.used + doc.len() > BLOCK { ((self.block + 1) % self.half, 0, vec![0u8; BLOCK]) } else { (self.block, self.used, self.tail.clone()) };
        img[used..used + doc.len()].copy_from_slice(doc);
        let image: &[u8; BLOCK] = img.as_slice().try_into().unwrap();
        let mut back = [0u8; BLOCK];
        if !(dev.write(block, image) && dev.read(block, &mut back) && back == *image && dev.write(self.half + block, image)) {
            // Not in: the ring stays where it was (A may hold the entry, B does not; the next open reads whichever copy verifies, and the next append writes over it).
            return Err(AppendError::Write);
        }
        self.tail = img;
        self.block = block;
        self.used = used + doc.len();
        Ok(())
    }

    /// Up to `max` of the newest entries, oldest first, with what was found wrong among them (the chain checked between every two).
    pub fn newest<D: Blocks>(&self, dev: &mut D, max: usize) -> Vec<Item> {
        if self.empty {
            return Vec::new();
        }
        // Back from the head block, a block at a time, until enough entries or the ring has been gone round.
        let mut blocks: Vec<(u32, Block)> = Vec::new();
        let mut count = 0usize;
        let mut k = self.block;
        loop {
            let b = load(dev, self.half, k, usize::MAX);
            if b.empty {
                break;
            }
            count += b.entries.len();
            blocks.push((k, b));
            if count >= max {
                break;
            }
            k = (k + self.half - 1) % self.half;
            if k == self.block {
                break;
            }
        }
        blocks.reverse();
        let mut items = Vec::new();
        let mut entries = 0usize;
        // WHY saturating: the ring can hold fewer entries than asked for (count < max), and then none are skipped.
        let skip = count.saturating_sub(max);
        // The entry before, across blocks: what the next must name.
        let mut last: Option<(u64, Hash)> = None;
        for (k, b) in blocks {
            // This block's findings and entries in the order they sit in it.
            let mut here: Vec<(usize, Option<Finding>, Option<(Vec<u8>, Stored)>)> = Vec::new();
            for f in b.findings.iter() {
                let mut f = f.clone();
                if k == self.block && Some(f.at) == b.unfinished {
                    f.why = Why::Torn;
                }
                here.push((f.at, Some(f), None));
            }
            for (at, doc, s) in b.entries {
                here.push((at, None, Some((doc, s))));
            }
            here.sort_by_key(|h| h.0);
            for (at, f, e) in here {
                if let Some(f) = f {
                    if entries >= skip {
                        items.push(Item::Finding(f));
                    }
                    continue;
                }
                let (doc, s) = e.unwrap();
                entries += 1;
                let shown = entries > skip;
                if let Some((seq, h)) = last {
                    let why = if s.place.seq != seq + 1 {
                        Some(Why::SeqGap { expected: seq + 1, found: s.place.seq })
                    } else if s.place.prev != h {
                        Some(Why::ChainBreak { seq: s.place.seq })
                    } else {
                        None
                    };
                    if let (Some(why), true) = (why, shown) {
                        items.push(Item::Finding(Finding { block: k, at, why }));
                    }
                }
                last = Some((s.place.seq, s.hash));
                if shown {
                    items.push(Item::Entry { block: k, at, doc, stored: s });
                }
            }
        }
        items.extend(torn_after(dev, self.half, self.block).map(Item::Finding));
        items
    }
}

/// A write cut off as it started the block after the head: nothing whole there and a copy blank, so it is the torn last write (the next entry goes over it).
fn torn_after<D: Blocks>(dev: &mut D, half: u32, head: u32) -> Option<Finding> {
    let n = (head + 1) % half;
    if n == head {
        return None;
    }
    let b = load(dev, half, n, 1);
    (b.entries.is_empty() && b.unfinished == Some(0)).then_some(Finding { block: n, at: 0, why: Why::Torn })
}

/// The head block by binary search: the last block whose first entry is at least block 0's (`s0`).
/// None if a block on the way is damaged in both copies (the caller then reads every block).
fn search<D: Blocks>(dev: &mut D, half: u32, s0: u64) -> Option<u32> {
    // Invariant: block lo is in the current lap; every block at or past hi is not.
    let (mut lo, mut hi) = (0u32, half);
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        let b = load(dev, half, mid, 1);
        let current = match b.first_seq() {
            Some(s) => s >= s0,
            None if b.empty => false,
            None => return None,
        };
        if current {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Some(lo)
}

/// The head block by reading every block: the one whose first entry is newest.
fn scan<D: Blocks>(dev: &mut D, half: u32) -> Option<u32> {
    let mut best: Option<(u64, u32)> = None;
    for k in 0..half {
        if let Some(s) = load(dev, half, k, 1).first_seq() {
            if best.is_none_or(|(b, _)| s > b) {
                best = Some((s, k));
            }
        }
    }
    best.map(|b| b.1)
}
