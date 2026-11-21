//! The GPU queue (docs/GPU-KISTS.md "The queue", 2026-11-20; Lumis's ask 3, 7 and 8: "a submit ring the kist writes from its own thread, a completion word plus a doorbell it waits on, with no service round trip per job"; per-job GPU time; faults with address and kind).
//!
//! A client's queue is a ring in its own GPU block, at an offset it chooses (a multiple of `PAGE`):
//! - the header page: `SUBMIT`, the jobs the client has submitted, which it alone writes; `DONE`, the jobs the GPU service has ended, which the service alone writes; `ENDED`, why the queue ended (0 while it runs).
//! - then 2^depth records of `RECORD` bytes, job n in record n mod 2^depth.
//!
//! A job: the client fills its record (the kernel, the grid, the uniforms), stores `SUBMIT` = n + 1 and wakes that word (SYS_WAKE on a word of its block).
//! The service's thread for the queue, waiting on `SUBMIT`, writes the job's command-stream words (`job_words`) into the client's own command stream and rings its doorbell.
//! The GPU stamps the job's start and end into the record (`STARTED`, `ENDED_AT`), and the service, at the job's interrupt, writes its status, `DONE` = n + 1 and wakes that word, which the client waits on.
//! The client never calls the service for a job: one call to open the queue, one a kernel it loads, one to close.
//!
//! Every word is touched only as an atomic u64 (two processes and the GPU share them), the client's stores to its record ordered before its `SUBMIT` (release), the service's result stores before its `DONE` (release).
//! The client may submit while at most 2^depth of its jobs are not yet ended (`room`): a record is its again once its job's `DONE` has passed it.
//!
//! No VSF here: the queue is memory a process and the GPU service share while it runs, never stored or sent (as a mailbox is).

#![no_std]

use core::sync::atomic::{AtomicU64, Ordering};

/// A page: the header's size, and the alignment of a queue in its block.
pub const PAGE: usize = 1 << 12;
/// A job's record, in bytes.
pub const RECORD: usize = 1 << 10;
/// The deepest queue: 2^4 jobs not yet ended. The command stream runs a queue's jobs one after another, so depth buys only the client's freedom to run ahead.
pub const DEPTH_MOST_LOG2: u32 = 4;
/// The most uniform words (u64) a job passes: the GPU service's 64 slots.
pub const UNIFORMS_MOST: usize = 1 << 6;

/// The header's words, by u64 index: the jobs submitted (the client's), the jobs ended (the service's) and why the queue ended (the service's; 0 while it runs), the last two in their own cache line away from the client's.
pub const SUBMIT: usize = 0;
pub const DONE: usize = 8;
pub const ENDED: usize = 9;
/// Bit 63 of `SUBMIT` or `DONE`: an end the other side cannot see coming.
/// On `SUBMIT`, set by the kernel at the client's end (the queue word the service named when it held the client's GPU work, ferros_kernel `gpu_hold`) or by the service at a close: its thread waiting there wakes to it.
/// On `DONE` (`SUBMIT`'s word plus 64 bytes), set by the kernel at the service's end: the client waiting there wakes to it (`ended` says `E_SERVICE`).
pub const GONE: u64 = 1 << 63;

/// A record's words, by u64 index from the record's start.
/// - `KERNEL`: the kernel's handle (from the service's load) in bits 0-7, the uniforms it passes in bits 32-63.
/// - `GRID`: workgroups in x (bits 0-31; 16 invocations each) and y (bits 32-63).
/// - `UNIFORMS`: the uniform words, read by the GPU from the record itself (no copy).
/// - the result's line: `STATUS`, the GPU's own stamps of the job's start and end (`STARTED`, `ENDED_AT`, counter ticks once the service has ended the job), when the service rang the doorbell (`SUBMITTED`) and saw it end (`SEEN`), and a fault's GPU address and the GPU MMU's status (`FAULT_ADDRESS`, `FAULT_STATUS`).
pub const KERNEL: usize = 0;
pub const GRID: usize = 1;
pub const UNIFORMS: usize = 8;
pub const STATUS: usize = 72;
pub const STARTED: usize = 73;
pub const ENDED_AT: usize = 74;
pub const SUBMITTED: usize = 75;
pub const SEEN: usize = 76;
pub const FAULT_ADDRESS: usize = 77;
pub const FAULT_STATUS: usize = 78;
/// The result's line in bytes from the record's start: the GPU stamps `STARTED` and `ENDED_AT` at this base plus 8 and 16 (`job_words`).
pub const RESULT_BYTES: usize = STATUS * 8;
const _: () = assert!(UNIFORMS + UNIFORMS_MOST <= STATUS && FAULT_STATUS * 8 < RECORD && RESULT_BYTES % 64 == 0, "the record's fields lie inside it, the result in a line of its own");

/// A job's status (`STATUS`): ended whole; failed in the GPU (a command-stream fault, or its guard passed: the service's console says which); reached outside the client's memory (`FAULT_ADDRESS`, `FAULT_STATUS`); lost (never run: the queue ended before it, by an earlier job's failure or a close); refused (its record names no loaded kernel, an empty grid or too many uniforms).
/// The numbers are the GPU service's run replies' (status 4 is ferros_sys::gpu `OUTSIDE` there too).
pub const S_DONE: u64 = 0;
pub const S_FAILED: u64 = 2;
pub const S_OUTSIDE: u64 = 4;
pub const S_LOST: u64 = 6;
pub const S_REFUSED: u64 = 7;

/// Why a queue ended (`ENDED`): a job of its failed (every later one lost); it was closed; the client submitted past its depth or named a record it could not have filled; the service let it go (restarted, or its slot taken); its camera view went with a job in flight.
pub const E_FAULT: u64 = 1;
pub const E_CLOSED: u64 = 2;
pub const E_OVERRUN: u64 = 3;
pub const E_SERVICE: u64 = 4;
/// The camera view the queue's client held was taken back (its CLOSE, or a view given anew) while a job of it was in flight: the service stops it before it unmaps the view (docs/GPU-KISTS.md "The camera ring on the GPU"); wait for the queue's jobs before a CLOSE to keep it running.
pub const E_CAMERA: u64 = 5;

/// The queue's bytes at depth 2^`depth_log2`: its header and records.
pub const fn bytes(depth_log2: u32) -> usize {
    PAGE + (RECORD << depth_log2)
}

/// Job `n`'s record's byte offset from the queue's start.
pub const fn record(n: u64, depth_log2: u32) -> usize {
    PAGE + RECORD * (n as usize & ((1 << depth_log2) - 1))
}

/// A queue as the client or the service sees it: its words, whatever memory they are in (a GPU block's window, or a host test's buffer).
#[derive(Clone, Copy)]
pub struct Ring<'a> {
    words: &'a [AtomicU64],
    depth_log2: u32,
}

/// A job as the client asks for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Job<'u> {
    pub kernel: u8,
    pub groups: (u32, u32),
    pub uniforms: &'u [u64],
}

/// A job as it ended (`Ring::result`): its status, and its times in counter ticks (CNTVCT, `freq()` a second).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ended {
    pub status: u64,
    /// When the service rang the doorbell for it, and when it saw it end (its interrupt).
    pub submitted: u64,
    pub seen: u64,
    /// The GPU's own stamps of its start (its command stream reaching it, after the jobs before it in the queue) and its end (its last invocation done), 0 when the GPU stamped none (a job that never ran, or a GPU without the stamps).
    pub started: u64,
    pub ended: u64,
    /// Where it reached outside its memory, and the GPU MMU's status (status `S_OUTSIDE`).
    pub fault_address: u64,
    pub fault_status: u64,
}

impl Ended {
    /// The GPU's time on the job, in counter ticks (`ended - started`), when the GPU stamped both.
    pub fn gpu_ticks(&self) -> Option<u64> {
        (self.started != 0 && self.ended >= self.started).then(|| self.ended - self.started)
    }
    /// From the doorbell to the service's seeing its end, in counter ticks: what a client waits beyond the GPU's own time, plus the jobs ahead of it.
    pub fn round_ticks(&self) -> u64 {
        self.seen.wrapping_sub(self.submitted)
    }
}

impl<'a> Ring<'a> {
    /// The queue over `words` (at least `bytes(depth_log2) / 8` of them, the queue's start first).
    pub fn new(words: &'a [AtomicU64], depth_log2: u32) -> Option<Ring<'a>> {
        (depth_log2 <= DEPTH_MOST_LOG2 && words.len() * 8 >= bytes(depth_log2)).then_some(Ring { words, depth_log2 })
    }

    pub fn depth_log2(&self) -> u32 {
        self.depth_log2
    }

    fn rec(&self, n: u64, w: usize) -> &AtomicU64 {
        &self.words[record(n, self.depth_log2) / 8 + w]
    }

    /// `SUBMIT`, `DONE` and `ENDED` now.
    pub fn submitted(&self) -> u64 {
        self.words[SUBMIT].load(Ordering::Acquire) & !GONE
    }
    pub fn done(&self) -> u64 {
        self.words[DONE].load(Ordering::Acquire) & !GONE
    }
    /// Why the queue ended, 0 while it runs: `E_SERVICE` once the kernel has marked `DONE` (the service ended).
    pub fn ended(&self) -> u64 {
        if self.words[DONE].load(Ordering::Acquire) & GONE != 0 {
            return E_SERVICE;
        }
        self.words[ENDED].load(Ordering::Acquire)
    }
    /// Whether `SUBMIT` carries `GONE`: the client ended (the kernel's mark) or the service closed the queue (its own). The service's thread looks at this when it wakes with no new job.
    pub fn submit_gone(&self) -> bool {
        self.words[SUBMIT].load(Ordering::Acquire) & GONE != 0
    }
    /// `SUBMIT` marked `GONE` by the service (a close, the slot let go), so its thread waiting there wakes and sees it, even when the mark lands between its look and its wait. The caller wakes `submit_word` after.
    pub fn mark_closed(&self) {
        self.words[SUBMIT].fetch_or(GONE, Ordering::SeqCst);
    }
    /// The raw `SUBMIT` and `DONE` words, for a futex wait's expected value.
    pub fn submit_raw(&self) -> u64 {
        self.words[SUBMIT].load(Ordering::Acquire)
    }
    pub fn done_raw(&self) -> u64 {
        self.words[DONE].load(Ordering::Acquire)
    }
    /// The words the client's doorbell and the service's are: the client wakes `SUBMIT` and waits on `DONE`, the service the other way round.
    pub fn submit_word(&self) -> &'a AtomicU64 {
        &self.words[SUBMIT]
    }
    pub fn done_word(&self) -> &'a AtomicU64 {
        &self.words[DONE]
    }

    /// Whether the client may submit job `n` now: fewer than 2^depth of its jobs are not yet ended.
    pub fn room(&self, n: u64) -> bool {
        n.wrapping_sub(self.done()) < 1 << self.depth_log2
    }

    // The client's side.

    /// Job `n` written into its record and submitted (`SUBMIT` = n + 1, released after the record's words): false, and nothing written, when there is no room for it, it has too many uniforms, or the queue has ended.
    /// The caller then wakes `submit_word` (SYS_WAKE), which is the service's doorbell.
    pub fn submit(&self, n: u64, job: &Job) -> bool {
        if !self.room(n) || job.uniforms.len() > UNIFORMS_MOST || self.ended() != 0 || self.submit_gone() {
            return false;
        }
        self.rec(n, KERNEL).store(job.kernel as u64 | (job.uniforms.len() as u64) << 32, Ordering::Relaxed);
        self.rec(n, GRID).store(job.groups.0 as u64 | (job.groups.1 as u64) << 32, Ordering::Relaxed);
        for (k, &u) in job.uniforms.iter().enumerate() {
            self.rec(n, UNIFORMS + k).store(u, Ordering::Relaxed);
        }
        for w in STATUS..=FAULT_STATUS {
            self.rec(n, w).store(0, Ordering::Relaxed);
        }
        self.words[SUBMIT].store(n + 1, Ordering::Release);
        true
    }

    /// Job `n`'s result, once `DONE` has passed it (None before).
    /// It lies in job n's record, which job n + 2^depth takes: read it before submitting that one.
    pub fn result(&self, n: u64) -> Option<Ended> {
        if self.done().wrapping_sub(n) == 0 || self.done().wrapping_sub(n) > 1 << 63 {
            return None;
        }
        let r = |w: usize| self.rec(n, w).load(Ordering::Relaxed);
        Some(Ended { status: r(STATUS), submitted: r(SUBMITTED), seen: r(SEEN), started: r(STARTED), ended: r(ENDED_AT), fault_address: r(FAULT_ADDRESS), fault_status: r(FAULT_STATUS) })
    }

    // The service's side.

    /// Job `n`'s request as the client wrote it: (kernel handle, uniforms, groups), read after its `SUBMIT` (acquire).
    /// Nothing here is checked: the client wrote it, and the service checks every field before a word of the command stream is written (`check`).
    pub fn request(&self, n: u64) -> (u64, u64, (u32, u32)) {
        let k = self.rec(n, KERNEL).load(Ordering::Relaxed);
        let g = self.rec(n, GRID).load(Ordering::Relaxed);
        (k & 0xFF, k >> 32, (g as u32, (g >> 32) as u32))
    }

    /// The job's result written (its `STARTED` and `ENDED_AT` the GPU's raw stamps less `stamp_offset`, the GPU's timestamp's lead on the CPU's counter; left 0 when the GPU stamped nothing), then nothing more: `DONE` moves by `finish`.
    pub fn end(&self, n: u64, status: u64, submitted: u64, seen: u64, fault: (u64, u64), stamp_offset: u64) {
        let w = |k: usize, v: u64| self.rec(n, k).store(v, Ordering::Relaxed);
        let raw = |k: usize| self.rec(n, k).load(Ordering::Relaxed);
        let (s, e) = (raw(STARTED), raw(ENDED_AT));
        w(STARTED, if s != 0 { s.wrapping_sub(stamp_offset) } else { 0 });
        w(ENDED_AT, if e != 0 { e.wrapping_sub(stamp_offset) } else { 0 });
        w(STATUS, status);
        w(SUBMITTED, submitted);
        w(SEEN, seen);
        w(FAULT_ADDRESS, fault.0);
        w(FAULT_STATUS, fault.1);
    }

    /// `DONE` = `n` (every job before `n` ended, its result written: released after them), and `ENDED` = `why` when the queue ended.
    pub fn finish(&self, n: u64, why: u64) {
        if why != 0 {
            self.words[ENDED].store(why, Ordering::Release);
        }
        self.words[DONE].store(n, Ordering::Release);
    }

    /// At the queue's open, by the service: `DONE` caught up with `SUBMIT` and `ENDED` cleared; the jobs from `SUBMIT` on are the ones it serves.
    pub fn open(&self) -> u64 {
        let s = self.submitted();
        // A close's or an end's `GONE` cleared: the client is in its open call, so nothing else writes `SUBMIT` now.
        self.words[SUBMIT].store(s, Ordering::Relaxed);
        self.words[ENDED].store(0, Ordering::Relaxed);
        self.words[DONE].store(s, Ordering::Release);
        s
    }

    /// Job `n`'s record's byte offset from the queue's start (its uniforms at + `UNIFORMS * 8`, its result line at + `RESULT_BYTES`).
    pub fn record_offset(&self, n: u64) -> usize {
        record(n, self.depth_log2)
    }
}

/// What the service checks of a request before it writes a command-stream word (the client wrote it): a loaded kernel, at most `UNIFORMS_MOST` uniforms, a grid of at least one workgroup each way and at most 2^20 workgroups in x and in y (2^24 invocations a row: beyond any layer or frame, and inside the 32-bit counts the command stream takes).
pub fn check(kernel: u64, uniforms: u64, groups: (u32, u32), kernels: u64) -> bool {
    kernel < kernels && uniforms as usize <= UNIFORMS_MOST && groups.0 > 0 && groups.1 > 0 && groups.0 <= 1 << 20 && groups.1 <= 1 << 20
}

// The command stream (CSF, Mali-G715): each instruction a u64, the opcode in bits 63:56 (Mesa's v11 genxml, the words the device ran since 2026-11-04, GPU.md).

/// MOVE48: register pair `r` = a 48-bit immediate.
pub const fn mv48(r: u64, v: u64) -> u64 {
    0x01 << 56 | r << 48 | (v & 0xFFFF_FFFF_FFFF)
}
/// MOVE32: register `r` = a 32-bit immediate.
pub const fn mv32(r: u64, v: u32) -> u64 {
    0x02 << 56 | r << 48 | v as u64
}
/// STORE_STATE (opcode 40, genxml "CS STORE_STATE"): the state `state` (0 the timestamp) stored as a u64 at the register pair `reg` plus `offset`, synchronous (wait mask 0, so no scoreboard slot: Mesa's `cs_instr_is_asynchronous`).
pub const fn store_state(reg: u64, offset: i16, state: u64) -> u64 {
    0x28 << 56 | reg << 40 | state << 32 | offset as u16 as u64
}
/// The register pair a job's stamps are stored thru (`job_words`): r76:r77, past the r70-r74 the job's end uses and inside the 96 the firmware reports (the service checks CS_FEATURES before it asks for stamps).
pub const STAMP_REG: u64 = 76;
/// The registers `job_words` uses, from 0: the highest is the stamp pair's.
pub const REGS_USED: u32 = STAMP_REG as u32 + 2;

/// A dispatch as the command stream runs it: the shader program descriptor's and the thread storage descriptor's GPU addresses, the uniforms' (and how many), the grid, the cores' thread limit (THREAD_MAX_THREADS, which sizes a task), where to stamp the start and end (a result line, or none), and the sync word the stream sets to `value` at the end.
#[derive(Clone, Copy, Debug)]
pub struct Dispatch {
    pub spd: u64,
    pub tsd: u64,
    pub fau: u64,
    pub uniforms: u32,
    pub groups: (u32, u32),
    pub max_threads: u32,
    pub stamps: Option<u64>,
    pub sync: u64,
    pub value: u32,
    /// The GPU's L2 and load-store caches cleaned and invalidated before the run (FLUSH_CACHE2 mode 3 for both, and its WAIT): a job reading memory a device writes without snooping thru a cacheable mapping (the camera ring cacheable, measured against non-cacheable: docs/GPU-KISTS.md "The camera ring on the GPU").
    pub invalidate: bool,
}

/// The most words `job_words` writes.
pub const JOB_WORDS_MOST: usize = 32;

/// The job's command-stream words into `out`; how many.
/// - REQ_RESOURCE compute; the scoreboard entries (endpoint 3, other 0); SRT 0, the FAU (address | count << 56), SPD, TSD; workgroup 16 x 1 x 1 with merging; job offsets 0; the grid.
/// - With stamps: r76:r77 = the result line, its timestamp at + 8 before the run.
/// - RUN_COMPUTE (the task's axis and size as Mesa's gallium chooses them, pan_csf.c: along X while the threads fit the core's limit, a task of every workgroup when they all fit; a row a task for a grid of more than one row); WAIT on its entry.
/// - With stamps: the timestamp at + 16, after the run's WAIT.
/// - FLUSH_CACHE2 (clean L2 and LSC, invalidate the others) and its WAIT; SYNC_SET32 the sync word = `value`, whose update raises the group's interrupt.
pub fn job_words(d: &Dispatch, out: &mut [u64; JOB_WORDS_MOST]) -> usize {
    let (axis, inc) = if d.groups.1 > 1 {
        (1u64, 1u64)
    } else if 16 * d.groups.0 >= d.max_threads {
        (0, (d.max_threads / 16) as u64)
    } else {
        (2, 1)
    };
    let mut n = 0;
    let mut put = |w: u64| {
        out[n] = w;
        n += 1;
    };
    put(0x2200_0000_0000_0001);
    put(0x1C00_0000_0000_0003);
    put(0x1C00_0001_0000_0000);
    put(mv48(0, 0));
    put(mv32(8, d.fau as u32));
    put(mv32(9, (d.fau >> 32) as u32 | d.uniforms << 24));
    put(mv48(16, d.spd));
    put(mv48(24, d.tsd));
    put(mv32(32, 0));
    put(mv32(33, 0x8000_000F));
    put(mv32(34, 0));
    put(mv32(35, 0));
    put(mv32(36, 0));
    put(mv32(37, d.groups.0));
    put(mv32(38, d.groups.1));
    put(mv32(39, 1));
    if d.invalidate {
        put(mv32(74, 0));
        put(0x2400_4A00_0000_0233);
        put(0x0300_0000_0001_0000);
    }
    if let Some(at) = d.stamps {
        put(mv48(STAMP_REG, at));
        put(store_state(STAMP_REG, 8, 0));
    }
    put(0x04 << 56 | axis << 14 | inc);
    put(0x0300_0000_0008_0000);
    if d.stamps.is_some() {
        put(store_state(STAMP_REG, 16, 0));
    }
    put(mv32(74, 0));
    put(0x2400_4A00_0000_0211);
    put(0x0300_0000_0001_0000);
    put(mv48(70, d.sync));
    put(mv32(72, d.value));
    put(0x2600_4648_0000_0000);
    n
}

/// A layer's tile coverage (COMPOSITOR.md rule 2; docs/GPU-KISTS.md "Layers", 2026-11-20): a byte a tile of 2^`TILE_LOG2` pixels square, row by row, the screen's tiles across (`tiles_across`) times down, in the layer's surface `AT` bytes from the slot's start (past the pixels on both boards: the Pro's 1344 × 2992 at 8 bytes a pixel end 30.7 MiB in).
/// A kernel counts each tile's pixels that hold anything and that are opaque (an atomic add a pixel, or one an invocation over its own pixels), and `class` makes the byte from the two counts; `CHANGED` set where the tile differs from the buffer's last publish.
/// The display reads it with the buffer's show (not yet: the display composes a layer by its window's rectangle and the show's changed rectangle today).
pub mod coverage {
    pub const TILE_LOG2: u32 = 6;
    pub const AT: usize = (32 << 20) - (64 << 10);
    pub const EMPTY: u8 = 0;
    pub const PARTIAL: u8 = 1;
    pub const OPAQUE: u8 = 2;
    pub const CHANGED: u8 = 1 << 7;

    /// The tiles across a screen `width` pixels wide (a partial tile at the right edge counts).
    pub const fn tiles_across(width: usize) -> usize {
        (width + (1 << TILE_LOG2) - 1) >> TILE_LOG2
    }

    /// A tile's class from its pixels that hold anything (`any`) and that are opaque (`opaque`) of the `pixels` it covers on the screen.
    pub const fn class(any: u32, opaque: u32, pixels: u32) -> u8 {
        if any == 0 {
            EMPTY
        } else if opaque == pixels {
            OPAQUE
        } else {
            PARTIAL
        }
    }
}
