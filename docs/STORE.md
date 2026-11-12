# The hold: items, not folders

The hold is where a person's things are kept (owner, 2026-11-11: "the hold? Is that the right word?", then "Yes, and I'd say not worry about the snag. it's common to hold a bag and to put shit in the hold."): kists are stowed, items go in the hold. "Held call" (a doorbell wait) keeps its meaning beside it.

Status: a draft for the owner to mark up, 2026-11-11.
Nothing here is built.
Every part is **Proposed** unless it quotes the owner; the decisions it leans on are cited where they were made (VAULT-OUT.md, KISTS.md, AUTH.md, IDENTITY-SPACE.md, GENESIS.md).

## The ask

The owner, 2026-11-11, on where a game kist's saves go:

> Two permissions, saving under the kist key and saving under the user key. We'll have to work out the filesystem since it won't have a directory structure. Find items by content, date, who created, whatever but never in a directory tree as that's a fixed organization and not morphable into a different arrangement/ordering easily.

And then:

> yes, a spec draft for the storage is in order. Everything I'm used to uses directory trees so this will be new to me.

So this spec does two jobs.
Part 0 teaches the model in plain words, with everyday things, for someone who has used folders all their life.
Parts 1 to 6 say how it is built on the vault as it stands, what it costs, and what the owner has to decide.

---

## 0. The model in plain words

### Shelves and a catalog

A folder tree is a set of shelves.
Every thing sits on exactly one shelf, and to get it back you walk to that shelf: `Pictures/2026/Raglan trip/IMG_0042`.
The shelf is the only way in.
If you later want "every photo Ana is in" or "everything from November", the shelves do not help: those things sit on twenty shelves, and you either search by hand or copy them onto a new shelf, and then there are two of each.

A library also has a catalog.
The catalog does not care which shelf a book is on; it finds the book by author, by subject, by title, by year, and a book is in every one of those lists at once.
The ferros store is all catalog and no shelves.
A thing is kept once, and found by anything that is true of it.

A music player already works this way, and most people never notice: a song is in your library once, and it shows up under its artist, its album, "recently added", and in every playlist you put it in.
Nobody asks which folder a song is in.
The store does that for everything a person keeps.

### What an item is

An item is one thing a person or a kist keeps: a photo, a game save, a document, a song, a downloaded map area.
It is a VSF document (its body), and with it a small card that says what is true of it:

| on the card | what it says | example |
|---|---|---|
| id | which item this is, for good: born when it is made, never changes thru edits | 32 bytes (`hp`) |
| kind | what sort of thing it is, as a language-free idea (IDEAS.md) | "photo", "Sega Genesis save state" |
| made by | the kist that made it (its developer key and option), the person who made it, the device it was made on | Lumis, by the owner, on the Pro |
| made, changed | when, in Eagle time | the person reads "Tuesday 14:02" |
| title | words a person reads, when it has any | "Sonic 2, before the Death Egg" |
| attributes | typed facts its kind defines | a photo's place and lens; a save's game and slot |
| relations | other items it belongs with | this save is of that ROM; this crop is of that photo |
| size, versions | how big, how many kept | 210 KB, 3 versions |

There is no path, no folder and no file name anywhere.
The card is what the store knows about the item, and everything the person does to find it is a question asked of cards.

### How a person finds things

Every way in is a question, and any question can be combined with any other:

- **by kind**: my photos, my saves, my documents;
- **by time**: last week, November, the summer of 2025, a scrub along a timeline;
- **by who made it**: what Lumis took, what Ana sent, what I wrote on the MacBook;
- **by its words**: the title and any text the kind keeps ("Raglan", "invoice");
- **by its facts**: photos taken at a place, saves of one game, songs by one artist;
- **by relation**: everything belonging with this one (the saves of this ROM, the edits of this photo).

### Arrangements, without shelves

A person still arranges things; an arrangement is just never the place a thing lives.

- **A find kept** is a question with a name: "Sonic saves", "photos at Raglan".
  It updates itself: a new save shows up in it without anyone filing it.
- **A collection** is a hand-made list: "the trip", "tax 2026".
  An item can be in any number of collections at once; putting it in one does not move it out of another, and removing a collection removes no item.
- **A kist's own view**: Genesis shows its saves by game and slot; Mahere shows its areas on the map.
  Those are finds the kist asks, not places it owns.

Both kinds of arrangement are items themselves (kinds "find" and "collection"), so they sync and can be found like anything else.
Changing how things are arranged never moves anything, so it is always cheap and always undoable: that is the "morphable into a different arrangement/ordering" the owner asked for.

### Five everyday things

**A photo.**
Lumis takes it.
It is kept for the person (part 1), kind "photo", made by Lumis, by the person, on the Pro, at 14:02, with its place, lens and exposure as attributes.
Found by "photos", by "last week", by the place, by "made by Lumis", in the collection "the trip" and the kept find "photos at Raglan" at once.
Cropped in an editor kist, the crop is a new version of the same item, the original kept (part 3); the person can also keep the crop as its own item, related to the original.
In a folder world it would be `DCIM/Camera/IMG_20261111_140212.jpg` and every other way of finding it would be a copy.

**A game save.**
Sonic 2 on the Genesis kist, saved in slot 0.
Kind "Sega Genesis save state", made by Genesis, attributes the ROM's hash (`rom`), the game's title and the slot, related to the ROM item it was played from.
Genesis finds it by asking "save states of this ROM", and the person finds it by typing "Sonic".
Reinstalling Genesis, a new device, or a different build of Genesis (part 1) all find the same saves, because nothing about them depends on where Genesis put them.

**A downloaded map area.**
Mahere downloads the terrain and roads around Raglan for use offline.
That is Mahere's own working data, kept under Mahere's key (part 1): only Mahere can open it.
The person still sees its card ("Raglan, offline area, 120 MB, kept by Mahere for itself") and can remove it to free space, without ever being able to read it.
The track the person walked there is different: it is theirs, kept for the person, kind "GPS track", and outlives Mahere.

**A document.**
Written in an editor kist and kept for the person, kind "document", title its first line or the one the person gives it.
To open it in another editor the person picks it there (part 1, handing over): the second editor never gets to look around, it gets that one document.
Found by its words, by "changed this week", by "made on the MacBook".

**A song.**
Bought or recorded, kept for the person, kind "song", Opus in VSF, attributes artist, album, track number, length.
A player kist with the right to open the person's songs finds them all by kind; playlists are collections.
Nobody ever chooses a folder for it.

### What someone used to folders will ask

- **"Where is it?"**
  Nowhere a person needs to know.
  It is in the vault, under an address that is a function of its id; the person never sees or types an address.
  The answer to "where" is "ask for it": by kind, by time, by maker, by words.
- **"How do I organize?"**
  Collections and kept finds (above).
  Most of what folders were used for (by year, by app, by project) is a find that keeps itself up to date.
- **"How do I move it to another app?"**
  Pick it from that app, or drop it on it (the clipboard's handoff, part 1).
  Nothing is copied into the other app's folder, because there is no such folder.
- **"How do I back it up?"**
  The person's things live on their devices, fleet first (part 3); the find page marks anything held by one device only.
- **"Two apps both want my photos?"**
  Each asks once, ever, to open the person's photos; or neither does, and the person picks a photo each time.

---

## 1. Two rights: keep for itself, keep for you

The owner named two permissions.
Both already have a home in the vault as VAULT-OUT.md decided it (2026-11-07): each kist's data is its own domain, keyed from its signature; the person has a common area keyed by them.

| | keep for itself (the kist's key) | keep for you (the person's key) |
|---|---|---|
| the right | `domain`, built (KISTS.md: "its own storage") | new: `items`, with the kinds it makes and opens declared in the capsule |
| sealed under | oracle(the identity key ‖ developer key ‖ option): this kist, this person, this device | the person's items domain, under the identity key (VAULT-OUT.md's common area) |
| who opens a body | that kist only, on this device, while the person is present | the kist that made it, any kist the person let open that kind, the person's find page, a kist handed it |
| who sees the card | the person (title and size, part 2), and the kist | the person, and kists that may open it |
| follows the person | no (IDENTITY-SPACE.md: a kist's own domain is the kist's business) | yes, to every device of their fleet (part 3) |
| survives the kist's removal | kept until the person clears it (open question 4) | yes: it is the person's |
| a rebuilt kist under another key | cannot open it, by design | finds it by kind, once allowed |
| for | caches, downloads it can fetch again, its working state | anything the person would miss: saves, photos, documents, tracks |

The owner's line for Genesis (2026-11-11, GENESIS.md): "Proper VSF all the way. We can also note on saves to use the user vault rather than the kist."
That is the rule in general: if losing it would hurt the person, it is kept for the person.

### Kinds, and the question asked

A kind is an idea (IDEAS.md: 32 bytes, `hi`, named by its first phrasing), so "Sega Genesis save state" means the same thing to every kist and reads in the person's language.
The ferros project mints the common kinds (photo, document, song, video, GPS track, contact, find, collection); a developer mints their own (a Genesis save state), carried in their capsule as ideas are.

A kist that keeps things for the person declares, in its signed capsule, each kind it **makes** and each kind it **opens** (made by anyone), with a reason, as every want is declared today (KISTS.md "Permissions" 1).

- **Making** a declared kind, and opening again what it made itself, is the `items` right.
- **Opening** a kind made by anyone (other kists, other builds, the person's other devices) is asked per kind.
- **Asked once, ever**, per person, per kist, per kind (owner, 2026-11-07: "1 means 1"), and at the person's first action that needs it, never at install or launch (owner, 2026-11-10, permissions_asked_once: "maybe I never share my location with anyone so there's no reason for Photon to ever ask me"): Genesis asks to open the person's save states the first time they open its load page.
- **Read and write apart**, as VAULT-OUT.md decision 2 has the common area's names: changing an item made by someone else is its own answer (and implies opening, as write implies read there), and makes a new version (part 3), never an overwrite.
- The kernel stamps each call with the kist's identity and its granted kinds, as it stamps domain keys today (VAULT-OUT.md "Built"), so the store never takes a kist's word for what it may open.

This turns the common area's named grants (VAULT-OUT.md decision 2: `wallpaper`, `images`, `contacts`, `theme`) into kinds: the "small documented vocabulary" is the project's kinds, and a developer can add theirs (open question 1).

### Genesis rebuilt under another key

Genesis is GPL-3 (GENESIS.md); anyone may rebuild it and sign it with their own key, and it is then another kist (KISTS.md: a kist's identity is its developer key and option).

- The owner's build (key A) keeps a save state for the person: kind "Sega Genesis save state", made by A.
- Someone's rebuild (key B) declares that it makes and opens that kind.
- The first time the person opens B's load page, B asks once to open their Genesis save states; yes, and B finds every save A made, by kind and ROM, and A finds B's the same way.
- What A kept for itself (its key) B can never open.
  Nothing about a save may live there.
- The save's VSF schema (GENESIS.md "Save states") names the core's version (`core`), so a build whose core cannot read a state refuses it rather than guessing.

### Handing an item from one kist to another

Most kists should never need to open a kind at all: the person hands them the one thing.
This is the clipboard's model (owner, 2026-11-10, clipboard_capability_handoff: "a capability handoff, brokered by whoever owns the trusted input path") and KISTS.md's planned picker ("a system picker that hands a kist just the file the person chose (no permission needed: the choice is the grant)").

- **Pick (open).**
  The kist's "open" button, under the input path's one-shot ticket from the tap (no gesture, no asking), asks the store to show the find page inside the kist's circle, filtered to the kinds it takes.
  The person taps one item; the kist gets a handle to that item and nothing else.
  The pick is kept with the person's answers, so the kist can open that item again next launch until the person takes it back (open question 3).
- **Keep (save).**
  A kist without `items` offers what it made (typed, lazily, as a copy is offered); under the tap's ticket the store shows the keep sheet: the card the kist proposes (kind, title), editable, and "keep".
  Kept, it is the person's, made by that kist, and the kist holds a handle to it (so "save" again makes a version).
  With `items` the kist keeps directly; the sheet is for kists that never asked.
- **Drop.**
  Dragging an item's circle from the find page onto a kist's window is a pick by another gesture, as drag and drop is paste by another gesture.
- **Copy and paste.**
  Copying an item offers its handle; pasting hands a one-shot read to the destination, exactly the clipboard's rule.

A handle is the store's token for one item to one process (part 2), not the item's id: a kist cannot pass it to another kist, and nothing it learns lets it name other items.

---

## 2. Indexing and finding

### The card, as VSF

One section `card` per item, every value typed, nothing positional:

| field | type | |
|---|---|---|
| `id` | `hp` | the item's provenance: BLAKE3 of 32 random bytes drawn when it is made and its first card, so the same bytes kept twice are two items, and nobody can confirm a guess |
| `kind` | `hi` | the idea naming its kind |
| `kist` | section: `developer` `ke`, `option` `hR`, `name` `a` | the kist that made it, and its name then, so the card still reads after it is removed |
| `device` | `ke` | the device it was made on |
| `person` | `ke` | when made by someone else and received (a photo Ana sent); absent for the person's own |
| `made`, `changed` | `e` | Eagle time |
| `title` | `x` | optional |
| `size` | `u` | the body's bytes |
| `body` | `hb` | the current version's BLAKE3 |
| `version` | `u` | from 0 |
| `of` | section, repeated: `item` `hp`, `as` `u` | relations: 0 made from (a crop of), 1 belongs with (a save of a ROM); a typed kind, never a word |
| `attr` | section | the kind's own facts, by name and type (a save: `rom` `hp`, `slot` `u`, `core` `a`) |
| `keeper` | `u` | 0 kept for the person, 1 kept by the kist for itself (its body under the kist's key) |

A card is typically 200 to 300 bytes; it is bounded to one 4 KiB block (a kist that wants more puts it in the body).

### Where the index lives

The vault engine is a key-value store: manifestus answers `put`, `get`, `delete` by 32-byte key, and nothing else (no ranges, no listing, by design: "Non-enumerability is structural, not policy").
ferros_store builds values of any size and a list of names per domain on top of it.
The index is built the same way: VSF documents kept as values in the person's items domain, sealed like everything else.
No change to manifestus, its blocks or its leaves.

```
the root (one value, one 64 KiB piece at most)
  per page: its time span (start, log2 of its length), the kinds and makers in it,
            its digest (for sync)
      │
      ├── page [2^25 s from G#…]  ─ cards made in that span, in time order
      ├── page [2^24 s …]          (a page that outgrew one piece was split in two halves)
      ├── page [2^24 s …]
      └── …
the kinds and makers tables: each kind's and maker's small number in this domain
the recent list: the last 2^8 items changed, for "recently changed"
the bodies: each version a ferros_store value, in 64 KiB pieces
```

- **Pages by the time an item was made.**
  Every item has a birth time and it never changes, so a card never moves page; new items land in the newest page, so writes go to one page that stays in memory.
- **Power-of-two spans, split when full.**
  A page covers 2^k Eagle seconds from a multiple of 2^k (2^25 s is about a year).
  When its document passes one 64 KiB piece it splits into its two halves (2^(k−1) each), down to 2^12 s (about 68 minutes); a page that is still too full at that span continues in overflow pages.
  Nothing is ever rebalanced; two halves that shrink back into one piece may merge.
- **The root says which pages to read.**
  For each page, the kinds and the makers it holds (their small numbers), so "save states of Sonic 2" reads only the pages that hold any save state.
  The root is rewritten only when a page splits or first takes a kind or maker it did not hold, not on every add.
- **In memory, sorted vectors and binary search**, no hash maps: the root's pages by start, a page's cards by time, the kinds and makers tables by number.
  The decrypted root and the pages in use are kept while the person is unlocked and dropped when the lock epoch moves, as the vault process drops its plaintext today.

### What it costs on the vault as it stands

Numbers from VAULT-OUT.md's measurements on the Pro (2026-11-07 to 2026-11-11), and the card size above:

- **A page** holds about 2^8 cards.
  100,000 items (about 2^17, a heavy photo library) are about 400 pages and 25 MB of index; the root for them is about 30 KB, one piece.
  10,000 items are about 2.5 MB.
- **Keeping an item** writes its body's pieces, then its page (one 64 KiB piece: 1 MiB was written in 154 ms, so about 10 ms), and the root only on a split.
  Each is one commit thru ferros_store's atomic rewrite, mirrored and verified as every write is; the card goes into its page only once the body is committed, and a body whose card never landed (a kill between the two) is on the pending list that `sweep` clears.
- **A find by kind and time** reads the root (cached) and the pages it names: a year of save states is a page or two, a few milliseconds.
- **A find by words** over everything reads every page once (1 MiB read in 45 ms; 25 MB in about a second cold, 2.5 MB in about a tenth), then holds them while the person is unlocked.
  Past that size it wants a word index (below).
- **The engine walk** per value is the HAMT's 4 to 6 node reads (HAMT.md), mostly cached.
- **Large bodies** have no limit: ferros_store pieces them (64 KiB each, sealed apart, ranged reads open only the pieces they touch); a 106 MB capsule was stored in 23 s and a 6 MiB value read back in 648 ms.
  The 3 MiB ceiling was the kernel's old store path (`STORE_VALUE_MAX`) and the bridge's get; the vault process has none.
  A kist moves a large body a transfer at a time (`STORE_MAX`, 256 KiB a write, up to 1 MiB a read).

**One change ferros_store needs.**
Its list of names per domain (`\0names`) is one VSF document rewritten whole on every put (`edit_list`).
At 100,000 items that is a 4 MB rewrite per keep.
Bodies and pages must be written without entering that list (ferros_store already writes its own books that way: `Writer::books`), with the item index as their book.
The pending list and `sweep` still cover an interrupted write.
No on-disk change: the same values, one fewer entry in a list.

### Words

- **Indexed from the start:** the card's `title` and any `x` text among its attributes, found by scanning pages in memory.
- **Not indexed:** the words inside bodies (a document's text, a message).
  A kind may put a short excerpt on its card.
- **Later, if the owner wants it** (open question 6): a word index in the same shape, pages by a keyed hash of each word, listing item ids; sealed like everything, so it leaks nothing at rest.

### The calls

A kist talks to the store over IPC, its call stamped by the kernel with what it is and what it may open (part 1).
Messages are op bytes as the vault's are, the find and the card as complete VSF documents.

| op | does | needs |
|---|---|---|
| 0 make | a new item: its proposed card; returns a handle and starts the body | `items` for that kind, or the keep sheet's ticket |
| 1 write | body bytes at an offset (a transfer at a time) | a handle it made or may change |
| 2 finish | commits the body as a version, atomically | |
| 3 read | body bytes at an offset | a handle |
| 4 card | the card, as the kist may see it | a handle |
| 5 change card | title, attributes, relations | as write |
| 6 find | a find document; a page of handles and cards | the kinds it may open, and its own items |
| 7 watch | a find document, held; answered when its answer changes | as find |
| 8 remove | an item, or one version | it made it, or may change that kind |
| 9 pick | the find page in its circle, filtered | the input path's ticket |
| 10 offer | a handle offered to the clipboard or the keep sheet | the input path's ticket |
| 11 versions | the kept versions of an item | a handle |

A kist keeping things for itself uses the same calls with `keeper` 1; its cards are in its own index under its key, and their titles and sizes in the person's (open question 4).

### The find document

A complete VSF document, one section `find`, every condition optional, all of them together:

| field | type | |
|---|---|---|
| `kind` | `hi`, repeated | any of these kinds |
| `kist`, `device`, `person` | as on the card | made by |
| `from`, `to` | `e` | made in [from, to) |
| `changed` | `e` | changed at or after |
| `words` | `x` | in the title or text attributes |
| `of` | `hp` | related to this item (handles are turned into ids by the store) |
| `where` | section, repeated: `name` `a`, `is` `u`, `value` (any type) | an attribute: 0 equal, 1 at least, 2 at most, 3 holds the words |
| `order` | `u` | 0 newest first, 1 oldest first, 2 by title, 3 largest first |
| `skip`, `count` | `u` | paging; `count` at most 2^8 |

Genesis's load page: `kind` = Sega Genesis save state, `where` `rom` equal to this ROM's hash, `order` 0.

The answer is a section `found`: `total`, `generation` (the domain index's change count), and per item its handle and card.
A handle is a 32-byte token keyed with a secret drawn for that process's life, so it means nothing to any other process and nothing after the kist ends; the item's id is never handed out.

### Watching, not polling

A watch is a held call (feedback_no_polling: push or doorbell, never a question every so often): the store answers it when a keep, change or removal moves that find's answer, with the new `generation`, and the kist asks again.
The find page holds one; Genesis's load page holds one while it is up; nothing asks on a timer.

---

## 3. Versions, removal, sync, large items

### Versions

- An item's id never changes; its body changes as versions, each a ferros_store value (a rewrite is atomic: the old version is whole until the new one is committed).
- **Kept:** version 0 (the original, as made) and the last 2^2 (proposed; open question 5), each with when and by which kist and device.
  Older ones go when a newer one is committed.
  No version expires by time.
- **The person** can keep any version as an item of its own (related to the original, `as` 0), or go back to one (it becomes the newest).
- **A kind may say keep only the newest** (a battery RAM that changes with every in-game save) or keep more (a document).
- **Save states**: each slot is one item; saving to a slot again is a new version, so the slot's last few saves are there for "I saved at the wrong moment".

### Removal and the person's control

- **The person can remove any item of theirs**, whoever made it, from the find page; a kist can remove what it made, or a kind it may change.
- **Removed is gone**: its body's blocks are zeroed on both mirrors (VAULT.md "Deletion"), its card leaves the index, and a tombstone goes to the person's other devices so the item goes there too (the identity space's rule: a removal is a tombstone and sticks).
  Undo is offered while the find page that removed it is still open, never on a timer (open question 7: a bin, or not).
- **A kist's own items** (its key): the person sees them per kist with their sizes on the Kists page and can remove one or clear all; the bodies need the kist's keys, which the kernel issues only against that kist's own live call (VAULT-OUT.md decision 1), so they go at its next call, or by a drop the kernel stamps as it stamps release's (open question 4).
- **Release** (the device leaves the fleet, AUTH.md): the person's items on it are dropped with their domains, as today.
  The departure's "save first" checks first that every item held only here has reached another device.

### Sync across the person's fleet

- **Kept for the person: follows them.**
  Cards and bodies are identity class: every device of their fleet holds them, as the kist list and the arrangement travel ("whatever is the person's lives in their identity space, and every device of their fleet holds it").
- **Kept by a kist for itself: stays.**
  Its key binds this device and this person (VAULT-OUT.md), and IDENTITY-SPACE.md leaves a kist's own domain out of sync.
  A downloaded map area is fetched again on another device.
- **Cards first, bodies on demand.**
  Cards are small and go everywhere at once; a body crosses on a device's first open of it, or by a kind's policy (save states and documents everywhere at once; photos and video everywhere, but over WiFi or a sibling on the LAN; open question 8).
  This is the clipboard's doorbell again: every device knows at once that the thing exists; the bytes move when they are wanted.
- **How:** the root's page digests are compared between fleet devices (as `space_want` sends a digest); a page that differs is exchanged and merged card by card by the space's total order (Eagle time written, writing device's key, gone over present); bodies are fetched by id and version hash in pieces over Photon Transport from a device that holds them.
  In transit, sealed under the fleet key; at rest, sealed again under each device's identity key.
- **Never one basket** (feedback_single_egg_rule): an item is "safe" once a second device of the fleet holds its body; until then the find page marks it "only on this device".
- **fgtw.org** keeps the identity space's one copy in a 1 MiB slot (IDENTITY-SPACE.md).
  Bodies there would be a different service (Photon's capsule question again: R2 by hash, sealed); the owner's call (open question 9).

### Large items

- Bodies are ferros_store values: any size, 64 KiB pieces sealed apart, written a transfer at a time and committed whole at finish; read by range, opening only the pieces touched.
- A video or a long recording is read as a stream by ranges; the store never assembles a body in memory.
- A kind may keep a small preview on the card's side (a photo's thumbnail cut to a circle, AV1 in VSF, the map's icon form), as its own value, so the find page never opens a 24 MB body to draw a circle.
- The person's export (`SYS_UNLOCK` op 5) is bounded today (a value over 1 MiB left out, the whole over 4 MiB refused, VAULT-OUT.md); items need it streamed, a body at a time.

---

## 4. What the person sees

### The find page

Not a file browser: there is nothing to browse into.
It is the system's, not a kist's, because it opens every kind of the person's things and no kist should (the map's rule for the GPS, KISTS.md: "a map that can hand out the GPS is a map that can read it"); proposed as its own process, opened from the settings circle as "your things" (open question 10).

In words, top to bottom:

- **A search line** (the honeycomb keyboard): words match titles and text, and a kind's name or a kist's name typed there becomes that condition ("Sonic", "photos", "Lumis").
- **Kinds**, a row of small circles with counts: photos, saves, documents, songs, tracks, and whatever kinds the person's kists make.
  A tap narrows; a second tap widens again.
- **Time**, a strip from the oldest thing to now, dense where there are many: drag to zoom from years to days and slide along it, smoothly (no steps between years and months).
- **Who made it**: kist icons, the person's devices, people from their phonebook.
- **The things themselves** as circles, as everything on ferros is: a photo's circle is its picture, a save's the game's icon with its slot, a document's its first words; in the order chosen (newest, oldest, by title, largest).
- A held circle shows its card: its kind, title, who made it on which device and when, its versions, which kists may open it ("Genesis can open this"), and whether it is only on this device.
  From there: open in, keep a version, add to a collection, remove.

Any find can be kept with a name, and appears among the kinds as its own circle; a kept find or a collection can also be put on the home map as a circle (it opens on the find page).

### Open and save, without paths

| a folder world | ferros |
|---|---|
| "Open…": a dialog of folders, the app sees everything in them | **pick**: the find page in the app's circle, showing only the kinds it takes; the app receives the one item |
| "Save as…": choose a folder and a file name | **keep**: the card the app proposes (kind, title), editable; "keep"; it is the person's, found by everything on its card |
| "Save": overwrite the file | a new version of the same item |
| "Recent files" | the kist's watch on its own kinds, newest first |
| "Move to folder" | add to a collection; nothing moves |
| "Duplicate" | keep a version as its own item, related to the original |

Nothing asks the person where.

---

## 5. Migration: what the vault holds today

Not everything stored becomes an item.
**State** is a program's own memory (where the circles sit, the fleet chain, a driver's firmware): named values in scopes and domains, as today, never shown on the find page.
**Items** are things a person would look for, keep, or move between kists.
Both are ferros_store values; items add a card and the index.

| today | becomes | why |
|---|---|---|
| `map.` (`map.place.*`, `map.view`) | state, unchanged | the arrangement; already synced as identity-space facts |
| `map.wallpaper.made` | state, per device | made for this screen's size (VAULT-OUT.md "First use"); a wallpaper the person chose is a reference to their photo item, not a copy |
| `kists.` (`kists.list`) | state, unchanged | the roster and answers; the pick answers (part 1) join it |
| `capsules.` | state, device class, unchanged | the developer's public capsule by content address |
| `ahau.`, `fgtw.` | state, unchanged | attest's and the identity service's own |
| `wifi.`, `gps.`, `aoc.`, `gpu.` | state, unchanged | drivers' firmware and credentials; `gps.last` is a driver's memory, not a track |
| `test.` | gone | scratch |
| Mahere's domain: `session`, `settings` | state in its own domain | its memory |
| Mahere's cells (downloaded terrain and features, its own cell index) | items kept by Mahere for itself, one per area the person downloaded, its cells inside | the person sees "offline areas" and their sizes and can remove one |
| Mahere's tracks (`track` chunks) | items for the person, kind "GPS track", attributes start, length, distance; chunks inside the body | the person's walk, outlives Mahere |
| Mahere's areas and ink (`area`, `user` cells) | items for the person, kind "map drawing" | the person's drawing |
| Genesis's battery RAM and save states (GENESIS.md, waiting) | items for the person, kinds "Sega Genesis battery RAM" and "Sega Genesis save state" | the first use (part 8) |
| Lumis's captures (KISTS.md had them in its own domain) | items for the person, kind "photo" | the person's photos |
| Photon's messages, contacts, avatars (kete) | Photon's own state; a photo or file the person keeps from a chat becomes an item, made by Photon, `person` the sender | Photon is one app on every platform (feedback_photon_is_the_full_app); its store stays its own |

The move runs once per kist, by the kist (it holds its old state and the new right), never by the vault guessing.

---

## 6. New structures to approve

Rule -1: new formats and storage locations are the owner's decisions.
What this spec would add, and what it would not:

- **No new on-disk format.** No change to manifestus (tract, plow, HAMT, spine), to ferros_store's head and pieces, or to the vault's location; no new file anywhere.
- **New VSF schemas at rest**, inside the existing vault: the `card`, the index `page` and `root`, the kinds and makers tables, the `recent` list, the `find` and `collection` items.
- **A new VSF document over IPC**: `find` and `found`.
- **New frames over Photon Transport** for sync (page digests, pages, body pieces), beside `space_push` and `space_want`.
- **A new right**, `items`, with kinds in the capsule's declaration (ferros_kist `RIGHTS` and the declaration's shape).
- **ferros_store writes without its names list** for bodies and pages (no format change).
- **Not proposed:** keeping foreign files as they are.
  A file that arrives in another format is decoded on the way in, or the owner decides what is kept (Rule -1); an item's body is always VSF.

---

## 7. Open questions for the owner

0. **The name.** Decided (owner, 2026-11-11): the store is **the hold**. Still open: "item" for what is in it, and the find page's name.
1. **Kinds replace the common area's names?** VAULT-OUT.md decision 2 made the common area a small set of named grants (`wallpaper`, `images`, `contacts`, `theme`); this spec makes them kinds (ideas), with developers minting their own. Same idea, wider, or keep names for system state and kinds for items?
2. **May a kist open what it made itself without asking?** Proposed yes (it is the `items` right); a rebuild under another key asks per kind.
3. **Picks remembered.** Proposed: a picked item stays openable by that kist until the person takes it back, kept with their answers. Or a pick lasts only while the kist runs?
4. **A kist's own items.** Do their cards (title and size, never the body) show to the person, so a download can be seen and removed? When the kist is removed, are they kept until cleared, or dropped with it? Removing them needs the kist's keys: wait for its next call, or a drop the kernel stamps as at release?
5. **How many versions.** Version 0 and the last 2^2, per item, by default; a kind may say otherwise. No expiry by time.
6. **Words in bodies.** Index the words inside documents and messages (a word index, sealed), or titles and attributes only?
7. **Removed means gone?** Or a bin the person empties by hand (no timer either way)?
8. **Bodies across the fleet.** Every device holds every body (simple, many copies), or cards everywhere and bodies by kind and by the person's choice per device (the Pro holds all photos, the 8a holds the last month)?
9. **A copy off the person's devices.** Should fgtw.org (or R2) keep sealed bodies as the space's backup, for a person whose only device is lost? Today it keeps 1 MiB of the identity space.
10. **The find page's home.** A page of the settings circle, its own built-in circle (no_baked_apps allows settings alone), or the picker only?
11. **Things from other people.** A photo Ana sends thru Photon: an item only when the person keeps it (proposed), or every received photo at once?
12. **Kinds' first phrasings.** The project's kinds need minting (IDEAS.md), and IDEAS.md's publishing order holds: no idea is published before vsf v11.

---

## 8. Build plan, smallest useful first

0. **Genesis saves kept and found** (the waiting user, GENESIS.md "Saves").
   - `ferros_items` (no_std, over ferros_store, host-tested): the card, the time pages and root, make, write, finish, read, card, find by kind and attributes, remove, versions; ferros_store's writes without its names list.
   - The vault process serves it in the person's items domain; the kernel stamps the caller's `items` right and kinds.
   - ferros_kist: the `items` right with kinds in the declaration; the Genesis kinds minted as ideas.
   - Genesis: battery RAM kept per game as it changes; save states (jgenesis's `vsf` feature) per game and slot; its load page finds them by ROM and watches.
   - The bridge: `items` commands (find, card, get, put) for the bench.
   - Done when: saves kept, the kist killed and launched again, the same saves found; then a second Genesis signed with another key, allowed once, finds them too.
1. **Pick and keep.** The find page as the picker in a kist's circle and the keep sheet, under the input path's ticket; picks remembered; a kist with no `items` keeps thru the sheet.
2. **The find page** ("your things"): search, kinds, the time strip, who made it, cards, versions, remove; kept finds and collections.
3. **Sync**: cards across the fleet by page digests, bodies on demand by kind, "only on this device", and the departure's check.
4. **Moves**: Mahere's tracks and drawings to the person's items and its downloads to its own with cards; Lumis's captures as photos; the wallpaper as a reference to a photo; Photon's kept attachments.
5. **Later, if wanted**: the word index; kept finds and collections as circles on the home map; a sealed copy off the fleet.
