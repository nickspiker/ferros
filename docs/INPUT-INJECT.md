# INPUT-INJECT — fingers and keys from the bridge

`ferros-bridge input …` drives the device the way `adb shell input` drives Android: taps, swipes, drags, pinches, turns and the power key, from the host, in development builds. Owner, 2026-11-08: "build some tools for the bridge so you can inject taps and buttons … do it with VSF proper".

Three pieces:
- **ferros_inject** (the crate): a script, its schema, and the checks, shared by both ends.
- **payloads/inject**: the process that plays a script into the touch process, frame by shown frame.
- **The touch process's injection op** (payloads/touch op 3): injected contacts enter the same table real contacts do, so every consumer (the map, the attest screen, a kist through fluor's ferros host) takes them by the same path.

Keys go through the kernel: `diag key power`.

## Commands

A position is two fractions of the panel's width and height (`0.5 0.25`), or two panel pixels (`672px 748px`). Fractions make one script drive the Pro and the 8a alike; pixels name one panel's exact spot. Every count is in shown frames.

| Command | What it sends |
|---|---|
| `input tap X Y [HOLD]` | a finger down, held HOLD frames (8), up where it landed |
| `input hold X Y FRAMES` | a finger held still |
| `input swipe X0 Y0 X1 Y1 [FRAMES]` | down, still 8 frames, a straight line over FRAMES (16), up at once: the consumer's fling follows |
| `input drag X0 Y0 X1 Y1 [FRAMES] [HOLD]` | down, still HOLD frames (32), the line, still HOLD frames, up: no fling |
| `input pinch CX CY R0 R1 [FRAMES]` | two fingers either side of the centre, R0 to R1 from it (fractions of the width) |
| `input rotate CX CY R DEGREES [FRAMES]` | two fingers R from the centre, turned DEGREES (clockwise positive) |
| `input key power` | a press of the power key, through the kernel |
| `input idle` | wait until the display is idle |
| `input shot [out.vsf]` | the screen, as tools/screencap reads it (a VSF visual) |
| `input script FILE.vsf` | a script document sent as it is |

After a gesture:
- `--idle` waits for the display to go idle.
- `--shot [out.vsf]` waits for idle, then takes a screencap.
- `--save FILE` writes the document and sends nothing; `input script FILE` sends it later.
- `--dry` prints the script only.

Every command prints the script it sends, one line per event with the frame it lands on, then the inject process's report. For example, `done: 11 events, 10 records, 25 frames (3 frame starts shown, the widest wait 1 frames, 1 idle waits), injected contacts left G#0`.

Pinch and rotate ask the display for the panel's size, so the two fingers' path is round on the glass and not stretched by its aspect.

Why the defaults:
- **The tap's 8 frames:** the map reads touch a thirtieth of a second apart at rest (four frames at 120 Hz). A shorter press can come and go between two reads.
- **The 8 frames still before a swipe moves:** a finger already moving when it is first seen is first seen somewhere else. On 2026-11-08 the home corner's swipe was read inside the app's circle.

## The script

A script is a complete VSF document: a header with its provenance hash, then an `input` section and one `event` section per event. VSF has no sections inside sections. Its way of holding a list is repeated sections of one name, in the order the header's table lists them (as ferros_kist's `kist` sections are).

`input` (schema `ferros_inject::input_schema`):

| Field | Type | |
|---|---|---|
| `events` | unsigned, required | how many `event` sections follow; a document with any other number is refused |
| `text` | ASCII text | what the script does, in words (the command line that made it) |

`event` (schema `ferros_inject::event_schema`):

| Field | Type | |
|---|---|---|
| `kind` | unsigned, required | 0 down, 1 move, 2 up, 3 key, 4 wait |
| `after` | unsigned, required | shown frames after the event before (0: the same frame) |
| `finger` | unsigned | down, move, up: which finger, from 0, below 8 |
| `x`, `y` | unsigned | down, move: fractions of the width and height, Q16 (G#10000 is the whole side; every fraction is below it) |
| `px`, `py` | unsigned | down, move: panel pixels, in place of `x`/`y` |
| `key` | unsigned | key: 0 power, 1 volume up, 2 volume down |
| `idle` | flag (no value) | wait: until the display is idle, not just `after` frames |

Both ends check the script the same way:
- The provenance hash (`vsf::verification::read_verified`).
- Each section against its schema, strictly. An unknown field, or a value of the wrong type, refuses the whole document.
- The order of events:
  - no finger goes down while it is down;
  - no finger moves or lifts while it is up;
  - every finger is up at the end;
  - every key is a known one.

The bridge builds the document and reads it back through the parser before sending it.

## Time: shown frames

An event's time is counted in the panel's frames, never in milliseconds. It follows the owner's frame timer rule: motion follows shown frames, not composited ones (Claude memories/feedback_no_animations.md).

The inject process counts frames on the display's `frame wait` (op 24):
- **A return with new frame starts** advances the count by every frame that started. A frame the process missed still counts, so a swipe of 16 frames takes 16 of the panel's frames.
- **A return with none** (nothing in flight, the op's one refresh period) advances it by one.

A wait for idle ends after 8 refresh periods in a row with no frame started. At rest the display flips at 10 Hz, one frame in twelve periods. A display still drawing after 1024 frames is reported, and the script carries on.

## On the device

**payloads/inject** has three grant slots: 0 console, 1 `use touch`, 2 `use display`. It works like this:
- It reads the script from its spawn input. It reads it again through `ferros_sys::input`, because the inline copy lies where a plain payload's zeroed data would be. Its linker script lays that data into the file, so the arc is sized for it.
- It checks the script, and refuses one with a key event (keys are the kernel's).
- It asks touch for the panel's largest coordinates (INFO) and turns fractions into pixels.
- It plays the events, sending everything due in one frame in one call. A pinch's fingers move together.
- If touch refuses a record, it lifts every finger it still holds and stops.

**The touch process's ops** (payloads/touch), development builds only. A build with `FERROS_PRODUCTION` set has neither op; they answer as unknown ops do.

- **3 INJECT**: `[count][count records]`, each record 12 bytes as the endpoint serves them, with the id a finger number. The reply is `[status][injected mask u16 LE]`.
  - Finger f takes contact slot 15 − f, from the top of the 16, so it rarely meets a real finger (the controller numbers those from 0).
  - A batch is taken whole or refused whole. It is refused when a finger enters while its slot is held, or moves or leaves while it is not an injected contact.
  - A real finger landing on an injected slot takes it over. The injector's next record for that slot is refused, and the injector stops.
- **4 INJECTED**: `[injected mask u16 LE][records taken u32 LE]`. This is the one place an injected contact differs from a real one, for diagnostics.

**What consumers see:** an injected contact is a finger (kind 1) with a light fingertip's pressure and size. CHANGES and HELD serve it exactly as they serve a real one.

## Keys

`diag key power`, development kernels only (inside the DIAG development block of service.rs):
- It sets `__power_pressed`, which is where the power key's interrupt records a press (main.rs `.Lirq_power`). The service loop's screen toggle and the proximity wake rule then take it as they take a finger's press.
- It is refused while a volume key is physically held. The press would make a chord with it: power + volume up is the hv's jump under Android, and both volume keys are the kill. No chord is ever injected.
- The volume keys have nothing to inject into. The killswitch and the "Ready?" gate read their pins' levels, and nothing queues their presses. `diag key volup` answers so.

A key event in a script is a press. The bridge cuts the script at its keys:
- The touch parts go to the inject process.
- The key goes to the kernel between them.
- A key's `after` becomes a wait at the end of the part before it, so the key still lands that many frames after the event before it.

## Development only

Each layer refuses injection in a production build:
- **The kernel**: a production kernel refuses SPAWN and every DIAG subcommand over USB (BOOT.md).
- **The touch process**: a production build compiles op 3 and op 4 out.
- **The bridge**: it builds the inject payload only when asked, and has nothing on the device to call without the other two.

**Open** (owner): in a development build, any process holding `use touch` can call op 3, which includes a kist granted touch. Two lines would narrow it to the bridge's injector: a `touch-inject` endpoint that touch serves (init's grants for touch, service.rs) and the injector uses.
