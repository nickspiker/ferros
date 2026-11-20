//! The person's hold (docs/HOLD-API.md): Lumis keeps its stills there as kind photo and its RAW clips as kind RAW video, so they are the person's things, found by Kūaha as well as by Lumis's own clips page. Every call goes thru the `items` grant (`ITEMS`); a person who said no to "your things" leaves it empty, every call is refused, and a still is kept in Lumis's own vault domain instead.

use ferros_items::kist::{Hold, Sys};
use ferros_items::{kinds, Card, Find, Maker, Proposed};

/// The `items` grant's slot: Lumis's capsule declares two wants, the camera then the items (tools/lumis-kist.sh), so after the six leading implicit rights (slots 1 to 6) the camera is in 7 and the items in 8 (ferros_kist `slots`), the GPU's three after them; a development spawn gives `--items` after `--use camera` for the same slot.
pub const ITEMS: u64 = 8;

/// Lumis's option (tools/lumis-kist.sh, minted 2026-11-12, never changed): its identity in a stowed kist's maker, whatever version made the item.
const OPTION: [u8; 32] = [0x0b, 0x94, 0x12, 0x9b, 0x0c, 0x38, 0x96, 0xfa, 0x46, 0x63, 0x99, 0x35, 0x40, 0xf7, 0x30, 0xcb, 0x62, 0x3d, 0x26, 0xde, 0x22, 0x68, 0x23, 0xb1, 0x1b, 0xb4, 0x03, 0x7c, 0x97, 0x06, 0x65, 0x56];

/// The hold thru the `items` grant, its documents stamped with Eagle time now. A handle it answers lasts as long as the vault process and the person's presence, whichever `Hold` asked.
pub fn hold() -> Hold<Sys> {
    Hold::new(Sys { cap: 0x204 + 32 * ITEMS }, vsf::types::eagle_time::eagle_time_oscillations)
}

/// Whether the `items` grant is held (not past the grants, not a right the person said no to).
pub fn held() -> bool {
    crate::sys::held(ITEMS)
}

/// Whether Lumis made an item: a stowed Lumis (its option, any version), or a development spawn of it (the kernel stamps a spawn with no developer and its process name, process.rs `items_stamp`). The hold answers a find with every item of the kinds the kist may open, which for now is any maker's (HOLD-API.md "Declaring it"), so the clips page keeps Lumis's own.
fn ours(m: &Maker) -> bool {
    m.option == OPTION || (m.developer == [0; 32] && m.name == "lumis")
}

/// Every item of `kind` Lumis made, newest first, a page of 2^8 at a time.
fn find_ours(kind: [u8; 32]) -> Result<Vec<([u8; 32], Card)>, String> {
    let mut h = hold();
    let mut out = Vec::new();
    let mut skip = 0;
    loop {
        let f = Find { kinds: vec![kind], order: ferros_items::ORDER_NEWEST, skip, count: ferros_items::FIND_MAX as u64, ..Default::default() };
        let found = h.find(&f).map_err(|e| format!("the hold's find: {e:?}"))?;
        let n = found.items.len() as u64;
        out.extend(found.items.into_iter().filter(|(_, c)| ours(&c.kist)));
        skip += n;
        if n == 0 || skip >= found.total {
            return Ok(out);
        }
    }
}

/// lumis_core's clips page's list: Lumis's RAW clips in the hold.
pub fn list_clips() -> Result<Vec<lumis_core::clips::Clip>, String> {
    if !held() {
        return Err("your things were not given to Lumis".into());
    }
    Ok(find_ours(kinds::RAW_VIDEO)?
        .into_iter()
        .map(|(handle, c)| lumis_core::clips::Clip {
            handle,
            bytes: c.size,
            fps: c.attr("fps").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            at: c.attr("at").and_then(|v| v.as_eagle()).unwrap_or(c.made),
            // A clip's body hash is the recorder's seal, none (zeros) until it is sealed (ferros_items `finish_extent`, `amend_extent`).
            closed: c.body != [0; 32],
        })
        .collect())
}

/// lumis_core's clips page's removal: the card, the clip's extent and its blocks, gone at once.
pub fn remove_clip(handle: &[u8; 32]) -> Result<(), String> {
    hold().remove(handle).map_err(|e| format!("the hold refused the removal: {e:?}"))?;
    println!("lumis: RAW video: a clip removed from the hold");
    Ok(())
}

/// A still kept as an item of kind photo, then read back from the hold whole and compared (blake3 of both, and the card's body hash), so "kept" means the hold gives the file back whole. The time each step took, in ms (write, read), or why not.
pub fn keep_still(bytes: &[u8]) -> Result<(u128, u128, Card), String> {
    let mut h = hold();
    let t = std::time::Instant::now();
    let (handle, card) = h.keep(&Proposed { kind: kinds::PHOTO, ..Default::default() }, bytes).map_err(|e| format!("the hold's keep: {e:?}"))?;
    let write_ms = t.elapsed().as_millis();
    let t = std::time::Instant::now();
    let mut back = vec![0u8; bytes.len()];
    let (n, size) = h.read(&handle, 0, &mut back).map_err(|e| format!("kept, then the read back failed: {e:?}"))?;
    let read_ms = t.elapsed().as_millis();
    let want = *blake3::hash(bytes).as_bytes();
    if n != bytes.len() || size != bytes.len() as u64 || *blake3::hash(&back).as_bytes() != want || card.body != want {
        return Err(format!("kept, then read back as {n} of {size} bytes, not the {} written", bytes.len()));
    }
    Ok((write_ms, read_ms, card))
}

/// The recorder's sink with the blocks it was given counted (the room a clip has: the time left under RECORD).
pub struct Counted<S> {
    pub sink: S,
    pub got: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl<S: ferros_clip::Sink> ferros_clip::Sink for Counted<S> {
    fn open(&mut self, blocks: u64) -> Result<u64, String> {
        let n = self.sink.open(blocks)?;
        self.got.store(n, std::sync::atomic::Ordering::Relaxed);
        Ok(n)
    }
    fn write(&mut self, block: u64, bytes: &[u8]) -> Result<(), String> {
        self.sink.write(block, bytes)
    }
    fn seal(&mut self, len: u64, hash: [u8; 32]) -> Result<(), String> {
        self.sink.seal(len, hash)
    }
}
