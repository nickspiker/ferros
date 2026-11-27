//! The recorder's sink as an item of the hold (docs/HOLD-API.md "Lumis"): the clip's card placed at its open (found while it records), its blocks written once straight to the flash thru the clip's own extent capability (or thru the vault when the kernel gave none), and its seal into its card.
//! `ExtentSink` (a single-copy extent of the process's own domain) stays beside it; a recorder takes either.

use std::sync::{Arc, Mutex};

use ferros_items::kist::{Clip, Hold, Transport};
use ferros_items::{kinds, Card, Proposed};
use vsf::types::eagle_time::EtType;
use vsf::VsfType;

/// What became of an item sink's clip, readable after the recorder took the sink: its handle from the open, whether it writes straight to the flash, and its card from the seal.
#[derive(Debug, Clone, Default)]
pub struct Receipt {
    pub handle: Option<[u8; 32]>,
    pub direct: bool,
    pub card: Option<Card>,
}

/// The recorder's sink as an item of the hold (`Sink`): `open` is `Hold::make_clip`, `write` is `write_clip` (whole blocks, at most a MiB, as the recorder's writer stages them), `seal` is `seal_clip`.
pub struct ItemSink<T: Transport> {
    hold: Hold<T>,
    p: Proposed,
    clip: Option<Clip>,
    receipt: Arc<Mutex<Receipt>>,
}

impl<T: Transport> ItemSink<T> {
    /// A sink keeping the clip as an item of the card `p` proposes, thru `hold`; the receipt is read after the recorder's `stop`.
    pub fn new(hold: Hold<T>, p: Proposed) -> (ItemSink<T>, Arc<Mutex<Receipt>>) {
        let receipt = Arc::new(Mutex::new(Receipt::default()));
        (ItemSink { hold, p, clip: None, receipt: receipt.clone() }, receipt)
    }
}

/// A RAW clip's proposed card: kind RAW video, the person's title if any, its start (`at`, Eagle oscillations, as `Mode::at`) and the caller's own facts (fps, width, height, the camera's name, ...).
pub fn raw_video_card(title: Option<String>, at: i64, attrs: Vec<(String, VsfType)>) -> Proposed {
    let mut a = vec![("at".to_string(), VsfType::e(EtType::e6(at)))];
    a.extend(attrs);
    Proposed { kind: kinds::RAW_VIDEO, title, attrs: a, ..Default::default() }
}

/// Lumis's constructor: the hold reached thru the `items` grant in slot `items_slot` (a std kist's grants page at its arc's base), the documents stamped with Eagle time now, the card `raw_video_card` makes.
#[cfg(target_os = "ferros")]
pub fn raw_video(items_slot: u64, title: Option<String>, at: i64, attrs: Vec<(String, VsfType)>) -> (ItemSink<ferros_items::kist::Sys>, Arc<Mutex<Receipt>>) {
    let hold = Hold::new(ferros_items::kist::Sys { cap: 0x204 + 32 * items_slot }, vsf::types::eagle_time::eagle_time_oscillations);
    ItemSink::new(hold, raw_video_card(title, at, attrs))
}

/// An item's body read thru the hold as a VSF byte source, a transfer at a time (`Hold::read`): a clip kept as an item opened by `vsf::visual::clip::Reader` without being assembled whole.
pub struct ItemSource<T: Transport> {
    hold: Mutex<Hold<T>>,
    handle: [u8; 32],
    len: u64,
}

impl<T: Transport> ItemSource<T> {
    /// The body of the item `handle` names (the caller's own handle), `len` bytes (its card's size).
    pub fn new(hold: Hold<T>, handle: [u8; 32], len: u64) -> ItemSource<T> {
        ItemSource { hold: Mutex::new(hold), handle, len }
    }
}

impl<T: Transport> vsf::container::ByteSource for ItemSource<T> {
    fn len(&self) -> u64 {
        self.len
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), vsf::container::SourceError> {
        // WHY: the offset and length are the reader's, from the clip's bytes; past the body is outside the source.
        if offset.checked_add(buf.len() as u64).is_none_or(|e| e > self.len) {
            return Err(vsf::container::SourceError::OutOfRange { offset, len: buf.len() as u64, size: self.len });
        }
        let mut hold = self.hold.lock().unwrap();
        let mut at = 0;
        while at < buf.len() {
            let (n, _) = hold.read(&self.handle, offset + at as u64, &mut buf[at..]).map_err(|e| vsf::container::SourceError::Io(format!("the hold's read at A#{}: {e:?}", offset + at as u64)))?;
            // WHY: a read answering no bytes inside the body would loop forever; it is the hold's fault, said.
            if n == 0 {
                return Err(vsf::container::SourceError::Io(format!("the hold's read at A#{} answered nothing", offset + at as u64)));
            }
            at += n;
        }
        Ok(())
    }
}

impl<T: Transport + Send> crate::Sink for ItemSink<T> {
    fn open(&mut self, blocks: u64) -> Result<u64, String> {
        let c = self.hold.make_clip(&self.p, blocks).map_err(|e| format!("the hold's make_clip of A#{blocks} blocks: {e:?}"))?;
        let n = c.blocks;
        {
            let mut r = self.receipt.lock().unwrap();
            r.handle = Some(c.handle);
            r.direct = c.direct();
        }
        self.clip = Some(c);
        Ok(n)
    }
    fn write(&mut self, block: u64, bytes: &[u8]) -> Result<(), String> {
        let c = self.clip.as_ref().ok_or("no clip open")?;
        self.hold.write_clip(c, block, bytes).map_err(|e| format!("the hold's write_clip of A#{} bytes at block A#{block}: {e:?}", bytes.len()))
    }
    fn seal(&mut self, len: u64, hash: [u8; 32]) -> Result<(), String> {
        let c = self.clip.take().ok_or("no clip open")?;
        let card = self.hold.seal_clip(c, len, &hash).map_err(|e| format!("the hold's seal_clip at A#{len} bytes: {e:?}"))?;
        self.receipt.lock().unwrap().card = Some(card);
        Ok(())
    }
}
