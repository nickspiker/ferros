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
}

impl Pacing {
    /// The binned main's pacing for a whole rate of 1 to 60 a second (the nearest frame length; the sensor's clock has factors 2, 5 and 7, so a rate with a factor 3 is never exact, and the others only where llp would divide too).
    pub fn of(fps: u32) -> Pacing {
        assert!((1..=60).contains(&fps), "fps {fps} outside 1 to 60");
        let lines = |rate: u64| (MAIN_PIXCLK + (MAIN_LLP * rate >> 1)) / (MAIN_LLP * rate);
        let mut keep = 1u64;
        while lines(fps as u64 * keep) > FLL_MOST {
            keep += 1;
        }
        let fll = lines(fps as u64 * keep);
        debug_assert!(fll >= FLL_LEAST);
        let sensor_ns = fll * MAIN_LLP * 1_000_000_000 / MAIN_PIXCLK;
        Pacing { fps, fll: fll as u16, keep: keep as u32, sensor_ns, period_ns: sensor_ns * keep }
    }

    /// The camera driver's open with its view for this pacing (payloads/camera `Spec`): `[1][camera][1 = with the view][kind 1, the binned readout][rows][fll][exposure lines][flags][ring][llp][cols][view flags: bit 0 write-back]`.
    /// `exposure_ns` the opening shutter (the driver lengthens the frame past `fll` when it is longer: the interval is then the shutter's).
    pub fn open_msg(&self, camera: u8, exposure_ns: u64) -> [u8; 17] {
        let lines = (exposure_ns * MAIN_PIXCLK / (MAIN_LLP * 1_000_000_000)).max(1).min(FLL_MOST) as u16; // WHY: the open's word is 16 bits of lines and at least one; a longer shutter is the SET's (shift) to set after the first frame
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
}

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
    /// Bytes coded and handed to the writer.
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

fn micros() -> u64 {
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
}

impl Recorder {
    /// Start a clip: the extent opened, generation 0 written, the workers and the writer started.
    pub fn start(cam: &Camera, mode: &Mode, mut sink: Box<dyn Sink>) -> Result<Recorder, String> {
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
            mosaic,
            channel_names: vec!["R".into(), "G".into(), "B".into()],
            black: vec![cam.black as f32; 3],
            white: vec![cam.white as f32; 3],
            depth: rc::BITS as u8,
            orientation: cam.orientation,
            band_rows: rc::BAND_ROWS,
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
        let workers = mode
            .workers
            .iter()
            .map(|&m| {
                let p = pool.clone();
                std::thread::Builder::new().name("clip coder".into()).spawn(move || worker(p, m)).map_err(|e| e.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let slots = mode.slots.max(1);
        let (to_writer, from_rec) = sync_channel::<ToWriter>(slots + 1);
        let (give_back, free) = sync_channel::<Box<Slot>>(slots);
        for _ in 0..slots {
            give_back.send(Box::new(Slot { meta: FrameMeta { index: 0, at: 0, exposure_s: 0., gain: 0., focus: 0 }, bands: (0..bands).map(|k| Mutex::new(BandOut { bytes: touched(rc::max_band_len(cam.width, rc::band_rows(cam.height, k))), ..BandOut::default() })).collect() })).expect("the free list holds every slot");
        }
        let written = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let ended = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (wr, en, wmask) = (written.clone(), ended.clone(), mode.writer);
        let writer = std::thread::Builder::new()
            .name("clip writer".into())
            .spawn(move || {
                pin(wmask);
                write_clip(cw, head, sink, got, from_rec, give_back, wr, en)
            })
            .map_err(|e| e.to_string())?;
        Ok(Recorder { pool, workers, to_writer, free, writer: Some(writer), written, ended, pacing: mode.pacing, stats: Stats::default(), last_count: None, last_kept_ns: None, last_src: [0; 32] })
    }

    /// The last recorded frame's source hash (`Mode::source_digests`): BLAKE3 of its bands' source digests, each BLAKE3 of the band's packed rows as the camera wrote them; a reader repacks a decoded frame and compares.
    pub fn source_hash(&self) -> [u8; 32] {
        self.last_src
    }

    pub fn stats(&self) -> Stats {
        Stats { written: self.written.load(Ordering::Relaxed), ..self.stats }
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
        let _ = self.to_writer.send(ToWriter::Close);
        let mut info = self.writer.take().expect("the writer runs until stop").join().unwrap_or_else(|_| ClipInfo { stats: Stats::default(), len: 0, hash: [0; 32], write_us: 0, error: Some("the writer died".into()) });
        {
            let mut st = self.pool.st.lock().unwrap();
            st.quit = true;
            self.pool.work.notify_all();
        }
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
        info.stats = Stats { written: info.len, ..self.stats };
        info
    }
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
    let mut st = Stage { buf: Vec::with_capacity(WRITE_MAX << 1), block: 0, us: 0 };
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
    if let Some(e) = failed {
        return ClipInfo { stats: Stats::default(), len: 0, hash: [0; 32], write_us: st.us, error: Some(format!("{e}: the clip is not closed; it reads to its last whole frame (vsf clip::Reader) and can be repaired and sealed")) };
    }
    let end = cw.end();
    let r = cw.close("ferros_clip", "clip close").map_err(|e| e.to_string()).and_then(|closing| {
        let hash = *blake3::hash(&closing).as_bytes();
        let len = end + closing.len() as u64;
        st.put(&mut *sink, &closing).and_then(|()| st.flush(&mut *sink)).and_then(|()| sink.seal(len, hash)).map(|()| (len, hash))
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
        self.us += micros() - t;
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
