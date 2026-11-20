# The hold's API for kists

What a kist calls to keep things for the person in the hold, find them, read their bodies by range and remove them (docs/STORE.md is the design; this is the interface as built on branch `hold`, 2026-11-19).
Lumis keeps stills and RAW clips with it; Kūaha (`kuaha` in code), the kist that views the person's items, finds, reads and removes with it.

Status: built and host-tested (ferros_items `tests/serve.rs`: a kist's calls thru the kernel's stamp into the vault's server over a RAM vault and clips area); the kernel, the vault process and the bridge build; nothing has run on a device yet.

## Declaring it

A kist that keeps things for the person asks for the right `items` (ferros_kist `ITEMS`, code 13, asked once, ever: "your things") and declares, in its signed capsule's `items` section, each kind it **makes** and each kind it **opens**, with one line of reason each (ferros_kist `ItemsDecl`, `capsule::unsigned_with_items`).

- Making a declared kind, and opening again what it made itself, is the right.
- Opening a kind made by anyone is the `open` declaration (for now granted with the right; per-kind answers are not built: STORE.md's handoff).
- The kernel stamps every call with the kist's identity and these kinds (process.rs `items_stamp`), so the vault never takes the kist's word.
- A right the person said no to is an empty slot: every call on it is refused, never answered with made-up items.

From the bench, a capsule declares them with `ferros-bridge capsule <code.bin> <out.vsf> --kist NAME --option HEX --want items "REASON" --makes "RAW video" "REASON" --opens scan "REASON"`; `ferros-bridge items kinds` lists the kinds by their first phrasings.
A development spawn gets any kind with `ferros-bridge spawn <payload.bin> --items` (grant 18).

The `items` grant's slot is the want's slot (ferros_kist `slots`: a kist that declares no implicit right has slots 1 to 6 for display, touch, surface, ira, domain and entropy, then its wants in order from slot 7).

## The calls

ferros_items `kist::Hold<T: Transport>`, no_std and alloc.
On a device the transport is `kist::Sys { cap }`, the grant's token offset in the arc (`0x204 + 32 * slot` in a std kist, `ferros_sys::cap(grants, slot)` in a payload); a payload may also call `ferros_sys::items_call` itself.
`Hold::new(Sys { cap }, clock)`, where `clock` gives Eagle time in oscillations (the documents a call carries are stamped with it).

| call | does | answers |
|---|---|---|
| `keep(&Proposed, &body)` | a new item and its whole body (`make`, `write`, `finish`) | its handle and card |
| `make(&Proposed)`, `write(&Body, &bytes)`, `finish(Body)` | the same, a body of any size sent a MiB a call | a `Body` token, then the handle and card |
| `keep_again(&handle)`, `write`, `finish` | keep it again in place: the new body replaces the old (no versions, owner §7.5) | the same handle |
| `find(&Find)` | a page of matching items the kist may open | `Found`: total, the index's change count, (handle, card) each |
| `card(&handle)` | the card | `Card` |
| `read(&handle, offset, &mut buf)` | body bytes from `offset`, a MiB a call, a clip's from its blocks | (bytes read, the body's size) |
| `remove(&handle)` | gone at once: card, body, a clip's extent; no bin (owner §7.7) | |
| `make_clip(&Proposed, blocks)` | a new item whose body is a single-copy extent of `blocks` 4 KiB blocks; its card placed at once, so it is found while it records | a `Clip`: its handle, the blocks reserved, whether it writes straight to the flash |
| `write_clip(&Clip, block, &bytes)` | whole blocks at `block`, at most a MiB: straight to the flash thru the extent's own capability, or thru the vault when the kernel gave none | |
| `seal_clip(Clip, len, &hash)` | sealed at `len` bytes with the recorder's seal; its tail given back | the card as it now stands |

`Proposed` is the card a kist proposes: `kind` (an idea, 32 bytes: ferros_items `kinds`), `title`, `of` (relations: another item's id and `OF_MADE_FROM` 0 or `OF_BELONGS` 1), `attrs` (the kind's own facts: a name and any typed VSF value), `keeper` (0 for the person).
The store adds the rest of the card: its id, the kist that made it, the device, made and changed (Eagle time), size and BLAKE3, and a clip's extent.

`Find` (every condition optional, all together): `kinds` (any of), `maker` (ferros_items `maker_tag(developer, option)`), `from` and `to` (made in, Eagle time), `changed` (at or after), `words` (in the title or a text attribute, any case), `of` (related to an item the caller holds a handle to), `wheres` (an attribute equal, at least, at most, or holding words: `IS_*`), `order` (newest, oldest, by title, largest: `ORDER_*`), `skip` and `count` (at most 256).

A handle is the vault's token for one item to one caller: another kist's handle for the same item differs, and none lasts past the vault process or a lock; find again then.

Every error is `ferros_items::Error`: `Refused` (no right to that kind, a handle not the kist's, a clip still recording when removed), `NotFound` (gone), `Format`, `TooLarge` (no room in the clips area, a card past 4 KiB).

### Bounds

- A transfer is a MiB (`wire::TRANSFER`): `write`, `read` and `write_clip` split larger buffers themselves.
- A find answers at most 256 cards in one document of at most a MiB; ask for fewer, or page with `skip`.
- At most 8 bodies in the making at once a kist (`serve::MAKINGS_MOST`).
- One clip at a time a process: the kernel takes a process's earlier extent back when it gives the next.

## Lumis

A still: its body is the still's VSF document.

```rust
let mut hold = Hold::new(Sys { cap: 0x204 + 32 * ITEMS_SLOT }, eagle_now);
let p = Proposed { kind: kinds::PHOTO, title: None, attrs: vec![("lens".into(), VsfType::u3(lens)), ("exposure".into(), VsfType::e(...))], ..Default::default() };
let (handle, card) = hold.keep(&p, &still_vsf)?;
```

A RAW clip, thru ferros_clip's recorder: the `Sink` the recorder writes to becomes an `ItemSink` (ferros_clip is another agent's tonight; specified here, to be written after):

```rust
/// The recorder's sink as an item of the hold: the clip's card placed at its open, its blocks written once straight to the flash, sealed into its card.
pub struct ItemSink { hold: Hold<Sys>, p: Proposed, clip: Option<Clip>, pub card: Option<Card> }

impl Sink for ItemSink {
    fn open(&mut self, blocks: u64) -> Result<u64, String> {
        let c = self.hold.make_clip(&self.p, blocks).map_err(|e| format!("{e:?}"))?;
        let n = c.blocks;
        self.clip = Some(c);
        Ok(n)
    }
    fn write(&mut self, block: u64, bytes: &[u8]) -> Result<(), String> {
        self.hold.write_clip(self.clip.as_ref().ok_or("no clip open")?, block, bytes).map_err(|e| format!("{e:?}"))
    }
    fn seal(&mut self, len: u64, hash: [u8; 32]) -> Result<(), String> {
        let c = self.clip.take().ok_or("no clip open")?;
        self.card = Some(self.hold.seal_clip(c, len, &hash).map_err(|e| format!("{e:?}"))?);
        Ok(())
    }
}
```

The proposed card: `kind: kinds::RAW_VIDEO`, a title if the person gave one, attributes such as `fps`, `width`, `height`, `camera` (the sensor's name as `x`), `gain`.
Its writes are the extent's own SYS_BLOCK capability, as `ExtentSink`'s are: no copy thru the vault, no mirror, the flash's rate (owner, 2026-11-19: "1:1 write speed, with no dupes since it's massive").
If Lumis dies mid-clip, the clip is still the person's: the vault's recoverer closes it at its last whole frame, and the next call on the hold (anyone's) corrects its card from the extents' table.

## Kūaha

Kūaha declares no `make` (it views) and `open` for the kinds it shows: photo, RAW video, video, scan, list, contact, document.

- **Find**: by kind, time (`from`, `to`), maker, words and attributes; newest first by default; a page of 256 at most, `skip` for the next.
- **Show**: the card (title, kind, who made it on which device and when, size).
- **Read**: `read(&handle, offset, &mut buf)` streams any body by range, a RAW clip as well as a still; nothing is assembled whole.
- **Remove**: `remove(&handle)`, gone at once; a clip still being recorded is refused until it is sealed.
- **Hand an item to the kist that opens it** (Opsin for stills, RAW clips and scans; Photon for contacts): not built.
  It is STORE.md's step 1 (pick and offer under the input path's ticket).
  Until then a kist that declares `open` for a kind finds the item itself, by its kind and attributes.

## The bench

`ferros-bridge items find [--kind K]... [--words W] [--order newest|oldest|title|largest] [--skip N] [--count N] | card <handle> | get <handle> <out> [--from N] [--bytes N] | put <kind> <file> [--title T] [--clip] | rm <handle> | kinds`, as the person (every kind), each call one STORE command (job G#F9).

## How it is wired

- The kist's call: `SYS_STORE` x1 bit 7, x0 the `items` token, x2/x3 the call (ferros_items `wire`: [op][handle 32][offset u64][length u32][payload]), x4 the reply's room, x1 bits 8-31 its length.
- The kernel (process.rs `items_forward`): the person's items domain's keys (`items.`, identity class: only while the person is present), the caller's stamp, the call; forwarded to the vault as `VOP_ITEMS` 8.
- The vault process (`OP_ITEMS`): ferros_items `serve::Server` over its store and its clips area (`ItemClips`); the index in time pages under a root, the bodies as ferros_store values off its names list, a clip's body an extent named by the item's id.
- At release the person's items domain is dropped with their other domains, its clips with it.
