# Notifications

Status: decided by the owner 2026-11-12; the first cut built the same day (branch `notify`): the kernel's notes (SYS_NOTIFY, process notify.rs), the note's VSF (ferros_kist::note), the deck's side (payloads/map notes.rs), the Deck button (display op 66) and the open-at event (display op 67, `EV_OPEN_AT`).
System questions (the Deck button red while the system waits on the person) decided and built the same evening ("System questions" below).
Each part says **Decided** (with the owner's words), **Built**, or **Proposed** (mine, not yet answered).

The owner, rejecting sliders and permission lists: "we have a brand new UI so we have an opportunity to do notification proper."
Then: "I think we try it with the in view, it can notify me, out of view, silent. Screen's big enough, we'll see."

## The model

- **Decided: reach is the view.** A source (a kist, later a person or a puck) whose circle is in the deck's resting view can notify; out of view it is silent.
  It may still glow where it sits, seen when the person looks there.
  To focus, the person zooms in on the circles they want; to be left alone, they zoom into empty space.
  This replaces do-not-disturb toggles and per-kist notification settings.
- **Decided: three kinds, by what the thing is, never chosen by the kist:**
  - **live**: a beam, an alarm the person set. It rings and takes the screen until it is answered, declined, or ended by the caller.
  - **personal**: a person wrote. One haptic tap, or one sound, by the sound mode, and the glow.
  - **status**: updates, downloads. The glow only, never a sound.
- **Decided: the glow** is the "for you" state the deck already has (ferros_glow `Glow::ForYou`, 400 nm; its reach grows with the count; display op 57).
- **Decided: a kist in front** is never covered by a note; only live takes the screen.
  What waits shows on the Deck button (below).
- **Decided: always thru:** alarms the person set, and safety (the battery dying).
- **Decided: a note clears when it is seen on any of the person's devices**, never on a timer.
  A kist may set one deadline on a note, like an alarm.

## A note

**Built** (ferros_kist::note): a complete VSF file with one section, `note`.

What the kist says:

| field | VSF | what |
|---|---|---|
| `kind` | `u` | the kind it asks for: 0 status, 1 personal, 2 live |
| `person` | `ke` | the person it is about, if a person wrote: their identity key |
| `idea` | `hi` | what it means, language-independent (IDEAS.md) |
| `words` | `a` | the kist's own words for it, at most 256 bytes: a line, never the message |
| `count` | `u` | how many things it stands for (five messages from Ana: one note, count 5); 1 when absent |
| `target` | `hb` | where a tap lands in the kist: a page's or an item's 32-byte id, the kist's own |
| `key` | `hb` | the note's own name on every device of the person's: a note posted with a key it has waiting already revises that note |
| `deadline` | `e` | at most one: past it the note goes as if withdrawn |

What the system adds, never taken from a kist: `source` (`hb`, the kist's capsule address), `number` (`u`, 0 to 15 among the kist's own), `asked` (`u`, the kind the kist asked for, beside the `kind` the system decided) and `posted` (`e`).
The kernel parses what a kist posts with the schema and rebuilds the note from what it parsed: what it stores and serves is its own encoding, never the kist's bytes.
A kist's note is untrusted input: anything out of shape (a hash that is not 32 bytes, words past their bound, a kind past live, a document past 1 KiB) refuses the whole note, never a part of it.

**Words in one script for now.**
The kernel and the map read VSF without the `x` codec (vsf's x-lang16 is 140 KB linked), so a note's words are ASCII in this first cut and a note in another script carries its meaning as an `idea`.
The system shows a note's idea in the person's language once the ideas directory serves phrasings (IDEAS.md); until then the live page shows the words, or "for you, now".

## Who decides the kind

**Decided** (owner, 2026-11-12): a beam is live because the system's beam path says a beam is open, not because Photon asks.
**Built** (kernel `decide`): the kind a note gets is the kind its kist asked for, lowered to what the system can see, never raised. A kist can always lower its own note.

| the system sees | the ceiling |
|---|---|
| the system's own note (an alarm the person set, the development command) | live |
| the system's question (source `ferros_kist::note::SYSTEM`, "System questions" below) | personal |
| a kist's note naming a `person`, from a kist holding the network (`udp`: a person's words reach a kist only over a link) | personal |
| any other kist's note | status |

No kist can ring live today: a beam is live once the system's beam path (BEAM.md) tells the kernel a beam is open for that kist, and an alarm once the person sets it thru the system's alarm (a gesture-ticket call, as the clipboard's rule: no gesture, no capability).
Neither is built, so a kist's live note is lowered to personal (it names a person) or status.

**Proposed:** check `person` against the person's contacts once ferros holds them (people_not_devices_split: a contact is a person with a fleet of device keys), so a kist cannot pass a stranger, or no one, for a person.

## Where notes live

**Decided here** (the brief asked for a decision and its reason): the person's, in the identity class.
**Built:** the kernel keeps them in RAM while a person is present and writes each change to its vault at once (AGENT.md: every change hits disk), one entry a note under the reserved `\0notes.` (kvault `IDENTITY`), an address from the source and the number.

Why the person's and not the device's:
- A note is about the person: who wrote to them and what about. It is never readable before attest, like the rest of what is theirs.
- It follows the person: the same note is cleared on every device of theirs (below), and a reset device restores it with their space.
- A release (a completed departure) wipes it with the rest of the identity, with no key needed (kvault `ID_ADDRS`).

Why the kernel keeps them, not the deck:
- It alone knows which kist is calling (its capsule, as launched), so a note's source is never a kist's word.
- It decides the kind and holds each kist to its counts.
- The deck reads one list on one bell, never asked on a timer.

## Counts, never time

**Built.** A kist may have at most 16 notes waiting (a note's number is a nibble), and may post at most 64 times between two looks from the person (a revision counts).
Past either, a post is refused (`Refused::Full`) until the person has looked at that kist, which gives its budget back.
A personal note taps the motor once a kist: the next tap waits until the person has looked at that kist.
No count is reset on a clock.

## The resting view

**Decided** (owner, 2026-11-12): the resting view is where the person last settled the deck, and what sits behind an open kist; a pan that does not settle does not change the reach.
**Built** (map `rest`): the camera as it stood when the deck last came to rest after a touch, a fling or a flight (where the map already saves the view), on the deck itself only (an open settings page or a question is not the deck).
With a kist in front the map's camera does not move, so the resting view is the deck behind it.

**In view** (`in_view`): a circle's disc meets the screen's rectangle as the resting view showed it.

## The glow

**Built** (map `notes_rang`): a kist circle with notes waiting glows for you, its count the sum of its notes' counts, thru the plumbing the deck already had (`waiting`, display op 57).
Every kind glows; a live note glows too while it waits unanswered.
The glow is shown in or out of view: it is the circle's state, seen when the person looks there.

## The Deck button

**Decided** (owner, 2026-11-12): "what if we change how we get notified. if anything on the deck has changed state, specifically 400nm notifications, we change the deck circle in the helm and then the user knows to go to the deck and see what's changed. when tapping on Photon it clears it and resumes regular 500nm, or something of the sorts?"
Then: "So ideally the deck circle shows the latest item icon preview and shows the violet ring?"

So the helm's Deck button (bottom right) is the deck's one summary signal, in place of lighting the Deck corner's wedge:
- While anything in the resting view waits (personal or status; live takes the screen), the button shows the icon of the latest note's source inside it, the source's own circle icon from the display's bundle, with the 400 nm violet ring around it: its stroke violet, and the for-you glow outside it reaching farther with the total waiting in view.
- No content, ever: the source's icon alone.
- When the latest is cleared, it shows the next latest; with none left, the plain Deck mark.
- A note from a person will later show the person's own mark (Photon's default avatars are tapuwae marks): op 66's icon byte is the bundle's today, and a person's image is a second source for it to grow.

**Built:** the map sends op 66 `helm note [icon u8][count u16][glow u8]` whenever what is in view changes (a note, a settle), the system's alone (a kist cannot light it: `NOT_YOURS`); its glow is ferros_glow's for you (4), or a question's (6, "System questions" below). The display rings its render bell as it takes it, so the helm shows it at once on an idle screen (until 2026-11-12 it waited for the next pass, the minute's at worst).
The display's helm draws it front to back with `under`: the violet stroke, the icon inside it (each pixel the average of the texels under it), the glow outside on a C∞ step (`draw::bump_step`), then the wedge behind; every edge smooth.
A source whose circle has no bundled icon (a puck the display draws, the system) shows the ring alone.

## System questions

The problem (2026-11-12): a `ferros send` stow left the Stow sheet waiting on the Pro while the owner was in Mahere.
Nothing showed it was waiting (the Deck page opens on an offer only once the person is back on the map), and meanwhile `ferros send` held the device's USB interface, shutting out every other host tool.

**Decided** (owner, 2026-11-12, to this design: "Yes. Good. So only difference is it would be 700nm red, rather than the regular 400nm notification glow"):
- **A system question** is anything the system is waiting on the person to answer: Stow or Cancel on a kist offered, a permission ask, the unstow sheet, a pairing, and any later one.
  It is a note from the system (source Settings), kind **personal**: it never takes the screen and never rings (an alarm the person set stays live, as it is).
- **It always lights the Deck button**, whatever the view rule (in view or out of view): the settings icon (the ring alone while the bundle has no raster of it) inside a **red ring at the system's 703 nm** (ferros_glow), in place of the regular 400 nm violet.
- **One tap on Deck goes straight to the waiting question**, not just to the deck.
- **It stays lit until answered**: any answer, Cancel included, clears it. No timeout.
- **No popups or banners** beyond that.

**Mine, the owner can change it:** with a question and kists' notes both waiting, the question wins the Deck button: it blocks something until answered (a host waiting on Stow, a device waiting on its departure), where a kist's note waits as well later as now.
Its notes show again as soon as the last question is answered.

**Built** (2026-11-12):
- **The questions** (map notes.rs `Question`), each read from what the map holds on every pass of its main loop (`questions_now`):

  | question | waits while | answered on | answered by |
  |---|---|---|---|
  | a kist offered over USB (KISTS.md "Stowing") | the stower's offer is asking (`STAGE_ASKING`) | the Deck page | Stow or Cancel (or the offer refused, or gone) |
  | the unstow sheet (KISTS.md "The unstow sheet") | the sheet is open on a kist's card, the circle shut or not | the Deck page, on the card | Unstow, Cancel or Back |
  | a first start's permissions (KISTS.md, the first-launch question) | the kist's circle is open on them | the circle | Start, or a tap outside it |
  | a device asking to leave the fleet | fgtw's departure asking here (status 0) | the Fleet page | Approve or Deny |
  | a device asking to join | the add flow's phase 3 (selected, Yes waiting) | the Fleet page | Yes or No |

  A kist's own ask at first need (KISTS.md: "a kist can put a question only while holding the one-shot ticket") is not built; it joins the list when it is.
- **The note:** a new question is posted (SYS_NOTIFY op 0, Settings, source `ferros_kist::note::SYSTEM`, all zeros: no capsule hashes to it), kind personal, its `key` naming it (the question's kind, then what it is about), its words a line in ASCII ("a kist offered over USB: Stow or Cancel"), never what it names.
  An answered or ended one is cleared (op 5, seen: the person answered it); a set unchanged calls nothing (`questions_sync`).
  The map clears every system question as it starts and posts again whatever still waits, so a question never outlives the map that asked it.
- **The kernel** (notify.rs `poster`, ferros_kist::note `decide`, `Poster::Question`): a note from the system's source is personal at most, whatever is asked, so a question can never take the screen.
  It is never written to the vault: it lasts as long as what asks it (an offer, an open sheet, a device asking), none of which outlives a restart; one a kernel from before kept is removed as the person's notes load.
  The map asks for personal itself, so a kernel from before shows the same.
- **One tap of the motor** as a question arrives unasked (an offer from a host, a device asking to leave), by the sound mode and not under do not disturb, as a personal note; the ones the person opened themselves (the sheet, a first start, the add flow) are silent: they are looking at them.
- **The Deck button** (map `notes_view`, display op 66 with its new glow byte): with any of the system's notes in the kernel's list, the settings circle's icon (the ferros mark, which the display draws from its geometry and the bundle has no raster of, so the ring alone), the questions' count, and glow 6, ferros_glow `Question`: for you's glow (the heavy peak at `FOR_YOU_LIGHT`, reaching as far by the count) at 703 nm, the red primary itself, so no multiply. The display's helm draws its ring, its glow and its icon in that colour, front to back with `under` as before.
- **Deck's tap** (map `helm_tap`, `question_open`): with a question waiting that is not already on the glass, the newest one opens: the app in front sent behind, then its page (the unstow sheet's card opened again), or the first start's circle, still open behind the app.
  With the question already showing, Deck does what it always does (closes the circle; the question still waits, the button still red).
- **`ferros send` lets the link go** (KISTS.md "Stowing", USB.md "Offering a kist"): it ends at "waiting on the person" (exit 3) and `ferros sent <capsule>` asks how it ended.

## Personal: one tap

**Built:** a personal note new since the deck last looked, whose source is in view, taps the motor once (ROM waveform 1 on the NFC driver's op 8, docs/NFC.md), in sound and vibrate modes and never in silent (preference 39), and not under do not disturb (preference 40), once a kist until the person has looked at it.
Out of view: silent, the glow alone.
The sound mode's sound needs the map to hold `audio` (an `ALERT` stream, which the audio service already plays only in the sound mode): not built; sound mode taps like vibrate today.

## The live takeover

**Decided:** live rings and takes the screen until answered, declined, or ended by the caller; the takeover is the system's.
**Built:** a live note sends the kist in front behind (op 60), opens its source's circle on the deck (the map is the system's window management, so nothing a kist shows can sit above it: a kist cannot show above the app level, WINDOWS.md "Whose level") and rings the motor once.
The page shows the kist's name, the note's words (or "for you, now"), Answer and Decline.
- **Answer**: the kist launched or raised, opened at the note (below), its notes cleared.
- **Decline**: the note goes; its kist tells the caller (the kist sees its note gone).
- **The caller ends**: the kist withdraws its note, and the page closes.
- A tap outside the circle does not close it; Deck does, and the note stays waiting, glowing.

**Proposed:** the ring held until answered without a timer: the audio service's `ALERT` tone looping for as long as the note is live, the hardware's period its only clock, and stopped when the note goes.
**Proposed:** a beam obeys the view like any note (in view rings, out of view glows), while an alarm the person set always rings; today the only live notes are the system's, which always ring.

## A tap

**Decided** (owner, 2026-11-12): opening a kist, by tapping its circle or it coming to the front any way, clears that kist's for-you notes, and its glow goes back to its state (500 nm running, 600 nm stopped, the dark ring not running).
**Built:**
- A tap on a glowing circle opens the kist at its latest note: the map tells the kernel which note was opened (op 5 with `opened`), which keeps that note's document for its kist, clears the rest, and the kist's window gets `EV_OPEN_AT` with the note's number (display op 67) once it is in front.
- The kist reads where to land with `ferros_sys::notify::opened(number, buf)`: the note as the kernel kept it, its `target` among it.
- A kist brought to the front any other way (a flip, Answer, a launch after its question) has its notes cleared the same way, with nothing opened.
- Unstowing a kist clears its notes.

## Seen on every device

**Decided:** a note clears when it is seen on any of the person's devices, never by a timer; a doorbell, "who rang last" (the clipboard's model: owner, 2026-11-10, "a doorbell is a doorbell. I don't have to get the package but I would like to know who rang it last"); devices first, never fgtw.org first (owner, 2026-11-10: "Should be BLE, then LAN, then WAN, then hopefully we shouldn't need to but fgtw.org").
**Proposed, not built:**
- Each device's kist posts its own note for the same thing (Photon on every device hears the same message), with the same `key`.
- Seen on one device, its deck tells its FGTW node: the kist by its gate identity (developer and option, which name it on every device whatever its capsule's version) and the key, or "every note of it posted before" an Eagle time when the note had none.
- The FGTW node rings the person's other devices: a doorbell carrying who rang last, the latest seen a kist, nothing of the note itself.
- Each device's FGTW node hands it to its deck, which clears the matching notes (op 5 by key: to build in the kernel).
- A device away when the bell rang learns the latest seen when it is next heard.

## Deadlines

**Built:** a kist may set one `deadline` on a note.
The deck's rest ends at the earliest deadline among the notes (as an alarm's sleep-until would), and its list read then has the kernel drop what is past it, which rings the bell.

## The attest screen

**Decided:** a count only.
**Proposed, not built:** the kernel keeps one number in the device class, how many notes wait, written with every change (EWE, no source, no words, no kinds), and the attest screen shows it under the mark; the notes themselves stay sealed in the identity class until attest.

## The calls

**Built.** Kernel SYS_NOTIFY (43; ferros_sys::notify):

| op | who | what |
|---|---|---|
| 0 post | a kist (its own); the system on a kist's behalf (Settings, not a kist) | the note → its number and the kind decided; `Full`, `NotANote` |
| 1 withdraw | a kist (its own); the system | a number, or every one |
| 2 opened | a kist | the note the person opened, by its number |
| 3 list | the deck (Settings, not a kist) | 56-byte records: source, number, kind, asked, flags, count, its bell count, deadline |
| 4 doorbell | the deck | the notes' bell (`BELL_NOTES`) |
| 5 seen | the deck | a source's note or all of them, `opened` keeping one for its kist |
| 6 read | the deck | one note's document |

Display: op 67 `open at [note u8]` to the app window in front (`EV_OPEN_AT` = 5, the low byte the number; op 65 and event 4 are the bake's, COMPOSITOR.md "Baked"); op 66 `helm note [icon u8][count u16][glow u8]`, the system's alone (no glow byte, from a map before it: for you).

## Seeing it on a device

**Built:** the kernel's development command posts a note from the system on a kist's behalf, as the system decides it, so each kind can be seen with no kist that posts:

```
ferros-bridge diag notify                                   # the notes waiting, the kists alive
ferros-bridge diag "notify status <kist> [words]"           # glow only
ferros-bridge diag "notify personal <kist> [words]"         # glow, one tap in view
ferros-bridge diag "notify live <kist> [words]"             # takes the screen
ferros-bridge diag "notify clear <kist>"                     # as if the person had looked
```

`<kist>` is a running kist's process name, or its capsule's address in hex (whole, or its first bytes as `diag notify` lists them).

## Open questions

0. **A stranger's beam** (owner's open question): does a beam from someone who is not the person's contact ring? Proposed: at most personal (a glow and a tap in view), never the takeover.
1. **Silent self-updates** (owner's open question): do updates stow themselves silently? Proposed: a kist's update is status (a glow on its circle until opened), or no note at all when the person chose automatic updates.
2. **Do not disturb**: the view replaces it as the owner's way to be left alone, but the quick row still has it (preference 40, the identity's). Kept for now: under it, nothing taps or sounds.
3. **The ring's sound and the sound mode's sound**: the map holding `audio`, or a system sound service.
4. **Words in any script**: the kernel's VSF reading `x` (x-lang16, 140 KB), or the words left out of the kernel's parse.
5. **Fleet clearing** as above: its message and the kernel's seen by key.
6. **A person's mark** on the Deck button for a note from a person.
7. **The ferros mark in the red Deck button**: a question shows the ring alone today (the bundle has no raster of the settings icon; the display draws the mark from its geometry into the scene, in panel codes, where the helm's bars are darkness space). Proposed: `draw::logo` taught the bars' format, so the mark sits inside the red ring.
8. **The settings circle while a question waits**: it keeps its own thin 703 nm glow today. Proposed: the question's heavy red glow on it too, as a kist's circle glows for you with its notes, so the deck shows where the question lives.
9. **Which question first**: Deck opens the newest waiting today. Proposed as is; the oldest would answer a blocked host first.
