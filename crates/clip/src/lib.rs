//! RAW video clips on ferros (docs/RAW-VIDEO.md "The recorder"): what Lumis hands its camera's frames to.
//!
//! - **Coding.** Each frame's bands (2^6 rows, `ferros_rawcodec`, lossless) are coded by a pool of worker threads, each pinned to the cores it is given (the A715s, the A510s, both), taking bands from a shared count so faster cores take more.
//!   [`Recorder::frame`] returns once every band is coded: the camera's buffer is read in place and may be given back then.
//! - **Writing.** A writer thread lays the clip out as one VSF document (`vsf::visual::clip`: generation 0, the frames, the closing generation) and writes it in whole 4 KiB blocks, a UFS command (1 MiB) at a time, to a [`Sink`]: on ferros a single-copy extent of the vault ([`ferros::ExtentSink`], `SYS_STORE` extents: one copy, at the flash's rate, WriteBooster on); on a host a vector ([`MemSink`]).
//!   Coded frames wait for it in `slots`; a frame that finds none free is dropped and counted, never waited for (the camera would skip frames meanwhile anyway).
//! - **Pacing.** The frame interval is the longer of 1 / fps and the shutter: [`Pacing::of`] gives the camera's frame length for any whole rate 1 to 60 (the driver lengthens a frame by itself when the shutter is longer), and the recorder keeps every frame or, below the sensor's slowest frame, every m-th by time.
//! - **The end.** [`Recorder::stop`] writes the closing generation and seals the extent with the clip's length and the BLAKE3 of its closing generation (which names every frame by its identity, and each frame its samples). A clip cut short stays readable to its last whole frame (vsf `clip::Reader` walks its chain) and can be sealed after.

#[cfg(target_os = "ferros")]
pub mod ferros;
/// The band coder's first half as a GPU kernel (the 60/s work): `gpu::kernel`, its words built on the host (`gpu::PREBUILT`), on ferros the GPU service's run (`gpu::Device`), and ferros_rawcodec `pack_band_strided` for the CPU's half.
pub mod gpu;
/// The recorder's sink as an item of the hold (`items::ItemSink`, docs/HOLD-API.md): the clip a card in the person's hold, found while it records.
pub mod items;
/// Stacked clips: frames of exact sums, packed at the width each needs (vsf ClipCodec::Packed).
pub mod packed;

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use ferros_rawcodec as rc;
use vsf::types::Tensor;
use vsf::visual::clip::{self, ClipSpec, FrameMeta, Mosaic};

/// Bytes a block of the flash.
pub const BLOCK: usize = 1 << 12;
/// Bytes a write takes at most: one UFS command (the kernel's EXTENT_MAX).
pub const WRITE_MAX: usize = 1 << 20;

/// The binned main's sensor clock (24.576 MHz / 3 x 245, ferros_cam ccs: 2^16 5^4 7^2 Hz) and its line in clocks (llp): every frame is a whole number of these lines.
pub const MAIN_PIXCLK: u64 = 2_007_040_000;
pub const MAIN_LLP: u64 = 6320;
/// The longest frame the sensor's 16-bit frame length holds, and the shortest the binned main streams (its 3072 rows and the driver's blank).
pub const FLL_MOST: u64 = (1 << 16) - 1;
pub const FLL_LEAST: u64 = 3072 + (1 << 8);
/// The lines the driver keeps between an integration and its frame's end at the open (payloads/camera, ccs `MARGIN`).
pub const OPEN_MARGIN: u64 = 1 << 5;

/// How the camera runs for a clip at `fps`: the frame length the open asks for, and every how many frames the recorder keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pacing {
    pub fps: u32,
    /// The sensor's frame length in lines (the open's mode block `fll`).
    pub fll: u16,
    /// The sensor runs at `keep` times the clip's rate when one frame length cannot hold 1 / fps (below 5 a second); the recorder keeps every `keep`-th frame, by time.
    pub keep: u32,
    /// The sensor's frame and the clip's, nanoseconds, as the clock makes them (whole lines).
    pub sensor_ns: u64,
    pub period_ns: u64,
    /// The readout's line the frame is counted in (the binned main's [`MAIN_LINE`] for [`Pacing::of`]).
    pub line: Line,
}

/// A readout's line, for pacing any camera's binned readout (2026-11-21, the promo's four cameras): the line's time as `num` / `den` seconds, and the shortest frame the driver streams whole frames of it at.
/// The binned main's is exact ([`MAIN_LINE`]: 6320 clocks of 2,007,040,000 Hz); any camera's from its list section ([`Line::of_ps`]: the readout's `line_ps` and `fll_least`, ferros_cam list), the line measured in picoseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Line {
    pub num: u64,
    pub den: u64,
    /// The shortest frame in lines: the readout's rows and the least blanking the driver asks (payloads/camera refuses a frame length SET under it).
    pub fll_least: u64,
}

/// The binned main's line, exact.
pub const MAIN_LINE: Line = Line { num: MAIN_LLP, den: MAIN_PIXCLK, fll_least: FLL_LEAST };

impl Line {
    /// A line of `ps` picoseconds (the list's `line_ps`) whose frames are at least `fll_least` lines (the list's `fll_least`).
    pub const fn of_ps(ps: u64, fll_least: u64) -> Line {
        Line { num: ps, den: 1_000_000_000_000, fll_least }
    }

    /// The fastest whole rate, 1 to 60, a whole frame of this readout runs at (the Pro, from the list: the main 60, the ultrawide 48, the tele 50, the front 33); 0 none.
    pub fn fps_most(&self) -> u32 {
        (1..=60).rev().find(|&f| Pacing::of_line(f, *self).is_some()).unwrap_or(0)
    }

    /// Camera `id`'s readout `mode` (its list section's `mode`, 0 the binned) from the camera driver's LIST reply (a VSF document, docs/CAMERA.md "The endpoint"): its `line_ps` and `fll_least`. None when the list has no such readout or a driver from before 2026-11-21 lists neither.
    pub fn of_list(list: &[u8], id: u8, mode: u8) -> Option<Line> {
        let d = vsf::container::Document::open(list).ok()?;
        for e in d.entries_named("camera") {
            let Ok(s) = d.section(e) else { continue };
            if s.uint("id") == Some(id as u64) && s.uint("mode").unwrap_or(0) == mode as u64 {
                return Some(Line::of_ps(s.uint("line_ps")?, s.uint("fll_least")?));
            }
        }
        None
    }

    /// Whole lines in `ns`, rounded down (in 128 bits: a picosecond line's 10^12 times a shutter of seconds passes 2^64).
    pub fn lines_in(&self, ns: u64) -> u64 {
        (ns as u128 * self.den as u128 / (self.num as u128 * 1_000_000_000)) as u64
    }

    /// `lines` of this line in nanoseconds, rounded down.
    pub fn ns_of(&self, lines: u64) -> u64 {
        (lines as u128 * self.num as u128 * 1_000_000_000 / self.den as u128) as u64
    }

    /// The nearest whole frame length to one frame at `rate` a second.
    fn frame_at(&self, rate: u64) -> u64 {
        let per = self.num as u128 * rate as u128;
        ((self.den as u128 + (per >> 1)) / per) as u64
    }
}

impl Pacing {
    /// The binned main's pacing for a whole rate of 1 to 60 a second (the nearest frame length; the sensor's clock has factors 2, 5 and 7, so a rate with a factor 3 is never exact, and the others only where llp would divide too).
    pub fn of(fps: u32) -> Pacing {
        assert!((1..=60).contains(&fps), "fps {fps} outside 1 to 60");
        Pacing::of_line(fps, MAIN_LINE).expect("the binned main holds every rate 1 to 60")
    }

    /// Any readout's pacing for a whole rate of 1 to 60 a second, from its line (`Line::of_ps` of its list section's `line_ps` and `fll_least`): the nearest frame length, the sensor running at a multiple below the rate its 16-bit frame holds.
    /// None: a rate outside 1 to 60, or faster than a whole frame of this readout runs (its nearest frame length under `fll_least`; [`Line::fps_most`] says the fastest).
    pub fn of_line(fps: u32, line: Line) -> Option<Pacing> {
        if !(1..=60).contains(&fps) {
            return None;
        }
        let mut keep = 1u64;
        while line.frame_at(fps as u64 * keep) > FLL_MOST {
            keep += 1;
        }
        let fll = line.frame_at(fps as u64 * keep);
        // WHY: a frame shorter than the readout's rows and least blanking is one the driver refuses (its SET) or the front end cannot run the next frame's program in (LATE_CONFIG); the rate is refused here, never capped to the fastest.
        if fll < line.fll_least {
            return None;
        }
        let sensor_ns = line.ns_of(fll);
        Some(Pacing { fps, fll: fll as u16, keep: keep as u32, sensor_ns, period_ns: sensor_ns * keep, line })
    }

    /// The camera driver's open with its view for this pacing (payloads/camera `Spec`): `[1][camera][1 = with the view][kind 1, the binned readout][rows][fll][exposure lines][flags][ring][llp][cols][view flags: bit 0 write-back]`.
    /// `exposure_ns` the opening shutter, at most the frame less the driver's margin (`ccs::Mode::check`: an integration of more refuses the open, "no such mode (Integration(..))"); a shutter longer than the frame is the caller's SET after the first frame, which lengthens the frame (the interval is then the shutter's). [`Pacing::open_needs_set`] says which.
    /// Whether a shutter of `exposure_ns` is longer than the open can carry: then SET it (exposure, what 0) after the first frame.
    pub fn open_needs_set(&self, exposure_ns: u64) -> bool {
        self.line.lines_in(exposure_ns) > self.fll as u64 - OPEN_MARGIN
    }

    pub fn open_msg(&self, camera: u8, exposure_ns: u64) -> [u8; 17] {
        // WHY the min: the driver refuses an open whose integration does not fit its frame less the margin (`ccs::Mode::check`, coarse + 32 <= fll); the longer shutter goes by SET once the stream runs (`open_needs_set`). PROOF: Lumis's 67.8 ms shutter at 30/s (21342 lines in a 10586-line frame) was refused, 2026-11-19.
        let lines = self.line.lines_in(exposure_ns).max(1).min(self.fll as u64 - OPEN_MARGIN) as u16;
        let mut m = [0u8; 17];
        m[0] = 1;
        m[1] = camera;
        m[2] = 1;
        m[3] = 1;
        m[6..8].copy_from_slice(&self.fll.to_le_bytes());
        m[8..10].copy_from_slice(&lines.to_le_bytes());
        m[16] = 1;
        m
    }

    /// The camera driver's SET of this pacing's frame length on a camera already streaming the binned readout (payloads/camera SET what 4; owner, 2026-11-20: still to RAW video without the 5 s reopen): `[3][4][fll: u64]`.
    /// Answered `[0]` taken, `[1]` refused (a frame under the readout's rows and blanking, or an exposure no plan holds against it).
    /// It lands at the next frame's start, in the sensor's hold as an exposure SET does; the frame then is the longer of this frame and the shutter with the sensor's margin (max(1 / fps, shutter)), as at a paced open.
    /// The frame in flight when it is answered keeps the old length: the frames at this pacing are those counted [`SET_SETTLES`] or more past the count the last frame before the SET carried.
    pub fn set_msg(&self) -> [u8; 10] {
        set_fll_msg(self.fll as u64)
    }
}

/// The SET that gives a streaming camera back the frame length its OPEN set (`[3][4][0]`): leaving RAW video for stills.
pub fn set_open_fll_msg() -> [u8; 10] {
    set_fll_msg(0)
}

/// The camera driver's SET of frame length `fll` lines (0: the OPEN's own).
pub fn set_fll_msg(fll: u64) -> [u8; 10] {
    let mut m = [0u8; 10];
    m[0] = 3;
    m[1] = 4;
    m[2..10].copy_from_slice(&fll.to_le_bytes());
    m
}

/// Frames past the last one before a frame length SET until the first at the new length: the frame in flight finishes at the old length (one), the next begins at the new.
pub const SET_SETTLES: u64 = 2;

/// Where a clip's bytes go: whole 4 KiB blocks, at most [`WRITE_MAX`] a call, from block 0 up.
pub trait Sink: Send {
    /// Make room for `blocks` blocks; the blocks it got.
    fn open(&mut self, blocks: u64) -> Result<u64, String>;
    fn write(&mut self, block: u64, bytes: &[u8]) -> Result<(), String>;
    /// The clip is `len` bytes long; `hash` its seal.
    fn seal(&mut self, len: u64, hash: [u8; 32]) -> Result<(), String>;
}

/// A host's sink: the clip in memory.
#[derive(Default)]
pub struct MemSink {
    pub bytes: Arc<Mutex<Vec<u8>>>,
    pub sealed: Arc<Mutex<Option<(u64, [u8; 32])>>>,
    blocks: u64,
}

impl Sink for MemSink {
    fn open(&mut self, blocks: u64) -> Result<u64, String> {
        self.blocks = blocks;
        Ok(blocks)
    }
    fn write(&mut self, block: u64, bytes: &[u8]) -> Result<(), String> {
        assert!(bytes.len() % BLOCK == 0 && bytes.len() <= WRITE_MAX && block + (bytes.len() / BLOCK) as u64 <= self.blocks);
        let mut b = self.bytes.lock().unwrap();
        let at = block as usize * BLOCK;
        if b.len() < at + bytes.len() {
            b.resize(at + bytes.len(), 0);
        }
        b[at..at + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }
    fn seal(&mut self, len: u64, hash: [u8; 32]) -> Result<(), String> {
        *self.sealed.lock().unwrap() = Some((len, hash));
        Ok(())
    }
}

/// What a camera's frames are: their geometry and the facts every frame shares (Lumis fills it from the camera's LIST and the MAP header).
#[derive(Debug, Clone)]
pub struct Camera {
    pub width: usize,
    pub height: usize,
    /// Bytes a line of the packed RAW10 buffer.
    pub stride: usize,
    /// The colour of each cell, row by row, of a 2 x 2 (Bayer) or 4 x 4 (quad) cell: 0 R, 1 G, 2 B.
    pub cfa: Vec<u8>,
    pub black: u16,
    pub white: u16,
    pub make: String,
    pub model: String,
    pub sensor_module: String,
    pub lens: String,
    pub f_number: Option<f32>,
    pub focal_mm: Option<f32>,
    pub characterization: Option<vsf::spectral_image::ColourProfile>,
    pub observer: String,
    pub orientation: u16,
}

/// The MAP header's word for the mosaic (`cfa` byte: two bits a cell, cell 0 lowest, of a 2 x 2 cell): the cells' colours.
pub fn cfa_of_byte(b: u8) -> Vec<u8> {
    (0..4).map(|i| (b >> (2 * i)) & 3).collect()
}

/// The recording space (`ferros::space`, the vault's EOP_SPACE, read only), in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Space {
    /// The clips window: 48 GiB.
    pub window_bytes: u64,
    /// Free in all (the window less every app's clips, an open one at its reservation).
    pub free_bytes: u64,
    /// The longest free stretch: what an OPEN can get now (a clip is one stretch, first fit), so the longest clip that fits.
    pub largest_free_bytes: u64,
    /// What this app's own clips hold: a sealed one its length rounded up to 4 KiB, an open one its whole reservation.
    pub mine_bytes: u64,
    /// How many clips this app has.
    pub clips: u64,
}

impl Space {
    /// From the vault's reply: five u64 LE, in 4 KiB blocks (the window, every clip's, the longest free stretch, this domain's) and this domain's count.
    pub fn from_reply(b: &[u8; 40]) -> Space {
        let u = |k: usize| u64::from_le_bytes(b[k * 8..k * 8 + 8].try_into().unwrap());
        Space { window_bytes: u(0) << 12, free_bytes: (u(0) - u(1)) << 12, largest_free_bytes: u(2) << 12, mine_bytes: u(3) << 12, clips: u(4) }
    }
}

/// One frame handed over: its packed rows in place (the camera's held buffer), and its facts from the MAP header.
pub struct FrameIn<'a> {
    pub rows: &'a [u8],
    /// The driver's frame count (gaps are frames it skipped).
    pub count: u64,
    /// When it landed, Eagle nanoseconds.
    pub eagle_ns: u64,
    pub exposure_ns: u64,
    /// 1024 is unity.
    pub gain: u32,
    pub focus: u32,
}

/// What became of a frame handed to [`Recorder::frame`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taken {
    Recorded,
    /// Not this clip's (below the sensor's slowest frame the recorder keeps every m-th).
    Paced,
    /// No coded frame's slot was free: the flash fell behind.
    Dropped,
    /// The extent is full or the writer failed: the clip ends here.
    Ended,
}

/// The counts so far, for Lumis's header words.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub frames: u64,
    /// Frames lost: no slot, or the camera's own count stepping past one between frames handed over.
    pub dropped: u64,
    pub paced: u64,
    /// Bytes coded and handed to the writer. On the GPU path frames are packed behind the hand-over, so this lags `frames` by the frames still being packed (a few); it is never 0 once a frame is recorded (the first one waits for its packing).
    pub coded: u64,
    pub raw: u64,
    /// Bytes the writer has written.
    pub written: u64,
    /// The longest a frame's coding took, microseconds, and the sum.
    pub code_us_most: u64,
    pub code_us_sum: u64,
}

/// What a clip came to.
#[derive(Debug, Clone)]
pub struct ClipInfo {
    pub stats: Stats,
    /// The clip's bytes and its seal.
    pub len: u64,
    pub hash: [u8; 32],
    /// The writer's time writing, microseconds.
    pub write_us: u64,
    pub error: Option<String>,
}

/// The recorder's choices.
#[derive(Debug, Clone)]
pub struct Mode {
    pub pacing: Pacing,
    /// The clip's name, as a person reads it (`visual` `title`).
    pub title: String,
    /// The worker threads' core masks, one a thread (husky: A715s G#00F0, A510s G#000F; 0 the kernel's choice).
    pub workers: Vec<u64>,
    /// The writer thread's mask.
    pub writer: u64,
    /// Coded frames waiting for the flash at most.
    pub slots: usize,
    /// The extent asked for, blocks (the sink may give fewer: the clip ends when it is full).
    pub blocks: u64,
    /// When it starts, Eagle oscillations.
    pub at: i64,
    /// Each band's source rows hashed as it is coded ([`Recorder::source_hash`]): a check on the clip, whole, against the frames as the camera wrote them; about 2^-4 of the coding's time.
    pub source_digests: bool,
}

/// A worker thread's call to pin itself (`SYS_PIN` on ferros, nothing on a host).
fn pin(mask: u64) {
    #[cfg(target_os = "ferros")]
    ferros::pin(mask);
    #[cfg(not(target_os = "ferros"))]
    let _ = mask;
}

/// Microseconds since the process first asked, one clock for every thread (Lumis lines its camera's turns up against a clip's writes by it).
pub fn micros() -> u64 {
    use std::time::Instant;
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_micros() as u64
}

/// One band's code: its two stream lengths, its digest, its bytes.
#[derive(Default)]
struct BandOut {
    la: u32,
    lb: u32,
    digest: [u8; 32],
    /// BLAKE3 of the band's rows as the camera wrote them (each row's samples, the stride's tail left out), when the mode asks (`Mode::source_digests`).
    src: [u8; 32],
    bytes: Vec<u8>,
}

/// A coded frame waiting for the writer.
struct Slot {
    meta: FrameMeta,
    bands: Vec<Mutex<BandOut>>,
}

/// The frame being coded, as the workers see it.
#[derive(Clone, Copy)]
struct Job {
    src: *const u8,
    len: usize,
    slot: *const Slot,
}
// SAFETY: a job's pointers are read only while `Recorder::frame` waits for every band (the frame's buffer and its slot outlive the job), and each band's output is behind its own lock.
unsafe impl Send for Job {}

struct PoolState {
    gen: u64,
    job: Option<Job>,
    quit: bool,
}

struct Pool {
    st: Mutex<PoolState>,
    work: Condvar,
    done: Condvar,
    next: AtomicU64,
    finished: AtomicUsize,
    bands: usize,
    width: usize,
    height: usize,
    stride: usize,
    cfa: rc::Cfa,
    hash_src: bool,
}

fn worker(pool: Arc<Pool>, mask: u64) {
    pin(mask);
    let mut work = Box::new(rc::Work::new());
    let mut out = vec![0u8; rc::max_band_len(pool.width, rc::BAND_ROWS)];
    let mut seen = 0u64;
    loop {
        let job = {
            let mut st = pool.st.lock().unwrap();
            while st.gen == seen && !st.quit {
                st = pool.work.wait(st).unwrap();
            }
            if st.quit {
                return;
            }
            seen = st.gen;
            st.job.expect("a new generation carries its job")
        };
        loop {
            // A band is claimed with its frame's generation in the same word (the generation above bit 32, the next band below): a worker that woke late for a frame already done finds another generation and claims nothing, so it never reads a buffer given back.
            let v = pool.next.load(Ordering::Acquire);
            let k = (v & 0xFFFF_FFFF) as usize;
            if v >> 32 != seen & 0xFFFF_FFFF || k >= pool.bands {
                break;
            }
            if pool.next.compare_exchange(v, v + 1, Ordering::AcqRel, Ordering::Relaxed).is_err() {
                continue;
            }
            // SAFETY: Job's: the frame's buffer is `len` bytes alive until every band is done, and the slot with it.
            let src = unsafe { core::slice::from_raw_parts(job.src, job.len) };
            let slot = unsafe { &*job.slot };
            let rows = rc::band_rows(pool.height, k);
            let (la, lb) = rc::encode_band(&src[rc::band_src(k, pool.height, pool.width, pool.stride)], pool.stride, pool.width, rows, pool.cfa, &mut work, &mut out);
            let digest = clip::band_digest(&out[..la], &out[la..la + lb]);
            let srcd = if pool.hash_src { band_source_digest(&src[rc::band_src(k, pool.height, pool.width, pool.stride)], pool.stride, pool.width, rows) } else { [0; 32] };
            {
                let mut b = slot.bands[k].lock().unwrap();
                b.src = srcd;
                b.la = la as u32;
                b.lb = lb as u32;
                b.digest = digest;
                b.bytes.clear();
                b.bytes.extend_from_slice(&out[..la + lb]);
            }
            if pool.finished.fetch_add(1, Ordering::AcqRel) + 1 == pool.bands {
                let _st = pool.st.lock().unwrap();
                pool.done.notify_all();
            }
        }
    }
}

/// A frame for the GPU path's coders: its bands packed from a GPU run's set, or (`set` None, after the GPU failed) coded whole from its rows.
struct PackJob {
    seq: u64,
    set: Option<usize>,
    /// The frame's rows: the set's copy, or the camera's buffer (the hand-over then waits for the frame).
    src: *const u8,
    len: usize,
    info: *const u32,
    info_len: usize,
    bodies: *const u32,
    bodies_len: usize,
    /// Taken by the coder that finishes the frame, for the writer; None for a frame's source hashes alone (`hash_only`).
    slot: Option<Box<Slot>>,
    /// The frame's slot, where its bands go.
    slot_at: *const Slot,
    /// The bands' source digests taken with the packing, or (`hash_only`) alone: a frame whose source hash is asked has them taken while the GPU runs, from the set's copy, its packing after.
    hash: bool,
    hash_only: bool,
    /// Bands claimed, and bands done.
    next: usize,
    done: usize,
    /// When it was handed over, for the time to its last band.
    t0: u64,
}
// SAFETY: a job's pointers are read only until its last band is done: the set stays busy (the hand-over writes no set that is), a CPU job's hand-over waits for it, and its slot is in the job.
unsafe impl Send for PackJob {}

struct PackState {
    jobs: std::collections::VecDeque<PackJob>,
    busy: [bool; gpu::SETS],
    /// The last frame handed to the writer.
    sent: u64,
    last_src: [u8; 32],
    quit: bool,
}

/// The GPU path's coders: frames packed behind the hand-over, in order to the writer.
struct Packers {
    st: Mutex<PackState>,
    work: Condvar,
    done: Condvar,
    bands: usize,
    width: usize,
    height: usize,
    stride: usize,
    cfa: rc::Cfa,
    hash_src: bool,
    to_writer: SyncSender<ToWriter>,
    ended: Arc<std::sync::atomic::AtomicBool>,
    coded: AtomicU64,
    /// Hand-over to the last band, microseconds: the most and the sum.
    behind_most: AtomicU64,
    behind_sum: AtomicU64,
}

/// One of a frame's bands as a coder claimed it.
struct Claim {
    seq: u64,
    k: usize,
    set: Option<usize>,
    src: *const u8,
    len: usize,
    info: *const u32,
    info_len: usize,
    bodies: *const u32,
    bodies_len: usize,
    slot: *const Slot,
    hash: bool,
    hash_only: bool,
}

fn packer(p: Arc<Packers>, mask: u64) {
    pin(mask);
    let mut work = Box::new(rc::Work::new());
    let mut out = vec![0u8; rc::max_band_len(p.width, rc::BAND_ROWS)];
    loop {
        let c = {
            let mut st = p.st.lock().unwrap();
            loop {
                if st.quit {
                    return;
                }
                // A frame's source hashes first: the hand-over waits for them, never for a packing.
                let at = st.jobs.iter().position(|j| j.hash_only && j.next < p.bands).or_else(|| st.jobs.iter().position(|j| j.next < p.bands));
                if let Some(j) = at.map(|i| &mut st.jobs[i]) {
                    j.next += 1;
                    break Claim { seq: j.seq, k: j.next - 1, set: j.set, src: j.src, len: j.len, info: j.info, info_len: j.info_len, bodies: j.bodies, bodies_len: j.bodies_len, slot: j.slot_at, hash: j.hash, hash_only: j.hash_only };
                }
                st = p.work.wait(st).unwrap();
            }
        };
        // SAFETY: a job's buffers and its slot stay until its every band is done (its set busy, the hand-over waiting for a CPU job, the slot in the job).
        let src = unsafe { core::slice::from_raw_parts(c.src, c.len) };
        let slot = unsafe { &*c.slot };
        let rows = rc::band_rows(p.height, c.k);
        let band_src = &src[rc::band_src(c.k, p.height, p.width, p.stride)];
        if c.hash_only {
            let srcd = band_source_digest(band_src, p.stride, p.width, rows);
            slot.bands[c.k].lock().unwrap().src = srcd;
        } else {
            let (la, lb) = match c.set {
                Some(_) => {
                    let (info, bodies) = unsafe { (core::slice::from_raw_parts(c.info, c.info_len), core::slice::from_raw_parts(c.bodies, c.bodies_len)) };
                    gpu::pack_band(info, bodies, c.k, p.width, p.height, &mut out)
                }
                None => rc::encode_band(band_src, p.stride, p.width, rows, p.cfa, &mut work, &mut out),
            };
            let digest = clip::band_digest(&out[..la], &out[la..la + lb]);
            let srcd = c.hash.then(|| band_source_digest(band_src, p.stride, p.width, rows));
            let mut b = slot.bands[c.k].lock().unwrap();
            if let Some(s) = srcd {
                b.src = s;
            }
            b.la = la as u32;
            b.lb = lb as u32;
            b.digest = digest;
            b.bytes.clear();
            b.bytes.extend_from_slice(&out[..la + lb]);
        }
        let mut st = p.st.lock().unwrap();
        let j = st.jobs.iter_mut().find(|j| j.seq == c.seq).expect("a frame stays queued until its last band");
        j.done += 1;
        // A frame's source hashes done: its hand-over may return, whatever is still queued ahead of them.
        if j.hash_only && j.done == p.bands {
            p.done.notify_all();
        }
        // The frames whose every band is done go to the writer, in order (the writer's channel holds every slot, so the send never waits); a frame's source hashes alone are done with when theirs are.
        while st.jobs.front().is_some_and(|j| j.done == p.bands) {
            let mut j = st.jobs.pop_front().expect("the front frame");
            let Some(slot) = j.slot.take() else {
                p.done.notify_all();
                continue;
            };
            if p.hash_src {
                st.last_src = source_hash(&slot.bands.iter().map(|b| b.lock().map(|b| b.src).unwrap_or([0; 32])).collect::<Vec<_>>());
            }
            let coded: u64 = slot.bands.iter().map(|b| b.lock().map(|b| (b.la + b.lb) as u64).unwrap_or(0)).sum();
            p.coded.fetch_add(coded, Ordering::Relaxed);
            let behind = micros() - j.t0;
            p.behind_most.fetch_max(behind, Ordering::Relaxed);
            p.behind_sum.fetch_add(behind, Ordering::Relaxed);
            if let Some(s) = j.set {
                st.busy[s] = false;
            }
            st.sent = j.seq;
            if p.to_writer.send(ToWriter::Frame(slot)).is_err() {
                p.ended.store(true, Ordering::Relaxed);
            }
            p.done.notify_all();
        }
    }
}

/// The GPU path's state in the recorder (the 60/s work, 2026-11-19).
struct GpuMode {
    dev: gpu::Device,
    packers: Arc<Packers>,
    next_set: usize,
    seq: u64,
    /// The GPU failed once: every frame after is coded on the CPU, the hand-over waiting for it.
    failed: Option<String>,
    run_most: u64,
    run_sum: u64,
    runs: u64,
}

/// What the GPU path did (`Recorder::gpu`).
#[derive(Debug, Clone, Default)]
pub struct GpuStats {
    /// Frames run on the GPU, and the GPU's time for them as the service measured it, microseconds.
    pub runs: u64,
    pub run_us_most: u64,
    pub run_us_sum: u64,
    /// From a frame's hand-over to its last band packed, microseconds.
    pub behind_us_most: u64,
    pub behind_us_sum: u64,
    /// Why the GPU stopped being used, if it did.
    pub failed: Option<String>,
}

enum ToWriter {
    Frame(Box<Slot>),
    Close,
}

/// The recorder: start, hand frames, stop.
pub struct Recorder {
    pool: Arc<Pool>,
    workers: Vec<JoinHandle<()>>,
    to_writer: SyncSender<ToWriter>,
    free: Receiver<Box<Slot>>,
    writer: Option<JoinHandle<ClipInfo>>,
    written: Arc<std::sync::atomic::AtomicU64>,
    ended: Arc<std::sync::atomic::AtomicBool>,
    pacing: Pacing,
    stats: Stats,
    last_count: Option<u64>,
    last_kept_ns: Option<u64>,
    last_src: [u8; 32],
    gpu: Option<GpuMode>,
    gpu_note: String,
}

impl Recorder {
    /// Start a clip: the extent opened, generation 0 written, the workers and the writer started.
    /// On ferros a Bayer clip is coded on the GPU when the process holds the GPU's grants (`ferros::gpu_grants`) and a kernel was built for its geometry (`gpu::prebuilt`), the workers then packing behind the hand-over; otherwise, or with `gpu::enable(false)`, on the workers alone. [`Recorder::gpu_note`] says which and why.
    pub fn start(cam: &Camera, mode: &Mode, sink: Box<dyn Sink>) -> Result<Recorder, String> {
        #[cfg(target_os = "ferros")]
        {
            let dev = if !gpu::enabled() {
                Err("the GPU turned off (gpu::enable)".to_string())
            } else if cam.cfa.len() != 4 {
                Err("a quad mosaic: the GPU codes Bayer only".to_string())
            } else {
                match ferros::gpu_grants() {
                    Some((s, b)) => gpu::Device::open(s, b, cam.width, cam.height, cam.stride).map_err(|e| format!("the GPU in grants A#{s} and A#{b}: {e}")),
                    None => Err("no GPU grants".to_string()),
                }
            };
            match dev {
                Ok(d) => Self::start_with(cam, mode, sink, Some(d), "the GPU".to_string()),
                Err(e) => Self::start_with(cam, mode, sink, None, format!("the CPU: {e}")),
            }
        }
        #[cfg(not(target_os = "ferros"))]
        Self::start_with(cam, mode, sink, None, "the CPU".to_string())
    }

    /// Start a clip coded on the given GPU (Bayer only): its workers pack each frame's bands behind the hand-over.
    pub fn start_gpu(cam: &Camera, mode: &Mode, sink: Box<dyn Sink>, dev: gpu::Device) -> Result<Recorder, String> {
        if cam.cfa.len() != 4 {
            return Err("the GPU codes Bayer only".into());
        }
        Self::start_with(cam, mode, sink, Some(dev), "the GPU".to_string())
    }

    /// What codes this clip: the GPU, or the CPU and why.
    pub fn gpu_note(&self) -> &str {
        &self.gpu_note
    }

    /// The GPU path's counts, when the clip is coded on the GPU.
    pub fn gpu(&self) -> Option<GpuStats> {
        self.gpu.as_ref().map(|g| GpuStats { runs: g.runs, run_us_most: g.run_most, run_us_sum: g.run_sum, behind_us_most: g.packers.behind_most.load(Ordering::Relaxed), behind_us_sum: g.packers.behind_sum.load(Ordering::Relaxed), failed: g.failed.clone() })
    }

    fn start_with(cam: &Camera, mode: &Mode, mut sink: Box<dyn Sink>, dev: Option<gpu::Device>, gpu_note: String) -> Result<Recorder, String> {
        let mosaic = match cam.cfa.len() {
            4 => Mosaic::Bayer,
            16 => Mosaic::Quad,
            n => return Err(format!("a mosaic of {n} cells: 4 (Bayer) or 16 (quad)")),
        };
        let p = if mosaic == Mosaic::Bayer { 2 } else { 4 };
        let capture = vsf::visual::Capture { mode: "video".into(), make: cam.make.clone(), model: cam.model.clone(), sensor_module: cam.sensor_module.clone(), lens: cam.lens.clone(), f_number: cam.f_number, focal_mm: cam.focal_mm, ..Default::default() };
        let spec = ClipSpec {
            title: mode.title.clone(),
            rate_num: mode.pacing.fps,
            rate_den: 1,
            period_ns: mode.pacing.period_ns,
            width: cam.width,
            height: cam.height,
            cfa: Tensor::new(vec![p, p], cam.cfa.clone()),
            channel_names: vec!["R".into(), "G".into(), "B".into()],
            black: vec![cam.black as f32; 3],
            white: vec![cam.white as f32; 3],
            depth: rc::BITS as u8,
            orientation: cam.orientation,
            // The 10-bit cfa_band codec, its mosaic and band rows (vsf 3bf262a: a clip's codec is ClipCodec, CfaBand here; Packed is a stacked clip's).
            codec: clip::ClipCodec::CfaBand { mosaic, band_rows: rc::BAND_ROWS },
            capture,
            characterization: cam.characterization.clone(),
            observer: cam.observer.clone(),
            at: Some(mode.at),
        };
        let (cw, head) = clip::Writer::new(&spec).map_err(|e| e.to_string())?;
        let got = sink.open(mode.blocks)?;
        let bands = rc::bands(cam.height);
        let pool = Arc::new(Pool {
            st: Mutex::new(PoolState { gen: 0, job: None, quit: false }),
            work: Condvar::new(),
            done: Condvar::new(),
            next: AtomicU64::new(0),
            finished: AtomicUsize::new(0),
            bands,
            width: cam.width,
            height: cam.height,
            stride: cam.stride,
            cfa: if mosaic == Mosaic::Bayer { rc::Cfa::Bayer } else { rc::Cfa::Quad },
            hash_src: mode.source_digests,
        });
        let slots = mode.slots.max(1);
        let (to_writer, from_rec) = sync_channel::<ToWriter>(slots + 1);
        let (give_back, free) = sync_channel::<Box<Slot>>(slots);
        for _ in 0..slots {
            give_back.send(Box::new(Slot { meta: FrameMeta { index: 0, at: 0, exposure_s: 0., gain: 0., focus: 0 }, bands: (0..bands).map(|k| Mutex::new(BandOut { bytes: touched(rc::max_band_len(cam.width, rc::band_rows(cam.height, k))), ..BandOut::default() })).collect() })).expect("the free list holds every slot");
        }
        let written = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let ended = Arc::new(std::sync::atomic::AtomicBool::new(false));
        // The workers: the band coders, or on the GPU path the packers behind the hand-over.
        let gpu = dev.map(|dev| GpuMode {
            dev,
            packers: Arc::new(Packers {
                st: Mutex::new(PackState { jobs: std::collections::VecDeque::new(), busy: [false; gpu::SETS], sent: 0, last_src: [0; 32], quit: false }),
                work: Condvar::new(),
                done: Condvar::new(),
                bands,
                width: cam.width,
                height: cam.height,
                stride: cam.stride,
                cfa: rc::Cfa::Bayer,
                hash_src: mode.source_digests,
                to_writer: to_writer.clone(),
                ended: ended.clone(),
                coded: AtomicU64::new(0),
                behind_most: AtomicU64::new(0),
                behind_sum: AtomicU64::new(0),
            }),
            next_set: 0,
            seq: 0,
            failed: None,
            run_most: 0,
            run_sum: 0,
            runs: 0,
        });
        let workers = mode
            .workers
            .iter()
            .map(|&m| {
                let b = std::thread::Builder::new().name("clip coder".into());
                match &gpu {
                    Some(g) => {
                        let p = g.packers.clone();
                        b.spawn(move || packer(p, m))
                    }
                    None => {
                        let p = pool.clone();
                        b.spawn(move || worker(p, m))
                    }
                }
                .map_err(|e| e.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let (wr, en, wmask) = (written.clone(), ended.clone(), mode.writer);
        let writer = std::thread::Builder::new()
            .name("clip writer".into())
            .spawn(move || {
                pin(wmask);
                write_clip(cw, head, sink, got, from_rec, give_back, wr, en)
            })
            .map_err(|e| e.to_string())?;
        Ok(Recorder { pool, workers, to_writer, free, writer: Some(writer), written, ended, pacing: mode.pacing, stats: Stats::default(), last_count: None, last_kept_ns: None, last_src: [0; 32], gpu, gpu_note })
    }

    /// The last recorded frame's source hash (`Mode::source_digests`): BLAKE3 of its bands' source digests, each BLAKE3 of the band's packed rows as the camera wrote them; a reader repacks a decoded frame and compares.
    pub fn source_hash(&self) -> [u8; 32] {
        self.last_src
    }

    pub fn stats(&self) -> Stats {
        let behind = self.gpu.as_ref().map_or(0, |g| g.packers.coded.load(Ordering::Relaxed));
        Stats { written: self.written.load(Ordering::Relaxed), coded: self.stats.coded + behind, ..self.stats }
    }

    /// The GPU path's hand-over: the frame copied into a free set and run on the GPU, its bands then packed by the workers behind (the hand-over waits for them only when the frame's source hash is asked for, or after the GPU failed, when the workers code the frame from the camera's buffer).
    fn frame_gpu(&mut self, f: &FrameIn, mut slot: Box<Slot>) -> Taken {
        let t0 = micros();
        let g = self.gpu.as_mut().expect("the GPU path");
        slot.meta = FrameMeta { index: self.stats.frames + self.stats.dropped, at: eagle_osc(f.eagle_ns), exposure_s: f.exposure_ns as f32 / 1e9, gain: f.gain as f32 / 1024., focus: f.focus };
        let p = g.packers.clone();
        let set = g.next_set;
        // The set free: the frame that used it last packed.
        {
            let mut st = p.st.lock().unwrap();
            while st.busy[set] {
                st = p.done.wait(st).unwrap();
            }
        }
        let slot_at: *const Slot = &*slot;
        // With the source hash asked, the frame's copy first and its bands' source digests taken by the workers while the GPU runs (2026-11-19, the Pro: Lumis's DEV_VIDEO run at 60/s took 20.6 ms a hand-over with them taken after the GPU, and dropped 69 of 249).
        let early = p.hash_src && g.failed.is_none();
        let run = if g.failed.is_some() {
            Err(String::new())
        } else if early {
            g.dev.put(set, f.rows);
            let fr = g.dev.frame(set);
            g.seq += 1;
            {
                let mut st = p.st.lock().unwrap();
                st.jobs.push_back(PackJob { seq: g.seq, set: None, src: fr.as_ptr(), len: fr.len(), info: core::ptr::null(), info_len: 0, bodies: core::ptr::null(), bodies_len: 0, slot: None, slot_at, hash: true, hash_only: true, next: 0, done: 0, t0 });
                p.work.notify_all();
            }
            g.dev.go(set)
        } else {
            g.dev.run(set, f.rows)
        };
        let (job_set, src, len) = match run {
            Ok(us) => {
                g.next_set = (set + 1) % gpu::SETS;
                g.runs += 1;
                g.run_most = g.run_most.max(us as u64);
                g.run_sum += us as u64;
                let fr = g.dev.frame(set);
                (Some(set), fr.as_ptr(), fr.len())
            }
            Err(e) => {
                if g.failed.is_none() {
                    g.failed = Some(e);
                }
                (None, f.rows.as_ptr(), f.rows.len())
            }
        };
        let (info, bodies) = match job_set {
            Some(s) => (g.dev.info(s), g.dev.bodies(s)),
            None => (&[][..], &[][..]),
        };
        g.seq += 1;
        let seq = g.seq;
        // The clip's first frame waits for its packing too, once: `stats().coded` is then never 0 once a frame is recorded, as on the CPU path (Lumis divides the room left by the bytes a frame has coded to: 2026-11-20, a 0 there panicked it at its first frame).
        let wait = job_set.is_none() || (p.hash_src && !early) || self.stats.frames == 0;
        {
            let mut st = p.st.lock().unwrap();
            if let Some(s) = job_set {
                st.busy[s] = true;
            }
            st.jobs.push_back(PackJob { seq, set: job_set, src, len, info: info.as_ptr(), info_len: info.len(), bodies: bodies.as_ptr(), bodies_len: bodies.len(), slot: Some(slot), slot_at, hash: p.hash_src && !early, hash_only: false, next: 0, done: 0, t0 });
            p.work.notify_all();
            if wait {
                while st.sent < seq && !st.quit {
                    st = p.done.wait(st).unwrap();
                }
                self.last_src = st.last_src;
            }
            // With the hashes taken while the GPU ran, the hand-over waits for them alone and the packing goes on behind (Lumis's DEV_VIDEO at 60/s, 2026-11-20: waiting for the packing too made a hand-over 16.9 ms and dropped 202 of 1756).
            if early && job_set.is_some() {
                let hash_seq = seq - 1;
                while !st.quit && st.jobs.iter().any(|j| j.seq == hash_seq && j.done < p.bands) {
                    st = p.done.wait(st).unwrap();
                }
            }
        }
        if early && job_set.is_some() {
            // SAFETY: a slot's box lives as long as the recorder (in a job, with the writer, or in the free list), and only a later hand-over (this thread's) writes its digests again; each band's was written under its lock before the hashes counted done.
            let slot = unsafe { &*slot_at };
            self.last_src = source_hash(&slot.bands.iter().map(|b| b.lock().map(|b| b.src).unwrap_or([0; 32])).collect::<Vec<_>>());
        }
        let us = micros() - t0;
        self.stats.code_us_most = self.stats.code_us_most.max(us);
        self.stats.code_us_sum += us;
        self.stats.raw += (p.height * rc::row_bytes(p.width)) as u64;
        if self.ended.load(Ordering::Relaxed) {
            return Taken::Ended;
        }
        self.stats.frames += 1;
        self.last_kept_ns = Some(f.eagle_ns);
        Taken::Recorded
    }

    /// Hand a frame over: coded before this returns (the buffer may then go back to the camera), written behind.
    pub fn frame(&mut self, f: &FrameIn) -> Taken {
        if self.ended.load(Ordering::Relaxed) {
            return Taken::Ended;
        }
        // The camera's own count: a step past one is frames it skipped, lost to the clip (the coding or the flash fell behind).
        if let Some(c) = self.last_count {
            let lost = f.count.saturating_sub(c + 1); // WHY/PROOF: the driver's count only rises; a restarted count (a reopened camera) would wrap a plain subtraction into a huge loss
            self.stats.dropped += lost / self.pacing.keep as u64;
        }
        self.last_count = Some(f.count);
        // Below the sensor's slowest frame: every keep-th by time (a frame lengthened by a long shutter is kept as soon as it is due).
        if let Some(t) = self.last_kept_ns {
            if self.pacing.keep > 1 && f.eagle_ns < t + self.pacing.period_ns - (self.pacing.sensor_ns >> 1) {
                self.stats.paced += 1;
                return Taken::Paced;
            }
        }
        let Ok(mut slot) = self.free.try_recv() else {
            self.stats.dropped += 1;
            return Taken::Dropped;
        };
        assert!(f.rows.len() >= (self.pool.height - 1) * self.pool.stride + rc::row_bytes(self.pool.width), "a frame shorter than its rows");
        if self.gpu.is_some() {
            return self.frame_gpu(f, slot);
        }
        let t0 = micros();
        // Code every band on the workers; wait for the last.
        assert!(f.rows.len() >= (self.pool.height - 1) * self.pool.stride + rc::row_bytes(self.pool.width), "a frame shorter than its rows");
        {
            let mut st = self.pool.st.lock().unwrap();
            self.pool.finished.store(0, Ordering::Relaxed);
            st.job = Some(Job { src: f.rows.as_ptr(), len: f.rows.len(), slot: &*slot });
            st.gen += 1;
            // The generation's low 32 bits above the band count: what a worker's claim checks.
            self.pool.next.store((st.gen & 0xFFFF_FFFF) << 32, Ordering::Release);
            self.pool.work.notify_all();
            while self.pool.finished.load(Ordering::Acquire) < self.pool.bands {
                st = self.pool.done.wait(st).unwrap();
            }
        }
        let us = micros() - t0;
        self.stats.code_us_most = self.stats.code_us_most.max(us);
        self.stats.code_us_sum += us;
        // The clip's own count: every frame it should hold, so a step past one is exactly the frames lost (`index` in vsf clip).
        slot.meta = FrameMeta { index: self.stats.frames + self.stats.dropped, at: eagle_osc(f.eagle_ns), exposure_s: f.exposure_ns as f32 / 1e9, gain: f.gain as f32 / 1024., focus: f.focus };
        if self.pool.hash_src {
            self.last_src = source_hash(&slot.bands.iter().map(|b| b.lock().map(|b| b.src).unwrap_or([0; 32])).collect::<Vec<_>>());
        }
        let coded: u64 = slot.bands.iter().map(|b| b.lock().map(|b| (b.la + b.lb) as u64).unwrap_or(0)).sum();
        self.stats.coded += coded;
        self.stats.raw += (self.pool.height * rc::row_bytes(self.pool.width)) as u64;
        if self.to_writer.send(ToWriter::Frame(slot)).is_err() {
            return Taken::Ended;
        }
        self.stats.frames += 1;
        self.last_kept_ns = Some(f.eagle_ns);
        Taken::Recorded
    }

    /// End the clip: the closing generation written and the extent sealed.
    pub fn stop(mut self) -> ClipInfo {
        // The GPU path's frames still being packed go to the writer first.
        if let Some(g) = &self.gpu {
            let mut st = g.packers.st.lock().unwrap();
            while !st.jobs.is_empty() {
                st = g.packers.done.wait(st).unwrap();
            }
        }
        let stats = self.stats();
        let _ = self.to_writer.send(ToWriter::Close);
        let mut info = self.writer.take().expect("the writer runs until stop").join().unwrap_or_else(|_| ClipInfo { stats: Stats::default(), len: 0, hash: [0; 32], write_us: 0, error: Some("the writer died".into()) });
        {
            let mut st = self.pool.st.lock().unwrap();
            st.quit = true;
            self.pool.work.notify_all();
        }
        if let Some(g) = &self.gpu {
            g.packers.st.lock().unwrap().quit = true;
            g.packers.work.notify_all();
            g.packers.done.notify_all();
        }
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
        info.stats = Stats { written: info.len, ..stats };
        info
    }
}

/// The coders' cores, one each: the k-th worker given a mask runs on that mask's (k mod its cores)-th core, so four workers on the A715s' mask are one on each A715.
/// Four threads sharing a cluster's mask were run on fewer cores than they asked for (2026-11-19, the Pro, rawrec job 6: a frame's 48 bands on four A715 threads with the shared mask G#00F0 in 52 ms, pinned one to a core in 24 ms; the kernel wakes one idle core for a queued thread, and the rest of the cluster sleeps on until its timer).
pub fn spread(masks: &[u64]) -> Vec<u64> {
    let mut seen: Vec<(u64, u32)> = Vec::new();
    masks
        .iter()
        .map(|&m| {
            let k = match seen.iter_mut().find(|s| s.0 == m) {
                Some(s) => {
                    s.1 += 1;
                    s.1 - 1
                }
                None => {
                    seen.push((m, 1));
                    0
                }
            };
            let n = m.count_ones();
            if n == 0 {
                return m;
            }
            // The (k mod n)-th set bit of the mask.
            let mut left = m;
            for _ in 0..k % n {
                left &= left - 1;
            }
            // Its lowest set bit (x & -x in two's complement: a bit trick, not an overflow guard).
            left & left.wrapping_neg()
        })
        .collect()
}

/// An empty buffer of `cap` bytes whose pages are already the process's (written once): a frame's first coding then grows nothing, where an arc's growth in the camera loop cost a clip its second frame (2026-11-19).
fn touched(cap: usize) -> Vec<u8> {
    let mut v = vec![0u8; cap];
    v.clear();
    v
}

/// The source hash of a frame from its bands' source digests (`Recorder::source_hash`): BLAKE3 of the digests in band order.
pub fn source_hash(digests: &[[u8; 32]]) -> [u8; 32] {
    clip::frame_hash(digests)
}

/// A band's source digest: BLAKE3 of its rows' samples as packed RAW10 (each row `row_bytes(width)` bytes, a stride's tail left out).
pub fn band_source_digest(rows: &[u8], stride: usize, width: usize, n: usize) -> [u8; 32] {
    let rb = rc::row_bytes(width);
    let mut x = blake3::Hasher::new();
    for y in 0..n {
        x.update(&rows[y * stride..y * stride + rb]);
    }
    *x.finalize().as_bytes()
}

/// A decoded frame's source hash: its samples packed back to RAW10 band by band, each band's digest as `band_source_digest`, combined as `source_hash`; equal to the recorder's for the frame when the round trip is bit exact.
pub fn samples_source_hash(px: &[u16], width: usize, height: usize) -> [u8; 32] {
    let rb = rc::row_bytes(width);
    let mut packed = vec![0u8; rb * rc::BAND_ROWS];
    let digests: Vec<[u8; 32]> = (0..rc::bands(height))
        .map(|k| {
            let (y0, n) = (rc::band_first_row(k), rc::band_rows(height, k));
            for y in 0..n {
                let row = &px[(y0 + y) * width..(y0 + y + 1) * width];
                let out = &mut packed[y * rb..(y + 1) * rb];
                for (g, v) in row.chunks_exact(4).enumerate() {
                    for i in 0..4 {
                        out[g * 5 + i] = (v[i] >> 2) as u8;
                    }
                    out[g * 5 + 4] = (v[0] & 3) as u8 | ((v[1] & 3) << 2) as u8 | ((v[2] & 3) << 4) as u8 | ((v[3] & 3) << 6) as u8;
                }
            }
            band_source_digest(&packed, rb, width, n)
        })
        .collect();
    source_hash(&digests)
}

/// Eagle nanoseconds as Eagle oscillations (VSF `e6`): the hydrogen line's 1,420,407,826 a second.
pub fn eagle_osc(ns: u64) -> i64 {
    (ns as i128 * vsf::types::eagle_time::OSCILLATIONS_PER_SECOND as i128 / 1_000_000_000) as i64
}


/// The writer: generation 0, then each coded frame as it comes, staged into whole blocks a UFS command at a time; at the end the closing generation and the seal.
/// A full extent ends the clip cleanly (the frame that did not fit is left out and the clip closed after the last one); a failed write leaves the clip unclosed, readable to its last whole frame by its chain, and says why.
#[allow(clippy::too_many_arguments)]
fn write_clip(mut cw: clip::Writer, head: Vec<u8>, mut sink: Box<dyn Sink>, blocks: u64, from: Receiver<ToWriter>, give_back: SyncSender<Box<Slot>>, written: Arc<std::sync::atomic::AtomicU64>, ended: Arc<std::sync::atomic::AtomicBool>) -> ClipInfo {
    let mut st = Stage { buf: Vec::with_capacity(WRITE_MAX << 1), block: 0, us: 0, longest: 0 };
    let mut failed: Option<String> = st.put(&mut *sink, &head).err();
    let mut full = false;
    while let Ok(msg) = from.recv() {
        let ToWriter::Frame(slot) = msg else { break };
        if failed.is_none() && !full {
            let bands: Vec<[u32; 2]> = slot.bands.iter().map(|b| b.lock().map(|b| [b.la, b.lb]).unwrap()).collect();
            let digests: Vec<[u8; 32]> = slot.bands.iter().map(|b| b.lock().map(|b| b.digest).unwrap()).collect();
            // This frame (its fields a few hundred bytes past its samples) and the closing generation (about 2^7 bytes a frame of index, a header) must fit what is left.
            let need = bands.iter().map(|b| (b[0] + b[1]) as u64).sum::<u64>() + (1 << 12) + (cw.frames().len() as u64 + 1) * (1 << 7) + (1 << 14);
            if cw.end() + need > blocks * BLOCK as u64 {
                full = true;
            } else {
                match cw.frame(&slot.meta, &bands, &digests) {
                    Ok(p) => {
                        let r = st.put(&mut *sink, &p.head).and_then(|()| slot.bands.iter().try_for_each(|b| st.put(&mut *sink, &b.lock().unwrap().bytes))).and_then(|()| st.put(&mut *sink, &p.tail));
                        failed = r.err();
                    }
                    Err(e) => failed = Some(e.to_string()),
                }
            }
            written.store(st.block * BLOCK as u64, Ordering::Relaxed);
            if failed.is_some() || full {
                ended.store(true, Ordering::Relaxed);
            }
        }
        let _ = give_back.send(slot);
    }
    seal_clip(cw, st, &mut *sink, failed, full, &written)
}

/// A clip's end, for both writers (`write_clip`, a coded clip's; `packed::write_packed`, a stacked one's): after a write that failed, nothing more (the clip reads to its last whole frame and can be repaired); else the closing generation written, flushed and the clip sealed, `full` said as why it ended early.
fn seal_clip(cw: clip::Writer, mut st: Stage, sink: &mut dyn Sink, failed: Option<String>, full: bool, written: &std::sync::atomic::AtomicU64) -> ClipInfo {
    if let Some(e) = failed {
        return ClipInfo { stats: Stats::default(), len: 0, hash: [0; 32], write_us: st.us, error: Some(format!("{e}: the clip is not closed; it reads to its last whole frame (vsf clip::Reader) and can be repaired and sealed")) };
    }
    let end = cw.end();
    let r = cw.close("ferros_clip", "clip close").map_err(|e| e.to_string()).and_then(|closing| {
        let hash = *blake3::hash(&closing).as_bytes();
        let len = end + closing.len() as u64;
        st.put(sink, &closing).and_then(|()| st.flush(sink)).and_then(|()| sink.seal(len, hash)).map(|()| (len, hash))
    });
    let (len, hash, error) = match r {
        Ok((len, hash)) => (len, hash, full.then(|| "the extent is full: the clip ends at its last frame that fit".to_string())),
        Err(e) => (0, [0; 32], Some(e)),
    };
    written.store(len, Ordering::Relaxed);
    ClipInfo { stats: Stats::default(), len, hash, write_us: st.us, error }
}

/// The bytes not yet written (less than a write) and the next block they go to.
struct Stage {
    buf: Vec<u8>,
    block: u64,
    us: u64,
    /// The longest single write since it was last read (`write_packed` says it a frame).
    longest: u64,
}

impl Stage {
    fn put(&mut self, sink: &mut dyn Sink, bytes: &[u8]) -> Result<(), String> {
        let mut rest = bytes;
        while !rest.is_empty() {
            let n = (WRITE_MAX - self.buf.len()).min(rest.len());
            self.buf.extend_from_slice(&rest[..n]);
            rest = &rest[n..];
            if self.buf.len() == WRITE_MAX {
                self.write(sink, WRITE_MAX)?;
            }
        }
        Ok(())
    }
    fn write(&mut self, sink: &mut dyn Sink, n: usize) -> Result<(), String> {
        let t = micros();
        sink.write(self.block, &self.buf[..n])?;
        let took = micros() - t;
        self.us += took;
        self.longest = self.longest.max(took);
        self.block += (n / BLOCK) as u64;
        self.buf.drain(..n);
        Ok(())
    }
    /// The last partial block, padded with zeros past the clip's end.
    fn flush(&mut self, sink: &mut dyn Sink) -> Result<(), String> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let n = self.buf.len().div_ceil(BLOCK) * BLOCK;
        self.buf.resize(n, 0);
        self.write(sink, n)
    }
}
