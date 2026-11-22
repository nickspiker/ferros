# The hold's API for kists

What a kist calls to keep things for the person in the hold, find them, read their bodies by range and remove them (docs/STORE.md is the design; this is the interface as built on branch `hold`, 2026-11-19).
Lumis keeps stills and RAW clips with it; Kūaha (`kuaha` in code), the kist that views the person's items, finds, reads and removes with it; Aemulus (GENESIS.md) lists the person's cartridges and keeps its saves with it.

Status: host-tested (ferros_items `tests/serve.rs`: a kist's calls thru the kernel's stamp into the vault's server over a RAM vault and clips area) and run on the Pro (2026-11-20, branch `hold` ddfd6437 flashed to both slots; "On the Pro" below).

## Declaring it

A kist that keeps things for the person asks for the right `items` (ferros_kist `ITEMS`, code 13, asked once, ever: "your things") and declares, in its signed capsule's `items` section, each kind it **makes** and each kind it **opens**, with one line of reason each (ferros_kist `ItemsDecl`, `capsule::unsigned_with_items`).

- Making a declared kind, and opening again what it made itself, is the right.
- Opening a kind made by anyone is the `open` declaration (for now granted with the right; per-kind answers are not built: STORE.md's handoff).
- The kernel stamps every call with the kist's identity and these kinds (process.rs `items_stamp`), so the vault never takes the kist's word.
- A right the person said no to is an empty slot: every call on it is refused, never answered with made-up items.

From the bench, a capsule declares them with `ferros-bridge capsule <code.bin> <out.vsf> --kist NAME --option HEX --want items "REASON" --makes "RAW video" "REASON" --opens scan "REASON"`; `ferros-bridge items kinds` lists the kinds by their first phrasings.
A development spawn gets any kind with `ferros-bridge spawn <payload.bin> --items` (grant 18).

The `items` grant's slot is the want's slot (ferros_kist `slots`: a kist that declares no implicit right has slots 1 to 6 for display, touch, surface, ira, domain and entropy, then its wants in order from slot 7).

## The kinds

A kind is an idea (IDEAS.md): 32 bytes, `hi`, pinned by its first phrasing in English, `ferros_idea::id(eng, words)`; ferros_items `kinds::KINDS` holds the project's, each held to its phrasing by the test `kinds_are_their_first_phrasings`.
The bench and a capsule's `--makes` and `--opens` name a kind by that phrasing (`ferros-bridge items kinds` lists them).

| first phrasing | ferros_items `kinds` | what its body is | made by | opened by |
|---|---|---|---|---|
| photo | `PHOTO` | a still's VSF document | Lumis | Kūaha |
| RAW video | `RAW_VIDEO` | a RAW clip, a single-copy extent | Lumis | Kūaha |
| video | `VIDEO` | | | Kūaha |
| document | `DOCUMENT` | | | Kūaha |
| scan | `SCAN` | | | Kūaha |
| list | `LIST` | | | Kūaha |
| contact | `CONTACT` | | | Kūaha |
| song | `SONG` | | | Kūaha |
| GPS track | `GPS_TRACK` | | | Kūaha |
| find | `FIND` | | | Kūaha |
| collection | `COLLECTION` | | | Kūaha |
| Sega Genesis save state | `GENESIS_SAVE_STATE` | a state as one complete VSF file (GENESIS.md "Saves") | Aemulus | Aemulus, Kūaha |
| Sega Genesis battery RAM | `GENESIS_BATTERY_RAM` | a cartridge's battery RAM as one complete VSF file | Aemulus | Aemulus, Kūaha |
| Sega Genesis cartridge | `GENESIS_CARTRIDGE` (G#E9052E5ACD3CF616..., 2026-11-21) | the ROM exactly as the person's file holds it (an ingest, nothing converted) | the person, from the bench (`items put`) | Aemulus |

A kind made by no kist yet is minted so every kist means the same by it; the person (the bench) may keep any kind.

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
| `usage()` | what is on the device, read only: the hold's totals by kind (every item, whoever made it), the clips area, every stowed kist with its own domain's size | `wire::Usage` |
| `usage_of(&[(developer, option)])` | one usage call sizing at most 64 named kists (`usage` makes one call to learn the kists and one a 64 to size them) | `wire::Usage` |
| `make_clip(&Proposed, blocks)` | a new item whose body is a single-copy extent of `blocks` 4 KiB blocks; its card placed at once, so it is found while it records | a `Clip`: its handle, the blocks reserved, whether it writes straight to the flash |
| `write_clip(&Clip, block, &bytes)` | whole blocks at `block`, at most a MiB: straight to the flash thru the extent's own capability, or thru the vault when the kernel gave none | |
| `seal_clip(Clip, len, &hash)` | sealed at `len` bytes with the recorder's seal; its tail given back | the card as it now stands |

`Proposed` is the card a kist proposes: `kind` (an idea, 32 bytes: ferros_items `kinds`), `title`, `of` (relations: another item's id and `OF_MADE_FROM` 0 or `OF_BELONGS` 1), `attrs` (the kind's own facts: a name and any typed VSF value), `keeper` (0 for the person).
The store adds the rest of the card: its id, the kist that made it, the device, made and changed (Eagle time), size and BLAKE3, and a clip's extent.

`Find` (every condition optional, all together): `kinds` (any of), `maker` (ferros_items `maker_tag(developer, option)`), `from` and `to` (made in, Eagle time), `changed` (at or after), `words` (in the title or a text attribute, any case), `of` (related to an item the caller holds a handle to), `wheres` (an attribute equal, at least, at most, or holding words: `IS_*`), `order` (newest, oldest, by title, largest: `ORDER_*`), `skip` and `count` (at most 256).

A handle is the vault's token for one item to one caller: another kist's handle for the same item differs, and none lasts past the vault process or a lock; find again then.

Every error is `ferros_items::Error`: `Refused` (no right to that kind, a handle not the kist's, a clip still recording when removed), `NotFound` (gone), `Format`, `TooLarge` (no room in the clips area, a card past 4 KiB).

### What Ok means (2026-11-22)

A keep's Ok (`finish`, `keep`, and `finish_verified` / `keep_verified`) is sent only after the body, its card and the commit naming them were each written and read back equal on every copy the vault holds; anything less is an Err, and an Err leaves the item as it was (a new item absent, a kept-again item its old body).
The vault holds two copies (manifestus's mirror: the first copy's whole batch written and read back before the second's first block, then each commit's spine entry the same way), so an app needs no read-back of its own (the owner, 2026-11-22: Lumis drops its read-back of each still).
`finish_verified` and `keep_verified` say it as a field, `wire::Kept { handle, card, copies }`:
- `copies` 2: kept and verified on both copies.
- `copies` 1: kept and verified on one; the vault runs on one copy, the other having failed at this write or earlier (dropped until the vault opens again, manifestus's degraded mirror). The item is kept; an app that wants two can say so to the person.
- `copies` None: a vault from before 2026-11-22, which does not say.
In the answer it is a `kept` head (`copies`, unsigned) beside the `card` section of OP_FINISH's document; `finish` and `keep` read the same answer and leave it out.
The store's own write op (a process's own values, `store put`) answers on the same terms: its last write is answered after the value's commit is verified on each copy.
A RAW clip's body is the exception by design: a single-copy extent, written once at the flash's rate with no mirror and no read-back (its card is kept as above).
The limit, stated plainly: "read back equal" is equal as the flash device returns it. ferros does not yet flush the device's own write cache (no SYNCHRONIZE CACHE or FUA; the vault's flush is a no-op, each command polled to completion), so a power cut in the moment after an Ok rests on the device's own power-loss handling.

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

A RAW clip, thru ferros_clip's recorder: its sink is ferros_clip `items::ItemSink` (branch `clip-items`): `open` is `make_clip`, `write` is `write_clip`, `seal` is `seal_clip`; `ExtentSink` stays beside it.

```rust
// Lumis: the `items` grant's slot, the person's title if any, the clip's start (Mode::at, Eagle oscillations), its own facts.
let (sink, receipt) = ferros_clip::items::raw_video(ITEMS_SLOT, title, at, vec![("fps".into(), VsfType::u3(fps)), ("camera".into(), VsfType::x(cam.name.clone()))]);
let rec = ferros_clip::Recorder::start(&camera, &mode, Box::new(sink))?;
// ... frames ...
let info = rec.stop();
let r = receipt.lock().unwrap().clone(); // r.handle from the open, r.direct (straight to the flash), r.card the sealed card
```

- `raw_video` stamps the documents with Eagle time now (vsf `eagle_time_oscillations`) and proposes `raw_video_card(title, at, attrs)`: kind RAW video, the attribute `at` (the clip's start, `e6`), then the caller's.
- `ItemSink::new(Hold<T>, Proposed)` takes any transport and any card (a host test's, another kind).
- `items::ItemSource::new(hold, handle, size)`: an item's body read by range as a VSF byte source, so `vsf::visual::clip::Reader` opens a clip kept as an item without assembling it (Kūaha's view, rawrec's check).
- A Lumis built with it needs the `items` right in its capsule, `--makes "RAW video"`, and the slot it lands in.

On the Pro (2026-11-20, hold ddfd6437 flashed, rawrec with `--items` on the GPU path, `clip-items` 22866c30):
- **A minute at 30/s kept as an item**: 1800 frames, 0 dropped, written straight to the flash (the extent's capability); found by `items find` at 0 bytes while it recorded, sealed at 8,616,222,347 bytes, its card's body the recorder's seal; read back on the device thru `ItemSource`, 225 hashed frames and 4 kept copies bit exact; pulled with `items get` (402 s, 21.4 MB/s) and every one of its 1800 frames bit exact against the camera's on the host (clip-check with rawrec's hashes).
- **Killed mid-clip** (`diag "kill rawrec"` at 9 s): the camera closed with its holder; the vault's recoverer closed the clip at its 274th whole frame (1,311,290,779 bytes, 6,568,956 blocks given back, in about 0.5 s) and the next `items find` showed the card corrected; pulled, its 274 frames decode against their own samples' hashes.

The proposed card: `kind: kinds::RAW_VIDEO`, a title if the person gave one, attributes such as `fps`, `width`, `height`, `camera` (the sensor's name as `x`), `gain`.
Its writes are the extent's own SYS_BLOCK capability, as `ExtentSink`'s are: no copy thru the vault, no mirror, the flash's rate (owner, 2026-11-19: "1:1 write speed, with no dupes since it's massive").
If Lumis dies mid-clip, the clip is still the person's: the vault's recoverer closes it at its last whole frame, and the next call on the hold (anyone's) corrects its card from the extents' table.

## Kūaha

Kūaha declares no `make` (it views) and `open` for the kinds it shows: photo, RAW video, video, scan, list, contact, document.

- **Find**: by kind, time (`from`, `to`), maker, words and attributes; newest first by default; a page of 256 at most, `skip` for the next.
- **Show**: the card (title, kind, who made it on which device and when, size).
- **Read**: `read(&handle, offset, &mut buf)` streams any body by range, a RAW clip as well as a still; nothing is assembled whole.
- **Remove**: `remove(&handle)`, gone at once; a clip still being recorded is refused until it is sealed.
- **The whole device**: `usage()` answers `wire::Usage`:
  - `kinds`: a `KindUse` per kind in the hold (`kind`, `items`, `bytes`), every item counted whoever made it and whatever Kūaha may open;
  - `clips`: `ClipsUse` (`blocks` of the clips area, 4 KiB each; `taken`; `largest_free`);
  - `kists`: a `KistUse` per stowed kist from the person's list (`developer`, `option`, `name`; `values` and `bytes` of its own domain; `clip_blocks`, the clips area its own extents take, Lumis's recordings until it keeps clips as items).
  It needs the `items` right and the person present.
  A kist's own domain is sized from its values' heads (ferros_store `size`: its names list, then one head a value, no value read), so its cost grows with its count of values: on the Pro the ten stowed kists took 1.17 s in all, nearly all of it Photon's 2,412 values; the hold's totals and the clips area are a few milliseconds. Show the hold and the clips area at once and the kists' sizes as they come, or cache them.
- **Hand an item to the kist that opens it** (Opsin for stills, RAW clips and scans; Photon for contacts): not built.
  It is STORE.md's step 1 (pick and offer under the input path's ticket).
  Until then a kist that declares `open` for a kind finds the item itself, by its kind and attributes.

## Aemulus

Aemulus (GENESIS.md) declares `items` with `--makes` "Sega Genesis save state" and "Sega Genesis battery RAM" and `--opens` those two and "Sega Genesis cartridge" (genesis-kist tools/aemulus-kist.sh); its grant is slot 7.

- **The chooser**: `find` with `kinds` the cartridge, `order` newest, `count` 256; each card's title and size; a pick read whole by `read` (a MiB a call).
- **A save**: `find` the newest of its kind whose `rom` attribute (`IS_EQUAL`, `hp`) is this game's; when there is one, `keep_again` it in place (one a kind and game, no versions), else `keep` a new one: the game's title, `rom` as an attribute, `of` the cartridge's id with `OF_MADE_FROM` when the game came from the hold.
- **A load**: the same find, then `read` the body whole; the battery RAM so found goes to the core at start.

Host-tested against `serve::Server` over a RAM vault (genesis-kist `hold.rs` tests); not yet run on a device.

## The bench

`ferros-bridge items find [--kind K]... [--words W] [--order newest|oldest|title|largest] [--skip N] [--count N] | card <handle> | get <handle> <out> [--from N] [--bytes N] | put <kind> <file> [--title T] [--clip] | rm <handle> | usage | kinds`, as the person (every kind), each call one STORE command (job G#F9).

## On the Pro (2026-11-20)

Branch `hold` ddfd6437 (raw-video-rec b3c55355 merged), RAM-reloaded, then flashed to both slots and cold-booted (keys/husky/images/boot_hold_ddfd6437_vaultcarried.img, read back equal; the slots before in keys/husky/images/before-hold-2026-11-20/):
- **The bench**: a 15.7 MB still put in 2.3 s, found, pulled whole (14.5 MB/s) and by a range, its BLAKE3 the file's, removed, its handle refused after. A 363 MB RAW clip put with `--clip` in 15.9 s (through the vault, the bench having no capability), its extent in `store extent-list` under the items domain's owner, pulled at 21.4 MB/s with the file's BLAKE3, removed, its extent gone.
- **A kist's calls** (payloads/holdtest, a development spawn with `--items`): a 3 MiB still kept in 324 ms, found, read back equal in 78 ms, a range across a MiB equal, `usage` answered, removed.
- **A clip killed mid-recording**: holdtest copied the bench's clip into its own clip straight to the flash (the extent's capability), its card found at 0 bytes while it recorded and its removal refused, then `diag "kill holdtest"` at about 140 MiB; the vault's recoverer closed it at its 26th whole frame (147,543,310 bytes, 52,652 blocks given back) and the next call on the hold corrected its card; the pulled clip equals the source to its frames' end and decodes 26 frames (clip-check).
- **A lock**: holdtest locked the identity mid-session (SYS_UNLOCK op 2): its find and its card by an old handle refused, the bench's too; the cold boot after unlocked at boot, an old handle refused, a find handing new ones.
- **The cold boot**: the vault from the kernel image, identity unlocked at boot, the vault's 125 names and the clips area's extents as before, both slots read back equal.
- **Lumis** (lumis-rv21.bin hand-spawned with `--use gpu --dma layers`): 796 frames in 13.3 s at 60 a second, 0 dropped, coded on the GPU, with an 8 ms shutter (at the scene's own 67 ms exposure the camera gives 15 frames a second: 186 frames in 12.6 s, none dropped). Note that Lumis reserves the whole free clips area for a recording, so a kist's `make_clip` is refused while Lumis records.

**Stowing a kist that asks for `items`** needs a stower, init and map that know the right (code 13): one from before refuses its capsule (reason 16, NOT_KIST). On the Pro (b1048) the stower and init (masked, sealed) and the map (native) were stored from `hold` 999fddb0, the old ones kept in target/agent-hold/stower/old-*.vsf; after a cold boot every puck started again, and Kūaha (capsule 8da57037...) was stowed automatically in developer mode, `items` unanswered until its first start asks.

## How it is wired

- The kist's call: `SYS_STORE` x1 bit 7, x0 the `items` token, x2/x3 the call (ferros_items `wire`: [op][handle 32][offset u64][length u32][payload]), x4 the reply's room, x1 bits 8-31 its length.
- The kernel (process.rs `items_forward`): the person's items domain's keys (`items.`, identity class: only while the person is present), the caller's stamp, the call; forwarded to the vault as `VOP_ITEMS` 8.
- The vault process (`OP_ITEMS`): ferros_items `serve::Server` over its store and its clips area (`ItemClips`); the index in time pages under a root, the bodies as ferros_store values off its names list, a clip's body an extent named by the item's id.
- At release the person's items domain is dropped with their other domains, its clips with it.
