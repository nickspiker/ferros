//! Lumis on ferros: Lumis's own core (the lumis_core library: the camera menu, the integrator, the camera UI, the VSF saves) with ferros as its platform, as Photon runs on ferros as itself (std/photon).
//! What Android's Kotlin layer and JNI do, this crate does with ferros's pieces (lumis_core's `app` and `platform` modules are the seam):
//! - the window and touch: fluor's ferros host; Lumis draws into an RGB888 frame (lumis_core `PixelSurface`), shown here as fluor's 16-bit g2t pixels;
//! - the sensor: the `camera` endpoint (docs/CAMERA.md): the list, an open, a frame per call, the manual controls Lumis asks for after each frame;
//! - Lumis's two processes (the UI and the camera service sharing ashmem) as two threads sharing the integrator's memory;
//! - its settings and saves: the kist's own vault domain (kete), never files.
//!
//! Its grant slots, as ferros lays out a kist that declares only the camera (ferros_kist `slots`): 1 display, 2 touch, 3 surface, 4 ira, 5 its vault domain, 6 entropy, 7 camera (an empty slot when the person said no), then the GPU's three and sound out, which it does not use.

mod camera;
mod gpu;
mod hold;
mod sys;

use fluor::coord::Coord;
use fluor::event::{CursorIcon, ElementState, Event, MouseButton};
use fluor::host::app::{Context, FluorApp};
use fluor::host::EventResponse;
use fluor::pixel::{G2t16, Pixel};
use lumis_core::app::{self, Buttons, CameraSetup, SensorColour};
use lumis_core::platform::{ConfigStore, PixelSurface};
use lumis_core::shared_memory::SharedMemory;
use lumis_core::ui_main_menu::MainMenu;
use lumis_core::{CameraIdentity, CameraInfo, UserInterface};
use lumis_core::shared_memory::{RawMode, CURRENT_MODE_IDX, CALIBRATING_BIT, CONTINUOUS_SAVE_BIT, CURRENTLY_SAVING, FLAGS_IDX, HEARTBEAT_NANOS_IDX, HEARTBEAT_SECS_IDX, MANUAL_SAVE_BIT, SHUTTER_NS_IDX};
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Whether the window is behind (the person went to the deck or another kist is in front): fluor's `Event::Focused(false)`, cleared by `Focused(true)`.
/// The UI sets it; the camera thread reads it at each frame and decides whether Lumis stays (owner, 2026-11-17: "lumis, if we're framing up and we go to the deck, should go bye bye, unless I'm doing a continuous save"; and of a calibration, "Basically if continuous save or cal is on").
static BEHIND: AtomicBool = AtomicBool::new(false);

/// Lumis is leaving and the camera has been (or is being) closed for it: set by whichever thread sends that CLOSE first (`close_to_leave`), so there is one, and the camera thread, back from a wait that came back empty or with a frame, reads nothing more of the view and ends.
static LEAVING: AtomicBool = AtomicBool::new(false);

/// Held by the camera thread whenever it may read the view (all of its turn but the driver's wait, `Source::next`), and by a thread that closes the camera for Lumis's leaving: the driver takes the view back at CLOSE, so a CLOSE sent from another thread must never land while a frame's rows are being read.
static VIEW: Mutex<()> = Mutex::new(());
/// The newest frame the camera thread has taken (the driver's count): a preview read after its buffer went back is whole while this is at most its frame's count plus the ring less 3 (`Preview`).
static NEWEST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// A preview is reading the view on its own thread (`Preview`): nothing closes the camera or touches the integrator until it ends (`preview_settle`).
static PREVIEWING: Mutex<bool> = Mutex::new(false);
static PREVIEW_IDLE: std::sync::Condvar = std::sync::Condvar::new();

const DISPLAY: u64 = 1;
const TOUCH: u64 = 2;
const SURFACE: u64 = 3;
const IRA: u64 = 4;
const CAMERA: u64 = 7;

/// The binning Lumis asks the driver for when the open gave no view (a driver or kernel from before it): the largest frame a call can carry is 8 MiB (process.rs `REPLY_MAX`), so the main camera's whole readout (4080 x 3072 samples, 24 MiB) cannot cross and bin 2 (2040 x 1536, 6 MiB) is the finest a copy carries.
/// With the view (docs/CAMERA.md "The view"; owner, 2026-11-15: "why the ridiculously low camera resolution?") Lumis integrates and saves whole sensor frames, read in place.
const BIN: u32 = 2;

/// Each phase-detect half's black, even and odd, in RAW10 counts: measured on the Pro main's pairs against their image's green, the same dark and bright (docs/CAMERA.md "The PD layout, measured": 64.0 to 64.4 and 62.4 to 62.6); no register is known.
const PD_BLACKS: (f32, f32) = (64., 62.5);

/// Where the camera service's frames come from.
enum Source {
    /// The view: frames read in place (`base` where this process reads it), the buffer held until the next MAP; the integrator reads each row straight out of it (`ViewRows`), so no whole frame is ever copied.
    View { base: usize, view: camera::View, held: u8 },
    /// Frames by copy at `BIN`, header and samples.
    Copy { buf: Vec<u8> },
    /// A slit ring (Slitscan): slit k in buffer (k - 1) mod the ring, drained by SLITS (`slits` its shape, as the open and the last SLITS said), each slit's centre band copied out of the non-cacheable view into `band` (`SlitRows`). Every slit up to `after` was taken or passed over; `take` the ones still to take of the last answer; `lost` the slits written over while read, `skipped` those passed over as too old to read safely (Lumis behind by more than half the ring).
    Slit { base: usize, slits: camera::Slits, after: u64, take: std::ops::RangeInclusive<u64>, band: Vec<u16>, lost: u64, skipped: u64 },
}

impl Source {
    fn is_slit(&self) -> bool {
        matches!(self, Source::Slit { .. })
    }
}

/// What the camera service takes from its source: a whole frame, or a slit (its count and Eagle time from its record, and its centre band).
enum Got<'a> {
    Frame(camera::FrameHeader, Taken<'a>),
    Slit { count: u64, time: i64, rows: SlitRows<'a> },
}

/// The rows of a slit lumis_core reads (`process_slit`: its centre period, the 2 x 2 cell's two rows binned), held as the four rows from `first`: whatever the period (2 or 4), its rows start on a multiple of it at the slit's middle, inside those four. A row outside them is a bug in that arithmetic, and the index fails.
struct SlitRows<'a> {
    band: &'a [u16],
    first: usize,
    width: usize,
    /// The slit's own rows (the integrator centres its band on them).
    rows: usize,
}

/// The rows a slit's band holds, from `SlitRows::first`.
const BAND_ROWS: usize = 1 << 2;

impl lumis_core::RowSource for SlitRows<'_> {
    fn read_row(&self, y: usize, out: &mut [u16]) {
        let r = y - self.first;
        out.copy_from_slice(&self.band[r * self.width..(r + 1) * self.width]);
    }
}

/// The camera opened again for whole frames, after a slit's open that failed or gave no slit ring (that one closed first): Lumis ends if it fails too, keeping what is encoded.
fn reopen_whole(cam: &camera::Camera, vault: Option<&kete::FlatStorage>, header: &mut [u64]) -> Source {
    match open(cam, false, None) {
        Ok(s) => s,
        Err(Refused::Why(why)) => {
            keep_saves(vault, header);
            lumis_core::platform::exit_because(&format!("camera {}: opened again for whole frames, it failed: {why}", cam.name));
        }
        Err(Refused::NoSlit) => unreachable!("a whole-frame open is never a slit's"),
    }
}

/// A RAW video clip recording (lumis_core RawMode::Video; owner, 2026-11-19: "VSF video, yes", lossless): the camera opened at the clip's pacing (`open` with a `Pacing`), every whole frame the view hands over coded by ferros_clip's recorder and written into the person's hold as an item of kind RAW video, its body a single-copy extent written straight to the flash (docs/HOLD-API.md "Lumis"). It runs until STOP (RECORDING_BIT cleared), the extent is full or a write fails (`Taken::Ended`), or Lumis leaves or ends: every one of those seals the clip (`end_clip`).
struct Clip {
    rec: ferros_clip::Recorder,
    /// The camera's name, for the console.
    cam: String,
    /// Its frames a second (1 to 60, ferros_clip `Pacing::of`).
    fps: u32,
    /// When it began, as the stills' stamp (local time): the clip as the console and the note under RECORD name it.
    name: String,
    /// The item's handle from its open and its card from its seal (ferros_clip `items::Receipt`).
    receipt: Arc<Mutex<ferros_clip::items::Receipt>>,
    began: std::time::Instant,
    /// The extent's bytes, as the vault gave them (it may give fewer than asked): what is left of them sets the time left.
    room: u64,
    /// When the preview last integrated a frame while this records (`PREVIEW_GAP`).
    preview: std::time::Instant,
    /// The clip's frames dropped as last said (`stats().dropped`), so each new drop is said once, with what may have caused it.
    dropped: u64,
    /// The camera's count of the frame last handed over.
    last_count: u32,
    /// When the clip last dropped a frame: no preview for 2^10 ms after (the camera thread's time back to the recorder while it falls behind).
    dropped_at: Option<std::time::Instant>,
    /// The last hand-over's time, us, and the sensor's frame period, us: no preview after a hand-over that took over half the period (the recorder running late).
    last_code_us: u64,
    period_us: u64,
    /// When the clip's first frame landed, Eagle ns by the driver's clock: the clip's rate is its frames over the time since, by that clock (VIDEO_RATE_IDX).
    first_ns: Option<i64>,
    /// The recording space used as it stood when the clip began (`VIDEO_SPACE_USED_IDX`): the bar counts this clip's written bytes on top, not its whole reservation.
    space_before: u64,
}

/// The clip recording, if any: the camera thread starts it and hands it each frame (holding this only for the frame), and whatever ends Lumis seals it (`seal_at_exit`, from lumis_core's `exit_because` on any thread).
static CLIP: Mutex<Option<Clip>> = Mutex::new(None);

/// lumis_core `set_before_exit`: a clip recording sealed before Lumis ends, so the Exit button (the UI thread) or an error never leaves one open. The camera thread never ends Lumis while it holds `CLIP` (it seals a clip itself before each of its own exits), so this waits at most for the frame in hand.
fn seal_at_exit() {
    if let Some(c) = CLIP.lock().unwrap().take() {
        end_clip(c, None, "Lumis ending");
    }
    // A save on the keeper's thread is kept before Lumis ends (`keep_saves`).
    saves_settle();
}

/// While a clip records, the preview integrates a frame at most every 2^`PREVIEW_GAP_LOG2` ms, beside that frame's hand-over (camera_service). Measured on the Pro, warm, 30 a second (2026-11-19): together they take 36 to 42 ms of a 33 ms frame (the coding 28 ms alone, 33 beside the integration on the A510s), so each preview frame leaves the camera thread a few ms late, made up over the frames after it; at 2^6 ms (14 previews a second) it never caught up and 28 of 870 frames were lost; at 2^7 (about 7.5 previews a second) a warm 32 s clip lost none of its 871 frames (the coding 30.2 ms on average). Since 2026-11-22 the preview is on its own thread, which the camera never waits for (`PreviewJob`): 2^5 ms, about 20 to 24 previews a second, measured on the Pro with 0 dropped at 60 a second on the main and 48 on the ultrawide (16 s clips, cool), where 2^7 and 2^8 gave about 4.
const PREVIEW_GAP_LOG2: u32 = 5;

/// Above 30 a second the gap was one power of two longer while the preview shared the camera thread (a minute at 60 a second lost 3 frames to previews beside hand-overs, 2026-11-20); off that thread it is the same gap at every rate.
const PREVIEW_GAP_FAST_LOG2: u32 = PREVIEW_GAP_LOG2;

/// The preview's gap while recording at `fps`: 2^`PREVIEW_GAP_LOG2` ms, or `PREVIEW_GAP_FAST_LOG2` above 30 a second, or the input's 24th byte's power of two (development: `DEV_VIDEO` runs measure where drops begin).
fn preview_gap(fps: u32) -> std::time::Duration {
    let mut b = [0u8; 1 << 5];
    let n = sys::input(0, &mut b) as usize;
    let at = 10 + camera::MODE_BLOCK;
    let log2 = if n > at && b[at] != 0 {
        b[at] as u32
    } else if fps > 30 {
        PREVIEW_GAP_FAST_LOG2
    } else {
        PREVIEW_GAP_LOG2
    };
    std::time::Duration::from_millis(1 << log2)
}

/// The clips extent asked for at most, blocks of `ferros_clip::BLOCK` (2^34 bytes, 16 GiB: about 35 minutes at 30 a second at the codec's measured 2 to 3.5 times), halved on each refusal (no free stretch that long) down to `CLIP_BLOCKS_LEAST`.
const CLIP_BLOCKS_MOST: u64 = 1 << 22;
/// The least worth recording into, 2^28 bytes (about 30 s at 30 a second at 2 times).
const CLIP_BLOCKS_LEAST: u64 = 1 << 16;

/// The GPU block grown to what the recorder's layout needs for frames of `height` rows `line` bytes apart (ferros_clip `gpu::Layout`, from its base past Lumis's part), when Lumis holds the GPU: the recorder's own grow at its start then finds it so.
fn video_block(name: &str, height: usize, line: usize) {
    let Some((_, block)) = ferros_clip::ferros::gpu_grants() else { return };
    let log = ferros_clip::gpu::Layout::of(ferros_clip::gpu::base(), height, line).log;
    match ferros_sys::gpu_block_ask(sys::cap(block), log) {
        Some(size) => println!("lumis: camera {name}: RAW video: the GPU block at G#{size:X} bytes for the recorder, before RECORD"),
        None => println!("lumis: camera {name}: RAW video: the GPU block did not grow to 2^A#{log} bytes (the recorder tries again at RECORD)"),
    }
}

/// The rate RAW video asks of the camera (lumis_core: VIDEO_FPS_IDX, 0 until the UI sets it: the UI's default).
fn video_fps(header: &[u64]) -> u32 {
    let f = match header[lumis_core::shared_memory::VIDEO_FPS_IDX] {
        0 => lumis_core::ui::ui::VIDEO_FPS_DEFAULT as u32,
        f => f as u32,
    };
    // WHY the min: a rate set on a faster camera stays in the header; on this one it is this one's fastest, as lumis_core's slider shows it (`UserInterface::fps`).
    f.min(lumis_core::platform::video_fps_most() as u32)
}

/// A clip started on the held frame `head`, `rows` its rows in the view: the extent opened (as large as the vault gives, from `CLIP_BLOCKS_MOST` halving), generation 0 written, the coder's workers and the writer started. The clip's facts are the still's (the camera's identity and optics, the black and white levels, the 2 x 2 mosaic from the frame), its orientation the EXIF code of the sensor's mount and the hold (as a still's tag), its time the frame's.
/// `digests`: each frame's source hash taken as it is coded (ferros_clip `Mode::source_digests`, about 2^-4 of the coding), for a development run to say (`DEV_VIDEO`) and a host to check the clip against (tools/clip-check).
/// `facts`: the sensor's mount and the hold (degrees) and the recording space used, as the header had them; `line` the view's bytes a row. Run on a thread of its own (`camera_service`): the extent's open can wait minutes (the vault closing and sealing a clip left open, a few seconds a GB), and the camera thread must go on taking frames meanwhile, or the driver closes.
fn start_clip(cam: &camera::Camera, facts: (u64, u64, u64), pacing: ferros_clip::Pacing, head: camera::FrameHeader, line: usize, digests: bool) -> Result<Clip, String> {
    let (mount, held, space_used) = facts;
    if !hold::held() {
        return Err("your things were not given to Lumis: no clip can be kept".into());
    }
    // The vault domain's grant asks the recording space (the clips area is one, the hold's clips in it).
    let slot = ferros_clip::ferros::store_slot().ok_or("no vault domain among Lumis's grants")?;
    // No room for a frame uncoded (the most a frame codes to): not started, said instead (the coordinator, 2026-11-19: the clips area at 63 MB free).
    let frame_bytes = (head.height * line) as u64;
    if let Some(sp) = ferros_clip::ferros::space(slot) {
        if sp.largest_free_bytes < frame_bytes {
            return Err(format!("no room to record: {} MiB free in one stretch, a frame needs up to {} MiB", sp.largest_free_bytes >> 20, frame_bytes >> 20));
        }
    }
    let orientation = match (mount + held) % 360 {
        90 => 6,
        180 => 3,
        270 => 8,
        _ => 1,
    };
    let clip_cam = ferros_clip::Camera {
        width: head.width,
        height: head.height,
        stride: line,
        cfa: ferros_clip::cfa_of_byte(head.cfa),
        black: head.black,
        white: head.white,
        // The still's Make and Model (`identity`: "Google", "Pixel 8 Pro cam 2"), so one IDT check fits both.
        make: identity(cam).make_tag(),
        model: identity(cam).model_tag(),
        sensor_module: cam.name.clone(),
        lens: String::new(),
        f_number: (cam.f_number > 0.).then_some(cam.f_number),
        focal_mm: (cam.focal_mm > 0.).then_some(cam.focal_mm),
        characterization: None,
        observer: String::new(),
        orientation,
    };
    // When it began as the stills' stamp: what the console and the note under RECORD call it (the hold names it by its card).
    let name = lumis_core::capture_stamp(std::time::SystemTime::now());
    let at = ferros_clip::eagle_osc(head.time as u64);
    // The longest free stretch of the clips window, whole (what an OPEN can get: a clip is one stretch), where the vault says it; else from CLIP_BLOCKS_MOST, halving on refusal.
    let mut blocks = ferros_clip::ferros::space(slot).map_or(CLIP_BLOCKS_MOST, |sp| sp.largest_free_bytes / ferros_clip::BLOCK as u64);
    loop {
        let mode = ferros_clip::Mode {
            pacing,
            title: format!("Lumis RAW video, {} a second", pacing.fps),
            // As the recorder's agent measured on the Pro (2026-11-19): a worker on each A715 and each A510, the writer on the A510s (on the A715s it dropped 47 of 64 frames), four coded frames waiting for the flash at most.
            workers: [[0x00F0u64; 4], [0x000F; 4]].concat(),
            writer: 0x000F,
            slots: 4,
            blocks,
            at,
            source_digests: digests,
        };
        // The card Lumis proposes (HOLD-API.md "Lumis"): kind RAW video, no title (the person gives none here), its start (`raw_video` adds `at`), its rate, its frame's size and the camera's name.
        let attrs = vec![
            ("fps".to_string(), vsf::VsfType::u(pacing.fps as usize, false)),
            ("width".to_string(), vsf::VsfType::u(head.width, false)),
            ("height".to_string(), vsf::VsfType::u(head.height, false)),
            ("camera".to_string(), vsf::VsfType::x(cam.name.clone())),
        ];
        let (sink, receipt) = ferros_clip::items::raw_video(hold::ITEMS, None, at, attrs);
        let got = Arc::new(std::sync::atomic::AtomicU64::new(blocks));
        match ferros_clip::Recorder::start(&clip_cam, &mode, Box::new(hold::Counted { sink, got: got.clone() })) {
            Ok(rec) => {
                let room = got.load(Ordering::Relaxed) * ferros_clip::BLOCK as u64;
                let direct = receipt.lock().unwrap().direct;
                // What codes it: the GPU when Lumis holds its grants (a kist's GPU slots, a spawn's --use gpu --dma layers; 60 a second held on the Pro, 2026-11-20), else the CPU's workers and why. Where it goes: straight to the flash (the clip's own extent capability) or thru the vault.
                println!("lumis: camera {}: RAW video: recording {name} into the hold at {} a second ({} MiB of room, {}, orientation {orientation}); coded by {}", cam.name, pacing.fps, room >> 20, if direct { "straight to the flash" } else { "thru the vault" }, rec.gpu_note());
                let now = std::time::Instant::now();
                return Ok(Clip { rec, cam: cam.name.clone(), fps: pacing.fps, name, receipt, began: now, room, preview: now, dropped: 0, last_count: head.count, dropped_at: None, last_code_us: 0, period_us: pacing.sensor_ns / 1000, first_ns: None, space_before: space_used });
            }
            Err(e) if blocks > CLIP_BLOCKS_LEAST => {
                println!("lumis: camera {}: RAW video: {e}; asking half", cam.name);
                blocks >>= 1;
            }
            Err(e) => return Err(e),
        }
    }
}

/// The recording space into the header (lumis_core's space bar in RAW video): the clips window, and as used what an OPEN cannot get (the window less its longest free stretch: what is really left to record into). Nothing on a kernel or vault without the query (the bar then not drawn).
fn read_space(header: &mut [u64]) {
    use lumis_core::shared_memory::{VIDEO_SPACE_TOTAL_IDX, VIDEO_SPACE_USED_IDX};
    if let Some(sp) = ferros_clip::ferros::store_slot().and_then(ferros_clip::ferros::space) {
        header[VIDEO_SPACE_TOTAL_IDX] = sp.window_bytes;
        header[VIDEO_SPACE_USED_IDX] = sp.window_bytes - sp.largest_free_bytes;
    }
}

/// A clip ended: its closing generation written and the item sealed (ferros_clip `stop`: its card then carries the length and the seal), its counts into the header (when the caller has it: not at Lumis's end), said on the console with why it ended.
fn end_clip(clip: Clip, header: Option<&mut [u64]>, why: &str) {
    use lumis_core::shared_memory::{RECORDING_BIT, VIDEO_DROPPED_IDX, VIDEO_FRAMES_IDX, VIDEO_LEFT_S_IDX};
    let ms = clip.began.elapsed().as_millis();
    let cam = clip.cam;
    // The GPU's own counts, when it coded the clip (the hand-over's time is code_us_* on that path, the GPU's runs and the packers behind it these).
    // WHY the max: a clip can end before the GPU has run a frame, and the average of none is said as 0.
    let gpu = clip.rec.gpu().map(|g| format!("; the GPU: {} runs, {} us at most, {} on average; the packers behind it {} us at most{}", g.runs, g.run_us_most, g.run_us_sum / g.runs.max(1), g.behind_us_most, g.failed.map(|e| format!(", failed: {e}")).unwrap_or_default())).unwrap_or_default();
    let info = clip.rec.stop();
    let s = info.stats;
    if let Some(header) = header {
        header[VIDEO_FRAMES_IDX] = s.frames;
        header[VIDEO_DROPPED_IDX] = s.dropped;
        header[VIDEO_LEFT_S_IDX] = 0;
        header[lumis_core::shared_memory::VIDEO_RATE_IDX] = 0;
        header[FLAGS_IDX] &= !RECORDING_BIT;
        // The space as the seal left it (the reservation given back past the clip's length).
        read_space(header);
    }
    // Under the RECORD button: the clip kept as the person's (Kūaha shows it), and why if it ended early.
    lumis_core::platform::set_video_note(Some(match &info.error {
        None => format!("{} kept, {} frames", clip.name, s.frames),
        Some(e) => format!("{} ended: {e}", clip.name),
    }));
    let hex: String = info.hash[..8].iter().map(|b| format!("{b:02x}")).collect();
    // The item as its seal left it: its id, which the bench finds it by (`ferros-bridge items find --kind "RAW video"`), and its card's size.
    match &clip.receipt.lock().unwrap().card {
        Some(c) => println!("lumis: camera {cam}: RAW video: {} is item {} in the hold, {} bytes", clip.name, c.id.as_bytes()[..8].iter().map(|b| format!("{b:02x}")).collect::<String>(), c.size),
        None => println!("lumis: camera {cam}: RAW video: {} not sealed into the hold (the vault closes it at its last whole frame)", clip.name),
    }
    println!(
        "lumis: camera {cam}: RAW video: {} ended ({why}): {} frames in {ms} ms, {} dropped, {} paced; {} bytes (coded {} of {}), sealed {hex}; the coding {} us a frame at most, {} on average{}",
        clip.name,
        s.frames,
        s.dropped,
        s.paced,
        info.len,
        s.coded,
        s.raw,
        s.code_us_most,
        // WHY the max: a clip can end with no frame (STOP before its first, its first ones dropped), and the average of none is said as 0.
        s.code_us_sum / s.frames.max(1),
        info.error.map(|e| format!("; {e}")).unwrap_or_default()
    );
    if !gpu.is_empty() {
        println!("lumis: camera {cam}: RAW video: {}{gpu}", clip.name);
    }
}

/// Whether what was taken is a slit.
fn src_is_slit(got: &Got) -> bool {
    matches!(got, Got::Slit { .. })
}

/// Whether the integrator wants slits: Slitscan, and no calibration (its darks are whole frames).
fn wants_slits(header: &[u64]) -> bool {
    header[CURRENT_MODE_IDX] == RawMode::Slitscan as u64 && header[FLAGS_IDX] & CALIBRATING_BIT == 0
}

/// A frame's time in the camera service, for the console every 64: waiting on the driver, the integrator (which reads and unpacks the view's rows itself), the settings and saves after it, the whole turn (each summed, microseconds).
/// With the driver's own count and Eagle time of the first and last frame taken, so the line also says the sensor's period by the driver's clock and how many frames the driver wrote that Lumis never took (2026-11-17, the preview's split).
#[derive(Default)]
struct Turns {
    wait: u64,
    integrate: u64,
    after: u64,
    since: Option<std::time::Instant>,
    n: u64,
    /// The driver's count and Eagle ns of the first frame of these 64 and of the last.
    first: Option<(u32, i64)>,
    last: (u32, i64),
    /// The integrator's image counter at the first frame of these 64: the images it completed since.
    images: u64,
}

/// The UI thread's side of the preview, said every 2^2 s while the camera screen is up (2026-11-17, the owner: "about 14 frames a second"; the split measured before anything is fixed).
/// A pass is one fluor frame: Lumis's draw (`ui_frame`), then its convert into fluor's scratch (`render`); fluor's own ten-second line has its bake (present) and op 22 (show).
/// A pass whose image counter moved draws the whole camera image (lumis_core draw_screen's full draw); any other repaints the controls' changing parts only, but posts the frame all the same, so it is converted, baked and shown whole.
struct Probe {
    since: std::time::Instant,
    passes: u64,
    /// Passes that found a new image, and the images completed that no pass drew (the counter moved by more than one between two passes).
    fresh: u64,
    unseen: u64,
    /// `ui_frame`'s time on a fresh pass and on any other, summed (us).
    draw_fresh: u64,
    draw_other: u64,
    /// The convert into fluor's scratch (`render`), summed (us), and how many.
    convert: u64,
    converts: u64,
    /// The image counter the last pass saw, and the one at the window's start.
    counter: Option<u64>,
    counter_at: Option<u64>,
}

/// The development knobs, the spawn input's first byte: every pass shown whole as before the redraw-on-change (to measure its win in the same build).
const DEV_WHOLE: u8 = 1 << 0;
/// Frames by copy at `BIN` instead of the view (the 6 MiB reply measured against it).
const DEV_COPY: u8 = 1 << 1;
/// Development: the scratch path even with the display baking (no `bakes_own`), for comparing the two on a device.
const DEV_SCRATCH: u8 = 1 << 2;
/// Development: open the first camera at once, no tap (a device with no identity attested gives every touch to the attest screen), and end through the camera's own close after 2^(n + 3) seconds, n the knobs' bits 4 and 5 (8 s to 64 s).
const DEV_OPEN: u8 = 1 << 3;
/// Development: the intake stays on the A715 workers with the UI (as before 0.2.17) instead of its own threads on the A510s, to compare the two on a device.
const DEV_BIG_INTAKE: u8 = 1 << 6;
/// The A510 cluster (husky cores 0 to 3), where the intake runs (lumis_core `set_intake_cores`) so the UI's draw keeps the A715s.
const CORES_LITTLE: u64 = 0x000F;
/// The A715 workers (husky cores 4 to 7), where a save encodes (lumis_core `set_save_cores`): a save is started from the intake on the A510s, and their shared NEON would hold up an AV1 encode.
const CORES_WORKERS: u64 = 0x00F0;
/// Development, with `DEV_OPEN`: sweep the shutter by itself, every two stops from 2^-13 s to 2^3 s, then 2^4 s (no further than the camera's longest: a long-exposure mode's frame takes its whole exposure in wall time), two frames each, each frame's exposure said beside the one asked.
const DEV_SWEEP: u8 = 1 << 7;

/// The spawn input as the knobs byte and the mode block after it: (0, none) for a process spawned with none (a kist opened from the map).
/// A second knobs byte may follow the whole block (the input's fifteenth byte, `dev_knobs2`); the block stops before it.
fn dev_input() -> (u8, Vec<u8>) {
    let mut b = [0u8; 1 << 5];
    let n = sys::input(0, &mut b);
    match n {
        0 => (0, Vec::new()),
        // WHY the min: the input is the person's; past the block is the second knobs byte, not the driver's.
        _ => (b[0], b[1..n.min(1 + camera::MODE_BLOCK)].to_vec()),
    }
}

/// Development, the second knobs byte (after the whole mode block): the view asked for non-cacheable as before 0.2.18, to compare the two on a device (docs/CAMERA.md "A cacheable view").
const DEV_UNCACHED: u8 = 1 << 0;
/// Development, the second knobs byte: the first open is the slit's, and Lumis starts in Slitscan, sized from the camera's list (its whole readout), so one hand-spawned driver (which refuses a second open) drains slits.
const DEV_SLIT: u8 = 1 << 1;
/// Development, with `DEV_SLIT`: the second knobs byte's bits 3 and 4 pick the shutter Lumis opens at, 2^-(11 - 2n) s (n 0 to 3: about 1/2000 s, 1/500, 1/128, 1/32), the camera's least (one line) being under the sensor's two.
const DEV_SLIT_SHUTTER: u32 = 3;
/// Development, with `DEV_OPEN`, the second knobs byte: open the first readout with a mode of its own (the main's 1:1, ferros cam-full-res) instead of the first listed, and say the gravity Lumis is given once, at the camera screen's first pass.
const DEV_FULL: u8 = 1 << 5;
/// Development, the second knobs byte: open at a 2^10 s shutter (1024 s, the longest the main has been measured to honour), so a long frame is in flight for the leave test (lumis-leave-long: sent behind, the camera closed at once).
const DEV_LONG: u8 = 1 << 6;
/// Development, the second knobs byte: never send the integrator's settings to the driver (gain, shutter, focus stay the open's), to tell a camera's own rate from what a setting does to it.
const DEV_NO_SET: u8 = 1 << 7;
/// Development, the fourth knobs byte (the input's seventeenth): the 16th frame's mean of each place in a 4 x 4 cell over the middle of the frame (the colour filter's table: a Bayer cell's two greens alike, a quad's 2 x 2 blocks alike); and a VSF 8B save at the 32nd (Lumis's developed frame, kept in the vault as any save).
const DEV_CELLS: u8 = 1 << 0;
const DEV_SAVE: u8 = 1 << 1;
/// Development, the fourth knobs byte: open in RAW video (on a camera that offers it) and record from the 16th frame until the run ends, at the input's 23rd byte's frames a second (`dev_video_fps`; 0 the UI's default): a clip with no tap. Every 2^3-th frame's source hash is said on the console ("lumis: RAW video: frame <index> source <hex>"), for tools/clip-check against the pulled clip.
const DEV_VIDEO: u8 = 1 << 2;
/// Development, the fourth knobs byte: with DEV_VIDEO, take and say the source hashes (tools/clip-check); without, a DEV_VIDEO clip is coded as a deck's is (the hashes held each hand-over for its own packing: 202 of 1760 frames lost at 60 a second against 55, the RAW agent, 2026-11-20).
const DEV_HASHES: u8 = 1 << 3;
/// Development, the fourth knobs byte: with DEV_VIDEO, stay in RAW video without recording (the idle preview's heat measured).
const DEV_VIDEO_IDLE: u8 = 1 << 4;

/// The input's 23rd byte: DEV_VIDEO's frames a second, 0 when shorter.
fn dev_video_fps() -> u8 {
    let mut b = [0u8; 1 << 5];
    let n = sys::input(0, &mut b) as usize;
    let at = 9 + camera::MODE_BLOCK;
    if n > at { b[at] } else { 0 }
}
/// Development, the input's 18th to 21st bytes: the shutter Lumis opens at, in us (u32 LE; 0 the open's own), for whole frames and slits alike (DEV_SLIT's own seed then gives way); and the 22nd: which settings Lumis sends the driver, bit 0 the gain, 1 the shutter, 2 the focus (0 all, as ever), to find which of them a camera answers by changing its frame.
fn dev_shutter_us() -> u32 {
    let mut b = [0u8; 1 << 5];
    let n = sys::input(0, &mut b) as usize;
    let at = 4 + camera::MODE_BLOCK;
    if n >= at + 4 { u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) } else { 0 }
}
fn dev_sets() -> u8 {
    let mut b = [0u8; 1 << 5];
    let n = sys::input(0, &mut b) as usize;
    let at = 8 + camera::MODE_BLOCK;
    if n > at { b[at] } else { 0 }
}
fn dev_knobs4() -> u8 {
    let mut b = [0u8; 1 << 5];
    let n = sys::input(0, &mut b);
    if n > 3 + camera::MODE_BLOCK { b[3 + camera::MODE_BLOCK] } else { 0 }
}

/// The mean of each place of a 4 x 4 cell over the middle half of the frame's columns and 2^8 of its rows (`DEV_CELLS`), said on the console as four rows of four.
fn say_cells(name: &str, rows: &dyn lumis_core::RowSource, width: usize, height: usize) {
    let mut sums = [[0u64; 4]; 4];
    let mut counts = [[0u64; 4]; 4];
    let mut row = vec![0u16; width];
    let y0 = (height / 2 - (1 << 7)) & !3;
    for y in y0..y0 + (1 << 8) {
        rows.read_row(y, &mut row);
        for x in (width / 4) & !3..(3 * width / 4) & !3 {
            sums[y & 3][x & 3] += row[x] as u64;
            counts[y & 3][x & 3] += 1;
        }
    }
    let table: Vec<String> = (0..4).map(|r| (0..4).map(|c| format!("{:5}", sums[r][c] / counts[r][c])).collect::<Vec<_>>().join(" ")).collect();
    println!("lumis: development: camera {name}'s 4 x 4 cell means (rows 0 to 3, from row {y0}): {}", table.join(" / "));
}
/// Development, the second knobs byte: whole frames first, then Slitscan by itself 2^12 ms in (no tap: the attest screen takes every touch), so the reopen (CLOSE, and an OPEN that starts a stored driver on demand) runs and is timed; with a driver from before the slit it falls back, and opens whole frames again.
const DEV_SLIT_LATER: u8 = 1 << 2;
/// Development, the third knobs byte (the input's sixteenth, after the second): with `DEV_OPEN`, open readout k - 1 of the list as `list` prints it, 0 the first (or `DEV_FULL`'s); for a run over every camera and mode with no tap.
fn dev_readout() -> Option<usize> {
    let mut b = [0u8; 1 << 5];
    let n = sys::input(0, &mut b);
    (n > 2 + camera::MODE_BLOCK && b[2 + camera::MODE_BLOCK] != 0).then(|| b[2 + camera::MODE_BLOCK] as usize - 1)
}

/// The second knobs byte, 0 when the input is shorter.
fn dev_knobs2() -> u8 {
    let mut b = [0u8; 1 << 5];
    let n = sys::input(0, &mut b);
    if n > 1 + camera::MODE_BLOCK { b[1 + camera::MODE_BLOCK] } else { 0 }
}

/// The UI probe's window: a line every 2^2 seconds.
const PROBE_MS: u128 = 1 << 12;

impl Probe {
    fn new() -> Self {
        Probe { since: std::time::Instant::now(), passes: 0, fresh: 0, unseen: 0, draw_fresh: 0, draw_other: 0, convert: 0, converts: 0, counter: None, counter_at: None }
    }

    /// One pass of the camera screen: the image counter as it read before the draw, and the draw's time.
    fn pass(&mut self, counter: u64, us: u64) {
        self.passes += 1;
        self.counter_at.get_or_insert(counter);
        match self.counter {
            Some(c) if c == counter => self.draw_other += us,
            Some(c) => {
                self.fresh += 1;
                self.unseen += counter - c - 1;
                self.draw_fresh += us;
            }
            None => {
                self.fresh += 1;
                self.draw_fresh += us;
            }
        }
        self.counter = Some(counter);
        let ms = self.since.elapsed().as_millis();
        if ms >= PROBE_MS {
            // WHY: a window can hold no pass of a kind (no new image while the camera stalls, or every pass a new one), and its mean is then said as 0 instead of dividing by zero.
            let mean = |t: u64, n: u64| if n == 0 { 0 } else { t / n };
            let completed = counter - self.counter_at.unwrap_or(counter);
            println!(
                "lumis: preview, the last A#{ms} ms: A#{} passes, A#{} with a new image drawn whole, of A#{completed} images the camera completed (A#{} never drawn); Lumis's draw A#{} us a new image, A#{} us otherwise; the convert A#{} us a pass",
                self.passes,
                self.fresh,
                self.unseen,
                mean(self.draw_fresh, self.fresh),
                mean(self.draw_other, self.passes - self.fresh),
                mean(self.convert, self.converts)
            );
            *self = Probe { counter: self.counter, ..Probe::new() };
        }
    }
}

/// `len` bytes from the non-cacheable view at `src` into `dst`, 64 bytes a load (four NEON registers): each load of non-cacheable memory is its own bus transaction, and the camera driver measured 8-byte loads at 121 ms a frame against 14.4 ms (docs/CAMERA.md).
fn copy_from_view(src: usize, dst: &mut [u8]) {
    #[cfg(target_arch = "aarch64")]
    for i in 0..dst.len() / 64 {
        unsafe {
            let v = core::arch::aarch64::vld1q_u8_x4((src + i * 64) as *const u8);
            core::arch::aarch64::vst1q_u8_x4(dst.as_mut_ptr().add(i * 64), v);
        }
    }
    #[cfg(not(target_arch = "aarch64"))]
    unsafe {
        core::ptr::copy_nonoverlapping(src as *const u8, dst.as_mut_ptr(), dst.len())
    };
}

/// A frame as the camera thread takes it: rows read in place out of the view, or a copy's samples (16 bits, little-endian, row by row).
enum Taken<'a> {
    View(ViewRows),
    Bytes(&'a [u8]),
}

/// A held frame in the view, read a row at a time by the integrator (lumis_core `RowSource`): each row copied out of the non-cacheable view in 64-byte loads into a buffer on the stack, then unpacked from RAW10.
/// A write-back view's rows are unpacked in place, thru the caches (docs/CAMERA.md "A cacheable view": the driver made the buffer's lines right as MAP handed it over).
/// The rows are read from the integrator's own bands across the cores, each once, while the buffer stays held (it is given back at the next MAP).
struct ViewRows {
    src: usize,
    line: usize,
    cached: bool,
    /// The frame's phase-detect rows in the same buffer, where the driver gives them (MAP +56 and +60: docs/CAMERA.md "Phase detect as data"): where they start in this process and how many, at the image's line.
    pd: Option<(usize, usize)>,
    /// The frame in the GPU's view of the camera (`CAMERA_VA` plus its place in the view; 0: none, as for its phase-detect rows) and its size in samples, for the GPU's stack (`RowSource::stack`).
    gpu: u64,
    width: usize,
    height: usize,
}

impl ViewRows {
    /// The phase-detect rows as rows of their own (the same line and caching as the image's) and their count.
    fn pd(&self) -> Option<(ViewRows, usize)> {
        self.pd.map(|(src, rows)| (ViewRows { src, line: self.line, cached: self.cached, pd: None, gpu: 0, width: 0, height: 0 }, rows))
    }
}

/// RAW video's preview while a clip records, on a thread of its own (capture first, the owner 2026-11-21): the camera thread hands it the held frame as the hand-over starts and takes the camera's next frame as soon as the hand-over is done, never waiting for the preview (on the Pro a preview beside the hand-over took 14 to 48 ms, past a 60 or 48 a second frame, and the frame after it was lost). The preview reads on after the buffer went back; the view's ring gives it the frames until the engine comes round to that buffer, and one that outran them is dropped unpublished (lumis_core `RowSource::whole`), the last whole preview staying on screen (the owner: only whole frames shown).
struct PreviewJob {
    rows: ViewRows,
    /// The last frame the camera may have taken for the read still to be whole: the frame's count plus the view's ring less 3 (its buffer is written again by the frame a ring after it, which starts as the one before that lands; one frame more for the camera thread's taking it).
    last: u64,
    iso: i32,
    exposure_ns: i64,
    quad: bool,
}

/// A preview's end: the settings the integrator gave back, its time, whether it was whole (published).
struct PreviewDone {
    settings: (i32, i64, f32),
    us: u64,
    whole: bool,
}

/// The frame a preview reads, whole while the camera has not taken past `last` (`NEWEST`), asked by the integrator before it publishes.
struct FreshRows {
    rows: ViewRows,
    last: u64,
    whole: AtomicBool,
}

impl lumis_core::RowSource for FreshRows {
    fn read_row(&self, y: usize, out: &mut [u16]) {
        self.rows.read_row(y, out)
    }

    fn whole(&self) -> bool {
        let w = NEWEST.load(Ordering::Acquire) <= self.last;
        self.whole.store(w, Ordering::Relaxed);
        w
    }
}

/// The preview thread for the integrator at `integrator`: jobs in, their ends out.
fn preview_thread(integrator: usize) -> Option<(std::sync::mpsc::Sender<PreviewJob>, std::sync::mpsc::Receiver<PreviewDone>)> {
    let (tx, jobs) = std::sync::mpsc::channel::<PreviewJob>();
    let (done_tx, done) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("lumis-preview".into())
        .spawn(move || {
            for j in jobs {
                let f = FreshRows { rows: j.rows, last: j.last, whole: AtomicBool::new(true) };
                let t = std::time::Instant::now();
                // SAFETY: the integrator is leaked for the process's life; while PREVIEWING is set (from the job's hand-over until here) the camera thread calls none of its methods and reads and writes only its header, the shared memory the UI thread writes too (`preview_settle` before anything else), and the view stays mapped (`close_camera` waits for this).
                let settings = unsafe { &mut *(integrator as *mut lumis_core::CameraIntegrator) }.process_rows(&f, j.iso, j.exposure_ns, 0., j.quad);
                let d = PreviewDone { settings, us: t.elapsed().as_micros() as u64, whole: f.whole.load(Ordering::Relaxed) };
                *PREVIEWING.lock().unwrap() = false;
                PREVIEW_IDLE.notify_all();
                if done_tx.send(d).is_err() {
                    return;
                }
            }
        })
        .ok()
        .map(|_| (tx, done))
}

/// Wait until no preview reads the view or uses the integrator.
fn preview_settle() {
    let mut busy = PREVIEWING.lock().unwrap();
    while *busy {
        busy = PREVIEW_IDLE.wait(busy).unwrap();
    }
}

fn previewing() -> bool {
    *PREVIEWING.lock().unwrap()
}

impl lumis_core::RowSource for ViewRows {
    /// The frame stacked on the GPU straight from the camera's buffer (lumis_gpu `stack_kernel`, the owner 2026-11-22: "I thought we stacked on the GPU for the memory bandwidth, no?"), when the camera's view is mapped in the GPU (`gpu::CAMERA_MAPPED`), the plane is in the integrator's part of the GPU block (`gpu::plane_in_block`), and the frame's groups of four samples come in whole groups of 16 invocations (every binned readout of the Pro); else the integrator reads the rows itself, as before. A GPU job that fails turns the GPU's stack off for the rest of the process, said once.
    fn stack(&self, plane: &mut [u16], add: bool) -> bool {
        if self.gpu == 0 || !gpu::CAMERA_MAPPED.load(Ordering::Relaxed) || gpu::STACK_OFF.load(Ordering::Relaxed) || self.width % 4 != 0 || (self.width / 4 * self.height) % (1 << 4) != 0 || plane.len() != self.width * self.height {
            return false;
        }
        let Some(at) = gpu::plane_in_block(plane.as_ptr(), plane.len()) else { return false };
        let mut shared = gpu::SHARED.lock().unwrap();
        let Some(g) = shared.as_mut() else { return false };
        match g.stack(self.gpu, self.line, self.width, self.height, at, add) {
            Ok(()) => true,
            Err(e) => {
                gpu::STACK_OFF.store(true, Ordering::Relaxed);
                println!("lumis: {e}: the stack on the CPU from now");
                false
            }
        }
    }

    fn read_row(&self, y: usize, out: &mut [u16]) {
        let at = self.src + y * self.line;
        if self.cached {
            // SAFETY: row y of the held buffer, `line` bytes inside the view (mapped read-only for the life of the camera, the buffer this process's until the next MAP); nothing writes it while it is held.
            let line = unsafe { std::slice::from_raw_parts(at as *const u8, self.line) };
            camera::unpack_raw10(line, out);
            return;
        }
        // A RAW10 line of the widest readout fits: 8160 samples are 10200 bytes.
        let mut tmp = [0u8; 1 << 14];
        let line = &mut tmp[..self.line];
        copy_from_view(at, line);
        camera::unpack_raw10(line, out);
    }
}

impl Source {
    /// The next frame, its header and its rows, with the wait counted (and a copy's own unpacking, none for the view).
    fn next(&mut self, t: &mut Turns) -> Option<Got<'_>> {
        let t0 = std::time::Instant::now();
        match self {
            Source::View { base, view, held } => {
                let mut r = [0u8; camera::FRAME_HEADER];
                let n = sys::call_live(CAMERA, &[camera::OP_MAP, *held], &mut r);
                let m = (n != sys::ERR).then(|| camera::parse_map(&r[..n as usize])).flatten()?;
                *held = m.buffer;
                let t1 = std::time::Instant::now();
                t.wait += (t1 - t0).as_micros() as u64;
                // The phase view is offered while the frames carry phase-detect rows in the layout ferros measured (the Pro main binned: a pair every two columns, a PD row every four image rows; docs/CAMERA.md "The PD layout, measured"), said when that changes. The 1:1 readout keeps its PD rows too, in a layout not measured, so it is not offered there.
                let pd = m.pd_rows != 0 && m.pd_offset != 0 && m.pd_rows << 2 == m.frame.height;
                if pd != lumis_core::platform::phase_offered() {
                    lumis_core::platform::set_phase_offered(pd);
                    println!("lumis: the phase view {}", if pd { format!("offered: {} phase-detect rows a frame, G#{:X} past the image", m.pd_rows, m.pd_offset) } else { "not offered: the frames carry no phase-detect rows".into() });
                }
                let src = *base + m.offset;
                Some(Got::Frame(m.frame, Taken::View(ViewRows { src, line: view.line, cached: view.cached, pd: pd.then_some((src + m.pd_offset, m.pd_rows)), gpu: ferros_sys::gpu::CAMERA_VA + m.offset as u64, width: m.frame.width, height: m.frame.height })))
            }
            Source::Copy { buf } => {
                let n = sys::call_live(CAMERA, &[camera::OP_FRAME, BIN as u8], buf);
                t.wait += t0.elapsed().as_micros() as u64;
                let h = (n != sys::ERR).then(|| camera::parse_frame(&buf[..n as usize])).flatten()?;
                Some(Got::Frame(h, Taken::Bytes(&buf[camera::FRAME_HEADER..camera::FRAME_HEADER + h.samples_bytes()])))
            }
            Source::Slit { base, slits, after, take, band, lost, skipped } => loop {
                let Some(k) = take.next() else {
                    // Every slit of the last answer taken: wait for newer ones. The count crosses as a u32 (SLITS' `after`), 40 days of slits at the main's 1218 a second.
                    let mut q = [camera::OP_SLITS, 0, 0, 0, 0];
                    q[1..5].copy_from_slice(&(*after as u32).to_le_bytes());
                    let mut r = [0u8; 1 << 6];
                    let t0 = std::time::Instant::now();
                    let n = sys::call_live(CAMERA, &q, &mut r);
                    t.wait += t0.elapsed().as_micros() as u64;
                    *slits = (n != sys::ERR).then(|| camera::parse_slits(&r[..n as usize])).flatten()?;
                    *take = camera::slits_to_take(*after, slits);
                    *skipped += take.start() - (*after + 1);
                    *after = slits.newest;
                    continue;
                };
                let at = *base + ((k - 1) % slits.ring) as usize * slits.buffer;
                let first = slits.rows / 2 / BAND_ROWS * BAND_ROWS;
                band.resize(BAND_ROWS * slits.width, 0);
                // A RAW10 line of the widest readout fits: 8160 samples are 10200 bytes.
                let mut tmp = [0u8; 1 << 14];
                for (r, out) in band.chunks_exact_mut(slits.width).enumerate() {
                    let line = &mut tmp[..slits.line];
                    copy_from_view(at + (first + r) * slits.line, line);
                    camera::unpack_raw10(line, out);
                }
                // The slit's record, after its rows (count, counter, Eagle ns, landed): still k once its band is copied, so the engine did not come round to its buffer while it was read.
                // SAFETY: the record's words are inside buffer (k - 1) mod the ring of the view (mapped read-only for the life of the open; the driver leaves a slit buffer 64 bytes past its rows for them, ferros_cam job `slit_bytes`), aligned (rows x line is a multiple of 8); read volatile, as the driver writes them under Lumis.
                let rec = (at + slits.rows * slits.line) as *const u64;
                let (count, time) = unsafe { (core::ptr::read_volatile(rec), core::ptr::read_volatile(rec.add(2)) as i64) };
                if count != k {
                    *lost += 1;
                    continue;
                }
                return Some(Got::Slit { count: k, time, rows: SlitRows { band: &band[..], first, width: slits.width, rows: slits.rows } });
            },
        }
    }
}

/// Lumis's focus control is in diopters (Camera2's LENS_FOCUS_DISTANCE: 0 at infinity, the nearest focus at LENS_INFO_MINIMUM_FOCUS_DISTANCE); the driver's is the actuator's position (`focus_min`..`focus_max`).
/// Unmeasured on the Pro: Lumis's range is taken as 0 to 2^3 diopters (12.5 cm), mapped straight onto the actuator's range, infinity at `focus_min`.
const NEAREST_DIOPTERS: f64 = (1 << 3) as f64;

/// ISO for the driver's analogue gain: unity (1024) as ISO 100, the convention Camera2 HALs report a sensor's base gain as.
const ISO_AT_UNITY: f64 = 100.;

/// The Pro's panel, 1344 x 2992 pixels over a 6.7 inch diagonal (3280 pixels), in dots an inch: Lumis's menu draws each sensor's size and field of view in millimetres.
/// A panel ferros does not tell its size draws those off by its own ratio to the Pro's.
const DENSITY_DPI: f32 = 489.;

/// Android's gravity vector for a device held upright in portrait (m/s², +y toward the top edge), what Lumis's UI takes while the sensors give none (`sys::gravity`: before the AoC is up, or a kernel without them).
const UPRIGHT: [f32; 3] = [0., 9.80665, 0.];

/// Lumis's console lines (the `log` crate, as Lumis logs everywhere), each on the kernel console with "lumis:" before it.
struct Console;

impl log::Log for Console {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Info
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            println!("lumis: {} {}", r.level(), r.args());
        }
    }
    fn flush(&self) {}
}

/// The kist's vault domain as Lumis's config store (kete's device scope: Lumis's own choices, readable from the first launch, before any identity).
struct Vault(Arc<kete::FlatStorage>);

impl ConfigStore for Vault {
    fn read(&self, name: &str) -> Option<Vec<u8>> {
        self.0.read_device(&format!("config/{name}")).ok().flatten()
    }
    fn write(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        self.0.write_device(&format!("config/{name}"), bytes).map_err(|e| format!("{e:?}"))
    }
}

/// The vault: its key from the kist's ira, the store the kernel scopes to the kist (slot 5). None without either: Lumis then keeps nothing (its settings reset each launch, its saves are refused with a line on the console).
fn open_vault() -> Option<Arc<kete::FlatStorage>> {
    let Some(secret) = sys::ira_key(IRA, "lumis vault") else {
        println!("lumis: no ira key (slot {IRA}): nothing is kept this launch");
        return None;
    };
    match kete::FlatStorage::open_device_shared(kete::App { id: "lumis", dir: "lumis" }, secret) {
        Ok(v) => Some(v),
        Err(e) => {
            println!("lumis: the vault did not open ({e:?}): nothing is kept this launch");
            None
        }
    }
}

/// What the camera thread tells the UI once, after the open.
enum Opened {
    /// The integrator's memory (it lives for the process: leaked at creation, as Android's camera process keeps it until it ends).
    Memory { ptr: usize, bytes: usize },
    Failed(String),
}

/// The camera list as Lumis's menu takes it: each camera at the binning Lumis integrates it at, Camera2's conventions where Lumis's code reads them.
/// The optical facts (sensor size, focal length, aperture) come from the driver's list since 2026-11-17 (owner: "Missing lens stats on the Lumis page"), so the menu draws each sensor and its field of view.
/// The menu's entries, one per readout (`list`'s order, which `StartCamera` indexes): a camera's readouts share its group (its `id`), the first carrying the group's size, as Android's Lumis groups a lens's capture modes; each says the widest readout's width as the pixel array's, so the page's pixel size is the sensor's, and a readout of more samples than the camera's first is `max_res` (Android's maximum resolution).
fn camera_infos(cameras: &[camera::Camera]) -> Vec<CameraInfo> {
    cameras
        .iter()
        .enumerate()
        .filter_map(|(index, c)| {
            let group = cameras.iter().filter(|o| o.id == c.id);
            let first = cameras.iter().position(|o| o.id == c.id) == Some(index);
            let (modes, widest) = group.fold((0u32, 0u32), |(n, w), o| (n + 1, w.max(o.width)));
            // The camera's first readout (its binned one, `list` sorts by mode): a readout of more samples is its maximum resolution (Android's max_res); one of the same or fewer (the tele's 11 degrees, the front's 8.4 MP) is another mode of the lens, not that.
            let base = cameras.iter().find(|o| o.id == c.id).map_or(0, |o| o.width as u64 * o.height as u64);
            // A readout imaging less of the sensor than the camera's widest field is a crop (the tele's 11 degrees, the front's 8.4 MP), as Android's Lumis marks one; a driver before the fields gives none, and nothing is.
            let widest_field = cameras.iter().filter(|o| o.id == c.id).fold(0f32, |w, o| w.max(o.field_mm.0));
            let Some(bayer) = camera::bayer_pattern(c.cfa) else {
                println!("lumis: camera {} has a colour filter Lumis does not read (G#{:02X}); left out", c.name, c.cfa);
                return None;
            };
            Some(CameraInfo {
                id: c.name.clone(),
                index,
                facing: if c.facing == 1 { 0 } else { 1 },
                width: c.width,
                height: c.height,
                white_level: c.white as i32,
                black_level: c.black as i32,
                bayer_pattern: bayer as i32,
                supports_raw: true,
                min_iso: (c.gain_min as f64 * ISO_AT_UNITY / camera::GAIN_UNITY as f64) as i32,
                max_iso: (c.gain_max as f64 * ISO_AT_UNITY / camera::GAIN_UNITY as f64) as i32,
                min_exposure: c.exposure_min as i64,
                max_exposure: c.exposure_max as i64,
                // The optics from the driver's list (Android's Camera2 values; 0 and none from a driver before 2026-11-17, which the page shows as unknown).
                sensor_width: c.sensor_mm.0,
                sensor_height: c.sensor_mm.1,
                focal_lengths: if c.focal_mm > 0. { vec![c.focal_mm] } else { Vec::new() },
                apertures: if c.f_number > 0. { vec![c.f_number] } else { Vec::new() },
                min_focus_distance: if c.focus_max > c.focus_min { NEAREST_DIOPTERS as f32 } else { 0. },
                has_ois: false,
                // Shown as "RAW": full manual control of a raw stream.
                hardware_level: 1,
                sensor_orientation: c.rotation as i32 * 90,
                pixel_array_width: widest,
                mode_count: if first { modes } else { 1 },
                group_id: c.id as i32,
                max_res: c.width as u64 * c.height as u64 > base,
                is_cropped: c.field_mm.0 > 0. && c.field_mm.0 < widest_field,
                fov_diagonal: c.fov_diagonal_deg,
                has_cal: false,
            })
        })
        .collect()
}

/// The camera's make, model and id as Lumis on Android writes them (Build.MANUFACTURER, Build.MODEL and the module's Camera2 physical id: Make "Google", Model "Pixel 8 Pro cam 2" for the main), so an IDT from an Android still or the derived files (derived/cam2-main.idt.vsf, cam3-ultrawide, cam4-tele, cam7-front) pastes onto a ferros still or clip without --force, and the main is told from the ultrawide (Opsin's check). The Pro's ids from those files, the 8a's from its camera dump; a module not in the tables says its ferros name, and a board ferros does not say keeps the old "ferros" / "camera".
fn identity(cam: &camera::Camera) -> CameraIdentity {
    let (make, model, id) = match (sys::board(), cam.name.as_str()) {
        (Some(1), "main") => ("Google", "Pixel 8 Pro", "2"),
        (Some(1), "ultrawide") => ("Google", "Pixel 8 Pro", "3"),
        (Some(1), "tele") => ("Google", "Pixel 8 Pro", "4"),
        (Some(1), "front") => ("Google", "Pixel 8 Pro", "7"),
        (Some(1), name) => ("Google", "Pixel 8 Pro", name),
        // The 8a's from its dumpsys media.camera (keys/akita/captures-2026-11-18): logical 0's physicals 2 (main) and 3 (ultrawide; 4 is the main's 2x crop), and the front logical 1 with no physical sub-ids, which Android's Lumis lists as itself and names by its logical id.
        (Some(2), "main") => ("Google", "Pixel 8a", "2"),
        (Some(2), "ultrawide") => ("Google", "Pixel 8a", "3"),
        (Some(2), "front") => ("Google", "Pixel 8a", "1"),
        (Some(2), name) => ("Google", "Pixel 8a", name),
        (_, name) => ("ferros", "camera", name),
    };
    CameraIdentity { make: make.into(), model: model.into(), camera_id: id.into() }
}

/// The display matrix for a camera: its Absolute IDT (camera to linear VSF RGB) with its sensitivity folded in, scaled so the sensor's white in all three channels lands at green 1 (Lumis's display then puts it at its white at display gain 1).
/// A camera whose IDT takes white to no green is the driver's bug, and the division fails loud as infinities on the screen.
fn display_matrix(c: &camera::Camera) -> [f32; 9] {
    let mut m = [0f32; 9];
    for r in 0..3 {
        for k in 0..3 {
            m[3 * r + k] = c.idt[3 * r + k] * c.sensitivity[k];
        }
    }
    let green = m[3] + m[4] + m[5];
    m.map(|v| v / green)
}

/// One control to the driver: [SET][what][value u64 LE]; the reply's [0] taken, [1] outside the camera's range.
fn set(what: u8, value: u64) -> bool {
    let mut m = [0u8; 10];
    m[0] = camera::OP_SET;
    m[1] = what;
    m[2..10].copy_from_slice(&value.to_le_bytes());
    let mut r = [0u8; 4];
    sys::call_live(CAMERA, &m, &mut r) != sys::ERR && r[0] == 0
}

/// Lumis's camera service on ferros (Android: CameraInterface.kt and CameraProcessor): open the camera, build the integrator from its list entry and first frame, then a frame at a time into the integrator, applying the settings it answers, saving what it asks to save.
/// A camera that fails ends Lumis, as Camera2's errors end Android's camera process.
/// Why an open gave no stream Lumis reads.
enum Refused {
    /// Said on the console, the camera not open.
    Why(String),
    /// A slit asked for, and the driver opened the camera with no slit ring (one from before the slit, which reads the mode block's kind as no slit): the camera is open and must be closed before whole frames are asked for.
    NoSlit,
}

/// The camera opened with the view (docs/CAMERA.md "The view"): whole sensor frames read in place, or with `slit` the binned slit's ring (docs/CAMERA.md "The ring drained"), drained by SLITS. A driver or kernel that gives no view answers a plain open, and frames come by copy at `BIN` (never for a slit).
/// A development spawn's input (`dev_input`): its knobs byte, then optionally the driver's mode block (docs/CAMERA.md "Modes, the bench and the slit": [kind][rows: 2][fll: 2][exposure: 2][flags][ring][line length: 2]), so the preview can be measured with the sensor faster than the init's 15 a second; a kist opened from the map has no input and opens as before. A slit's open takes its own block.
/// With `video` (a pacing and the opening shutter, ns) the open is RAW video's: the binned readout at the pacing's frame length, write-back (ferros_clip `Pacing::open_msg`).
fn open(cam: &camera::Camera, slit: bool, video: Option<(ferros_clip::Pacing, u64)>) -> Result<Source, Refused> {
    let mut ask = vec![camera::OP_OPEN, cam.id, camera::OPEN_VIEW];
    let (knobs, block) = dev_input();
    if let Some((pacing, shutter_ns)) = video {
        ask = pacing.open_msg(cam.id, shutter_ns).to_vec();
        println!("lumis: camera {}: RAW video: the open at {} a second (frame length {} lines, every {} kept)", cam.name, pacing.fps, pacing.fll, pacing.keep);
    } else if slit {
        ask.extend_from_slice(&camera::slit_block());
        // A slit ring is read by SLITS and never handed over by a MAP, the one that makes a buffer's lines right for the caches, so it is asked for non-cacheable (a slit's band is a few rows).
        ask.push(0);
    } else if knobs & DEV_COPY != 0 && cam.mode == 0 {
        ask.truncate(2);
        println!("lumis: camera {}: the open asks for frames by copy (no view)", cam.name);
    } else if cam.mode != 0 {
        // A readout of its own (the 1:1 full resolution): its kind and the rest the driver's (its frame and integration the replayed init's), written back by the view (MAP only: the driver gives no copy at 1:1).
        ask.push(cam.mode);
        ask.resize(3 + camera::MODE_BLOCK, 0);
        ask.push(if dev_knobs2() & DEV_UNCACHED != 0 { 0 } else { camera::OPEN_VIEW_CACHED });
        println!("lumis: camera {}: the open asks for readout {} ({} x {})", cam.name, cam.mode, cam.width, cam.height);
    } else {
        if !block.is_empty() {
            println!("lumis: camera {}: the open asks for the mode block G#{}", cam.name, block.iter().map(|b| format!("{b:02X}")).collect::<String>());
        }
        // The mode block whole (zeros past what the input gave: the init's mode, as no block is), then the view write-back (docs/CAMERA.md "A cacheable view"): the rows read thru the caches, 2.8 ms a frame on the A510s against 41.7 non-cacheable (cambench, the Pro, 2026-11-18).
        ask.extend_from_slice(&block);
        ask.resize(3 + camera::MODE_BLOCK, 0);
        ask.push(if dev_knobs2() & DEV_UNCACHED != 0 { 0 } else { camera::OPEN_VIEW_CACHED });
    }
    // An open with a mode block is answered with 64 bytes (the ring's count and the slit's facts after the view's 48).
    let mut r = [0u8; 1 << 6];
    let n = sys::call_live(CAMERA, &ask, &mut r);
    match (n, r[0]) {
        (sys::ERR, _) => return Err(Refused::Why("the camera driver did not answer the open".into())),
        (_, 0) => {}
        (_, 1) => return Err(Refused::Why("the driver has no such camera".into())),
        (_, 2) => return Err(Refused::Why("another kist has this camera open".into())),
        (_, k) => return Err(Refused::Why(format!("the driver refused the open ({k})"))),
    }
    // The view's token is kept in this process's memory for the life of the camera: the kernel answers its base from it (32 bytes an open).
    match camera::parse_view(&r[..n as usize]) {
        Some(v) if v.format == camera::FORMAT_RAW10 => {
            if slit && ((n as usize) < camera::VIEW_REPLY_MODE || r[52] != 1) {
                return Err(Refused::NoSlit);
            }
            let token = Box::leak(Box::new(v.token));
            let base = sys::dma(token);
            if base == sys::ERR {
                return Err(Refused::Why("the view's token was refused".into()));
            }
            if slit {
                println!("lumis: camera {}: slits, a ring of {} buffers of {} x {} RAW10 at G#{base:X}", cam.name, u32::from_le_bytes(r[48..52].try_into().unwrap()), v.width, v.height);
                let slits = camera::Slits { newest: 0, ring: u32::from_le_bytes(r[48..52].try_into().unwrap()) as u64, buffer: v.buffer, rows: v.height, line: v.line, width: v.width };
                return Ok(Source::Slit { base: base as usize, slits, after: 0, take: 1..=0, band: Vec::new(), lost: 0, skipped: 0 });
            }
            println!("lumis: camera {}: the view, {} buffers of {} x {} RAW10 at G#{base:X}, {}", cam.name, v.buffers, v.width, v.height, if v.cached { "write-back" } else { "non-cacheable" });
            // The GPU told: the view mapped in Lumis's GPU space, where the stack reads each frame (`ViewRows::stack`).
            gpu::camera_changed(true);
            Ok(Source::View { base: base as usize, view: v, held: camera::NONE })
        }
        Some(v) => Err(Refused::Why(format!("the view's format {} is not one Lumis reads", v.format))),
        None if slit => Err(Refused::NoSlit),
        None if video.is_some() => Err(Refused::Why("no view for RAW video: its frames are read in place only".into())),
        None if cam.mode != 0 => Err(Refused::Why(format!("no view for readout {}: it is read in place only", cam.mode))),
        None => {
            println!("lumis: camera {}: no view; frames by copy at bin {BIN}", cam.name);
            // A frame's reply: the header and up to 8 MiB of samples (REPLY_MAX).
            Ok(Source::Copy { buf: vec![0u8; camera::FRAME_HEADER + (1 << 23)] })
        }
    }
}

fn camera_service(cam: camera::Camera, calibration: Option<bool>, vault: Option<Arc<kete::FlatStorage>>, opened: Arc<Mutex<Option<Opened>>>) {
    let fail = |why: String| {
        println!("lumis: camera {}: {why}", cam.name);
        *opened.lock().unwrap() = Some(Opened::Failed(why));
        // Sent behind while the camera opened: no menu to go back to, so Lumis ends (a window sent behind after this store finds the failure in its tick and ends there).
        if BEHIND.load(Ordering::Acquire) {
            close_camera(&cam.name);
            lumis_core::platform::exit_because("sent behind");
        }
    };
    let (knobs, _) = dev_input();
    let dev_slit = dev_knobs2() & DEV_SLIT != 0;
    let mut src = match open(&cam, dev_slit, None) {
        Ok(s) => s,
        Err(Refused::Why(why)) => return fail(why),
        Err(Refused::NoSlit) => {
            close_camera(&cam.name);
            return fail("the development slit open: the driver gave no slit ring".into());
        }
    };
    let mut turns = Turns::default();
    // The last frame or slit taken (whether a slit, the driver's count, Eagle ns): the sensor's rate from one to the next (`set_sensor_rate`).
    let mut last_frame: Option<(bool, u64, i64)> = None;
    let first = if dev_slit {
        println!("lumis: camera {}: development: slits from the open, Lumis sized from the list ({} x {})", cam.name, cam.width, cam.height);
        let shutter_ns = match dev_shutter_us() {
            0 => (1_000_000_000u64 >> 11) << (2 * ((dev_knobs2() >> DEV_SLIT_SHUTTER) & 3)),
            us => us as u64 * 1000,
        };
        println!("lumis: camera {}: development: the shutter {shutter_ns} ns", cam.name);
        camera::FrameHeader { count: 0, width: cam.width as usize, height: cam.height as usize, bin: 1, cfa: cam.cfa, black: cam.black as u16, white: cam.white as u16, exposure_ns: shutter_ns, gain: camera::GAIN_UNITY, focus: cam.focus_min, time: 0, cfa_period: cam.cfa_period }
    } else {
        let Some(Got::Frame(first, _)) = src.next(&mut turns) else {
            return fail("no first frame".into());
        };
        first
    };
    println!("lumis: camera {}: first frame, {} x {} samples (bin {}), exposure {} us, gain {}/1024", cam.name, first.width, first.height, first.bin, first.exposure_ns / 1000, first.gain);
    // Sent behind while the camera opened: nothing framed and nothing to save, so the camera closes before the integrator's memory is taken.
    // A calibration asked for stays: once begun, it runs behind as a continuous save does (`kept`).
    if BEHIND.load(Ordering::Acquire) && calibration.is_none() {
        close_camera(&cam.name);
        lumis_core::platform::exit_because("sent behind");
    }
    let iso = |gain: u32| gain as f64 * ISO_AT_UNITY / camera::GAIN_UNITY as f64;
    let (min_iso, max_iso) = (iso(cam.gain_min), iso(cam.gain_max));
    // The first open's seed (Android: the one-shot AE's metered values, or the calibration's extremes): the driver's own exposure and gain, as its first frame says.
    let (seed_iso, seed_shutter) = match calibration {
        // A dark map opens at lumis_core's dark seed (16 s, Android's longest) or the camera's longest if shorter: the combined driver offers years (the main's shift modes), and a calibration must not open on one. The shutter control keeps the whole range.
        Some(dark) => (max_iso, if dark { (cam.exposure_max as f64).min(app::DARK_CALIBRATION_SHUTTER_NS) } else { cam.exposure_min as f64 }),
        None if dev_knobs2() & DEV_LONG != 0 => (iso(first.gain), ((1u64 << 10) * 1_000_000_000) as f64),
        None if dev_shutter_us() != 0 => (iso(first.gain), dev_shutter_us() as f64 * 1000.),
        None => (iso(first.gain), first.exposure_ns as f64),
    };
    let setup = CameraSetup {
        width: first.width,
        height: first.height,
        white_level: first.white,
        black_level: first.black,
        bayer_pattern: camera::bayer_pattern(first.cfa).expect("the list's colour filter was read; the frame's is the same sensor's"),
        facing: if cam.facing == 1 { 0 } else { 1 },
        orientation: cam.rotation as i32 * 90,
        min_iso,
        max_iso,
        min_exposure_ns: cam.exposure_min as f64,
        max_exposure_ns: cam.exposure_max as f64,
        min_focus: if cam.focus_max > cam.focus_min { NEAREST_DIOPTERS } else { 0. },
        initial_iso: seed_iso,
        initial_shutter_ns: seed_shutter,
        focal_length_mm: 0.,
        aperture_fnum: 0.,
        sensor_diag_mm: 0.,
        colour: SensorColour::CameraToDisplay(display_matrix(&cam)),
    };
    lumis_core::set_camera_identity(identity(&cam));
    // The integrator's memory is the UI's too, for the life of the process (the UI reads it every frame), so the integrator is never dropped.
    let integrator = Box::leak(app::camera_new(&setup));
    if let Some(dark) = calibration {
        app::calibration_start(integrator, dark);
    }
    if dev_slit {
        integrator.header[CURRENT_MODE_IDX] = RawMode::Slitscan as u64;
    }
    // RAW video on any binned Bayer readout (mode 0: the pacing's open is the binned kind's) whose line the driver gives (ferros_clip `Line`, forge 117fadb5: each camera paced by its own clock, the GPU coding every binned Bayer readout), its frames read in place from the view; the FPS slider to its fastest whole frame (the Pro: the main 60, the ultrawide 48, the tele 51, the front 33). Quad readouts record none: the GPU codes Bayer only.
    let line = cam.line.filter(|l| cam.mode == 0 && cam.cfa_period == 2 && l.fps_most() != 0 && matches!(src, Source::View { .. }));
    let video_ok = line.is_some();
    if let Some(l) = line {
        lumis_core::platform::set_video_fps_most(l.fps_most() as u64);
    }
    lumis_core::platform::set_video_offered(video_ok);
    // A rate's pacing on this readout: every rate `video_fps` gives is one it holds.
    let pace = |fps: u32| ferros_clip::Pacing::of_line(fps, line.expect("RAW video only where the driver gives a line (video_ok)")).expect("a rate up to the readout's fastest is paced");
    if video_ok && dev_knobs4() & DEV_VIDEO != 0 {
        integrator.header[CURRENT_MODE_IDX] = RawMode::Video as u64;
        integrator.header[lumis_core::shared_memory::VIDEO_FPS_IDX] = dev_video_fps() as u64;
        println!("lumis: development: RAW video at {} a second, recording from the 16th frame", video_fps(&integrator.header));
    }
    *opened.lock().unwrap() = Some(Opened::Memory { ptr: integrator.get_shared_memory_ptr() as usize, bytes: app::shared_memory_bytes(first.width, first.height) });
    let gain_of = |iso: i32| (iso as f64 * camera::GAIN_UNITY as f64 / ISO_AT_UNITY) as u64;
    let focus_of = |diopters: f32| cam.focus_min as u64 + ((diopters as f64 / NEAREST_DIOPTERS) * (cam.focus_max - cam.focus_min) as f64) as u64;
    // What the sensor was last told, so a setting goes to the driver only when it changes (as CameraProcessor.updateCameraSettings).
    let mut sent: (i32, i64, f32) = (-1, -1, -1.);
    let mut frames = 0u64;
    // Development runs (`DEV_OPEN`, `DEV_SWEEP`): the run's length, and the shutter sweep's steps (ns) with the step in hand and when its frames began.
    // The run's length: 2^(3 to 6) s by the knobs' bits 4 and 5, or 2^n s by the input's 25th byte (longer runs: a RAW video clip until its space is full).
    let run = (knobs & DEV_OPEN != 0).then(|| {
        let mut b = [0u8; 1 << 5];
        let n = sys::input(0, &mut b) as usize;
        let at = 11 + camera::MODE_BLOCK;
        let log2 = if n > at && b[at] != 0 { b[at] as u32 } else { ((knobs >> 4) & 3) as u32 + 3 };
        std::time::Duration::from_secs(1 << log2)
    });
    let run_start = std::time::Instant::now();
    let sweep: Vec<f64> = if knobs & DEV_OPEN != 0 && knobs & DEV_SWEEP != 0 {
        (0..9).map(|k| (2f64).powi(-13 + 2 * k)).chain([(1 << 4) as f64]).map(|s| s * 1e9).filter(|&ns| ns <= cam.exposure_max as f64).collect()
    } else {
        Vec::new()
    };
    let mut sweep_frames = 0usize;
    let mut sweep_last = std::time::Instant::now();
    if let Some(r) = run {
        println!("lumis: development run of {} s{}", r.as_secs(), if sweep.is_empty() { String::new() } else { format!(", sweeping the shutter over {} steps", sweep.len()) });
    }
    // What this thread last said of the window, so each change is said once.
    let mut stay = Stay::Front;
    // The UI's heartbeat as it stood while the window was behind: until the UI draws again and moves it, the integrator would read it as a UI gone quiet past its 1.6 s and end Lumis (lumis_core integrator `process_frame`), so the frames it would integrate are passed over instead.
    let mut quiet: Option<[u64; 2]> = None;
    // Whether this camera gives slits: until an open says it does not (a driver from before the slit, a camera with no slit mode, a slit narrower or wider than the frame), Slitscan opens it for slits; after, Slitscan takes its band from whole frames, as on Android.
    let mut slit_ok = knobs & DEV_COPY == 0 && cam.mode == 0;
    // Said once at the open, as every other reason Slitscan takes whole frames is (the slit is the driver's binned mode's, so another readout of the camera, its 1:1 or a crop, has none of its own).
    if cam.mode != 0 {
        println!("lumis: camera {}: readout {} ({} x {}) has no slit of its own: Slitscan takes whole frames on it", cam.name, cam.mode, cam.width, cam.height);
    }
    // RAW video: the rate the camera is open at (None: opened as ever, not paced), and the clip recording.
    let mut paced: Option<u32> = None;
    // The driver's count from which frames run at `paced`'s length after a frame length SET in place (0: from the open).
    let mut paced_from: u64 = 0;
    // A rate whose RAW video open the driver refused (until another is chosen).
    let mut refused_fps: Option<u32> = None;
    // A clip being started (`start_clip` on its own thread): its answer, taken at the first turn after it comes.
    let mut starting: Option<std::sync::mpsc::Receiver<Result<Clip, String>>> = None;
    // A RAW video preview frame's settings, integrated beside its hand-over this turn.
    let mut preview_done: Option<(i32, i64, f32)> = None;
    // When RAW video's preview last integrated a frame while not recording.
    let mut idle_preview = std::time::Instant::now();
    // The last frame's integration, us: said with a RAW video drop that may follow it.
    let mut last_preview_us = 0u64;
    // RAW video's preview while a clip records (`PreviewJob`), its thread started once a camera; previews it dropped as not whole, said on the console.
    let preview = preview_thread(&mut *integrator as *mut lumis_core::CameraIntegrator as usize);
    if preview.is_none() {
        println!("lumis: camera {}: no thread for RAW video's preview: no preview while a clip records", cam.name);
    }
    let mut previews_dropped = 0u64;
    // The settings came from a preview on its thread this turn (its time is `last_preview_us` already).
    let mut preview_async = false;
    // The GPU block grown to the recorder's size for this open's RAW video (`video_block`), once.
    let mut video_grown = false;
    // The view (its token) whose frame length was set to the readout's fastest for stills (`still_fast`): each open, and each return from RAW video, sets it again.
    let mut still_fast: Option<[u8; 32]> = None;
    // When the last reopen began (its CLOSE) and when its OPEN was answered, until its first frame or slit: the console says the three apart (a stored driver ends at CLOSE and the next OPEN starts it again on demand, a path first run here).
    let mut reopened: Option<(std::time::Instant, std::time::Instant)> = None;
    let slit_later = (dev_knobs2() & DEV_SLIT_LATER != 0).then(|| std::time::Instant::now() + std::time::Duration::from_millis(1 << 12));
    // The first frame again, now that the integrator exists: the next one (the first went to its sizing and seed).
    loop {
        // A development run ends at its time whatever the turn did (a RAW video turn that hands a frame over and skips the preview reaches no check further down).
        if let Some(r) = run {
            if run_start.elapsed() >= r {
                println!("lumis: development run of {} s ended after {frames} frames", r.as_secs());
                if let Some(c) = CLIP.lock().unwrap().take() {
                    end_clip(c, Some(&mut *integrator.header), "the development run ended");
                }
                keep_saves(vault.as_deref(), &mut *integrator.header);
                close_camera(&cam.name);
                lumis_core::platform::exit_because("the development run ended");
            }
        }
        // The view read only while this is held (`VIEW`), released for the driver's wait below.
        let mut reading = VIEW.lock().unwrap();
        if LEAVING.load(Ordering::Acquire) {
            drop(reading);
            if let Some(c) = CLIP.lock().unwrap().take() {
                end_clip(c, Some(&mut *integrator.header), "Lumis leaving");
            }
            finish_leaving(integrator, vault.as_deref());
        }
        if slit_later.is_some_and(|t| std::time::Instant::now() >= t) && integrator.header[CURRENT_MODE_IDX] != RawMode::Slitscan as u64 {
            println!("lumis: development: Slitscan");
            integrator.header[CURRENT_MODE_IDX] = RawMode::Slitscan as u64;
        }
        // Out of Slitscan on whole frames, slits are asked for again at the next entry: a slit that failed or stopped once (a driver restart, a refused open) cost Slitscan its slits for the rest of the camera's session, its reason said once and every later entry silent on whole frames (the owner, 2026-11-20: "it's doing it again ... it actually says 14.9:F"). Each entry tries once and says why when it cannot.
        if !slit_ok && !wants_slits(&integrator.header) && !src.is_slit() && knobs & DEV_COPY == 0 && cam.mode == 0 {
            slit_ok = true;
        }
        // Slitscan takes slits, every other mode whole frames: the camera is closed (the driver ends) and opened again for the other (owner, 2026-11-18: "Do wire it up so I get more than 30 lines per second"). The new driver starts at the init's settings, so the integrator's are sent again.
        if slit_ok && wants_slits(&integrator.header) != src.is_slit() {
            let slit = !src.is_slit();
            let t_close = std::time::Instant::now();
            report(&src, &mut turns, &cam.name);
            keep_saves(vault.as_deref(), &mut *integrator.header);
            close_camera(&cam.name);
            println!("lumis: camera {}: {}", cam.name, if slit { "Slitscan: opening for slits" } else { "out of Slitscan: opening for whole frames" });
            src = match open(&cam, slit, None) {
                Ok(s) => match &s {
                    Source::Slit { slits, .. } if slits.width != first.width => {
                        println!("lumis: camera {}: its slits are {} wide, its frames {}: Slitscan takes whole frames", cam.name, slits.width, first.width);
                        slit_ok = false;
                        close_camera(&cam.name);
                        reopen_whole(&cam, vault.as_deref(), &mut *integrator.header)
                    }
                    _ => s,
                },
                Err(Refused::NoSlit) => {
                    println!("lumis: camera {}: the driver opened it with no slit ring (one from before the slit, or a camera with no slit mode): Slitscan takes whole frames", cam.name);
                    slit_ok = false;
                    close_camera(&cam.name);
                    reopen_whole(&cam, vault.as_deref(), &mut *integrator.header)
                }
                Err(Refused::Why(why)) if slit => {
                    println!("lumis: camera {}: the slit's open failed ({why}): Slitscan takes whole frames", cam.name);
                    slit_ok = false;
                    reopen_whole(&cam, vault.as_deref(), &mut *integrator.header)
                }
                Err(Refused::Why(why)) => {
                    keep_saves(vault.as_deref(), &mut *integrator.header);
                    lumis_core::platform::exit_because(&format!("camera {}: opened again for whole frames, it failed: {why}", cam.name));
                }
            };
            sent = (-1, -1, -1.);
            paced = None;
            turns = Turns::default();
            // A reopen starts the driver's count over: its frames are at their length from the first.
            paced_from = 0;
            reopened = Some((t_close, std::time::Instant::now()));
        }
        // RAW video's rate is fixed at the camera's open (the driver has no SET for it): entering RAW video, a new rate (the UI holds it while a clip records) or leaving it closes the camera and opens it again, paced or as ever. A clip still recording is ended first (the UI stops it on leaving the mode).
        // A clip deleted from the clips page: the space read again.
        if integrator.header[FLAGS_IDX] & lumis_core::shared_memory::SPACE_STALE_BIT != 0 {
            integrator.header[FLAGS_IDX] &= !lumis_core::shared_memory::SPACE_STALE_BIT;
            read_space(&mut *integrator.header);
        }
        // A rate whose open the driver refused is not asked again until another is chosen (the camera then stays at whole frames as ever, and the note under RECORD says why).
        let want = if video_ok && integrator.header[CURRENT_MODE_IDX] == RawMode::Video as u64 { Some(video_fps(&integrator.header)).filter(|&f| refused_fps != Some(f)) } else { None };
        if refused_fps.is_some_and(|f| Some(f) != Some(video_fps(&integrator.header))) {
            refused_fps = None;
        }
        // Into or out of RAW video, or a new rate, on a camera streaming whole frames: the frame length SET in place (ferros_clip `Pacing::set_msg`, payloads/camera SET what 4; the owner, 2026-11-20: "from still to video, why the mode switch? it takes like 5 seconds"), the camera left running; only a refused SET (a driver from before it, a frame it cannot hold) closes and opens it again below. The frames at the new length are those SET_SETTLES or more past the last one taken (`paced_from`): no clip starts before them.
        if want != paced && !src.is_slit() {
            let msg = match want {
                Some(fps) => pace(fps).set_msg(),
                None => ferros_clip::set_open_fll_msg(),
            };
            let mut r = [0u8; 4];
            let n = sys::call_live(CAMERA, &msg, &mut r);
            if n != sys::ERR && n >= 1 && r[0] == 0 {
                if let Some(c) = CLIP.lock().unwrap().take() {
                    end_clip(c, Some(&mut *integrator.header), "the rate changed");
                }
                paced_from = last_frame.map_or(0, |(_, c, _)| c + ferros_clip::SET_SETTLES);
                match want {
                    Some(fps) => {
                        println!("lumis: camera {}: RAW video at {fps} a second, the frame length set in place (no reopen)", cam.name);
                        read_space(&mut *integrator.header);
                    }
                    None => println!("lumis: camera {}: out of RAW video, the open's frame length set back in place (no reopen)", cam.name),
                }
                paced = want;
            } else {
                println!("lumis: camera {}: the frame length SET was refused ({}): the camera is opened again", cam.name, if n == sys::ERR { "no answer".to_string() } else { format!("answer {}", r[0]) });
            }
        }
        // Stills (every mode but RAW video) at the readout's fastest whole frame (Line::fps_most: the Pro's main 60, ultrawide 48, tele 50, front 33): the open's own frame length is the init's, 15 a second on the main, so integrations topped out at about 14 a second at any exposure (the owner, 2026-11-22: "it's topped out at 14 integrations per second on the pro, doesn't matter if it's a year long image or ten seconds"). The driver lengthens a frame when the shutter needs it, so a long shutter is as before.
        if let (None, None, Some(l), Source::View { view, .. }) = (want, paced, line, &src) {
            if still_fast != Some(view.token) {
                still_fast = Some(view.token);
                match ferros_clip::Pacing::of_line(l.fps_most(), l) {
                    Some(p) => {
                        let mut r = [0u8; 4];
                        let n = sys::call_live(CAMERA, &p.set_msg(), &mut r);
                        if n != sys::ERR && n >= 1 && r[0] == 0 {
                            println!("lumis: camera {}: stills at the readout's fastest frame, {} a second (its shutter lengthens it as it needs)", cam.name, l.fps_most());
                        } else {
                            println!("lumis: camera {}: the stills' frame length SET was refused ({}): the open's own frame length stays", cam.name, if n == sys::ERR { "no answer".to_string() } else { format!("answer {}", r[0]) });
                        }
                    }
                    None => println!("lumis: camera {}: no pacing for the readout's fastest frame: the open's own frame length stays", cam.name),
                }
            }
        }
        if paced.is_some() {
            still_fast = None;
        }
        if want != paced && !src.is_slit() {
            if let Some(c) = CLIP.lock().unwrap().take() {
                end_clip(c, Some(&mut *integrator.header), "the camera reopened");
            }
            let t_close = std::time::Instant::now();
            report(&src, &mut turns, &cam.name);
            keep_saves(vault.as_deref(), &mut *integrator.header);
            close_camera(&cam.name);
            src = match want {
                // Opened with no shutter of its own (the open's least, a line), as std/rawrec opens: the driver takes no integration past the paced frame at the open ("no such mode (Integration(21342, 10586))", 2026-11-19, the Pro: Lumis's 67 ms in a 30 a second frame), while a SET is planned, the frame lengthened when the shutter is longer; the settings are all sent again after a reopen.
                // The stored driver ends at the CLOSE and starts again on demand; an OPEN that lands while the old one exits finds no server ("the camera driver did not answer the open", 2026-11-19, the Pro: answered 3 ms after the CLOSE, the owner's 60 a second lost to it), so a refused one is asked again, 2^8 ms apart, up to 2^2 times.
                Some(fps) => match (0..1 << 2).find_map(|k| {
                    if k > 0 {
                        std::thread::sleep(std::time::Duration::from_millis(1 << 8));
                    }
                    match open(&cam, false, Some((pace(fps), 0))) {
                        Err(Refused::Why(why)) if k + 1 < 1 << 2 => {
                            println!("lumis: camera {}: RAW video's open at {fps} a second refused ({why}); asking again", cam.name);
                            None
                        }
                        r => Some(r),
                    }
                }).expect("the last try answers") {
                    Ok(s) => {
                        paced = want;
                        read_space(&mut *integrator.header);
                        s
                    }
                    Err(Refused::Why(why)) => {
                        // Whole frames as ever, said under RECORD; this rate is not asked again until another is chosen (2026-11-19, the owner at 60 a second: RAW video was switched off for the session without a word, and RECORD did nothing).
                        println!("lumis: camera {}: RAW video's open at {fps} a second failed ({why}): whole frames as ever", cam.name);
                        lumis_core::platform::set_video_note(Some(format!("{fps} a second would not open: {why}")));
                        refused_fps = Some(fps);
                        paced = None;
                        reopen_whole(&cam, vault.as_deref(), &mut *integrator.header)
                    }
                    Err(Refused::NoSlit) => unreachable!("RAW video's open is never a slit's"),
                },
                None => {
                    paced = None;
                    reopen_whole(&cam, vault.as_deref(), &mut *integrator.header)
                }
            };
            sent = (-1, -1, -1.);
            turns = Turns::default();
            paced_from = 0;
            reopened = Some((t_close, std::time::Instant::now()));
        }
        let slit_now = src.is_slit();
        drop(reading);
        // The view's ring (its buffers), read before the frame borrows the source: RAW video's preview off the camera thread needs four or more (`PreviewJob`).
        let ring = match &src {
            Source::View { view, .. } => view.buffers as u64,
            _ => 0,
        };
        let got = src.next(&mut turns);
        reading = VIEW.lock().unwrap();
        // Closed for leaving while this waited: the frame it may have brought is not read (the view is the driver's again).
        if LEAVING.load(Ordering::Acquire) {
            drop(reading);
            if let Some(c) = CLIP.lock().unwrap().take() {
                end_clip(c, Some(&mut *integrator.header), "Lumis leaving");
            }
            finish_leaving(integrator, vault.as_deref());
        }
        let got = match got {
            Some(got) => got,
            // A slit stream that ends (the Pro, 2026-11-18: a stored driver from before the detached slit stops after one; the forge driver's at 1296 and 3236 slits on LATE_CONFIG) ends that driver, not Lumis: whole frames are opened (no CLOSE: the driver closed itself, and a call would start a stored one on demand only to close it), and Slitscan takes its band from them.
            None if slit_now => {
                println!("lumis: camera {}: the slits stopped (the driver ended); whole frames again, and Slitscan takes its band from them", cam.name);
                slit_ok = false;
                src = reopen_whole(&cam, vault.as_deref(), &mut *integrator.header);
                sent = (-1, -1, -1.);
                turns = Turns::default();
                paced_from = 0;
                continue;
            }
            None => {
                if let Some(c) = CLIP.lock().unwrap().take() {
                    end_clip(c, Some(&mut *integrator.header), "the camera gave no frame");
                }
                // What is encoded already is kept first; a save still encoding is named as lost (lumis_core platform::exit_because).
                keep_saves(vault.as_deref(), &mut *integrator.header);
                lumis_core::platform::exit_because(&format!("camera {}: no frame (the driver refused or ended), as Android's camera process ends on a camera error", cam.name));
            }
        };
        if let Some((t_close, t_open)) = reopened.take() {
            println!("lumis: camera {}: reopened: the OPEN answered A#{} ms after the CLOSE began, the first {} A#{} ms after it", cam.name, (t_open - t_close).as_millis(), if src_is_slit(&got) { "slit" } else { "frame" }, t_close.elapsed().as_millis());
        }
        if let Got::Frame(head, _) = &got {
            if head.width != first.width || head.height != first.height {
                keep_saves(vault.as_deref(), &mut *integrator.header);
                lumis_core::platform::exit_because(&format!("camera {}: a frame of {} x {} after {} x {}: the integrator is sized for the first", cam.name, head.width, head.height, first.width, first.height));
            }
        }
        // Behind, decided at each frame (the frame is what this thread waits on; nothing here polls or times): Lumis leaves, unless a continuous save or a calibration runs (it keeps capturing, and the UI draws nothing) or a save the person asked for is still on its way (it is finished and kept first).
        let behind = BEHIND.load(Ordering::Acquire);
        let flags = integrator.header[FLAGS_IDX];
        let kept = kept(flags);
        let now = match (behind, kept, in_flight(flags)) {
            (false, _, _) => Stay::Front,
            (true, true, _) if flags & lumis_core::shared_memory::RECORDING_BIT != 0 => Stay::Recording,
            (true, true, _) if flags & CONTINUOUS_SAVE_BIT != 0 => Stay::Continuous,
            (true, true, _) => Stay::Calibrating,
            (true, false, true) => Stay::Finishing,
            (true, false, false) => leave(&cam.name, vault.as_deref(), &mut *integrator.header),
        };
        if now != stay {
            match now {
                Stay::Front => println!("lumis: in front again: carrying on"),
                Stay::Recording => println!("lumis: sent behind (or the screen off) while a RAW video clip records: staying for it, recording on, drawing nothing; the clip ends as always (STOP, its room full, a write failing), then Lumis ends if still behind"),
                Stay::Continuous => println!("lumis: sent behind during a continuous save: staying for the save, capturing and saving, drawing nothing; Lumis ends if the save ends while behind"),
                Stay::Calibrating => println!("lumis: sent behind during a calibration: staying for it, capturing, drawing nothing; Lumis ends if it ends while behind"),
                Stay::Finishing => println!("lumis: sent behind with a save on its way: finishing it, then Lumis ends"),
            }
            stay = now;
        }
        let heartbeat = [integrator.header[HEARTBEAT_SECS_IDX], integrator.header[HEARTBEAT_NANOS_IDX]];
        if behind {
            quiet = Some(heartbeat);
        } else if quiet != Some(heartbeat) {
            quiet = None;
        }
        // F, the sensor's own rate (the owner, 2026-11-20): the frames, or in Slitscan the slits, the driver counted since the last one taken over the time its clock says passed, at every one taken (one the preview passes over too), not Lumis's intake. A slit's count and time are its record's (the owner: "the :Fps number up top is wrong for slitscan. I doubt it's 14": it kept the whole frames' rate).
        // WHY the order and kind checks: the count and the time are the driver's, both start over when the camera reopens, and a slit's count is not a frame's; such a one only starts the next measure.
        let stamp = match &got {
            Got::Frame(head, _) => (false, head.count as u64, head.time),
            Got::Slit { count, time, .. } => (true, *count, *time),
        };
        if !stamp.0 {
            NEWEST.store(stamp.1, Ordering::Release);
        }
        // No clip recording: a preview still on its thread ends before the integrator is used here (during a clip only its header is, `PreviewJob`).
        if CLIP.lock().unwrap().is_none() {
            preview_settle();
        }
        // A preview's end on its thread: its settings sent as an integration's are (below), its time said with a drop, a preview that was not whole counted.
        preview_async = false;
        if let Some(d) = preview.as_ref().and_then(|(_, done)| done.try_recv().ok()) {
            last_preview_us = d.us;
            if !d.whole {
                previews_dropped += 1;
                if previews_dropped.is_power_of_two() {
                    println!("lumis: camera {}: RAW video: A#{previews_dropped} previews dropped so far, read past the view's ring (the last whole one stayed on screen)", cam.name);
                }
            }
            preview_done = Some(d.settings);
            preview_async = true;
        }
        if let Some((_, c0, t0)) = last_frame.filter(|&(slit, c0, t0)| slit == stamp.0 && stamp.1 > c0 && stamp.2 > t0) {
            let fps = (stamp.1 - c0) as f64 * 1e9 / (stamp.2 - t0) as f64;
            // While a preview uses the integrator on its thread, the rate goes straight into the header (what set_sensor_rate writes once it has been called, as it has by any clip).
            if previewing() {
                integrator.header[lumis_core::shared_memory::FPS_IDX] = fps.to_bits();
            } else {
                integrator.set_sensor_rate(fps);
            }
        }
        last_frame = Some(stamp);
        // A continuous save or a calibration integrates on: the integrator never ends Lumis on a quiet UI while either runs (lumis ferros-cal-behind).
        // Otherwise, behind (or in front again before the UI has drawn), nothing is integrated: the save on its way is the image already made, so it is started if asked for and not yet begun, kept when encoded, and Lumis leaves once nothing is left on its way.
        if !kept && quiet.is_some() {
            integrator.try_save();
            if integrator.check_and_finalize_calibration() {
                println!("lumis: calibration finalized");
            }
            keep_saves(vault.as_deref(), &mut *integrator.header);
            if behind && !in_flight(integrator.header[FLAGS_IDX]) {
                leave(&cam.name, vault.as_deref(), &mut *integrator.header);
            }
            continue;
        }
        let t_integrate = std::time::Instant::now();
        let (count, time) = match &got {
            Got::Frame(head, _) => (head.count, head.time),
            // The count crosses the console's arithmetic as a u32, as a frame's does (40 days of slits).
            Got::Slit { count, time, .. } => (*count as u32, *time),
        };
        if turns.first.is_none() {
            turns.first = Some((count, time));
            turns.images = integrator.header[lumis_core::shared_memory::IMAGE_COUNTER_IDX];
        }
        turns.last = (count, time);
        if let (false, Got::Frame(head, _)) = (sweep.is_empty(), &got) {
            // Each frame's exposure as the driver says it took it, beside the step asked two frames before it; then the next step's shutter into the integrator's header, which it hands back to be set.
            let step = sweep_frames / 2;
            let now = std::time::Instant::now();
            if step < sweep.len() {
                println!("lumis: sweep: frame {} took {} ns (asked {} ns), {} ms after the last", head.count, head.exposure_ns, sweep[step] as u64, (now - sweep_last).as_millis());
                integrator.header[SHUTTER_NS_IDX] = sweep[step].to_bits();
            } else if sweep_frames == 2 * sweep.len() {
                println!("lumis: sweep: done after {} frames", sweep_frames);
            }
            sweep_last = now;
            sweep_frames += 1;
        }
        // In RAW video, the GPU block grown to the recorder's size at its first paced frame, not at RECORD: the grow makes Lumis's UI queue another client to the GPU service, which the UI opens again at its next frame, and that reopen beside a clip's first frames cost each clip its first frame at 48 to 60 a second (the Pro, 2026-11-21: one dropped at clip frame 1 on the main, the ultrawide and the tele, none at the front's 33). Capture first: the reopen lands here, before RECORD.
        if let (false, Some(_), Got::Frame(head, Taken::View(rows))) = (video_grown, paced, &got) {
            video_grown = true;
            video_block(&cam.name, head.height, rows.line);
        }
        {
            use lumis_core::shared_memory::{RECORDING_BIT, VIDEO_DROPPED_IDX, VIDEO_FRAMES_IDX, VIDEO_LEFT_S_IDX};
            if video_ok && dev_knobs4() & (DEV_VIDEO | DEV_VIDEO_IDLE) == DEV_VIDEO && frames == 16 && paced.is_some() {
                integrator.header[FLAGS_IDX] |= RECORDING_BIT;
            }
            let asked = integrator.header[FLAGS_IDX] & RECORDING_BIT != 0;
            // Held for this frame's hand-over only (`seal_at_exit` takes it from another thread otherwise).
            let mut clip = CLIP.lock().unwrap();
            // A clip starts on a thread of its own (`start_clip`) and takes the first frame after it answers: its extent's open can wait minutes (2026-11-19, the Pro: the vault sealing a 16 GB clip left open; the camera thread waiting in it stopped taking frames and the driver closed), so this thread goes on taking and previewing frames meanwhile.
            let mut started = false;
            if let Some(rx) = starting.take() {
                match rx.try_recv() {
                    Ok(Ok(c)) if asked => {
                        *clip = Some(c);
                        started = true;
                        lumis_core::platform::set_video_note(None);
                        // The clip takes the next frame; this one is neither handed over nor integrated here, so the camera thread is back for the clip's first (an integration on this turn, 16 ms at 60 a second, cost the clip its frame 1 on the Pro, 2026-11-22).
                        keep_saves(vault.as_deref(), &mut *integrator.header);
                        frames += 1;
                        continue;
                    }
                    Ok(Ok(c)) => end_clip(c, Some(&mut *integrator.header), "STOP before its first frame"),
                    Ok(Err(e)) => {
                        println!("lumis: camera {}: RAW video: no clip ({e})", cam.name);
                        // Under the RECORD button, why nothing records.
                        lumis_core::platform::set_video_note(Some(e));
                        integrator.header[FLAGS_IDX] &= !RECORDING_BIT;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => starting = Some(rx),
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        println!("lumis: camera {}: RAW video: the clip's start ended with no answer", cam.name);
                        lumis_core::platform::set_video_note(Some("the clip did not start".into()));
                        integrator.header[FLAGS_IDX] &= !RECORDING_BIT;
                    }
                }
            }
            match (asked, clip.is_some(), starting.is_some(), paced, &got) {
                // RECORD with Lumis holding the GPU's grants but no GPU service running (`gpu::NO_SERVICE`, after a jump from Android): the recorder's runs would wait for it forever, so no clip, and the note under RECORD says why.
                (true, false, false, Some(_), _) if gpu::NO_SERVICE.load(Ordering::Relaxed) => {
                    lumis_core::platform::set_video_note(Some("RAW video needs the GPU, which is not running".into()));
                    integrator.header[FLAGS_IDX] &= !RECORDING_BIT;
                }
                (true, false, false, Some(fps), Got::Frame(head, Taken::View(rows))) if head.count as u64 >= paced_from => {
                    let h = &integrator.header;
                    let facts = (h[lumis_core::shared_memory::SENSOR_ORIENTATION_IDX], h[lumis_core::shared_memory::DEVICE_ROTATION_IDX], h[lumis_core::shared_memory::VIDEO_SPACE_USED_IDX]);
                    let pacing = pace(fps);
                    let (c, head, line, digests) = (cam.clone(), *head, rows.line, dev_knobs4() & (DEV_VIDEO | DEV_HASHES) == DEV_VIDEO | DEV_HASHES);
                    let (tx, rx) = std::sync::mpsc::channel();
                    match std::thread::Builder::new().name("lumis-clip-start".into()).spawn(move || {
                        let _ = tx.send(start_clip(&c, facts, pacing, head, line, digests));
                    }) {
                        Ok(_) => {
                            lumis_core::platform::set_video_note(Some("opening the clip's space".into()));
                            starting = Some(rx);
                        }
                        Err(e) => {
                            println!("lumis: camera {}: RAW video: no thread to start the clip ({e})", cam.name);
                            integrator.header[FLAGS_IDX] &= !RECORDING_BIT;
                        }
                    }
                }
                (false, true, ..) => end_clip(clip.take().unwrap(), Some(&mut *integrator.header), "STOP"),
                // RECORD with the camera not at a RAW video rate (its open refused): nothing to record, said.
                (true, false, false, None, _) if refused_fps.is_some() => {
                    integrator.header[FLAGS_IDX] &= !RECORDING_BIT;
                }
                _ => {}
            }
            if let (false, Some(c), Got::Frame(head, Taken::View(rows))) = (started, &mut *clip, &got) {
                // SAFETY: the held buffer's rows, `height` lines of `line` bytes from its offset in the view (mapped read-only for the life of the open, this process's until the next MAP gives it back); `frame` codes every band before it returns.
                let f = ferros_clip::FrameIn {
                    rows: unsafe { std::slice::from_raw_parts(rows.src as *const u8, head.height * rows.line) },
                    count: head.count as u64,
                    eagle_ns: head.time as u64,
                    exposure_ns: head.exposure_ns,
                    gain: head.gain,
                    focus: head.focus,
                };
                let index = c.rec.stats().frames;
                // A preview frame is integrated while the recorder codes the same held buffer (owner, 2026-11-19: "a faster preview"), now on the preview's own thread, which the camera thread never waits for (`PreviewJob`, 2026-11-21: a preview past the frame cost the frame after it); before, both were done before the next MAP gave the buffer back, and the preview cost the coding's own time, not its own after it (at 2^9 ms apart, one after the other, the camera thread had to make an integration up over five frames). Between preview frames the hand-over is all.
                // No preview in a clip's first 2^10 ms: its first hand-overs run slow while the coder's workers start (40 to 64 ms on the Pro, 2026-11-20), and a preview beside them lost 8 of the first 9 frames.
                // Nor in the 2^10 ms after a drop: a warm minute at 60 a second lost its only frames (22, its last 5 s) to previews beside hand-overs as the device heated (2026-11-20); the preview gives the camera thread its time back while it falls behind, and comes back by itself.
                // Nor after a hand-over that took over half the sensor's period: the recorder is running late, and a preview beside the next would make it later (a minute at 60 a second lost its last frame so, 2026-11-20; the owner asked for a lock).
                let preview_now = c.preview.elapsed() >= preview_gap(c.fps) && c.began.elapsed() >= std::time::Duration::from_millis(1 << 10) && c.dropped_at.is_none_or(|t| t.elapsed() >= std::time::Duration::from_millis(1 << 10)) && c.last_code_us <= c.period_us >> 1;
                // The preview on its thread (`PreviewJob`), started as the hand-over starts and never waited for: none behind or with the screen off (nothing shows it, and heat is the recording's limit at 48 to 60 a second), only with a ring of four or more (the read then has two frames past its own before the engine comes round), and only when the last one has ended.
                if let (true, Some((jobs, _))) = (preview_now && !behind && ring >= 4 && !previewing(), preview.as_ref()) {
                    c.preview = std::time::Instant::now();
                    *PREVIEWING.lock().unwrap() = true;
                    let job = PreviewJob { rows: ViewRows { src: rows.src, line: rows.line, cached: rows.cached, pd: None, gpu: rows.gpu, width: rows.width, height: rows.height }, last: head.count as u64 + ring - 3, iso: iso(head.gain) as i32, exposure_ns: head.exposure_ns as i64, quad: head.quad() };
                    if jobs.send(job).is_err() {
                        *PREVIEWING.lock().unwrap() = false;
                    }
                }
                let t_code = std::time::Instant::now();
                let taken = c.rec.frame(&f);
                let code_us = t_code.elapsed().as_micros() as u64;
                if taken == ferros_clip::Taken::Recorded && dev_knobs4() & (DEV_VIDEO | DEV_HASHES) == DEV_VIDEO | DEV_HASHES && index & ((1 << 3) - 1) == 0 {
                    let h: String = c.rec.source_hash().iter().map(|b| format!("{b:02x}")).collect();
                    println!("lumis: RAW video: frame {index} source {h}");
                }
                c.last_code_us = code_us;
                let st = c.rec.stats();
                integrator.header[VIDEO_FRAMES_IDX] = st.frames;
                integrator.header[VIDEO_DROPPED_IDX] = st.dropped;
                // Each drop said as it happens, with its likely cause: the camera's count stepping past one (Lumis late to the MAP: this frame's coding, the last preview's integration), or the writer holding every slot (taken Dropped).
                if st.dropped != c.dropped {
                    println!("lumis: camera {}: RAW video: {} dropped at clip frame {index} ({:?}; the camera's count {} after {}; this frame's hand-over {code_us} us; the last preview's integration {last_preview_us} us)", cam.name, st.dropped - c.dropped, taken, head.count, c.last_count);
                    c.dropped = st.dropped;
                    c.dropped_at = Some(std::time::Instant::now());
                }
                c.last_count = head.count;
                integrator.header[lumis_core::shared_memory::VIDEO_SPACE_USED_IDX] = c.space_before + st.written;
                // The clip's frames a second by the camera's own clock: the frames after the first over the time from it to this one (the sensor's frame is whole lines of its clock, so 30 a second is 29.999; a rate from Lumis's own clock would count the wait for the first frame and the seal).
                let first = *c.first_ns.get_or_insert(head.time);
                if st.frames > 1 && head.time > first {
                    integrator.header[lumis_core::shared_memory::VIDEO_RATE_IDX] = ((st.frames - 1) as f64 * 1e9 / (head.time - first) as f64).to_bits();
                }
                // The room left at the bytes a frame has coded to so far, at the clip's rate; 0 (not known, not said) until a frame's bytes are counted: no frame yet, or frames handed over whose packing has not yet counted its bytes (the GPU path's first frame, 2026-11-20: coded 0 after one frame, and a division by it panicked a recording from the deck).
                let per_frame = match st.frames {
                    0 => 0,
                    n => st.coded / n,
                };
                integrator.header[VIDEO_LEFT_S_IDX] = match per_frame {
                    0 => 0,
                    b => c.room.saturating_sub(st.coded) / b / c.fps as u64, // WHY/PROOF: the coded bytes pass the room by up to a frame before the writer finds it full (Taken::Ended), and the subtraction would wrap to years left
                };
                if taken == ferros_clip::Taken::Ended {
                    end_clip(clip.take().unwrap(), Some(&mut *integrator.header), "the clip's space is full, or a write failed");
                } else if preview_done.is_none() {
                    // The preview is its thread's (`PreviewJob`): the camera's next frame comes first, unless a preview ended with settings to send (below).
                    keep_saves(vault.as_deref(), &mut *integrator.header);
                    frames += 1;
                    continue;
                }
            }
        }
        // RAW video not recording: the preview integrates a frame only every preview gap (2^7 ms at 30 a second or under, 2^8 above), the camera's other frames taken and given back: it integrated every frame, and after a 24 a second clip the A510s reached 88 C with nothing recording (the coordinator, 2026-11-20, for the desert).
        // While a clip starts (`start_clip` on its thread), no frame is integrated here either: the clip's first frames come right after it answers.
        if let (Some(fps), true) = (paced, CLIP.lock().unwrap().is_none()) {
            if starting.is_some() || idle_preview.elapsed() < preview_gap(fps) {
                keep_saves(vault.as_deref(), &mut *integrator.header);
                frames += 1;
                continue;
            }
            idle_preview = std::time::Instant::now();
        }
        let k4 = dev_knobs4();
        if let (true, Got::Frame(head, Taken::View(rows))) = (k4 & DEV_CELLS != 0 && frames == 16, &got) {
            say_cells(&cam.name, rows, head.width, head.height);
        }
        if k4 & DEV_SAVE != 0 && frames == 32 {
            println!("lumis: development: a VSF 8B save asked");
            integrator.header[lumis_core::shared_memory::SAVE_FORMAT_IDX] = lumis_core::shared_memory::SAVE_FORMAT_VSF8;
            integrator.header[FLAGS_IDX] |= MANUAL_SAVE_BIT;
        }
        // A RAW video clip's preview frame was integrated beside its hand-over (above); any other frame is integrated here.
        // A clip that ended this turn may have left a preview on its thread: it ends before the integrator is used here.
        if preview_done.is_none() {
            preview_settle();
        }
        let (iso, shutter_ns, focus) = if let Some(r) = preview_done.take() { r } else { match got {
            // With its phase-detect rows where it has them (the phase view renders them; every other mode passes them over).
            Got::Frame(head, Taken::View(rows)) => {
                let pd = rows.pd();
                let pd = pd.as_ref().map(|(p, n)| lumis_core::PdRows { rows: p, count: *n, blacks: PD_BLACKS });
                integrator.process_rows_pd(&rows, pd, iso(head.gain) as i32, head.exposure_ns as i64, 0., head.quad())
            }
            // A copy is 16-bit samples of a binned readout (the driver gives none at 1:1); process_frame's flag is RAW10 bytes, not the filter.
            Got::Frame(head, Taken::Bytes(bytes)) => integrator.process_frame(bytes, iso(head.gain) as i32, head.exposure_ns as i64, 0., false, head.width * 2),
            // A slit carries no settings of its own: the last sent are what it was taken at (the integrator reads none of them for a slit).
            Got::Slit { rows, .. } => integrator.process_slit(&rows, rows.rows, sent.0, sent.1, sent.2),
        } };
        let t_after = std::time::Instant::now();
        turns.integrate += (t_after - t_integrate).as_micros() as u64;
        if !preview_async {
            last_preview_us = (t_after - t_integrate).as_micros() as u64;
        }
        if dev_knobs2() & DEV_NO_SET != 0 {
            sent = (iso, shutter_ns, focus);
        }
        // Only the settings asked for go (`dev_sets`); the rest are taken as sent.
        let only = dev_sets();
        if only != 0 {
            if only & 1 == 0 {
                sent.0 = iso;
            }
            if only & 2 == 0 {
                sent.1 = shutter_ns;
            }
            if only & 4 == 0 {
                sent.2 = focus;
            }
        }
        if iso != sent.0 && !set(camera::SET_GAIN, gain_of(iso)) {
            println!("lumis: the camera did not take gain {} (ISO {iso})", gain_of(iso));
        }
        if shutter_ns != sent.1 && !set(camera::SET_EXPOSURE, shutter_ns as u64) {
            println!("lumis: the camera did not take exposure {shutter_ns} ns");
        }
        if focus != sent.2 && cam.focus_max > cam.focus_min && !set(camera::SET_FOCUS, focus_of(focus)) {
            println!("lumis: the camera did not take focus G#{:X}", focus_of(focus));
        }
        sent = (iso, shutter_ns, focus);
        if integrator.check_and_finalize_calibration() {
            println!("lumis: calibration finalized");
        }
        integrator.try_save();
        keep_saves(vault.as_deref(), &mut *integrator.header);
        turns.after += t_after.elapsed().as_micros() as u64;
        frames += 1;
        turns.n += 1;
        if let Some(r) = run {
            if run_start.elapsed() >= r {
                println!("lumis: development run of {} s ended after {frames} frames", r.as_secs());
                if let Some(c) = CLIP.lock().unwrap().take() {
                    end_clip(c, Some(&mut *integrator.header), "the development run ended");
                }
                keep_saves(vault.as_deref(), &mut *integrator.header);
                close_camera(&cam.name);
                lumis_core::platform::exit_because("the development run ended");
            }
        }
        if src.is_slit() {
            if turns.n == 1 << SLIT_LINE {
                report(&src, &mut turns, &cam.name);
            } else if turns.since.is_none() {
                turns.since = Some(std::time::Instant::now());
            }
        } else if turns.n == 1 << 6 {
            let whole = turns.since.map_or(0, |s| s.elapsed().as_micros() as u64);
            println!("lumis: {frames} frames of {} x {} integrated; the last 64 every {} us: the driver's answer {} us, the integrator {} us (reading the view, unpacking and adding, lumis_core process_rows)", first.width, first.height, whole / 64, turns.wait / 64, turns.integrate / 64);
            // The driver's side of the same 64: its frames between the first and the last taken (the ones Lumis skipped are the rest), their period by its own Eagle clock, and what the integrator made of them.
            let (c0, t0) = turns.first.unwrap_or(turns.last);
            // 64 distinct frames of the driver lie between the first and the last, so the span is at least 63.
            let span = (turns.last.0 - c0) as u64;
            let period = (turns.last.1 - t0) as u64 / span / 1000;
            let images = integrator.header[lumis_core::shared_memory::IMAGE_COUNTER_IDX] - turns.images;
            println!("lumis: camera: the sensor's period A#{period} us by the driver's clock; the driver wrote A#{} frames from the first of these 64 to the last, A#{} of them never taken; the settings and saves A#{} us a frame; A#{images} images completed; A#{} worker threads", span + 1, span + 1 - (1 << 6), turns.after / 64, rayon::current_num_threads());
            turns = Turns { since: Some(std::time::Instant::now()), ..Turns::default() };
        } else if turns.since.is_none() {
            turns.since = Some(std::time::Instant::now());
        }
    }
}

/// Slits a console line, 2^12: about every 3.4 s at the main's 1218 a second.
const SLIT_LINE: u32 = 12;

/// A slit source's console line, for the slits since the last (`turns`), and those counted afresh: how often one was taken, the driver's wait and the integrator's share, the slits written over while read and those passed over as too old to read, the column's period by the driver's clock. Said at every `SLIT_LINE` slits and when Slitscan ends; nothing for whole frames, which say their own every 64.
fn report(src: &Source, turns: &mut Turns, name: &str) {
    let Source::Slit { lost, skipped, after, .. } = src else {
        return;
    };
    if turns.n != 0 {
        let whole = turns.since.map_or(0, |s| s.elapsed().as_micros() as u64);
        let (c0, t0) = turns.first.unwrap_or(turns.last);
        let span = (turns.last.0 - c0) as u64;
        // WHY the max: one slit since the last line has no span; its period is said as 0.
        let period_ns = (turns.last.1 - t0) as u64 / span.max(1);
        println!("lumis: camera {name}: {after} slits so far; the last A#{} taken every A#{} us (the driver's answer A#{} us a slit, the integrator A#{} us), the sensor's slit every A#{} ns by the driver's clock; A#{lost} written over while read, A#{skipped} too old to read (Lumis behind by half the ring)", turns.n, whole / turns.n, turns.wait / turns.n, turns.integrate / turns.n, period_ns);
    }
    *turns = Turns { since: Some(std::time::Instant::now()), ..Turns::default() };
}

/// Each save the integrator finished (Android: drainPendingSave, MediaStore) kept (`keep_one`: a still in the person's hold as an item of kind photo, so Kūaha shows it; the calibration maps, and a still when the hold is not given, in the kist's vault under "save/" and its file name), then marked done.
/// On the keeper's own thread (`KEEPER`), never the camera thread's: a still's write to the hold and its read back took 5 s on the Pro (2026-11-22, 25 MB), and the camera thread asking for no frame for 2^22 us is a holder gone to the driver, which closed the camera, so the owner's first shot ended Lumis. The camera thread hands each save over and takes its next frame; an exit waits for the keeper first (`seal_at_exit`, `saves_settle`), so no save is lost. One save is in flight at a time (CURRENTLY_SAVING), so one keeper keeps them in order.
fn keep_saves(vault: Option<&kete::FlatStorage>, header: &mut [u64]) {
    while let Some(save) = lumis_core::get_pending_save() {
        match KEEPER.get_or_init(keeper) {
            Some(keeper) => {
                *KEEPING.lock().unwrap() += 1;
                if let Err(e) = keeper.send((save, header.as_mut_ptr() as usize, header.len())) {
                    *KEEPING.lock().unwrap() -= 1;
                    let (save, at, len) = e.0;
                    // SAFETY: the header handed over is this one.
                    keep_one(save, vault, unsafe { std::slice::from_raw_parts_mut(at as *mut u64, len) });
                }
            }
            None => keep_one(save, vault, header),
        }
    }
}

/// The keeper's thread: saves in, each kept (`keep_one`) in the vault the process opened (`VAULT`), its header marked as the camera thread's would be.
type Keep = (lumis_core::PendingSave, usize, usize);
static KEEPER: std::sync::OnceLock<Option<std::sync::mpsc::Sender<Keep>>> = std::sync::OnceLock::new();
/// Saves handed to the keeper and not yet kept.
static KEEPING: Mutex<u64> = Mutex::new(0);
static KEPT: std::sync::Condvar = std::sync::Condvar::new();
/// The vault main opened, for the keeper.
static VAULT: std::sync::OnceLock<Arc<kete::FlatStorage>> = std::sync::OnceLock::new();

fn keeper() -> Option<std::sync::mpsc::Sender<Keep>> {
    let (tx, rx) = std::sync::mpsc::channel::<Keep>();
    std::thread::Builder::new()
        .name("lumis-keep".into())
        .spawn(move || {
            for (save, at, len) in rx {
                // SAFETY: the integrator's header, shared memory for the process's life (the integrator is never dropped), written from the UI and the camera threads as ever.
                keep_one(save, VAULT.get().map(|v| &**v), unsafe { std::slice::from_raw_parts_mut(at as *mut u64, len) });
                *KEEPING.lock().unwrap() -= 1;
                KEPT.notify_all();
            }
        })
        .ok()
        .map(|_| tx)
}

/// Wait until the keeper has kept every save handed to it.
fn saves_settle() {
    let mut n = KEEPING.lock().unwrap();
    while *n != 0 {
        n = KEPT.wait(n).unwrap();
    }
}

/// One save kept: a still in the person's hold (`hold::keep_still`), a calibration's maps (and a still with no hold given) in Lumis's vault domain under "save/" and its name, as a bulk value (kete `write_device_bulk`: kept out of the read cache, which would otherwise hold a second copy of every still); then marked done. Nothing is read back: the vault's Ok is "written, verified, mirrored and the mirror verified" (the owner, 2026-11-22), and an error is said on the console and the save counts as not kept.
fn keep_one(save: lumis_core::PendingSave, vault: Option<&kete::FlatStorage>, header: &mut [u64]) {
    let lumis_core::PendingSave { bytes, name, encode_ms } = save;
    let photo = !name.starts_with("cal_");
    if photo && hold::held() {
        let kept = match hold::keep_still(&bytes) {
            Ok((write_ms, card, copies)) => {
                // The copies the vault verified: both (fully kept), or one while it runs on one: kept, and the camera screen warns (1 COPY) until the next touch (the coordinator, 2026-11-22; HOLD-API.md "What Ok means"). A RAW clip's body is single-copy by design and never warns (it is not kept here).
                let on = match copies {
                    Some(2) => ", on both copies".to_string(),
                    Some(n) => {
                        lumis_core::platform::set_save_note(lumis_core::platform::SAVE_NOTE_ONE_COPY);
                        format!(", on A#{n} copy only: the vault runs on one")
                    }
                    None => String::new(),
                };
                println!("lumis: kept {name} in the hold as item {}: {} bytes, encoded in {encode_ms} ms, written and verified in {write_ms} ms{on}", card.id.as_bytes()[..8].iter().map(|b| format!("{b:02x}")).collect::<String>(), bytes.len());
                true
            }
            Err(e) => {
                println!("lumis: {name} ({} bytes, encoded in {encode_ms} ms) NOT kept in the hold: {e}", bytes.len());
                false
            }
        };
        // Not kept: the camera screen says so until the next touch (lumis_core `platform::save_failed`).
        if !kept {
            lumis_core::platform::set_save_note(lumis_core::platform::SAVE_NOTE_NOT_SAVED);
        }
        app::save_done(header, kept);
        return;
    }
    let key = format!("save/{name}");
    let t = std::time::Instant::now();
    let kept = match vault {
        Some(v) => match v.write_device_bulk(&[(key, bytes.clone())]) {
            Ok(()) => {
                println!("lumis: kept {name}: {} bytes, encoded in {encode_ms} ms, written and verified in {} ms", bytes.len(), t.elapsed().as_millis());
                true
            }
            Err(e) => {
                println!("lumis: {name} ({} bytes, encoded in {encode_ms} ms) NOT kept: {e:?}", bytes.len());
                false
            }
        },
        None => {
            println!("lumis: {name} ({} bytes) not kept: no vault this launch", bytes.len());
            false
        }
    };
    if !kept {
        lumis_core::platform::set_save_note(lumis_core::platform::SAVE_NOTE_NOT_SAVED);
    }
    // A calibration map is not a photo: it never counts (as on Android).
    app::save_done(header, kept && photo);
}

/// What the camera thread does with the window behind, said on the console as it changes.
#[derive(Clone, Copy, PartialEq)]
enum Stay {
    /// In front: Lumis as always.
    Front,
    /// Behind (the screen off counts: the display sends EV_BEHIND) while a RAW video clip records: recording on, nothing drawn (capture first, the owner 2026-11-21: the screen going off never stops a recording).
    Recording,
    /// Behind during a continuous save: capturing and saving on, nothing drawn.
    Continuous,
    /// Behind during a calibration: capturing on, nothing drawn.
    Calibrating,
    /// Behind with a save on its way: that save finished and kept, then Lumis ends.
    Finishing,
}

/// Whether Lumis stays behind for as long as it runs: a continuous save or a calibration (owner, 2026-11-17: "Basically if continuous save or cal is on"), or a RAW video clip (2026-11-21: capture first), the same bits lumis_core's integrator keeps the camera alive on with the UI quiet (lumis ferros-cal-behind).
/// A calibration whose finish was tapped holds CALIBRATING_BIT until its maps are written, so it stays thru its own save.
fn kept(flags: u64) -> bool {
    flags & (CONTINUOUS_SAVE_BIT | CALIBRATING_BIT | lumis_core::shared_memory::RECORDING_BIT) != 0
}

/// Whether a save the person asked for is still on its way, by the integrator's flags: a single save asked for and not yet begun (MANUAL_SAVE_BIT, which try_save starts from the image already made), or one encoding or encoded and not yet kept (CURRENTLY_SAVING, cleared by `keep_saves` once the vault has it).
/// An exposure still integrating with no save asked for is not on its way: nothing was asked to be kept.
fn in_flight(flags: u64) -> bool {
    flags & (MANUAL_SAVE_BIT | CURRENTLY_SAVING) != 0
}

/// The driver's CLOSE (docs/CAMERA.md, op 4): the view taken back, the stream stopped, the module powered down, and the driver ends.
/// Said by the camera thread (or before it reads any frame), so nothing reads the view after the driver takes it back.
fn close_camera(name: &str) {
    // A preview still reading a buffer the CLOSE takes back ends first, and no stack starts on the GPU from a view going away; after it the GPU is told the view is gone.
    preview_settle();
    gpu::CAMERA_MAPPED.store(false, Ordering::Relaxed);
    let mut r = [0u8; 4];
    if sys::call_live(CAMERA, &[camera::OP_CLOSE], &mut r) == sys::ERR {
        println!("lumis: camera {name}: the CLOSE was not answered; the driver closes when Lumis ends");
    } else {
        println!("lumis: camera {name}: closed");
    }
    gpu::camera_changed(false);
}

/// The camera closed for Lumis's leaving, once whoever asks first (`LEAVING`); the caller holds `VIEW` (the camera thread in its turn, or `close_from_behind`).
fn close_to_leave(name: &str) {
    if !LEAVING.swap(true, Ordering::AcqRel) {
        close_camera(name);
    }
}

/// Lumis sent behind with no continuous save or calibration running and no save on its way: anything encoded is kept first, the camera closed, and Lumis ends saying so. Called by the camera thread in its turn (it holds `VIEW`).
fn leave(name: &str, vault: Option<&kete::FlatStorage>, header: &mut [u64]) -> ! {
    keep_saves(vault, header);
    close_to_leave(name);
    lumis_core::platform::exit_because("sent behind");
}

/// The window sent behind with nothing that keeps Lumis (no continuous save or calibration): the camera closed from a thread of its own, not at the camera thread's next frame, which on a long exposure may be years away (2026-11-18, the Pro: a shutter dragged to its last stop, about 14 years, left the driver waiting on that frame and the next Lumis unable to open). It takes `VIEW`, so it waits for a frame being read to be done; the driver answers a CLOSE while a MAP or SLITS waits (ferros cam-serve-while-waiting), and that wait comes back empty. On its own thread so the window never waits on the driver.
fn close_from_behind(name: String) {
    let spawned = std::thread::Builder::new().name("lumis-leave".into()).spawn(move || {
        let _reading = VIEW.lock().unwrap();
        close_to_leave(&name);
    });
    if spawned.is_err() {
        println!("lumis: no thread to close the camera; it closes at its next frame");
    }
}

/// The camera closed for Lumis's leaving while this thread waited on it (`LEAVING`): nothing more is read from the view; a save asked for is started, every save on its way finished and kept, and Lumis ends.
fn finish_leaving(integrator: &mut lumis_core::CameraIntegrator, vault: Option<&kete::FlatStorage>) -> ! {
    println!("lumis: the camera closed for leaving; finishing the saves on their way");
    loop {
        integrator.try_save();
        keep_saves(vault, &mut *integrator.header);
        if !in_flight(integrator.header[FLAGS_IDX]) {
            lumis_core::platform::exit_because("sent behind");
        }
        // A save encoding on its own thread: looked at again every 2^6 ms until it is kept.
        std::thread::sleep(std::time::Duration::from_millis(1 << 6));
    }
}

/// The parts of the screen lumis_core's partial draw writes (draw_screen with the controls up and nothing whole): the two exposure labels, the counters, and the progress band along the exposure slider (drawn only for an exposure longer than nothing).
/// Overlapping ones made one, so the host gets disjoint rects (fluor `DamageRects`), at most four.
fn partial_parts(ui: &lumis_core::UserInterface, camera: &[fluor::canvas::PixelRect]) -> Vec<fluor::canvas::PixelRect> {
    use fluor::canvas::PixelRect;
    let mut parts = vec![
        PixelRect::new(ui.left_label_x, ui.left_label_y, ui.left_label_end_x, ui.left_label_end_y),
        PixelRect::new(ui.right_label_x, ui.right_label_y, ui.right_label_end_x, ui.right_label_end_y),
    ];
    if let Some(&(x, y, ex, ey)) = ui.counter_areas.first() {
        let mut c = PixelRect::new(x, y, ex, ey);
        for &(x, y, ex, ey) in &ui.counter_areas[1..] {
            c = c.union(PixelRect::new(x, y, ex, ey));
        }
        parts.push(c);
    }
    // The progress band: an exposure's, or in RAW video the recording space's (lumis_core draw_progress, from the platform's space words).
    let space = ui.in_video() && ui.header[lumis_core::shared_memory::VIDEO_SPACE_TOTAL_IDX] > 0;
    if ui.slider_thickness > 0. && (ui.exposure_time_ms > 0 || space) {
        // The band is the slider's line widened by its thickness each side, the dot's cup inside it; a float below zero casts to 0, and the host cuts the rect at the frame.
        let t = ui.slider_thickness;
        let (xa, xb) = (ui.slider_start_x.min(ui.slider_end_x), ui.slider_start_x.max(ui.slider_end_x));
        let (ya, yb) = (ui.slider_start_y.min(ui.slider_end_y), ui.slider_start_y.max(ui.slider_end_y));
        parts.push(PixelRect::new((xa - t) as usize, (ya - t) as usize, (xb + t) as usize + 1, (yb + t) as usize + 1));
    }
    // What a new image changed of the camera's part (lumis_core screen::camera_damage: the fit view's rect, or the slitscan wiper's band of rows), beside the controls that tick with it.
    parts.extend_from_slice(camera);
    parts.retain(|r| !r.is_empty());
    // Overlapping parts joined until none overlap.
    let overlap = |a: &PixelRect, b: &PixelRect| a.x0 < b.x1 && b.x0 < a.x1 && a.y0 < b.y1 && b.y0 < a.y1;
    let mut k = 0;
    while k < parts.len() {
        match (k + 1..parts.len()).find(|&j| overlap(&parts[k], &parts[j])) {
            Some(j) => {
                let b = parts.swap_remove(j);
                parts[k] = parts[k].union(b);
                k = 0;
            }
            None => k += 1,
        }
    }
    // The host takes at most DAMAGE_RECTS (fluor canvas): any past that join the last kept, a larger rect than needed but never one missed.
    while parts.len() > fluor::canvas::DAMAGE_RECTS {
        let b = parts.pop().unwrap();
        let last = parts.len() - 1;
        parts[last] = parts[last].union(b);
    }
    parts
}

/// Where Lumis is: Android's UserInterface.kt states.
enum Screen {
    Menu,
    /// The camera opening: the menu stays as it was, touches ignored.
    Opening,
    Camera(Box<UserInterface>),
}

struct Lumis {
    width: usize,
    height: usize,
    cameras: Vec<camera::Camera>,
    vault: Option<Arc<kete::FlatStorage>>,
    screen: Screen,
    menu: Option<Box<MainMenu>>,
    menu_dirty: bool,
    /// The camera last opened, for the console.
    camera_name: String,
    /// `DEV_OPEN` has opened its camera (once a launch).
    dev_opened: bool,
    surface: Option<PixelSurface>,
    opened: Arc<Mutex<Option<Opened>>>,
    /// The finger, in frame pixels, and the camera UI's tap bookkeeping (UserInterface.kt: a tap shorter than a frame still reaches a draw as a down before its up).
    finger: Option<(f32, f32)>,
    down_sampled: bool,
    pending_up: bool,
    /// Lumis posted a frame since the last show: the next draw converts it whole from Lumis's own buffer straight into fluor's scratch (no copy of the frame here: neither its bytes nor its pixels, 44 MB on the Pro).
    changed: bool,
    /// On the camera screen, a pass that drew no new image and nothing whole: the controls' parts lumis_core repainted (its partial draw), the only rects converted and shown (redraw on change; the owner's go, 2026-11-17). Empty with `changed` false: nothing to show.
    parts: Vec<fluor::canvas::PixelRect>,
    /// RAW video's RECORD / STOP label and its note as last drawn whole, and when: the buttons are drawn only in a whole draw, so a change is one (at most once a second while a clip records, at once when one starts or stops).
    video_label: (String, Option<String>, bool, u64, bool),
    video_drawn: std::time::Instant,
    /// When the camera screen last made a pass (while recording, a pass is made only when something on it changes, `tick`).
    ui_pass: std::time::Instant,
    /// The development knobs from the spawn input (`DEV_WHOLE`, `DEV_COPY`, `DEV_SCRATCH`).
    knobs: u8,
    /// A display byte (Lumis's gamma-2 encoded light, 255 its white) as a 16-bit g2t darkness: the code VSF's encoder gives (atanh(code / 2^16)^2 = the light), Lumis's white at g2t's diffuse white.
    lut: [u32; 256],
    /// The preview's split on the UI thread (`Probe`).
    probe: Probe,
    /// The GPU (`gpu::SHARED`, the camera thread's too) asked for: once, with the first menu.
    gpu_asked: bool,
    /// The menu changed since its last frame on the GPU (its parts written into the block again), and the surface that frame went into, for the show (`own_surface`).
    menu_fresh: bool,
    own: Option<u64>,
    /// The app scale changed since the menu was last laid out (`follow_scale`): it is laid out again at its next frame.
    relayout: bool,
    /// Relayouts since the last line, and their time.
    relaids: u64,
    relaid_us: u64,
}

impl Lumis {
    fn new(cameras: Vec<camera::Camera>, vault: Option<Arc<kete::FlatStorage>>) -> Self {
        let mut lut = [0u32; 256];
        for (v, d) in lut.iter_mut().enumerate() {
            let light = (v as f32 / 255.) * (v as f32 / 255.);
            *d = G2t16::ONE - vsf::colour::convert::delinearize_gamma2tanh_bits_f32(light, 16);
        }
        Lumis { width: 0, height: 0, cameras, vault, screen: Screen::Menu, menu: None, menu_dirty: true, camera_name: String::new(), dev_opened: false, surface: None, opened: Arc::new(Mutex::new(None)), finger: None, down_sampled: false, pending_up: false, changed: false, parts: Vec::new(), video_label: (String::new(), None, false, 0, false), video_drawn: std::time::Instant::now(), ui_pass: std::time::Instant::now(), knobs: dev_input().0, lut, probe: Probe::new(), gpu_asked: false, menu_fresh: true, own: None, relayout: false, relaids: 0, relaid_us: 0 }
    }

    /// The menu asked to open camera `index` (its place in the list).
    fn open(&mut self, index: usize, calibration: Option<bool>) {
        let cam = self.cameras[index].clone();
        println!("lumis: opening {} (camera {}){}", cam.name, cam.id, match calibration {
            Some(true) => ", dark calibration",
            Some(false) => ", bias calibration",
            None => "",
        });
        *self.opened.lock().unwrap() = None;
        self.camera_name = cam.name.clone();
        self.screen = Screen::Opening;
        let (vault, opened) = (self.vault.clone(), self.opened.clone());
        if std::thread::Builder::new().name("lumis-camera".into()).spawn(move || camera_service(cam, calibration, vault, opened)).is_err() {
            println!("lumis: no thread for the camera; back to the menu");
            self.screen = Screen::Menu;
        }
    }

    /// The person's app scale as the host now runs Lumis (its RU, 2^(s/64) of preference 14, which ferros_fluor keeps): lumis_core lays its controls out at it (`set_ui_scale`: the camera screen's control block, its buttons, arrows, knobs and words, and the counters, the clips page, the calibration screens; the menu's slots), everything drawn again when it changes; at 1 every layout is the one before the scale.
    fn follow_scale(&mut self, ctx: &mut Context) {
        let ru = ctx.viewport.ru;
        if ru != lumis_core::platform::ui_scale() {
            lumis_core::platform::set_ui_scale(ru);
            // The menu laid out again once, at its next frame, at the scale then: a pinch brings several steps a frame, each a relayout before (the Pro, 2026-11-21: 703 relayouts of 10 to 26 ms for 59 frames shown).
            self.relayout = true;
            self.changed = true;
            self.menu_dirty = true;
            ctx.window.request_redraw();
        }
    }

    /// The window sent behind or brought to the front (fluor's `Event::Focused`, from the display's EV_BEHIND and EV_FRONT).
    /// Behind on the menu, Lumis ends at once: no camera is open and nothing is on its way.
    /// Behind with the camera opening or open, the camera thread decides at its next frame: it closes the camera and ends Lumis, unless a continuous save or a calibration runs or a save is on its way (`Stay`).
    /// The finger is let go either way, so a touch held when the window went behind is not taken as a tap when it comes back.
    fn focus(&mut self, focused: bool) {
        BEHIND.store(!focused, Ordering::Release);
        self.finger = None;
        self.down_sampled = false;
        self.pending_up = false;
        if focused {
            // Back in front: the frame shown whole (fluor draws the first frame back whole; Lumis's buffer still holds what it last drew).
            self.changed = true;
            return;
        }
        match &self.screen {
            Screen::Menu => lumis_core::platform::exit_because("sent behind"),
            // Nothing keeps Lumis (no continuous save or calibration): the camera closed now, not at the camera thread's next frame (`close_from_behind`); the camera thread then finishes any save on its way and ends Lumis.
            Screen::Camera(ui) if !kept(ui.header[FLAGS_IDX]) => close_from_behind(self.camera_name.clone()),
            // Kept, or still opening (the camera thread decides as its first frame comes).
            _ => {}
        }
    }

}

/// Whether RAW video's RECORD / STOP label wants a whole draw (`Lumis::video_label`, as last drawn whole at `drawn`), noting it as drawn when it does.
fn video_label_moved(label: &mut (String, Option<String>, bool, u64, bool), drawn: &mut std::time::Instant, ui: &lumis_core::UserInterface) -> bool {
    if !ui.in_video() {
        return false;
    }
    let recording = ui.header[FLAGS_IDX] & lumis_core::shared_memory::RECORDING_BIT != 0;
    // The dropped count under STOP, and whether it is red (climbing within the last 2^10 ms): a change of either redraws, so red goes when the count holds.
    let climbing = ui.video_drops_rose.is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(1 << 10));
    let now = (ui.get_button_info(6, false).0, lumis_core::platform::video_note(), recording, ui.header[lumis_core::shared_memory::VIDEO_DROPPED_IDX], climbing);
    if now == *label || (now.2 == label.2 && drawn.elapsed() < std::time::Duration::from_secs(1)) {
        return false;
    }
    *label = now;
    *drawn = std::time::Instant::now();
    true
}

impl FluorApp<G2t16> for Lumis {
    type UserEvent = ();

    fn init(&mut self, _ctx: &mut Context) {}

    fn on_resize(&mut self, width: u32, height: u32, ctx: &mut Context) {
        self.follow_scale(ctx);
        let (w, h) = (width as usize, height as usize);
        if (w, h) == (self.width, self.height) {
            return;
        }
        // The frame is the panel, set once at start (ferros windows are the whole screen); Lumis is built for that size, as Android builds its menu and UI for the surface's.
        (self.width, self.height) = (w, h);
        // Lumis's frames on the GPU (`gpu::Gpu`): asked for with the first frame; a frame of another size later is the CPU's (the GPU's parts were laid out for the first).
        let mut shared = gpu::SHARED.lock().unwrap();
        let fits = shared.as_ref().map(|g| g.fits(w, h));
        match fits {
            Some(true) => shared.as_mut().unwrap().new_menu(),
            Some(false) => {
                println!("lumis: a A#{w} x A#{h} frame: Lumis drawn on the CPU from now (its GPU parts were laid out for the first frame)");
                *shared = None;
            }
            None if !self.gpu_asked => {
                self.gpu_asked = true;
                match gpu::Gpu::open(sys::cap(SURFACE), w, h, &self.lut) {
                    Ok(g) => {
                        println!("lumis: the menu and the camera screen on the GPU, every pixel a frame (lumis_gpu menu_kernel and frame_kernel), into two surfaces of Lumis's own; the stack on it too (stack_kernel)");
                        *shared = Some(g);
                    }
                    Err(e) => println!("lumis: Lumis drawn on the CPU: {e}"),
                }
            }
            None => {}
        }
        // Lumis's own frame where the GPU reads it (in the block) when it has one, else on the heap.
        self.surface = Some(match shared.as_ref() {
            // SAFETY: the frame's w h 3 bytes in Lumis's part of the GPU block, which is the process's for its life; only this thread draws them, and the GPU reads them only during `Gpu::camera`, on this thread.
            Some(g) => unsafe { PixelSurface::at(g.frame_bits(), w, h) },
            None => PixelSurface::new(w, h),
        });
        self.changed = true;
        self.menu = Some(app::menu_new(width, height, DENSITY_DPI, camera_infos(&self.cameras)));
        self.menu_dirty = true;
        self.menu_fresh = true;
        ctx.window.request_redraw();
    }

    /// The person's app scale set from outside (ferros_fluor following preference 14: the Settings page, another kist's pinch, or Lumis's own pinch written back at its end): Lumis laid out at it.
    fn on_zoom_set(&mut self, _factor: f32, ctx: &mut Context) {
        self.follow_scale(ctx);
    }

    fn on_event(&mut self, event: &Event, ctx: &mut Context) -> EventResponse {
        if let Event::Focused(focused) = event {
            self.focus(*focused);
            return EventResponse::Handled;
        }
        // A second finger: the host takes the pointer back (a pinch begins) and sends no release for it, so whatever the first finger held lets go without acting.
        if let Event::CursorLeft = event {
            if self.finger.take().is_some() {
                self.pending_up = false;
                match &mut self.screen {
                    Screen::Camera(ui) => lumis_core::ui::touch::touch_cancel(ui),
                    // The finger moved off the menu, then lifted there: a press it held lets go (the move off clears it), nothing opens.
                    Screen::Menu => {
                        if let Some(menu) = self.menu.as_mut() {
                            self.menu_dirty |= app::menu_touch(menu, 2, -1., -1.).redraw;
                            self.menu_dirty |= app::menu_touch(menu, 1, -1., -1.).redraw;
                        }
                    }
                    Screen::Opening => {}
                }
                ctx.window.request_redraw();
            }
            return EventResponse::Handled;
        }
        // Android's MotionEvent actions, as Lumis's menu reads them.
        let (action, at) = match event {
            Event::MouseInput { state: ElementState::Pressed, button: MouseButton::Left } => (0, (ctx.cursor_x as f32, ctx.cursor_y as f32)),
            Event::CursorMoved { x, y } if self.finger.is_some() => (2, (*x as f32, *y as f32)),
            Event::MouseInput { state: ElementState::Released, button: MouseButton::Left } => (1, self.finger.unwrap_or((ctx.cursor_x as f32, ctx.cursor_y as f32))),
            _ => return EventResponse::Pass,
        };
        match &self.screen {
            Screen::Menu => {
                self.finger = (action != 1).then_some(at);
                let Some(menu) = self.menu.as_mut() else { return EventResponse::Pass };
                let touch = app::menu_touch(menu, action, at.0, at.1);
                self.menu_dirty |= touch.redraw;
                if let Some(index) = touch.open {
                    self.open(index, touch.calibration);
                }
            }
            Screen::Opening => {}
            Screen::Camera(_) => match action {
                0 | 2 => {
                    if action == 0 {
                        self.down_sampled = false;
                        self.pending_up = false;
                    }
                    self.finger = Some(at);
                }
                // A release before any draw saw its press keeps the press one more frame, then lets go.
                _ if self.down_sampled => self.finger = None,
                _ => self.pending_up = true,
            },
        }
        ctx.window.request_redraw();
        EventResponse::Handled
    }

    fn tick(&mut self, _ctx: &mut Context) -> bool {
        if matches!(self.screen, Screen::Opening) {
            match self.opened.lock().unwrap().take() {
                Some(Opened::Memory { ptr, bytes }) => {
                    let ui = app::ui_new(SharedMemory::view(ptr as *mut u64, bytes), self.width as u32, self.height as u32);
                    self.finger = None;
                    self.screen = Screen::Camera(ui);
                    // Its first draw is whole whatever its counters say (draw_screen finds the menu's corner pixel and draws everything).
                    self.changed = true;
                }
                Some(Opened::Failed(why)) => {
                    println!("lumis: back to the menu ({why})");
                    self.screen = Screen::Menu;
                    self.menu_dirty = true;
                }
                None => {}
            }
        }
        // Behind: nothing drawn. A camera that failed to open while behind left only the menu, which ends Lumis as going behind on it does; a camera open is the camera thread's to end or keep.
        if BEHIND.load(Ordering::Acquire) {
            if matches!(self.screen, Screen::Menu) {
                lumis_core::platform::exit_because("sent behind");
            }
            return false;
        }
        let Some(surface) = self.surface.as_ref() else { return false };
        match &mut self.screen {
            Screen::Menu if self.knobs & DEV_OPEN != 0 && !self.dev_opened && !self.cameras.is_empty() => {
                self.dev_opened = true;
                let index = match dev_readout() {
                    Some(k) if k < self.cameras.len() => k,
                    _ if dev_knobs2() & DEV_FULL != 0 => self.cameras.iter().position(|c| c.mode != 0).unwrap_or(0),
                    _ => 0,
                };
                self.open(index, None);
                return true;
            }
            Screen::Menu => {
                if std::mem::take(&mut self.relayout) {
                    if let Some(menu) = self.menu.as_mut() {
                        let t = std::time::Instant::now();
                        menu.relayout();
                        self.relaid_us += t.elapsed().as_micros() as u64;
                        self.relaids += 1;
                        if self.relaids == 1 << 5 {
                            println!("lumis: the menu laid out again A#{} times, A#{} us each", self.relaids, self.relaid_us >> 5);
                            (self.relaids, self.relaid_us) = (0, 0);
                        }
                    }
                }
                if self.menu_dirty {
                    self.menu_dirty = false;
                    // On the GPU the frame is drawn at the show (`render_own`), its parts from the menu as it is then; else here on the CPU.
                    if gpu::SHARED.lock().unwrap().is_some() {
                        self.menu_fresh = true;
                        self.changed = true;
                    } else if let Some(menu) = self.menu.as_mut() {
                        app::menu_draw(menu, surface, false);
                    }
                }
            }
            Screen::Opening => {}
            Screen::Camera(ui) => {
                use lumis_core::shared_memory::CAL_SHOW_RESULT_BIT;
                // In RAW video (a clip recording or not), a pass only for a new preview image, a finger, or every 2^9 ms (the counters, the space bar, the UI's heartbeat, which the integrator wants within 1.6 s): a pass a display frame, a 4.3 ms draw and an 8.5 ms present with nothing new, was most of the 2 W more Lumis drew than the recorder's harness and the heat that cost a minute at 60 a second its frames (the RAW agent, 2026-11-20).
                // Not recording too: the idle screen made a pass every display frame as well (the coordinator, 2026-11-20, for the desert).
                let video = ui.in_video();
                if video && self.finger.is_none() && !self.pending_up && ui.header[lumis_core::shared_memory::IMAGE_COUNTER_IDX] == ui.previous_image_counter && self.ui_pass.elapsed() < std::time::Duration::from_millis(1 << 9) {
                    return false;
                }
                self.ui_pass = std::time::Instant::now();
                // What lumis_core's draw_screen last drew with, before this pass and after it: a field that moved is a full draw (the camera image and the controls whole).
                let before = (ui.previous_image_counter, ui.previous_exposure_start, ui.previous_histogram_counter, ui.previous_hold_present, ui.last_touch_valid, (ui.device_rotation, ui.painted_roll, ui.painted_pitch));
                let t = std::time::Instant::now();
                // The AoC's gravity, as Android's Lumis gets TYPE_GRAVITY (owner, 2026-11-18: "looks like we aren't relaying gravity data to lumis?"): the controls turn with the device, the level indicator follows, and a save's orientation tag is the way it is held.
                let gravity = sys::gravity();
                static SAID: AtomicBool = AtomicBool::new(false);
                if self.knobs & DEV_OPEN != 0 && dev_knobs2() & DEV_FULL != 0 && !SAID.swap(true, Ordering::Relaxed) {
                    println!("lumis: development: the gravity given, {gravity:?} (None: upright)");
                }
                app::ui_frame(ui, surface, Buttons::default(), gravity.unwrap_or(UPRIGHT), self.finger);
                self.probe.pass(ui.previous_image_counter, t.elapsed().as_micros() as u64);
                // Every reason draw_screen draws the whole screen that this side can read: a new image, a new exposure, a new histogram, a calibration hold or its end, the calibration screens, and any touch (a touch may change any control).
                // Not readable from here: a buffer whose corner pixel another drew over (never on ferros: the buffer is Lumis's own). A turn of the device and the level indicator are read below, from the fields lumis_core keeps for them.
                // A new image (its counter, or the exposure's start that moves with it at every completion) is what it changed of the camera's part and the controls that tick (`partial_parts` with lumis_core `camera_damage`: the fit view's rect, the bars around the image holding controls that did not move; in Slitscan the wiper's band of rows, owner 2026-11-18). Anything else that moved draws whole.
                let new_image = ui.previous_image_counter != before.0 || ui.previous_exposure_start != before.1;
                let other = ui.previous_histogram_counter != before.2
                    || ui.previous_hold_present
                    || before.3
                    || ui.header[FLAGS_IDX] & (CALIBRATING_BIT | CAL_SHOW_RESULT_BIT) != 0
                    || before.4
                    || ui.last_touch_valid
                    || self.finger.is_some()
                    || video_label_moved(&mut self.video_label, &mut self.video_drawn, ui)
                    // A turn of the device, or the level indicator repainted (lumis_core handle_input_and_draw: past its deadband, or any change after a second): the controls are drawn whole.
                    || (ui.device_rotation, ui.painted_roll, ui.painted_pitch) != before.5;
                self.parts.clear();
                if surface.take_posted() {
                    let camera: Option<Vec<fluor::canvas::PixelRect>> = lumis_core::ui::screen::camera_damage(ui).map(|v| v.into_iter().map(|(x0, y0, x1, y1)| fluor::canvas::PixelRect::new(x0, y0, x1, y1)).collect());
                    if other || self.knobs & DEV_WHOLE != 0 {
                        self.changed = true;
                    } else if new_image {
                        match &camera {
                            Some(camera) => self.parts = partial_parts(ui, camera),
                            None => self.changed = true,
                        }
                    } else if ui.controls_visible {
                        self.parts = partial_parts(ui, &[]);
                    }
                }
                if self.finger.is_some() {
                    self.down_sampled = true;
                }
                if self.pending_up {
                    self.finger = None;
                    self.pending_up = false;
                    self.down_sampled = false;
                }
            }
        }
        // The camera screen took its post above; the menu's (and the first camera pass after the opening, `changed` set there) are whole.
        if !matches!(self.screen, Screen::Camera(_)) {
            self.changed |= surface.take_posted();
        }
        self.changed || !self.parts.is_empty()
    }

    /// The whole frame when Lumis drew it whole, the controls' repainted parts when it drew only those, nothing otherwise (a touch that drew nothing shows nothing new; fluor's default, the whole viewport, would clear the scratch for a frame never converted).
    fn damage_rects(&mut self, viewport: fluor::geom::Viewport, out: &mut [fluor::canvas::PixelRect; fluor::canvas::DAMAGE_RECTS]) -> usize {
        if self.changed {
            out[0] = fluor::canvas::PixelRect::new(0, 0, viewport.width_px as usize, viewport.height_px as usize);
            return 1;
        }
        for (o, r) in out.iter_mut().zip(&self.parts) {
            *o = *r;
        }
        self.parts.len()
    }

    /// Lumis's frame (gamma-2 bytes) into fluor's pixels, the rows of the damage clip, straight from Lumis's buffer, a row per task on the worker cores. The frame is opaque everywhere, so every pixel is written and nothing lies under it.
    /// Lumis's buffer starts zeroed, black, so the host's first frame (always the whole viewport) shows black wherever Lumis has not drawn yet.
    fn render(&mut self, target: &mut [G2t16], ctx: &mut Context) {
        let Some(surface) = self.surface.as_ref() else { return };
        self.changed = false;
        let t = std::time::Instant::now();
        let (w, lut) = (self.width, &self.lut);
        // Each damage rect (the host cut them at the frame), its rows across the workers.
        for &fluor::canvas::PixelRect { x0, y0, x1, y1 } in ctx.damage_rects() {
            target[y0 * w..y1 * w].par_chunks_mut(w).zip(surface.pixels()[y0 * w * 3..y1 * w * 3].par_chunks(w * 3)).for_each(|(px, src)| {
                for (p, rgb) in px[x0..x1].iter_mut().zip(src[x0 * 3..x1 * 3].chunks_exact(3)) {
                    *p = G2t16::from_channels(G2t16::ONE, [lut[rgb[0] as usize], lut[rgb[1] as usize], lut[rgb[2] as usize]]);
                }
            });
        }
        if matches!(self.screen, Screen::Camera(_)) {
            self.probe.convert += t.elapsed().as_micros() as u64;
            self.probe.converts += 1;
        }
    }

    /// The menu (and the menu still shown while a camera opens) drawn whole on the GPU straight into a surface of Lumis's own (`gpu::Gpu::frame`), which the host shows (`own_surface`): no scratch, no conversion, no bake on the CPU. A GPU that fails leaves the menu to the CPU from then, said on the console.
    fn render_own(&mut self, _ctx: &mut Context) -> bool {
        self.own = None;
        let mut shared = gpu::SHARED.lock().unwrap();
        let (Some(gpu), Some(menu), Some(surface)) = (shared.as_mut(), self.menu.as_mut(), self.surface.as_ref()) else { return false };
        let camera = matches!(self.screen, Screen::Camera(_));
        let t = std::time::Instant::now();
        // The camera screen: Lumis's frame as lumis_core drew it (in the block), whole, in place of the CPU's present of its changed rects (the owner, 2026-11-21: the GPU composes full frames).
        let drawn = if camera { gpu.camera() } else { gpu.menu(menu, std::mem::take(&mut self.menu_fresh)) };
        match drawn {
            Ok(base) => {
                self.own = Some(base);
                self.changed = false;
                if camera {
                    self.probe.convert += t.elapsed().as_micros() as u64;
                    self.probe.converts += 1;
                }
                true
            }
            Err(e) => {
                println!("lumis: {e}: Lumis drawn on the CPU from now");
                if !camera {
                    app::menu_draw(menu, surface, true);
                    surface.take_posted();
                }
                *shared = None;
                false
            }
        }
    }

    fn own_surface(&self) -> Option<u64> {
        self.own
    }

    fn cursor_for(&self, _x: Coord, _y: Coord, _ctx: &Context) -> CursorIcon {
        CursorIcon::Default
    }

    fn opaque(&self) -> bool {
        true
    }

    /// With the display baking, Lumis's frame goes from its own RGB888 buffer straight into the surface in the display's codes (`present_baked`): fluor neither clears nor renders a scratch nor bakes it.
    fn bakes_own(&self) -> bool {
        self.surface.is_some() && self.knobs & DEV_SCRATCH == 0
    }

    /// One changed rect, baked: each pixel's three bytes as the 16-bit g2t codes they are (the `lut`'s darkness from white), opaque, thru the display's own `Bake::row`, rows across the worker cores. The same codes the host's bake of `render`'s G2t16 pixels gives, bit for bit (fluor host::panel `baking_codes_equals_baking_the_opaque_frame`), in one pass from Lumis's buffer.
    fn present_baked(&mut self, rect: fluor::canvas::PixelRect, rows: &mut [u64], stride: usize, fx: usize, bake: &fluor::host::panel::Bake) {
        let Some(surface) = self.surface.as_ref() else { return };
        self.changed = false;
        let t = std::time::Instant::now();
        let w = self.width;
        // A byte's code: the darkness the lut holds, from white.
        let code: [u16; 256] = core::array::from_fn(|v| (G2t16::ONE - self.lut[v]) as u16);
        // The opaque lane: the transparency 16384 - 65535 / 4, in bits 49-62.
        let opaque = ((16384 - (G2t16::ONE >> 2)) << 1) as u16;
        let src = surface.pixels();
        rows.par_chunks_mut(stride).enumerate().for_each(|(k, row)| {
            let line = &src[(rect.y0 + k) * w * 3..];
            bake.row(
                |i| {
                    let p = (rect.x0 + i) * 3;
                    ([code[line[p] as usize], code[line[p + 1] as usize], code[line[p + 2] as usize]], opaque)
                },
                &mut row[fx + rect.x0..fx + rect.x1],
            );
        });
        if matches!(self.screen, Screen::Camera(_)) {
            self.probe.convert += t.elapsed().as_micros() as u64;
            self.probe.converts += 1;
        }
    }
}

/// The driver's camera list, or the reason there is none (each said on the console; the menu then lists no camera).
fn list() -> Vec<camera::Camera> {
    if !sys::held(CAMERA) {
        println!("lumis: no camera grant in slot {CAMERA}: the cameras were not given");
        return Vec::new();
    }
    let mut r = vec![0u8; 1 << 16];
    let n = sys::call_live(CAMERA, &[camera::OP_LIST], &mut r);
    if n == sys::ERR {
        println!("lumis: the camera endpoint has no server: no camera driver on this device");
        return Vec::new();
    }
    match camera::parse_list(&r[..n as usize]) {
        Ok(mut c) => {
            // Each camera's readouts together, the binned one first (Android's order: the menu shows a lens by its first readout and offers the rest on its page).
            c.sort_by_key(|c| (c.id, c.mode));
            // A driver from before the lines: the binned main paces by ferros_clip's own line; the rest record no video.
            for r in c.iter_mut().filter(|r| r.line.is_none() && r.id == 0 && r.mode == 0) {
                r.line = Some(ferros_clip::MAIN_LINE);
            }
            println!("lumis: {} readouts listed", c.len());
            for (k, r) in c.iter().enumerate() {
                println!("lumis: readout {k}: {} (camera {}), mode {}, {} x {}, filter period {}, rotation {}, facing {}, exposures {} ns to {} ns", r.name, r.id, r.mode, r.width, r.height, r.cfa_period, r.rotation, r.facing, r.exposure_min, r.exposure_max);
            }
            c
        }
        Err(e) => {
            println!("lumis: {e}");
            Vec::new()
        }
    }
}

fn main() {
    std::panic::set_hook(Box::new(|info| {
        let at = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
        println!("lumis: PANIC at {at}: {info}");
    }));
    let _ = log::set_logger(&Console).map(|()| log::set_max_level(log::LevelFilter::Info));
    println!("lumis: Lumis {} on ferros, {} grants", env!("CARGO_PKG_VERSION"), sys::grant_count());
    // The intake on the A510s (four threads, one a core) unless the knobs keep it with the UI.
    if dev_input().0 & DEV_BIG_INTAKE == 0 {
        lumis_core::set_intake_cores(|| {
            sys::pin(CORES_LITTLE);
        }, 4);
        lumis_core::set_save_cores(|| {
            sys::pin(CORES_WORKERS);
        });
        // A save on three of the four A715s (2026-11-22: on all four, at 60 a second, the owner's first shot ended Lumis, the UI drawing on the same cores; the fourth is the UI's now), a thread each (the coordinator, 2026-11-19: on two of them a 12 MP VSF 8B took 15.9 s, 9.6 s of it AV1): the preview loses frames for the seconds a save takes, the camera none (the X3, which answers the camera's frames, stays out of it).
        lumis_core::platform::set_save_threads(3);
        println!("lumis: the intake on the A510s (cores G#{CORES_LITTLE:04X}), the UI and saves on the A715s (G#{CORES_WORKERS:04X})");
    }
    // Whatever the intake's placement: a clip recording is sealed before Lumis ends, and the clips page lists and removes Lumis's clips in the person's hold.
    lumis_core::platform::set_before_exit(seal_at_exit);
    lumis_core::clips::set_hold(lumis_core::clips::Hold { list: hold::list_clips, remove: hold::remove_clip });
    println!("lumis: the hold (your things) {}", if hold::held() { format!("in slot {}", hold::ITEMS) } else { format!("not given (slot {} empty): stills kept in Lumis's own domain, no RAW clips", hold::ITEMS) });
    let vault = open_vault();
    if let Some(v) = &vault {
        lumis_core::platform::set_config_store(Box::new(Vault(v.clone())));
        // The keeper (`keep_saves`) keeps saves in it from its own thread.
        let _ = VAULT.set(v.clone());
    }
    let cameras = list();
    // The menu drawn as ink (lumis_core `ui::draw::set_ink`): the CPU draws its words and lines, the GPU (or, without one, the CPU) the splash and the slots under them.
    lumis_core::ui::draw::set_ink(true);
    // The camera's integrator keeps its shared memory in the GPU block when the GPU is open (lumis_core `platform::set_shared_place`, `gpu::shared_place`), where the GPU stacks each frame.
    lumis_core::platform::set_shared_place(gpu::shared_place);
    let caps = fluor::host::ferros::Caps { display: DISPLAY, touch: TOUCH, surface: SURFACE };
    // In the person's app scale and the device's theme, following both (ferros_fluor); a pinch anywhere in Lumis is the app scale (the owner, 2026-11-20: "RU everywhere. Should inherit the global RU"), written back with the display grant at its end.
    match ferros_fluor::run_app(Lumis::new(cameras, vault), caps, None, sys::cap(DISPLAY)) {
        Ok(()) => println!("lumis: closed"),
        Err(e) => println!("lumis: {e}"),
    }
}
