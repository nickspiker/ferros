//! The AoC's side of sound: its IPC services (the control queue `audio_output_control`, the playback ring `audio_playback5`), its mailboxes, and the audio commands, spoken from this process as Google's ALSA driver speaks them from Linux.
//!
//! Sources, every step marked with its file and line:
//! - the IPC layout and ring arithmetic: `private/google-modules/aoc_ipc/aoc_ipc_core_internal.h` and `aoc_ipc_core.c` (the husky kernel tree, /mnt/Harbor/husky-kernel), written `ipc_core.c:N` and `ipc_internal.h:N` below;
//! - the commands' layouts and ids: `aoc-interface-zuma.h` (/mnt/Harbor/ferros-ref/aoc), written `zuma.h:N`;
//! - what Linux sends and in what order: `alsa/aoc_alsa_hw.c`, `alsa/aoc_alsa_pcm.c`, `alsa/aoc_alsa.h` (same tree), written `hw.c:N`, `pcm.c:N`, `alsa.h:N`; the order as seen on husky in Android's boot log (/mnt/Harbor/tmp/linux_full_dmesg.txt:7426-7490: "bind: src:5 - sink:0", "audio_playback5 supports interrupt-driven", "be_prepare: dai TDM_0_RX");
//! - the mailboxes: `mailbox-wc.c` (ferros-ref/aoc) and `zuma-aoc.dtsi` (ferros-ref/soc-gs), written `wc.c:N`, `aoc.dtsi:N`.
//!
//! The IPC block is Device memory, reached thru the `aoc-ipc` grant's handle: every access an aligned word or a byte (payloads/sensors usf.rs does the same).

use crate::{say, S_IPC, S_MBOX};
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

/// The control queue and the playback ring, found and checked.
pub struct Services {
    pub control: Svc,
    pub playback: Svc,
}

/// The two services this process speaks, by the names Linux allocates them by (alsa.h:42 `CMD_OUTPUT_CHANNEL`; aoc_alsa_drv.c:34-74, the playback ring of entry point 5 is `audio_playback5`, Android's low-latency path on husky: dmesg:7426-7430).
pub fn services() -> Result<Services, &'static str> {
    let control = find(b"audio_output_control").ok_or("no audio_output_control service")?;
    let playback = find(b"audio_playback5").ok_or("no audio_playback5 service")?;
    if control.ring() || !control.fits(DOWN) || !control.fits(UP) {
        return Err("audio_output_control is not a queue inside the aoc-ipc window");
    }
    if !playback.ring() || !playback.fits(DOWN) {
        return Err("audio_playback5 is not a ring inside the aoc-ipc window");
    }
    // WHY: this process writes the ring a 32-bit word at a time (Device memory takes no unaligned access), so its start and its length must both be whole words; the AoC sets them, so it is checked, not assumed.
    let (off, len) = playback.span(DOWN);
    if off & 3 != 0 || len & 3 != 0 {
        return Err("audio_playback5's ring is not word-aligned");
    }
    Ok(Services { control, playback })
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

/// A command sent and its reply awaited (hw.c:196-311: stale replies drained first, the command written, then the first message back is its reply; 4 bytes back is an error code, hw.c:289-294). The reply's bytes, or why not.
///
/// The wait is a bounded sleep loop, the one place this process does that, and provisional: the AoC rings the AP on the control service's channel when it answers, but whether that channel sits in f1 (whose line this process holds) is only known on the device (`STATE` prints it). If it does, this becomes an interrupt wait; until it is measured, commands go only at a stream's start and end, never on the sound's clock.
pub fn command(s: &Services, cmd: &mut [u8], reply: &mut [u8]) -> Result<usize, &'static str> {
    let mut stale = [0u8; 64];
    while queue_read(s.control, &mut stale).is_some() {}
    // The container header's counter (zuma.h:352-356, :2782-2794: the type 0 = DATA_TYPE_CMD, a counter, the whole length).
    cmd[1] = next_counter();
    if !queue_write(s.control, cmd) {
        return Err("the control queue is full");
    }
    let f = freq();
    for _ in 0..REPLY_STEPS {
        if let Some(n) = queue_read(s.control, reply) {
            if n == 4 {
                return Err("the AoC answered with an error code");
            }
            return Ok(n);
        }
        sleep_until(now() + f / 1024);
    }
    Err("the AoC did not answer in 512 ms")
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
    let mut c = [0u8; 28];
    header(&mut c, CMD_EP_SETUP);
    c[8..12].copy_from_slice(&0u32.to_le_bytes());
    c[12..16].copy_from_slice(&48_000u32.to_le_bytes());
    c[16..20].copy_from_slice(&0u32.to_le_bytes());
    c[20] = 2;
    c[21] = 1;
    c[22] = 5;
    c[23] = 1;
    c[24] = 0;
    c[25] = EP;
    c[26] = 1;
    c[27] = mode;
    command(s, &mut c, &mut [0u8; 64])?;
    let mut c = [0u8; 18];
    header(&mut c, CMD_EP_SETUP2);
    c[8] = EP;
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
    c[8] = EP;
    let mut r = [0u8; 64];
    let n = command(s, &mut c, &mut r)?;
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
            wr32(off + wp, f);
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

// ── The AoC-to-AP line (wc.c:150-175) ──

/// The f1 block's channels this process listens to unmasked, every other one of the block masked (wc.c:122-134: a clear bit is unmasked): only this process uses f1's line on ferros, so the line rises for its channels alone.
pub fn listen(channels: u16) {
    let g = cap(crate::grants(), S_MBOX);
    mmio_write(g, MBOX_BLOCKS[F1 as usize] + MB_INTMR1, !channels as u32 & 0xFFFF);
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
