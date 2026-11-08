//! inject: plays an input script (ferros_inject, docs/INPUT-INJECT.md) into the touch process, frame by shown frame, so its fingers reach every consumer by the path real ones take (payloads/touch → the map, the attest screen, the kists). Development builds only: the bridge spawns it, and a production touch process has no injection op.
//! Spawn: `ferros-bridge input …` (or `ferros-bridge spawn inject.bin <document hex> --name inject --use touch --use display`). Grant slots: 0 console, 1 touch (use), 2 display (use; its `frame wait` is the clock).
//! The input is the script, a complete VSF document. It is read again through `ferros_sys::input` into the heap: the inline copy the kernel lays after the code shares its bytes with this image's zeroed data.
//! Each event waits its `after` frames, counted on the display's `frame wait` (op 24): a return with new frame starts advances by every frame that started (a frame this process missed still counts, so motion keeps pace with the screen), a return with none advances by one refresh period. Everything due in the same frame goes to the touch process in one call, so consumers see a pinch's fingers move together.
//! Key events are refused here: keys are the kernel's, and the bridge sends them (`diag key`) between this process's parts of a script.
//! What it says on the console: one line at the end, `done …` with the counts, or `refused …` with why.
#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::vec::Vec;
use ferros_inject::{touch as op, At, Ev, Script, FINGERS, IDLE_FRAMES};
use ferros_sys::{call, cap, console, exit, ERR};

const S_TOUCH: u64 = 1;
const S_DISPLAY: u64 = 2;

/// The heap the script is parsed in. The linker script lays the zeroed data into the file (payload.ld), so the heap and the allocator lie inside the arc the kernel sizes for the file, never under the spawn input, and syscalls may land in it.
const HEAP_SIZE: usize = 1 << 17;
#[repr(C, align(64))]
struct Heap([u8; HEAP_SIZE]);
static mut HEAP: Heap = Heap([0; HEAP_SIZE]);
#[global_allocator]
static ALLOC: ferros_heap::LockedHeap = ferros_heap::LockedHeap::empty();

/// An injected finger's pressure on the controller's own 0-255 scale, and its axes in the controller's units (sixteenths of a pixel, `AREA_SCALE`): a light touch of a fingertip. Consumers filter on the kind (a finger), not on these.
const PRESSURE: u8 = 1 << 6;
const MAJOR: u16 = 1 << 8;
const MINOR: u16 = 1 << 7;
/// A wait for idle gives up after this many frames with the display still drawing (something animating without end), and says so.
const IDLE_GIVE_UP: u32 = 1 << 10;

fn say(grants: u64, s: &str) {
    console(grants, s.as_bytes());
}

fn refuse(grants: u64, why: &str) -> ! {
    say(grants, &format!("refused: {why}\n"));
    exit(1);
}

/// The display's frames as this process counts them.
struct Clock {
    display: u64,
    last: u32,
    /// Frame starts seen, refresh periods counted, the most frames one wait spanned.
    starts: u64,
    frames: u64,
    widest: u32,
}

impl Clock {
    /// One wait on the display: the frames it advanced, and whether a frame started.
    fn tick(&mut self) -> (u32, bool) {
        let mut fr = [0u8; 4];
        call(self.display, &[24], &mut fr);
        let c = u32::from_le_bytes(fr);
        let d = c.wrapping_sub(self.last);
        self.last = c;
        // No frame started: op 24 came back after one refresh period with nothing in flight, which is one frame of the panel's clock.
        let step = if d == 0 { 1 } else { d };
        self.starts += d as u64;
        self.frames += step as u64;
        if step > self.widest {
            self.widest = step;
        }
        (step, d != 0)
    }
}

/// The fingers this process holds, where each last was, and the batch for the touch process.
struct Hand {
    touch: u64,
    down: u8,
    at: [(u16, u16); FINGERS as usize],
    batch: Vec<u8>,
    records: u64,
}

impl Hand {
    fn add(&mut self, finger: u8, phase: u8, (x, y): (u16, u16)) {
        self.batch.push(finger);
        self.batch.extend_from_slice(&[phase, op::FINGER_KIND, PRESSURE]);
        for v in [x, y, MAJOR, MINOR] {
            self.batch.extend_from_slice(&v.to_le_bytes());
        }
    }
    /// The batch to the touch process, in calls of at most one record per finger (its message buffer). False when it refused one.
    fn flush(&mut self) -> bool {
        let batch = core::mem::take(&mut self.batch);
        for part in batch.chunks(op::REC * FINGERS as usize) {
            let mut msg = Vec::with_capacity(2 + part.len());
            msg.extend_from_slice(&[op::OP_INJECT, (part.len() / op::REC) as u8]);
            msg.extend_from_slice(part);
            let mut r = [0u8; 4];
            let n = call(self.touch, &msg, &mut r);
            if n != 3 || r[0] != op::OK {
                return false;
            }
            self.records += (part.len() / op::REC) as u64;
        }
        true
    }
    /// After a refusal: every finger still held lifted where it was, one call each so one refused leave does not keep the others down.
    fn lift_all(&mut self) {
        for f in 0..FINGERS as u8 {
            if self.down >> f & 1 == 1 {
                self.add(f, 2, self.at[f as usize]);
                let _ = self.flush();
            }
        }
        self.down = 0;
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    unsafe { ALLOC.init((&raw mut HEAP) as usize, HEAP_SIZE) };
    let mut doc = alloc::vec![0u8; input_len as usize];
    let mut got = 0;
    while got < doc.len() {
        let n = ferros_sys::input(got as u64, &mut doc[got..]);
        if n == 0 {
            break;
        }
        got += n;
    }
    if got != doc.len() {
        refuse(grants, "the input could not be read whole");
    }
    let script = match Script::parse(&doc) {
        Ok(s) => s,
        Err(e) => refuse(grants, &format!("the script: {e}")),
    };
    if script.events.iter().any(|e| matches!(e.ev, Ev::Key { .. })) {
        refuse(grants, "a key event: keys are the kernel's, and the bridge sends them (diag key) between the touch parts");
    }
    let (touch, display) = (cap(grants, S_TOUCH), cap(grants, S_DISPLAY));
    // The panel as the touch process measures it: its largest coordinates.
    let mut info = [0u8; 8];
    if call(touch, &[2], &mut info) < 5 {
        refuse(grants, "the touch process did not answer INFO");
    }
    let (x_max, y_max) = (u16::from_le_bytes([info[0], info[1]]), u16::from_le_bytes([info[2], info[3]]));
    // The frame count now, from the display's info (op 5, bytes 4-7), so the first wait counts only frames from here.
    let mut di = [0u8; 16];
    let n = call(display, &[5], &mut di);
    if n == ERR || n < 8 {
        refuse(grants, "the display did not answer INFO");
    }
    let mut clock = Clock { display, last: u32::from_le_bytes([di[4], di[5], di[6], di[7]]), starts: 0, frames: 0, widest: 0 };
    let mut hand = Hand { touch, down: 0, at: [(0, 0); FINGERS as usize], batch: Vec::new(), records: 0 };
    let mut owed: u64 = 0;
    let mut idles = 0u32;
    let mut i = 0;
    loop {
        // Everything due now, in order; the batch goes out once the frame's events are all in it.
        while i < script.events.len() && script.events[i].after as u64 <= owed {
            let e = script.events[i];
            owed -= e.after as u64;
            i += 1;
            let place = |a: At| a.on_panel(x_max, y_max);
            match e.ev {
                Ev::Down { finger, at } | Ev::Move { finger, at } => {
                    let Some(p) = place(at) else {
                        hand.lift_all();
                        refuse(grants, &format!("event {}: pixels off this panel ({x_max} × {y_max} at most)", i - 1));
                    };
                    let phase = if matches!(e.ev, Ev::Down { .. }) { 0 } else { 1 };
                    hand.add(finger, phase, p);
                    hand.at[finger as usize] = p;
                    hand.down |= 1 << finger;
                }
                Ev::Up { finger } => {
                    hand.add(finger, 2, hand.at[finger as usize]);
                    hand.down &= !(1 << finger);
                }
                Ev::Wait { idle: true } => {
                    if !hand.flush() {
                        hand.lift_all();
                        refuse(grants, "the touch process refused a record (a real finger took an injected slot, or not a development touch)");
                    }
                    // Until IDLE_FRAMES waits in a row see no frame start; frames counted from here on start again at the wait's end.
                    let mut quiet = 0;
                    let mut spent = 0;
                    while quiet < IDLE_FRAMES {
                        let (step, started) = clock.tick();
                        quiet = if started { 0 } else { quiet + 1 };
                        spent += step;
                        if spent > IDLE_GIVE_UP {
                            say(grants, &format!("still drawing after {spent} frames: carrying on without idle\n"));
                            break;
                        }
                    }
                    idles += 1;
                    owed = 0;
                }
                Ev::Wait { idle: false } | Ev::Key { .. } => {}
            }
        }
        if !hand.flush() {
            hand.lift_all();
            refuse(grants, "the touch process refused a record (a real finger took an injected slot, or not a development touch)");
        }
        if i == script.events.len() {
            break;
        }
        owed += clock.tick().0 as u64;
    }
    let mut st = [0u8; 8];
    let mask = if call(touch, &[op::OP_INJECTED], &mut st) == 6 { u16::from_le_bytes([st[0], st[1]]) } else { 0 };
    say(
        grants,
        &format!(
            "done: {} events, {} records, {} frames ({} frame starts shown, the widest wait {} frames, {} idle waits), injected contacts left G#{mask:X}\n",
            script.events.len(),
            hand.records,
            clock.frames,
            clock.starts,
            clock.widest,
            idles
        ),
    );
    exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
