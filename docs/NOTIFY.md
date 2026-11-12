# Notifications

Status: decided by the owner 2026-11-12; the first cut built the same day (branch `notify`): the kernel's notes (SYS_NOTIFY, process notify.rs), the note's VSF (ferros_kist::note), the deck's side (payloads/map notes.rs), the Deck button (display op 66) and the open-at event (display op 65, `EV_OPEN_AT`).
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

**Built:** the map sends op 66 `helm note [icon u8][count u16]` whenever what is in view changes (a note, a settle), the system's alone (a kist cannot light it: `NOT_YOURS`).
The display's helm draws it front to back with `under`: the violet stroke, the icon inside it (each pixel the average of the texels under it), the glow outside on a C∞ step (`draw::bump_step`), then the wedge behind; every edge smooth.
A source whose circle has no bundled icon (a puck the display draws, the system) shows the ring alone.

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
- A tap on a glowing circle opens the kist at its latest note: the map tells the kernel which note was opened (op 5 with `opened`), which keeps that note's document for its kist, clears the rest, and the kist's window gets `EV_OPEN_AT` with the note's number (display op 65) once it is in front.
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

Display: op 65 `open at [note u8]` to the app window in front (`EV_OPEN_AT` = 4, the low byte the number); op 66 `helm note [icon u8][count u16]`, the system's alone.

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
