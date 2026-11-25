//! The AoC's side of sound: its IPC services (the control queue `audio_output_control`, the playback ring `audio_playback5`), its mailboxes, and the audio commands, spoken from this process as Google's ALSA driver speaks them from Linux.
//!
//! Sources, every step marked with its file and line:
//! - the IPC layout and ring arithmetic: `private/google-modules/aoc_ipc/aoc_ipc_core_internal.h` and `aoc_ipc_core.c` (the husky kernel tree, /mnt/Harbor/husky-kernel), written `ipc_core.c:N` and `ipc_internal.h:N` below;
//! - the commands' layouts and ids: `aoc-interface-zuma.h` (/mnt/Harbor/ferros-ref/aoc), written `zuma.h:N`;
//! - what Linux sends and in what order: `alsa/aoc_alsa_hw.c`, `alsa/aoc_alsa_pcm.c`, `alsa/aoc_alsa.h` (same tree), written `hw.c:N`, `pcm.c:N`, `alsa.h:N`; the order as seen on husky in Android's boot log (/mnt/Harbor/tmp/linux_full_dmesg.txt:7426-7490: "bind: src:5 - sink:0", "audio_playback5 supports interrupt-driven", "be_prepare: dai TDM_0_RX");
//! - the mailboxes: `mailbox-wc.c` (ferros-ref/aoc) and `zuma-aoc.dtsi` (ferros-ref/soc-gs), written `wc.c:N`, `aoc.dtsi:N`.
//!
//! The IPC block is Device memory, reached thru the `aoc-ipc` grant's handle: every access an aligned word or a byte (payloads/sensors usf.rs does the same).

use crate::{say, S_CAPTURE, S_HEAPS, S_IPC, S_MBOX, S_PLAY_HI, S_PLAY_LO};
use ferros_sys::{cap, mmio_read, mmio_write, now, sleep_until, freq, Handle};

type Ipc = Handle<S_IPC>;

fn rd32(off: u64) -> u32 {
    Ipc::read32(off)
}
fn wr32(off: u64, v: u32) {
    Ipc::write32(off, v)
}
fn rd8(off: u64) -> u8 {
    Ipc::read8(off)
}
fn wr8(off: u64, v: u8) {
    Ipc::write8(off, v)
}

/// Where the IPC block lies (kernel cachemap.rs AOC_IPC), and the ring windows past it (AOC_PLAY_LO, AOC_PLAY_HI, AOC_CAPTURE): the AoC gives every offset from the IPC block's start, and a ring past the `aoc-ipc` window is reached thru the windows that hold it, each byte thru one that holds it (the playback ring straddles G#9600_0000, so it takes two: docs/AUDIO.md "The rings' windows").
const IPC_BASE: u64 = 0x95C0_0000;
const PLAY_LO_BASE: u64 = 0x95C0_0000;
const PLAY_HI_BASE: u64 = 0x9600_0000;
const CAPTURE_BASE: u64 = 0x96A0_0000;

/// A granted window a ring's bytes are reached thru.
#[derive(Clone, Copy, PartialEq)]
pub enum Win {
    Ipc,
    PlayLo,
    PlayHi,
    Capture,
}

const WINS: [Win; 4] = [Win::Ipc, Win::PlayLo, Win::PlayHi, Win::Capture];

impl Win {
    /// Its start as an offset from the IPC block's, and its length (0 for a grant this process was not given: a kernel from before has no ring window).
    fn span(self) -> (u64, u64) {
        match self {
            Win::Ipc => (0, Ipc::len()),
            Win::PlayLo => (PLAY_LO_BASE - IPC_BASE, Handle::<S_PLAY_LO>::len()),
            Win::PlayHi => (PLAY_HI_BASE - IPC_BASE, Handle::<S_PLAY_HI>::len()),
            Win::Capture => (CAPTURE_BASE - IPC_BASE, Handle::<S_CAPTURE>::len()),
        }
    }
    fn end(self) -> u64 {
        let (b, l) = self.span();
        b + l
    }
    /// Of the windows holding IPC offset `off`, the one reaching furthest past it (None: no window holds it).
    fn at(off: u64) -> Option<Win> {
        WINS.into_iter().filter(|w| {
            let (b, l) = w.span();
            l != 0 && off >= b && off < b + l
        }).max_by_key(|w| w.end())
    }
    /// A word or a byte at IPC offset `off`, which the ring's map has put inside this window. A word never crosses a window's edge: words sit at multiples of four, edges at multiples of a MiB.
    fn rd32(self, off: u64) -> u32 {
        let o = off - self.span().0;
        match self {
            Win::Ipc => Ipc::read32(o),
            Win::PlayLo => Handle::<S_PLAY_LO>::read32(o),
            Win::PlayHi => Handle::<S_PLAY_HI>::read32(o),
            Win::Capture => Handle::<S_CAPTURE>::read32(o),
        }
    }
    fn rd8(self, off: u64) -> u8 {
        let o = off - self.span().0;
        match self {
            Win::Ipc => Ipc::read8(o),
            Win::PlayLo => Handle::<S_PLAY_LO>::read8(o),
            Win::PlayHi => Handle::<S_PLAY_HI>::read8(o),
            Win::Capture => Handle::<S_CAPTURE>::read8(o),
        }
    }
    fn wr32(self, off: u64, v: u32) {
        let o = off - self.span().0;
        match self {
            Win::Ipc => Ipc::write32(o, v),
            Win::PlayLo => Handle::<S_PLAY_LO>::write32(o, v),
            Win::PlayHi => Handle::<S_PLAY_HI>::write32(o, v),
            Win::Capture => Handle::<S_CAPTURE>::write32(o, v),
        }
    }
    fn name(self) -> &'static str {
        match self {
            Win::Ipc => "aoc-ipc",
            Win::PlayLo => "aoc-play-lo",
            Win::PlayHi => "aoc-play-hi",
            Win::Capture => "aoc-capture",
        }
    }
}

/// A ring's bytes as the windows hold them: one window from its start, and a second from `split` on when the first ends inside it.
#[derive(Clone, Copy)]
pub struct RingMap {
    first: Win,
    split: u64,
    second: Win,
}

impl RingMap {
    /// The windows that hold `len` bytes from IPC offset `off` between them, at most two; None when no two do (a byte no window holds).
    fn of(off: u64, len: u64) -> Option<RingMap> {
        if len == 0 {
            return None;
        }
        let first = Win::at(off)?;
        let split = first.end();
        if split >= off + len {
            return Some(RingMap { first, split, second: first });
        }
        let second = Win::at(split)?;
        (second.end() >= off + len).then_some(RingMap { first, split, second })
    }
    /// The window for IPC offset `off`, inside the ring.
    fn win(&self, off: u64) -> Win {
        if off < self.split { self.first } else { self.second }
    }
    fn rd32(&self, off: u64) -> u32 {
        self.win(off).rd32(off)
    }
    fn rd8(&self, off: u64) -> u8 {
        self.win(off).rd8(off)
    }
    fn wr32(&self, off: u64, v: u32) {
        self.win(off).wr32(off, v)
    }
    /// Its windows' names, for the console.
    fn names(&self) -> (&'static str, &'static str) {
        (self.first.name(), if self.second == self.first { "" } else { self.second.name() })
    }
}

/// The control block (ipc_internal.h:61-74): the magic at 0 once the firmware is up, the service count at 24, each header's size at 28, the table's offset at 32.
const AOC_MAGIC: u32 = 0xA0C0_0A0C;
const CB_SERVICES: u64 = 24;
const CB_SERVICE_SIZE: u64 = 28;
const CB_SERVICES_OFFSET: u64 = 32;
/// A service header (ipc_internal.h:93-98): its name (32 bytes), its flags, then two regions of seven words each (ipc_internal.h:76-84: offset, size, slots, tx, rx, wp, rp).
const NAME_LEN: u64 = 32;
const H_FLAGS: u64 = 32;
const H_REGIONS: u64 = 36;
const REGION: u64 = 28;
const R_OFFSET: u64 = 0;
const R_SIZE: u64 = 4;
const R_SLOTS: u64 = 8;
const R_TX: u64 = 12;
const R_RX: u64 = 16;
const R_WP: u64 = 20;
const R_RP: u64 = 24;
/// The two directions (aoc_ipc_core.h:42-43): UP is the AoC's to the AP, DOWN the AP's to the AoC; a header's regions are indexed by them.
const UP: u64 = 0;
const DOWN: u64 = 1;
/// Flags (ipc_internal.h:47-53): the type in bits 16-18 (0 queue, 1 ring), the mailbox interrupt in bits 19-23, a ring the AP must signal on every write in bit 24.
const TYPE_MASK: u32 = 0x0007_0000;
const TYPE_RING: u32 = 1 << 16;
const IRQ_SHIFT: u32 = 19;
const IRQ_MASK: u32 = 0x1F;
const RING_PUSH: u32 = 1 << 24;
/// A queue message's header (ipc_internal.h:100-105): its length in a u16, then three u16 of zero.
const MSG_HDR: u64 = 8;

/// The three mailbox blocks as offsets in the `aoc-mbox` grant (G#1520_0000 a32, G#1522_0000 f1, G#1524_0000 p6: aoc.dtsi:9-46), channel k in block k / 16 (aoc.dtsi:79-130, 48 channels).
const MBOX_BLOCKS: [u64; 3] = [0, 0x2_0000, 0x4_0000];
/// The AP's side of a block (wc.c:24-32): INTGR0 rings the AoC (a bit a channel, written as a set: wc.c:91-97), INTCR1 clears what the AoC rang the AP with (wc.c:100-107), INTMR1 masks it (a set bit masked: wc.c:109-148), INTSR1 reads it (wc.c:162).
const MB_INTGR0: u64 = 0x20;
const MB_INTCR1: u64 = 0x44;
const MB_INTMR1: u64 = 0x48;
const MB_INTSR1: u64 = 0x4C;
/// The mailbox block whose AoC-to-AP line this process holds (`aoc-mbox-f1`, kernel process.rs IRQS, SPI 78): channels 16-31.
pub const F1: u64 = 1;

/// Whether the AoC's firmware is up: its control block holds the magic.
pub fn online() -> bool {
    rd32(0) == AOC_MAGIC
}

/// One service: its header's offset in the IPC block.
#[derive(Clone, Copy)]
pub struct Svc(u64);

impl Svc {
    fn flags(&self) -> u32 {
        rd32(self.0 + H_FLAGS)
    }
    /// Its mailbox channel (ipc_core.c:242-246).
    pub fn irq(&self) -> u32 {
        (self.flags() >> IRQ_SHIFT) & IRQ_MASK
    }
    pub fn ring(&self) -> bool {
        self.flags() & TYPE_MASK == TYPE_RING
    }
    fn r(&self, dir: u64, field: u64) -> u64 {
        self.0 + H_REGIONS + dir * REGION + field
    }
    fn get(&self, dir: u64, field: u64) -> u32 {
        rd32(self.r(dir, field))
    }
    fn set(&self, dir: u64, field: u64, v: u32) {
        wr32(self.r(dir, field), v)
    }
    /// Where a direction's memory lies and how long it is: a ring's bytes, or a queue's slots times their size.
    fn span(&self, dir: u64) -> (u64, u64) {
        let size = self.get(dir, R_SIZE) as u64;
        let len = if self.ring() { size } else { size * self.get(dir, R_SLOTS) as u64 };
        (self.get(dir, R_OFFSET) as u64, len)
    }
    /// WHY this check: the offsets and sizes are the AoC's, read from its memory, and the grant's handle wraps an offset past its end back inside the window (ferros_sys `Handle`) instead of faulting: a region that ran past the `aoc-ipc` window (2 MiB, kernel cachemap.rs `AOC_IPC`) would have this process write over another service's rings, the sensors' among them. Every region this process touches is checked once, when found; a refusal names the service, and the window is what grows.
    fn fits(&self, dir: u64) -> bool {
        let (off, len) = self.span(dir);
        len != 0 && off + len <= Ipc::len()
    }
    /// The granted windows that hold a ring's direction whole between them (the same WHY as `fits`: an offset past a handle's region would wrap inside it, onto another service's memory).
    fn ring_win(&self, dir: u64) -> Option<RingMap> {
        let (off, len) = self.span(dir);
        RingMap::of(off, len)
    }
    /// The AoC rung on this service's channel (ipc_core.c via aoc_service_core.c:194-195 and :560-563).
    fn signal(&self) {
        signal(self.irq())
    }
}

/// Ring the AoC on mailbox channel `ch`.
fn signal(ch: u32) {
    mmio_write(cap(crate::grants(), S_MBOX), MBOX_BLOCKS[(ch >> 4) as usize] + MB_INTGR0, 1 << (ch & 15));
}

/// A service by name: the control block's table walked once (a few thousand Device reads).
pub fn find(name: &[u8]) -> Option<Svc> {
    if !online() {
        return None;
    }
    let count = rd32(CB_SERVICES) as u64;
    let size = rd32(CB_SERVICE_SIZE) as u64;
    let base = rd32(CB_SERVICES_OFFSET) as u64;
    // WHY: count, size and base are the AoC's; the walk stays inside the window for the same reason as `Svc::fits`.
    if size < H_REGIONS + 2 * REGION || base >= Ipc::len() {
        return None;
    }
    let room = (Ipc::len() - base) / size;
    for i in 0..count.min(room) {
        let h = base + i * size;
        let mut k = 0u64;
        let matched = loop {
            let c = rd8(h + k);
            let want = name.get(k as usize).copied().unwrap_or(0);
            if c != want {
                break false;
            }
            if c == 0 {
                break true;
            }
            k += 1;
            if k == NAME_LEN {
                break false;
            }
        };
        if matched {
            return Some(Svc(h));
        }
    }
    None
}

/// The control queue and the playback ring, found and checked, and the window the ring's bytes are reached thru.
#[derive(Clone, Copy)]
pub struct Services {
    pub control: Svc,
    pub playback: Svc,
    play: RingMap,
    /// The entry point the ring feeds.
    pub ep: u8,
    /// The samples live in the playback heap (an MMAP stream, `map_heap`), at the ring's places.
    pub heap: bool,
}

/// The playback ring of an entry point, by the name Linux allocates it by (aoc_alsa_drv.c:34-74; the device tree's front ends, gs101-audio.dtsi:63-190 and :426-440, name each ring and its entry point, google-aoc.h:27-36), and what Android routes thru it on husky (aoc_alsa_path.c:43-58, mixer_paths.xml:363-418):
/// - 0, `audio_playback0`: mmap-playback (EP1), AAudio's MMAP NOIRQ;
/// - 1, `audio_playback1`: low-latency-playback (EP2);
/// - 5, `audio_playback5`: deep-buffer-playback (EP6), 70 ms to the speaker in Android's own table (audio_platform_configuration.xml:80). The one this service used from 2026-11-11 to 2026-11-24, taken for the low-latency path from the boot log's bind of entry point 5;
/// - 9, `audio_raw`: raw-playback (`RAW`).
pub fn ring_name(ep: u8) -> Option<&'static [u8]> {
    match ep {
        0 => Some(b"audio_playback0"),
        1 => Some(b"audio_playback1"),
        5 => Some(b"audio_playback5"),
        9 => Some(b"audio_raw"),
        _ => None,
    }
}

/// The two services this process speaks for entry point `ep` (`ring_name`), with the control queue Linux sends every output command on (alsa.h:42 `CMD_OUTPUT_CHANNEL`).
pub fn services_for(ep: u8) -> Result<Services, &'static str> {
    let control = find(b"audio_output_control").ok_or("no audio_output_control service")?;
    let name = ring_name(ep).ok_or("no playback ring is known for that entry point")?;
    let playback = find(name).ok_or("the entry point's playback ring is not in the AoC's service table")?;
    if control.ring() || !control.fits(DOWN) || !control.fits(UP) {
        return Err("audio_output_control is not a queue inside the aoc-ipc window");
    }
    let Some(play) = playback.ring_win(DOWN).filter(|_| playback.ring()) else {
        return Err("the playback ring is not a ring inside the granted windows (aoc-ipc, aoc-play-lo, aoc-play-hi: a kernel from before has no ring window)");
    };
    // WHY: this process writes the ring a 32-bit word at a time (Device memory takes no unaligned access), so its start and its length must both be whole words; the AoC sets them, so it is checked, not assumed.
    let (off, len) = playback.span(DOWN);
    if off & 3 != 0 || len & 3 != 0 {
        return Err("the playback ring is not word-aligned");
    }
    Ok(Services { control, playback, play, ep, heap: false })
}

/// The default entry point's services.
pub fn services() -> Result<Services, &'static str> {
    services_for(EP)
}

// ── The control queue (ipc_core.c:654-689 write, :613-652 read; aoc_service_core.c:104-202) ──

/// One message onto the control queue's DOWN slots, then the AoC rung (a queue is always rung: aoc_service_core.c:194-195).
fn queue_write(s: Svc, msg: &[u8]) -> bool {
    let (off, _) = s.span(DOWN);
    let size = s.get(DOWN, R_SIZE) as u64;
    let slots = s.get(DOWN, R_SLOTS);
    let (tx, rx) = (s.get(DOWN, R_TX), s.get(DOWN, R_RX));
    if msg.len() as u64 + MSG_HDR > size || tx.wrapping_sub(rx) >= slots {
        return false;
    }
    let dst = off + size * (tx % slots) as u64;
    wr32(dst, msg.len() as u32);
    wr32(dst + 4, 0);
    for (i, &b) in msg.iter().enumerate() {
        wr8(dst + MSG_HDR + i as u64, b);
    }
    unsafe { core::arch::asm!("dsb sy") };
    s.set(DOWN, R_TX, tx.wrapping_add(1));
    s.signal();
    true
}

/// One message off the control queue's UP slots, if one waits: its length.
fn queue_read(s: Svc, buf: &mut [u8]) -> Option<usize> {
    let (off, _) = s.span(UP);
    let size = s.get(UP, R_SIZE) as u64;
    let slots = s.get(UP, R_SLOTS);
    let (tx, rx) = (s.get(UP, R_TX), s.get(UP, R_RX));
    if tx == rx {
        return None;
    }
    let src = off + size * (rx % slots) as u64;
    // The length is the AoC's: held to the buffer and the slot.
    let n = ((rd32(src) & 0xFFFF) as usize).min(buf.len()).min((size - MSG_HDR) as usize);
    for (i, b) in buf[..n].iter_mut().enumerate() {
        *b = rd8(src + MSG_HDR + i as u64);
    }
    s.set(UP, R_RX, rx.wrapping_add(1));
    Some(n)
}

/// How long a command's reply may take: Linux waits 500 ms before calling the AoC dead (alsa.h:67 `WAITING_TIME_MS`, hw.c:261-288). Here a 1 ms step, 512 of them.
const REPLY_STEPS: u32 = 1 << 9;
/// What `command` says when no reply came: the one error that means the control queue's answer never reached this process (an error code back still proves the round trip).
pub const NO_ANSWER: &str = "the AoC did not answer in 512 ms";

/// A command on the output control queue (`audio_output_control`): `command_on`.
pub fn command(s: &Services, cmd: &mut [u8], reply: &mut [u8]) -> Result<usize, &'static str> {
    command_on(s.control, cmd, reply)
}

/// A command sent on a control queue and its reply awaited (hw.c:196-311: stale replies drained first, the command written, then the first message back is its reply; 4 bytes back is an error code, hw.c:289-294). The reply's bytes, or why not. A message back that is not this command's reply (its type byte not 0, DATA_TYPE_CMD, or its id not the command's: the input queue can carry the AoC's notifications, zuma.h:340-342 and :2755-2771, NOTIF_AUDIO_INPUT_AP_MIC_RECORD_DATA_AVAILABLE) is passed over, never taken for the reply.
///
/// The wait is a bounded sleep loop, the one place this process does that, and provisional: the AoC rings the AP on the control service's channel when it answers, but whether that channel sits in f1 (whose line this process holds) is only known on the device (`STATE` prints it). If it does, this becomes an interrupt wait; until it is measured, commands go only at a stream's start and end, never on the sound's clock.
fn command_on(q: Svc, cmd: &mut [u8], reply: &mut [u8]) -> Result<usize, &'static str> {
    let mut stale = [0u8; 64];
    while queue_read(q, &mut stale).is_some() {}
    // The container header's counter (zuma.h:352-356, :2782-2794: the type 0 = DATA_TYPE_CMD, a counter, the whole length).
    cmd[1] = next_counter();
    if !queue_write(q, cmd) {
        return Err("the control queue is full");
    }
    let f = freq();
    for _ in 0..REPLY_STEPS {
        while let Some(n) = queue_read(q, reply) {
            if n == 4 {
                return Err("the AoC answered with an error code");
            }
            // The first message of type 0 (DATA_TYPE_CMD) back is the reply, as Linux takes it (hw.c:278-302); a notification (type 1, zuma.h:340-342) is passed over. Measured on the Pro 2026-11-22: MIC_RECORD_AP_SET_PARAMS (224) came back as 22 bytes of type 0 with id 103 in the id's place, and nothing else came, so the id is not checked; a reply whose id differs is said with its bytes, for the record.
            if reply[0] == 0 {
                if n < 8 || reply[4..6] != cmd[4..6] {
                    let mut l = crate::Line { b: [0; 256], n: 0 };
                    for b in &reply[..n.min(32)] {
                        let _ = core::fmt::Write::write_fmt(&mut l, format_args!(" {b:02X}"));
                    }
                    say!("audio: command A#{} answered with A#{n} bytes of another id, taken as its reply:{}", u16::from_le_bytes([cmd[4], cmd[5]]), core::str::from_utf8(&l.b[..l.n]).unwrap_or(""));
                }
                return Ok(n);
            }
            say!("audio: passed over a message on a control queue: type A#{}, A#{n} bytes", reply[0]);
        }
        sleep_until(now() + f / 1024);
    }
    Err(NO_ANSWER)
}

static mut COUNTER: u8 = 0;
fn next_counter() -> u8 {
    unsafe {
        COUNTER = COUNTER.wrapping_add(1);
        COUNTER
    }
}

// ── The commands (aoc-interface-zuma.h) ──

/// Command ids (zuma.h:92-284).
const CMD_SOURCE: u16 = 201;
const CMD_EP_SETUP: u16 = 206;
const CMD_CFG_SIGGEN: u16 = 225;
const CMD_GET_EP_TOT_SAMPLES: u16 = 231;
const CMD_BIND2: u16 = 271;
const CMD_EP_SETUP2: u16 = 298;

/// A command's eight-byte header (zuma.h:352-356 CONTAINER_HDR, :443-447 CMD_HDR): type 0, the counter (set when sent), the whole length, the id, the reply word (the AoC's).
fn header(buf: &mut [u8], id: u16) {
    let len = buf.len() as u16;
    buf[0] = 0;
    buf[1] = 0;
    buf[2..4].copy_from_slice(&len.to_le_bytes());
    buf[4..6].copy_from_slice(&id.to_le_bytes());
    buf[6..8].copy_from_slice(&0u16.to_le_bytes());
}

/// The speaker's sink (zuma.h:905 `ASNK_SPEAKER`; hw.c:41 `PORT_TDM_0_RX` → `SINK_SPEAKER`; dmesg:7427 "sink:0").
pub const SINK_SPEAKER: u8 = 0;
/// The entry point this process plays thru: 5, Android's on husky (dmesg:7426 "set ep 5 hw_id 0x6"; pcm.c:405 entry point = the PCM device's index; hw.c:58-73 a plain index is its own source).
pub const EP: u8 = 5;
/// Entry point modes (zuma.h:984-995).
pub const MODE_PLAYBACK: u8 = 0;
pub const MODE_SIGGEN: u8 = 5;

/// BIND2 (zuma.h:941-946; hw.c:1322-1343): a source to a sink, or apart.
pub fn bind(s: &Services, src: u8, dst: u8, on: bool) -> Result<(), &'static str> {
    let mut c = [0u8; 11];
    header(&mut c, CMD_BIND2);
    c[8] = src;
    c[9] = dst;
    c[10] = on as u8;
    command(s, &mut c, &mut [0u8; 64]).map(|_| ())
}

/// SOURCE (zuma.h:930-934; hw.c:1256-1287, the stream's start and stop: hw.c:2685-2729).
pub fn source(s: &Services, src: u8, on: bool) -> Result<(), &'static str> {
    let mut c = [0u8; 10];
    header(&mut c, CMD_SOURCE);
    c[8] = src;
    c[9] = on as u8;
    command(s, &mut c, &mut [0u8; 64]).map(|_| ())
}

/// EP_SETUP then EP_SETUP2 for the entry point (hw.c:1729-1815, zuma.h:998-1017 and :2727-2733), as Linux fills them for 48 kHz, two channels, 16-bit fixed point: the descriptor's address and length 0, the watermark Linux's default (alsa.h:100, 48000), wraparound on; the metadata's channel count 2 (hw.c:1764 writes the count; zuma.h:810-817 CH_STEREO is 2), width WIDTH_16_BIT 1 (zuma.h:803-808), rate SR_48KHZ 5 (zuma.h:784-793), FRMT_FIXED_POINT 1 (zuma.h:797-801). EP_SETUP2 carries the buffer and period in bytes (hw.c:1690-1706); Linux gives ALSA's, this process the ring it keeps ahead and its period.
pub fn setup(s: &Services, mode: u8, buffer_bytes: u32, period_bytes: u32) -> Result<(), &'static str> {
    setup_as(s, mode, buffer_bytes, period_bytes, 1, 1)
}

/// `setup` with the sample's width and format codes (zuma.h:797-808: WIDTH_16_BIT 1, WIDTH_32_BIT 3; FRMT_FLOATING_POINT 0, FRMT_FIXED_POINT 1).
pub fn setup_as(s: &Services, mode: u8, buffer_bytes: u32, period_bytes: u32, width: u8, format: u8) -> Result<(), &'static str> {
    let mut c = [0u8; 28];
    header(&mut c, CMD_EP_SETUP);
    c[8..12].copy_from_slice(&0u32.to_le_bytes());
    c[12..16].copy_from_slice(&48_000u32.to_le_bytes());
    c[16..20].copy_from_slice(&0u32.to_le_bytes());
    c[20] = 2;
    c[21] = width;
    c[22] = 5;
    c[23] = format;
    c[24] = 0;
    c[25] = s.ep;
    c[26] = 1;
    c[27] = mode;
    command(s, &mut c, &mut [0u8; 64])?;
    let mut c = [0u8; 18];
    header(&mut c, CMD_EP_SETUP2);
    c[8] = s.ep;
    c[9..13].copy_from_slice(&buffer_bytes.to_le_bytes());
    c[13..17].copy_from_slice(&period_bytes.to_le_bytes());
    c[17] = mode;
    command(s, &mut c, &mut [0u8; 64]).map(|_| ())
}

/// CFG_SIGGEN (zuma.h:1244-1249): the AoC's own generator's wave, never sent by Linux (AUDIO.md: wave types 0-3 DC, square, triangular, sawtooth by the firmware's strings, a guess).
pub fn siggen(s: &Services, wave: u8, period: u16, amplitude: u32) -> Result<(), &'static str> {
    let mut c = [0u8; 15];
    header(&mut c, CMD_CFG_SIGGEN);
    c[8] = wave;
    c[9..11].copy_from_slice(&period.to_le_bytes());
    c[11..15].copy_from_slice(&amplitude.to_le_bytes());
    command(s, &mut c, &mut [0u8; 64]).map(|_| ())
}

/// GET_EP_TOT_SAMPLES (zuma.h:1255-1263): how many samples the entry point has taken, the AoC's own count (a check that it plays without ears).
pub fn samples(s: &Services) -> Result<u64, &'static str> {
    let mut c = [0u8; 17];
    header(&mut c, CMD_GET_EP_TOT_SAMPLES);
    c[8] = s.ep;
    let mut r = [0u8; 64];
    let n = command(s, &mut c, &mut r)?;
    if n < 17 {
        return Err("a short GET_EP_TOT_SAMPLES reply");
    }
    let mut w = [0u8; 8];
    w.copy_from_slice(&r[9..17]);
    Ok(u64::from_le_bytes(w))
}

/// GET_EP_TOT_SAMPLES on the output control queue alone (the probe's round trip, needing no ring).
pub fn control_samples() -> Result<u64, &'static str> {
    let control = find(b"audio_output_control").ok_or("no audio_output_control service")?;
    let mut c = [0u8; 17];
    header(&mut c, CMD_GET_EP_TOT_SAMPLES);
    c[8] = EP;
    let mut r = [0u8; 64];
    let n = command_on(control, &mut c, &mut r)?;
    if n < 17 {
        return Err("a short GET_EP_TOT_SAMPLES reply");
    }
    let mut w = [0u8; 8];
    w.copy_from_slice(&r[9..17]);
    Ok(u64::from_le_bytes(w))
}

// ── The playback ring (ipc_core.c:185-210 write, :53-65 advance, :332-365 room, :578-600 reset) ──

impl Services {
    /// Bytes written and not yet taken by the AoC: tx - rx, both running byte counts (ipc_core.c:87-100, :248-272).
    pub fn queued(&self) -> u32 {
        self.playback.get(DOWN, R_TX).wrapping_sub(self.playback.get(DOWN, R_RX))
    }

    /// The ring's write pointer moved to its start, the bytes skipped counted as written (ipc_core.c:578-600), as Linux does before every setup (pcm.c:564-571).
    pub fn reset_write(&self) {
        let p = self.playback;
        let size = p.get(DOWN, R_SIZE);
        let wp = p.get(DOWN, R_WP);
        p.set(DOWN, R_TX, p.get(DOWN, R_TX).wrapping_add(size - wp));
        p.set(DOWN, R_WP, 0);
    }

    /// The running count of bytes the AoC took (rx) (ipc_core.c:87-100): what it has read, not what has played (aoc_alsa_pcm.c:194-199).
    pub fn taken(&self) -> u32 {
        self.playback.get(DOWN, R_RX)
    }

    /// The ring's size in bytes.
    pub fn ring_bytes(&self) -> u32 {
        self.playback.get(DOWN, R_SIZE)
    }

    /// Frames onto the ring at its write pointer, wrapping at its end, then tx and wp advanced (ipc_core.c:672-679: the data, a barrier, then the counters). A frame is one word (left in the low half, right in the high: S16_LE interleaved). The AoC is rung only for a push ring (aoc_service_core.c:194-195).
    pub fn write(&self, frames: &[u32]) {
        let p = self.playback;
        let (off, _) = p.span(DOWN);
        let size = p.get(DOWN, R_SIZE) as u64;
        let mut wp = p.get(DOWN, R_WP) as u64;
        for &f in frames {
            self.play.wr32(off + wp, f);
            wp += 4;
            if wp == size {
                wp = 0;
            }
        }
        unsafe { core::arch::asm!("dsb sy") };
        p.set(DOWN, R_TX, p.get(DOWN, R_TX).wrapping_add(4 * frames.len() as u32));
        p.set(DOWN, R_WP, wp as u32);
        if p.flags() & RING_PUSH != 0 {
            p.signal();
        }
    }
}

// ── The MMAP entry point (entry point 0, `audio_playback0`: AAudio's MMAP path, aoc_alsa_pcm.c:604-614 and :215-221) ──
//
// Linux marks the whole ring written at the stream's prepare and again at every interrupt (`aoc_service_advance_write_index` by all that is free: tx = rx + size), so the AoC reads the ring round and round from its own read place, and the writer puts samples just ahead of it. Written as a plain ring instead (tx moved by what was written), entry point 0 takes the samples at its 1 ms rate and plays none of them (2026-11-24, the Pro: a 1 kHz tone at -24 dBFS left the microphones at their noise floor).

impl Services {
    /// The byte the AoC reads at running count `count` (counts as tx and rx are kept: running byte counts, ipc_core.c:87-100), from its read place: its rp is rx's place in the ring.
    fn place(&self, count: u32) -> u64 {
        let p = self.playback;
        let size = p.get(DOWN, R_SIZE) as i64;
        let (rx, rp) = (p.get(DOWN, R_RX), p.get(DOWN, R_RP) as i64);
        // A count behind rx is a place already read (the zeroing of what was played), ahead a place to come: both as a signed distance from rx, inside one turn of the ring.
        (rp + count.wrapping_sub(rx) as i32 as i64).rem_euclid(size) as u64
    }

    /// Every free byte marked written (`aoc_service_advance_write_index(avail)`: tx and wp moved by size less what waits), so the AoC never finds the ring short.
    pub fn mark_full(&self) {
        let p = self.playback;
        let size = p.get(DOWN, R_SIZE);
        let free = size - p.get(DOWN, R_TX).wrapping_sub(p.get(DOWN, R_RX));
        unsafe { core::arch::asm!("dsb sy") };
        p.set(DOWN, R_TX, p.get(DOWN, R_TX).wrapping_add(free));
        p.set(DOWN, R_WP, (p.get(DOWN, R_WP) + free) % size);
    }

    /// Frames written at running count `count` (at or after the AoC's rx), wrapping at the ring's end.
    pub fn put_at(&self, count: u32, frames: &[u32]) {
        let (off, _) = self.playback.span(DOWN);
        let size = self.playback.get(DOWN, R_SIZE) as u64;
        let mut at = self.place(count);
        for &f in frames {
            self.play.wr32(off + at, f);
            at += 4;
            if at == size {
                at = 0;
            }
        }
    }

    /// A word at ring place `at`: in the playback heap for a mapped MMAP stream, in the IPC ring otherwise.
    fn put_word(&self, off: u64, at: u64, v: u32) {
        if self.heap {
            heap_wr32(HEAP_PLAYBACK.0 + at, v)
        } else {
            self.play.wr32(off + at, v)
        }
    }

    /// Frames written at running count `count` as 32-bit floats, two words a frame (left, right): AAudio's MMAP port is PCM_FLOAT (audio_policy_configuration.xml:66-68), the format entry point 0 is set up for here (main.rs `start`).
    pub fn put_at_f32(&self, count: u32, frames: &[u32]) {
        let (off, _) = self.playback.span(DOWN);
        let size = self.playback.get(DOWN, R_SIZE) as u64;
        let mut at = self.place(count);
        for &f in frames {
            for x in [f as u16 as i16, (f >> 16) as u16 as i16] {
                // A sample over 2^15: full scale at 1.
                self.put_word(off, at, (x as f32 * (1. / ((1u32 << 15) as f32))).to_bits());
                at += 4;
                if at == size {
                    at = 0;
                }
            }
        }
    }

    /// `bytes` of silence at running count `count`: what the AoC has played, so a writer late by a turn plays nothing rather than the turn before.
    pub fn zero_at(&self, count: u32, bytes: u32) {
        let (off, _) = self.playback.span(DOWN);
        let size = self.playback.get(DOWN, R_SIZE) as u64;
        let mut at = self.place(count);
        for _ in 0..bytes >> 2 {
            self.put_word(off, at, 0);
            at += 4;
            if at == size {
                at = 0;
            }
        }
    }

    /// The whole ring silent (its start).
    pub fn zero_all(&self) {
        let (off, len) = self.playback.span(DOWN);
        for k in 0..len >> 2 {
            self.put_word(off, 4 * k, 0);
        }
    }
}

// ── The AAudio heaps (measured from Android's HAL on the Pro, 2026-11-24: docs/AUDIO.md "What Android sends") ──
//
// An MMAP stream's samples are not in its IPC ring: Android's HAL hands the AoC a buffer inside the playback or the capture heap (the kernel's `parameters` declares both at the AoC's start) with CMD_MEM_MAP_REGION on the AoC's `control` service, and the AoC reads and writes the stream there at the ring's own places; the ring keeps only the counts.

type Heaps = Handle<S_HEAPS>;
/// The grant's start (kernel cachemap.rs AOC_HEAPS), the two heaps in it, and how the AoC sees the carve-out (aoc.c:187-198 `aoc_dram_translate_to_aoc`: G#9400_0000 on the AP is G#9800_0000 to the AoC).
const HEAPS_BASE: u64 = 0x96BE_0000;
pub const HEAP_CAPTURE: (u64, u32) = (0x96BE_C000, 64 << 10);
pub const HEAP_PLAYBACK: (u64, u32) = (0x96BF_C000, 16 << 10);
fn to_aoc(ap: u64) -> u32 {
    (ap - 0x9400_0000 + 0x9800_0000) as u32
}
const CMD_MEM_MAP_REGION: u16 = 212;
/// This process's names for its two mappings (any u32 the AP chooses: Linux passes its dma-buf's id).
pub const MAP_PLAYBACK: u32 = 1;
pub const MAP_CAPTURE: u32 = 2;

/// Whether this process holds the heaps (a kernel from before 2026-11-24 grants nothing at slot 12).
pub fn heaps() -> bool {
    Heaps::len() >= 128 << 10
}

/// A heap mapped into the AoC, or unmapped (CMD_MEM_MAP_REGION, zuma.h:2438-2444, on `control`: the generic header's four zero bytes, then handle, base, size, mapped), as aoc_control_dev.c:587-609 sends it. `handle` is the caller's own name for the mapping.
pub fn map_heap(handle: u32, heap: (u64, u32), on: bool) -> Result<(), &'static str> {
    let control = find(b"control").ok_or("no control service")?;
    if control.ring() || !control.fits(DOWN) || !control.fits(UP) {
        return Err("the control service is not a queue inside the aoc-ipc window");
    }
    let mut c = [0u8; 25];
    header(&mut c, CMD_MEM_MAP_REGION);
    c[12..16].copy_from_slice(&handle.to_le_bytes());
    c[16..20].copy_from_slice(&to_aoc(heap.0).to_le_bytes());
    c[20..24].copy_from_slice(&heap.1.to_le_bytes());
    c[24] = on as u8;
    command_on(control, &mut c, &mut [0u8; 64]).map(|_| ())
}

fn heap_wr32(ap: u64, v: u32) {
    Heaps::write32(ap - HEAPS_BASE, v)
}
fn heap_rd32(ap: u64) -> u32 {
    Heaps::read32(ap - HEAPS_BASE)
}

// ── The speaker's tuning (measured from Android's HAL on the Pro, 2026-11-24: docs/AUDIO.md "What Android sends") ──
//
// Before an MMAP stream's setup Android's HAL writes its speaker tuning (11,272 bytes: ids with float values, biquad sets, the volume's gain) into the ring `audio_bulk_tx`, the whole 16 KiB of it, then sends SET_GET_PARAM_BULK (zuma.h:1112-1122) on the queue `audio_output_tuning`: block 16, component 2, the tuning's offset in the ring and its size. Without it entry point 0 takes samples and plays none.

const CMD_SET_GET_PARAM_BULK: u16 = 213;

/// The tuning `blob` sent as Android sends it: the bulk ring written whole from its write place (the blob, then zeros), then the command naming it.
pub fn tune(blob: &[u8]) -> Result<(), &'static str> {
    let bulk = find(b"audio_bulk_tx").ok_or("no audio_bulk_tx service")?;
    let tun = find(b"audio_output_tuning").ok_or("no audio_output_tuning service")?;
    // WHY: both are the AoC's, read from its memory; written only inside the aoc-ipc window (as `Svc::fits`).
    if !bulk.ring() || !bulk.fits(DOWN) || tun.ring() || !tun.fits(DOWN) || !tun.fits(UP) {
        return Err("audio_bulk_tx or audio_output_tuning is not where it should be");
    }
    let (off, size) = bulk.span(DOWN);
    // WHY: the command carries the size in a u16 and the blob must fit the ring.
    if blob.len() as u64 > size || blob.len() > u16::MAX as usize {
        return Err("the tuning is larger than the bulk ring");
    }
    let wp = bulk.get(DOWN, R_WP) as u64;
    let mut k = 0u64;
    while k < size {
        let mut w = [0u8; 4];
        for (j, b) in w.iter_mut().enumerate() {
            *b = blob.get(k as usize + j).copied().unwrap_or(0);
        }
        wr32(off + (wp + k) % size, u32::from_le_bytes(w));
        k += 4;
    }
    unsafe { core::arch::asm!("dsb sy") };
    bulk.set(DOWN, R_TX, bulk.get(DOWN, R_TX).wrapping_add(size as u32));
    let mut c = [0u8; 26];
    header(&mut c, CMD_SET_GET_PARAM_BULK);
    c[8] = 16;
    c[9] = 2;
    c[10..12].copy_from_slice(&(wp as u16).to_le_bytes());
    c[22..24].copy_from_slice(&(blob.len() as u16).to_le_bytes());
    command_on(tun, &mut c, &mut [0u8; 64]).map(|_| ())
}

// ── The AoC-to-AP line (wc.c:150-175) ──

/// The f1 block's channels this process listens to unmasked, every other one of the block masked (wc.c:122-134: a clear bit is unmasked): only this process uses f1's line on ferros, so the line rises for its channels alone. The playback ring's and the capture ring's channels are added and taken away apart (they may be the same channel: the bit stays while either listens).
static LISTEN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// A listener's channels on (`on`) or off: bits 0-15 the playback's, 16-31 the capture's, so one going leaves the other's.
pub fn listen(who: u32, channels: u16, on: bool) {
    use core::sync::atomic::Ordering;
    let shift = if who == 0 { 0 } else { 16 };
    let bits = (channels as u32) << shift;
    let all = if on { LISTEN.fetch_or(bits, Ordering::AcqRel) | bits } else { LISTEN.fetch_and(!bits, Ordering::AcqRel) & !bits };
    let unmasked = (all | all >> 16) & 0xFFFF;
    let g = cap(crate::grants(), S_MBOX);
    mmio_write(g, MBOX_BLOCKS[F1 as usize] + MB_INTMR1, !unmasked & 0xFFFF);
}

/// What the AoC rang on f1 since the last look, cleared (wc.c:162-166).
pub fn take() -> u16 {
    let g = cap(crate::grants(), S_MBOX);
    let st = mmio_read(g, MBOX_BLOCKS[F1 as usize] + MB_INTSR1) as u32 & 0xFFFF;
    if st != 0 {
        mmio_write(g, MBOX_BLOCKS[F1 as usize] + MB_INTCR1, st);
    }
    st as u16
}

/// A service's channel and type as a line for the console.
pub fn describe(name: &str, s: Svc) {
    let (off, len) = s.span(DOWN);
    say!("audio: {name}: channel A#{}, {}, DOWN at G#{off:X} A#{len} bytes", s.irq(), if s.ring() { "ring" } else { "queue" });
}

// ── The microphones (research 2026-11-22 from the AoC's ALSA driver, every step marked; docs/AUDIO.md "Microphones") ──
//
// A plain built-in microphone capture, as Linux runs PCM device 8 (`fe_ep1_tx`, the ring `audio_capture0`: gs101-audio.dtsi:63-205, aoc_alsa.h:52-57): the commands on the input control queue `audio_input_control` (aoc_alsa.h:41, aoc_alsa_drv.c:36), in the order its prepare and trigger send them (hw.c:1839-1983, hw.c:432-438, pcm.c:633-640), the samples read off the ring's UP region (ipc_core.c:613-652, :132-183). Command ids collide between the two queues (zuma.h:119-123: 208 is AP_INPUT_START here, OUTPUT_AEC_NOTIFY there), so every one of these goes on the input queue only.

/// Commands on `audio_input_control` (zuma.h:100-332).
const CMD_MIC_POWER_ON: u16 = 203;
const CMD_MIC_POWER_OFF: u16 = 204;
const CMD_AP_INPUT_START: u16 = 208;
const CMD_AP_INPUT_STOP: u16 = 209;
const CMD_MIC_RECORD_AP_SET_PARAMS: u16 = 224;
const CMD_INPUT_SET_PARAMETER: u16 = 233;
const CMD_SET_MIC_DC_BLOCKER: u16 = 252;
const CMD_MIC_RECORD_AP_START_DATA: u16 = 293;
const CMD_MIC_RECORD_AP_SET_PARAMS2: u16 = 359;
const CMD_INPUT_GET_PARAMETER: u16 = 235;
const CMD_MMAP_ENABLE_2: u16 = 313;
const CMD_RAW_ENABLE_2: u16 = 314;
const CMD_MIC_MMAP_DISABLE: u16 = 295;
const CMD_MIC_RAW_DISABLE: u16 = 306;
const CMD_GET_MIC_CURRENT_HW_GAIN: u16 = 250;
const CMD_SET_MIC_HIGH_POWER_HW_GAIN: u16 = 254;

/// The record soft gain's place among the input's parameters (hw.c:2236-2255 `aoc_mic_record_gain_set`: block 136, component 30, key 16, whole dB as an integer, which Android's mixer_paths.xml:240, :758, :848 set to -40, 22 and 5).
const SOFT_GAIN: (u8, u8, u32) = (136, 30, 16);

/// The microphones' gains as the AoC holds them (measure before fixing): the hardware gain in centibels (GET_MIC_CURRENT_HW_GAIN, hw.c:560-588) and the record soft gain (INPUT_GET_PARAMETER, zuma.h:1460-1466). None for one not answered.
fn gains(c: &Capture) -> (Option<i32>, Option<i32>) {
    let mut m = [0u8; 12];
    header(&mut m, CMD_GET_MIC_CURRENT_HW_GAIN);
    let mut r = [0u8; 64];
    let hw = command_on(c.control, &mut m, &mut r).ok().filter(|&n| n >= 12).map(|_| i32::from_le_bytes([r[8], r[9], r[10], r[11]]));
    let mut m = [0u8; 18];
    header(&mut m, CMD_INPUT_GET_PARAMETER);
    m[8] = SOFT_GAIN.0;
    m[9] = SOFT_GAIN.1;
    m[10..14].copy_from_slice(&SOFT_GAIN.2.to_le_bytes());
    let soft = command_on(c.control, &mut m, &mut r).ok().filter(|&n| n >= 18).map(|_| i32::from_le_bytes([r[14], r[15], r[16], r[17]]));
    (hw, soft)
}

/// The gains set: the record soft gain (whole dB) and the hardware gain at high power (dB, sent in centibels: hw.c:590-620, zuma.h:1625-1628; Android's range -720 to 240 cB, alsa.h:102-103).
fn set_gains(c: &Capture, soft: i8, hw: i8) {
    let mut m = [0u8; 18];
    header(&mut m, CMD_INPUT_SET_PARAMETER);
    m[8] = SOFT_GAIN.0;
    m[9] = SOFT_GAIN.1;
    m[10..14].copy_from_slice(&SOFT_GAIN.2.to_le_bytes());
    m[14..18].copy_from_slice(&(soft as i32).to_le_bytes());
    let _ = mic_command(c, "INPUT_SET_PARAMETER (the record soft gain)", &mut m);
    // The MMAP record's own gain (hw.c:2280-2295 `aoc_mmap_record_gain_set`: block 136, component 0, key 16, whole dB), set to the same: Android's trace never showed it, so its value is whatever the AoC defaults to.
    m[9] = 0;
    let _ = mic_command(c, "INPUT_SET_PARAMETER (the MMAP record gain)", &mut m);
    let mut m = [0u8; 12];
    header(&mut m, CMD_SET_MIC_HIGH_POWER_HW_GAIN);
    m[8..12].copy_from_slice(&(hw as i32 * 10).to_le_bytes());
    let _ = mic_command(c, "SET_MIC_HIGH_POWER_HW_GAIN", &mut m);
}

/// The gains as last read at a capture's start (hardware cB, soft dB; `i32::MIN` for one not answered or not read), for the bench's report.
pub static GAINS: [core::sync::atomic::AtomicI32; 2] = [const { core::sync::atomic::AtomicI32::new(i32::MIN) }; 2];

fn say_gains(when: &str, g: (Option<i32>, Option<i32>)) {
    GAINS[0].store(g.0.unwrap_or(i32::MIN), core::sync::atomic::Ordering::Relaxed);
    GAINS[1].store(g.1.unwrap_or(i32::MIN), core::sync::atomic::Ordering::Relaxed);
    match g {
        (Some(h), Some(sg)) => say!("audio: microphone gains {when}: hardware A#{h} cB, record soft gain A#{sg} dB"),
        (Some(h), None) => say!("audio: microphone gains {when}: hardware A#{h} cB, the soft gain not answered"),
        (None, Some(sg)) => say!("audio: microphone gains {when}: the hardware gain not answered, record soft gain A#{sg} dB"),
        (None, None) => say!("audio: microphone gains {when}: neither answered"),
    }
}

/// husky's built-in microphones: ids 0, 1 and 2, slot 3 unused in every path (mixer_paths.xml:750-753, :840-843; mixer_paths_factory.xml:126-134). Their places (the HAL's bottom, back, top, audio_platform_configuration.xml:58-69) are marked "fake data" by the HAL itself, so the page names them by id.
pub const MICS: usize = 3;
/// The capture's period as Linux asks for it (hw.c:1901-1902: 10 ms, four of them), and its sample width: 16 bits.
/// Both are a capture's own since 2026-11-24 (`Shape`): the field is whole milliseconds (zuma.h:832 `period_ms`, a u8), so 1, 2 and 5 ms are what can be asked, never 2.5.
pub const CAPTURE_MS: u8 = 10;
/// The widths a capture may ask for (zuma.h:803-808 `BitsPerSample`): WIDTH_16_BIT 1, WIDTH_24_BIT 2, WIDTH_32_BIT 3.
pub const WIDTH_16: u8 = 1;
pub const WIDTH_32: u8 = 3;

/// A capture's shape: its period in milliseconds, its sample width's code, and the gains to set at its start (the record soft gain in whole dB and the hardware gain in dB; None leaves the AoC's own).
#[derive(Clone, Copy, PartialEq)]
pub struct Shape {
    pub ms: u8,
    pub width: u8,
    pub gain: Option<(i8, i8)>,
    /// Which of the AoC's record paths (aoc_alsa.h:52-57, the stream types aoc_alsa_hw.c:171-178): `REC_NORMAL` (device 8, `audio_capture0`, Android's audio record, three microphones), `REC_MMAP` (device 9, `audio_capture1`, AAudio's MMAP record) or `REC_RAW` (device 10, `audio_capture2`, the low-latency record, started as a raw stream); the last two carry at most two channels (aoc_alsa_hw.c:1352-1356, :1384-1388).
    pub kind: u8,
}

pub const REC_NORMAL: u8 = 0;
pub const REC_MMAP: u8 = 1;
pub const REC_RAW: u8 = 2;

impl Shape {
    pub const LINUX: Shape = Shape { ms: CAPTURE_MS, width: WIDTH_16, gain: None, kind: REC_NORMAL };
    /// Channels in the ring: the three microphones on the normal path, two on the others.
    pub fn chans(self) -> usize {
        // Android's MMAP and low-latency records ask for one channel (the HAL's commands, measured 2026-11-24); asked for two, the MMAP ring filled with zeros.
        if self.kind == REC_NORMAL { MICS } else { 1 }
    }
    /// The PCM device the record path is (SET_PARAMS2's channel, hw.c:1822 and aoc_alsa.h:52-57).
    fn device(self) -> u8 {
        8 + self.kind
    }
    fn ring_name(self) -> &'static [u8] {
        match self.kind {
            REC_MMAP => b"audio_capture1",
            REC_RAW => b"audio_capture2",
            _ => b"audio_capture0",
        }
    }
    /// Bytes a sample: two for 16 bits, four for 24 and 32 (ALSA's S24_LE, which the AoC's playback side lists, is a 24-bit value in a 32-bit word: aoc_alsa_pcm.c:177-179).
    pub fn sample_bytes(self) -> usize {
        if self.width == WIDTH_16 { 2 } else { 4 }
    }
    /// Frames a period at 48 kHz.
    pub fn period_frames(self) -> usize {
        48 * self.ms as usize
    }
    pub fn period_bytes(self) -> u32 {
        (self.period_frames() * self.chans() * self.sample_bytes()) as u32
    }
}

/// The input control queue and the capture ring, found and checked.
#[derive(Clone, Copy)]
pub struct Capture {
    pub control: Svc,
    pub ring: Svc,
    win: RingMap,
    /// The record path (`Shape::kind`), for its stop.
    pub kind: u8,
    /// The samples live in the capture heap (an MMAP record, `map_heap`), at the ring's places.
    pub heap: bool,
}

/// The capture's two services by the names Linux allocates them by: the normal record's ring.
pub fn capture_services() -> Result<Capture, &'static str> {
    capture_services_for(Shape::LINUX)
}

/// The input control queue and the ring of `shape`'s record path.
pub fn capture_services_for(shape: Shape) -> Result<Capture, &'static str> {
    let control = find(b"audio_input_control").ok_or("no audio_input_control service")?;
    let ring = find(shape.ring_name()).ok_or("the record path's ring is not in the AoC's service table")?;
    if control.ring() || !control.fits(DOWN) || !control.fits(UP) {
        return Err("audio_input_control is not a queue inside the aoc-ipc window");
    }
    let Some(win) = ring.ring_win(UP).filter(|_| ring.ring()) else {
        return Err("the record ring is not inside the granted windows (aoc-ipc, aoc-capture: a kernel from before has no ring window)");
    };
    // WHY: samples are read two bytes at a time at the ring's byte positions; an odd start or length would split one across the wrap. The AoC sets both, so they are checked.
    let (off, len) = ring.span(UP);
    if off & 1 != 0 || len & 1 != 0 {
        return Err("the record ring is not sample-aligned");
    }
    Ok(Capture { control, ring, win, kind: shape.kind, heap: false })
}

/// A capture command's reply checked only for arriving: the AoC's answer, or its error said with the command's name.
fn mic_command(c: &Capture, name: &str, cmd: &mut [u8]) -> Result<(), &'static str> {
    let mut r = [0u8; 64];
    command_on(c.control, cmd, &mut r).map(|_| ()).map_err(|e| {
        say!("audio: {name}: {e}");
        e
    })
}

/// The capture started in `shape`: the microphones powered (as the factory test does, mixer_paths_factory.xml:56-66; Linux's own capture leaves it to the firmware, uncertain on a cold AoC, so sent and its refusal only said), the DC blocker on (the HAL's boot setting, mixer_paths.xml:238), then SET_PARAMS (before AP_INPUT_START: the firmware refuses them while the input runs, its string "SET_MIC_RECORD_AP_PARAMS called while APInputProcessor is started"), SET_PARAMS2, the microphone list, AP_INPUT_START, the ring flushed (hw.c:1979-1983), START_DATA (hw.c:1497-1520).
pub fn capture_start(c: &Capture, shape: Shape) -> Result<(), &'static str> {
    let r = capture_steps(c, shape);
    // A start that failed part way leaves nothing on: the input stopped (refused harmlessly if it never started) and the microphones' power taken back.
    if r.is_err() {
        capture_stop(c);
    }
    r
}

fn capture_steps(c: &Capture, shape: Shape) -> Result<(), &'static str> {
    for mic in 0..MICS as u8 {
        let mut m = [0u8; 9];
        header(&mut m, CMD_MIC_POWER_ON);
        m[8] = mic;
        let _ = mic_command(c, "MIC_POWER_ON", &mut m);
    }
    let mut m = [0u8; 9];
    header(&mut m, CMD_SET_MIC_DC_BLOCKER);
    m[8] = 1;
    let _ = mic_command(c, "SET_MIC_DC_BLOCKER", &mut m);
    say_gains("as found", gains(c));
    if let Some((soft, hw)) = shape.gain {
        set_gains(c, soft, hw);
        say_gains("as set", gains(c));
    }
    // MIC_RECORD_AP_SET_PARAMS (zuma.h:819-842; hw.c:1839-1949): three channels, the width (Linux: WIDTH_16_BIT 1), SR_48KHZ 5 (twice: byte 10 and byte 20, hw.c:1941), FRMT_FIXED_POINT 1, offset 0, the PDM mask of microphones 0-2, the period in milliseconds (Linux: 10), four of them, channel k from microphone k (G#FF unused), AP_MIC_PROCESS_RAW 0.
    let mut m = [0u8; 22];
    header(&mut m, CMD_MIC_RECORD_AP_SET_PARAMS);
    m[8] = shape.chans() as u8;
    m[9] = shape.width;
    m[10] = 5;
    m[11] = 1;
    m[12] = 0;
    // The PDM mask and the interleave as Android sends them for every record path: all three microphones, channel k from microphone k (a one-channel record takes the first).
    m[13] = (1 << MICS) - 1;
    // Android keeps 10 ms here on the MMAP and raw paths and gives the real period in SET_PARAMS2's bytes.
    m[14] = if shape.kind == REC_NORMAL { shape.ms } else { CAPTURE_MS };
    m[15] = 4;
    m[16..20].copy_from_slice(&[0, 1, 2, 0xFF]);
    m[20] = 5;
    m[21] = 0;
    mic_command(c, "MIC_RECORD_AP_SET_PARAMS", &mut m)?;
    // MIC_RECORD_AP_SET_PARAMS2 (zuma.h:2738-2743; hw.c:1818-1837): the PCM device's index 8 (not an entry point, hw.c:1822), the buffer and period in bytes (Linux gives ALSA's: four periods of 10 ms).
    let period = shape.period_bytes();
    let mut m = [0u8; 17];
    header(&mut m, CMD_MIC_RECORD_AP_SET_PARAMS2);
    m[8] = shape.device();
    // The buffer: four periods on the normal path, the whole ring on the MMAP and raw paths (Android's 46,080 for `audio_capture1`, measured).
    let buffer = if shape.kind == REC_NORMAL { 4 * period } else { c.ring.span(UP).1 as u32 };
    m[9..13].copy_from_slice(&buffer.to_le_bytes());
    m[13..17].copy_from_slice(&period.to_le_bytes());
    mic_command(c, "MIC_RECORD_AP_SET_PARAMS2", &mut m)?;
    // The microphone list as INPUT_SET_PARAMETER (zuma.h:1433-1439; hw.c:4334-4348): block 139 (ABLOCK_INPUT_PDM_MIC), component 0, key 0, the value's byte k microphone k's id, G#FF unused.
    let mut m = [0u8; 18];
    header(&mut m, CMD_INPUT_SET_PARAMETER);
    m[8] = 139;
    m[9] = 0;
    m[10..14].copy_from_slice(&0u32.to_le_bytes());
    m[14..18].copy_from_slice(&0xFF02_0100u32.to_le_bytes());
    mic_command(c, "INPUT_SET_PARAMETER (the microphone list)", &mut m)?;
    // The MMAP and raw record paths start with their own enable and its channel count (hw.c:1345-1407: MMAP_ENABLE_2, RAW_ENABLE_2), once the microphones' input runs (the backend Linux's mixer starts before the trigger, hw.c:1584-1633; on the device the enable alone delivered nothing, 2026-11-24).
    if shape.kind != REC_NORMAL {
        let mut m = [0u8; 9];
        header(&mut m, CMD_AP_INPUT_START);
        m[8] = 1;
        let _ = mic_command(c, "AP_INPUT_START", &mut m);
        c.flush();
        let mut m = [0u8; 9];
        header(&mut m, if shape.kind == REC_MMAP { CMD_MMAP_ENABLE_2 } else { CMD_RAW_ENABLE_2 });
        m[8] = shape.chans() as u8;
        return mic_command(c, "MMAP_ENABLE_2 or RAW_ENABLE_2", &mut m);
    }
    // AP_INPUT_START (zuma.h:1843-1862; hw.c:359-374, :411-413): input 1, AP_INPUT_PROCESSOR_MIC_INPUT_INDEX.
    let mut m = [0u8; 9];
    header(&mut m, CMD_AP_INPUT_START);
    m[8] = 1;
    mic_command(c, "AP_INPUT_START", &mut m)?;
    c.flush();
    let mut m = [0u8; 8];
    header(&mut m, CMD_MIC_RECORD_AP_START_DATA);
    mic_command(c, "MIC_RECORD_AP_START_DATA", &mut m)
}

/// The capture stopped (hw.c:446-457, :1522-1538: AP_INPUT_STOP) and the microphones' power taken back.
pub fn capture_stop(c: &Capture) {
    let mut m = [0u8; 8];
    header(&mut m, match c.kind { REC_MMAP => CMD_MIC_MMAP_DISABLE, REC_RAW => CMD_MIC_RAW_DISABLE, _ => CMD_AP_INPUT_STOP });
    let _ = mic_command(c, "the record path's stop", &mut m);
    for mic in 0..MICS as u8 {
        let mut m = [0u8; 9];
        header(&mut m, CMD_MIC_POWER_OFF);
        m[8] = mic;
        let _ = mic_command(c, "MIC_POWER_OFF", &mut m);
    }
    if c.heap {
        let _ = map_heap(MAP_CAPTURE, HEAP_CAPTURE, false);
    }
}

impl Capture {
    /// Everything the AoC has written dropped (ipc_core.c:559-571: rx up to tx, rp by as much), as Linux does after AP_INPUT_START.
    pub fn flush(&self) {
        let r = self.ring;
        let size = r.get(UP, R_SIZE);
        let delta = r.get(UP, R_TX).wrapping_sub(r.get(UP, R_RX));
        r.set(UP, R_RX, r.get(UP, R_RX).wrapping_add(delta));
        r.set(UP, R_RP, (r.get(UP, R_RP) + delta % size) % size);
    }

    /// Bytes the AoC wrote over before they were read (more than a ring behind: ipc_core.c:145-149) skipped, rx and rp moved past them so the ring's last full turn is what `read` reads: their count, so the caller keeps its place in the frame.
    pub fn skip_overrun(&self) -> u32 {
        let r = self.ring;
        let size = r.get(UP, R_SIZE);
        let delta = r.get(UP, R_TX).wrapping_sub(r.get(UP, R_RX));
        if delta <= size {
            return 0;
        }
        let skipped = delta - size;
        r.set(UP, R_RX, r.get(UP, R_RX).wrapping_add(skipped));
        r.set(UP, R_RP, (r.get(UP, R_RP) + skipped % size) % size);
        skipped
    }

    /// The samples the AoC has written since the last read (at most a ring: `skip_overrun` first), each handed to `f` in order (interleaved, microphone 0 first), then rx and rp moved past them (ipc_core.c:67-79, :634: rx first, then rp). `bytes` a sample, 2 or 4: a 4-byte sample is handed on as its top 16 bits, so meters and recordings keep one scale whatever the width (the whole word is in `raw` for the first few, to see how the AoC packs 24 bits). A word read where the position is aligned, two bytes where it is not (Device memory takes no unaligned access). Whole samples only: a part waits for the rest. The bytes read.
    pub fn read(&self, bytes: u64, mut f: impl FnMut(i16, u32)) -> u32 {
        let r = self.ring;
        let (off, size) = r.span(UP);
        let rx = r.get(UP, R_RX);
        let delta = r.get(UP, R_TX).wrapping_sub(rx);
        // A delta past the ring is `skip_overrun`'s; here it is the smaller of the two counts, never more than one turn read.
        let n = (if delta as u64 > size { size as u32 } else { delta } as u64) & !(bytes - 1);
        let mut rp = r.get(UP, R_RP) as u64;
        let mut done = 0u64;
        while done < n {
            if rp & 3 == 0 && n - done >= 4 && rp + 4 <= size {
                let w = if self.heap { heap_rd32(HEAP_CAPTURE.0 + rp) } else { self.win.rd32(off + rp) };
                if bytes == 4 {
                    f((w >> 16) as u16 as i16, w);
                } else {
                    f(w as u16 as i16, w & 0xFFFF);
                    f((w >> 16) as u16 as i16, w >> 16);
                }
                rp += 4;
                done += 4;
            } else {
                let x = self.win.rd8(off + rp) as u16 | (self.win.rd8(off + rp + 1) as u16) << 8;
                if bytes == 4 {
                    // The low half now, the high half after it: a 4-byte sample split by an unaligned place is read in two.
                    let hi = self.win.rd8(off + (rp + 2) % size) as u32 | (self.win.rd8(off + (rp + 3) % size) as u32) << 8;
                    f(hi as u16 as i16, x as u32 | hi << 16);
                    rp = (rp + 4) % size;
                    done += 4;
                    continue;
                }
                f(x as i16, x as u32);
                rp += 2;
                done += 2;
            }
            if rp == size {
                rp = 0;
            }
        }
        r.set(UP, R_RX, rx.wrapping_add(n as u32));
        r.set(UP, R_RP, rp as u32);
        n as u32
    }

    /// The capture ring's mailbox channel.
    pub fn irq(&self) -> u32 {
        self.ring.irq()
    }
}

/// A service's header as the AoC wrote it, raw, for the probe (measure before fixing): its flags (type, channel), and each direction's offset, size, slots, tx, rx, wp and rp, with the `aoc-ipc` window's size beside them. None found: said.
pub fn dump(name: &[u8]) {
    let shown = core::str::from_utf8(name).unwrap_or("?");
    let Some(s) = find(name) else {
        say!("audio: {shown}: not in the AoC's service table");
        return;
    };
    let f = s.flags();
    say!("audio: {shown}: header at G#{:X}, flags G#{f:X} (type A#{}, channel A#{}, push {}), the window A#{} bytes", s.0, (f & TYPE_MASK) >> 16, (f >> IRQ_SHIFT) & IRQ_MASK, (f & RING_PUSH != 0) as u8, Ipc::len());
    for (d, dn) in [(UP, "UP"), (DOWN, "DOWN")] {
        let (w0, w1) = s.ring_win(d).map_or(("no granted window", ""), |m| m.names());
        say!("audio: {shown} {dn}: offset G#{:X} size A#{} slots A#{} tx A#{} rx A#{} wp A#{} rp A#{}; as a ring, in {w0} {w1}", s.get(d, R_OFFSET), s.get(d, R_SIZE), s.get(d, R_SLOTS), s.get(d, R_TX), s.get(d, R_RX), s.get(d, R_WP), s.get(d, R_RP));
    }
}

/// The capture ring as a console line: its channel, where it lies, its size.
pub fn describe_up(name: &str, s: Svc) {
    let (off, len) = s.span(UP);
    say!("audio: {name}: channel A#{}, {}, UP at G#{off:X} A#{len} bytes", s.irq(), if s.ring() { "ring" } else { "queue" });
}
