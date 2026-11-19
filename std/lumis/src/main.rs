//! Lumis on ferros: Lumis's own core (the lumis_core library: the camera menu, the integrator, the camera UI, the VSF saves) with ferros as its platform, as Photon runs on ferros as itself (std/photon).
//! What Android's Kotlin layer and JNI do, this crate does with ferros's pieces (lumis_core's `app` and `platform` modules are the seam):
//! - the window and touch: fluor's ferros host; Lumis draws into an RGB888 frame (lumis_core `PixelSurface`), shown here as fluor's 16-bit g2t pixels;
//! - the sensor: the `camera` endpoint (docs/CAMERA.md): the list, an open, a frame per call, the manual controls Lumis asks for after each frame;
//! - Lumis's two processes (the UI and the camera service sharing ashmem) as two threads sharing the integrator's memory;
//! - its settings and saves: the kist's own vault domain (kete), never files.
//!
//! Its grant slots, as ferros lays out a kist that declares only the camera (ferros_kist `slots`): 1 display, 2 touch, 3 surface, 4 ira, 5 its vault domain, 6 entropy, 7 camera (an empty slot when the person said no), then the GPU's three and sound out, which it does not use.

mod camera;
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

const DISPLAY: u64 = 1;
const TOUCH: u64 = 2;
const SURFACE: u64 = 3;
const IRA: u64 = 4;
const CAMERA: u64 = 7;

/// The binning Lumis asks the driver for when the open gave no view (a driver or kernel from before it): the largest frame a call can carry is 8 MiB (process.rs `REPLY_MAX`), so the main camera's whole readout (4080 x 3072 samples, 24 MiB) cannot cross and bin 2 (2040 x 1536, 6 MiB) is the finest a copy carries.
/// With the view (docs/CAMERA.md "The view"; owner, 2026-11-15: "why the ridiculously low camera resolution?") Lumis integrates and saves whole sensor frames, read in place.
const BIN: u32 = 2;

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
    match open(cam, false) {
        Ok(s) => s,
        Err(Refused::Why(why)) => {
            keep_saves(vault, header);
            lumis_core::platform::exit_because(&format!("camera {}: opened again for whole frames, it failed: {why}", cam.name));
        }
        Err(Refused::NoSlit) => unreachable!("a whole-frame open is never a slit's"),
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
/// Development, the second knobs byte: whole frames first, then Slitscan by itself 2^12 ms in (no tap: the attest screen takes every touch), so the reopen (CLOSE, and an OPEN that starts a stored driver on demand) runs and is timed; with a driver from before the slit it falls back, and opens whole frames again.
const DEV_SLIT_LATER: u8 = 1 << 2;
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
}

impl lumis_core::RowSource for ViewRows {
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
                Some(Got::Frame(m.frame, Taken::View(ViewRows { src: *base + m.offset, line: view.line, cached: view.cached })))
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
/// The menu's entries, one per readout (`list`'s order, which `StartCamera` indexes): a camera's readouts share its group (its `id`), the first carrying the group's size, as Android's Lumis groups a lens's capture modes; each says the widest readout's width as the pixel array's, so the page's pixel size is the sensor's, and the 1:1 readout is `max_res` (Android's maximum resolution).
fn camera_infos(cameras: &[camera::Camera]) -> Vec<CameraInfo> {
    cameras
        .iter()
        .enumerate()
        .filter_map(|(index, c)| {
            let group = cameras.iter().filter(|o| o.id == c.id);
            let first = cameras.iter().position(|o| o.id == c.id) == Some(index);
            let (modes, widest) = group.fold((0u32, 0u32), |(n, w), o| (n + 1, w.max(o.width)));
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
                max_res: c.mode != 0,
                is_cropped: false,
                has_cal: false,
            })
        })
        .collect()
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
fn open(cam: &camera::Camera, slit: bool) -> Result<Source, Refused> {
    let mut ask = vec![camera::OP_OPEN, cam.id, camera::OPEN_VIEW];
    let (knobs, block) = dev_input();
    if slit {
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
            Ok(Source::View { base: base as usize, view: v, held: camera::NONE })
        }
        Some(v) => Err(Refused::Why(format!("the view's format {} is not one Lumis reads", v.format))),
        None if slit => Err(Refused::NoSlit),
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
    let mut src = match open(&cam, dev_slit) {
        Ok(s) => s,
        Err(Refused::Why(why)) => return fail(why),
        Err(Refused::NoSlit) => {
            close_camera(&cam.name);
            return fail("the development slit open: the driver gave no slit ring".into());
        }
    };
    let mut turns = Turns::default();
    let first = if dev_slit {
        println!("lumis: camera {}: development: slits from the open, Lumis sized from the list ({} x {})", cam.name, cam.width, cam.height);
        let shutter_ns = (1_000_000_000u64 >> 11) << (2 * ((dev_knobs2() >> DEV_SLIT_SHUTTER) & 3));
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
    lumis_core::set_camera_identity(CameraIdentity { make: "ferros".into(), model: "camera".into(), camera_id: cam.name.clone() });
    // The integrator's memory is the UI's too, for the life of the process (the UI reads it every frame), so the integrator is never dropped.
    let integrator = Box::leak(app::camera_new(&setup));
    if let Some(dark) = calibration {
        app::calibration_start(integrator, dark);
    }
    if dev_slit {
        integrator.header[CURRENT_MODE_IDX] = RawMode::Slitscan as u64;
    }
    *opened.lock().unwrap() = Some(Opened::Memory { ptr: integrator.get_shared_memory_ptr() as usize, bytes: app::shared_memory_bytes(first.width, first.height) });
    let gain_of = |iso: i32| (iso as f64 * camera::GAIN_UNITY as f64 / ISO_AT_UNITY) as u64;
    let focus_of = |diopters: f32| cam.focus_min as u64 + ((diopters as f64 / NEAREST_DIOPTERS) * (cam.focus_max - cam.focus_min) as f64) as u64;
    // What the sensor was last told, so a setting goes to the driver only when it changes (as CameraProcessor.updateCameraSettings).
    let mut sent: (i32, i64, f32) = (-1, -1, -1.);
    let mut frames = 0u64;
    // Development runs (`DEV_OPEN`, `DEV_SWEEP`): the run's length, and the shutter sweep's steps (ns) with the step in hand and when its frames began.
    let run = (knobs & DEV_OPEN != 0).then(|| std::time::Duration::from_secs(1 << (((knobs >> 4) & 3) + 3)));
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
    // When the last reopen began (its CLOSE) and when its OPEN was answered, until its first frame or slit: the console says the three apart (a stored driver ends at CLOSE and the next OPEN starts it again on demand, a path first run here).
    let mut reopened: Option<(std::time::Instant, std::time::Instant)> = None;
    let slit_later = (dev_knobs2() & DEV_SLIT_LATER != 0).then(|| std::time::Instant::now() + std::time::Duration::from_millis(1 << 12));
    // The first frame again, now that the integrator exists: the next one (the first went to its sizing and seed).
    loop {
        // The view read only while this is held (`VIEW`), released for the driver's wait below.
        let mut reading = VIEW.lock().unwrap();
        if LEAVING.load(Ordering::Acquire) {
            drop(reading);
            finish_leaving(integrator, vault.as_deref());
        }
        if slit_later.is_some_and(|t| std::time::Instant::now() >= t) && integrator.header[CURRENT_MODE_IDX] != RawMode::Slitscan as u64 {
            println!("lumis: development: Slitscan");
            integrator.header[CURRENT_MODE_IDX] = RawMode::Slitscan as u64;
        }
        // Slitscan takes slits, every other mode whole frames: the camera is closed (the driver ends) and opened again for the other (owner, 2026-11-18: "Do wire it up so I get more than 30 lines per second"). The new driver starts at the init's settings, so the integrator's are sent again.
        if slit_ok && wants_slits(&integrator.header) != src.is_slit() {
            let slit = !src.is_slit();
            let t_close = std::time::Instant::now();
            report(&src, &mut turns, &cam.name);
            keep_saves(vault.as_deref(), &mut *integrator.header);
            close_camera(&cam.name);
            println!("lumis: camera {}: {}", cam.name, if slit { "Slitscan: opening for slits" } else { "out of Slitscan: opening for whole frames" });
            src = match open(&cam, slit) {
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
            turns = Turns::default();
            reopened = Some((t_close, std::time::Instant::now()));
        }
        let slit_now = src.is_slit();
        drop(reading);
        let got = src.next(&mut turns);
        reading = VIEW.lock().unwrap();
        // Closed for leaving while this waited: the frame it may have brought is not read (the view is the driver's again).
        if LEAVING.load(Ordering::Acquire) {
            drop(reading);
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
                continue;
            }
            None => {
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
        let flags = integrator.header[FLAGS_IDX];
        let behind = BEHIND.load(Ordering::Acquire);
        let kept = kept(flags);
        let now = match (behind, kept, in_flight(flags)) {
            (false, _, _) => Stay::Front,
            (true, true, _) if flags & CONTINUOUS_SAVE_BIT != 0 => Stay::Continuous,
            (true, true, _) => Stay::Calibrating,
            (true, false, true) => Stay::Finishing,
            (true, false, false) => leave(&cam.name, vault.as_deref(), &mut *integrator.header),
        };
        if now != stay {
            match now {
                Stay::Front => println!("lumis: in front again: carrying on"),
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
        let (iso, shutter_ns, focus) = match got {
            Got::Frame(head, Taken::View(rows)) => integrator.process_rows(&rows, iso(head.gain) as i32, head.exposure_ns as i64, 0., head.quad()),
            // A copy is 16-bit samples of a binned readout (the driver gives none at 1:1); process_frame's flag is RAW10 bytes, not the filter.
            Got::Frame(head, Taken::Bytes(bytes)) => integrator.process_frame(bytes, iso(head.gain) as i32, head.exposure_ns as i64, 0., false, head.width * 2),
            // A slit carries no settings of its own: the last sent are what it was taken at (the integrator reads none of them for a slit).
            Got::Slit { rows, .. } => integrator.process_slit(&rows, rows.rows, sent.0, sent.1, sent.2),
        };
        let t_after = std::time::Instant::now();
        turns.integrate += (t_after - t_integrate).as_micros() as u64;
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

/// Each save the integrator finished (Android: drainPendingSave, MediaStore), kept in the kist's vault under "save/" and its file name, then marked done.
/// Each save goes into the vault as a bulk value (kete `write_device_bulk`: kept out of the read cache, which would otherwise hold a second copy of every still), is read back from the vault itself (`read_device_bulk`, never the cache) and compared with what was written (blake3 of both), so "kept" means the vault gives the file back whole: one console line with its size, its encode's time, the write's, the read's, and the match.
fn keep_saves(vault: Option<&kete::FlatStorage>, header: &mut [u64]) {
    while let Some(lumis_core::PendingSave { bytes, name, encode_ms }) = lumis_core::get_pending_save() {
        let key = format!("save/{name}");
        let t = std::time::Instant::now();
        let kept = match vault {
            Some(v) => match v.write_device_bulk(&[(key.clone(), bytes.clone())]) {
                Ok(()) => {
                    let write_ms = t.elapsed().as_millis();
                    let t = std::time::Instant::now();
                    let back = v.read_device_bulk(&key);
                    let read_ms = t.elapsed().as_millis();
                    let want = blake3::hash(&bytes);
                    match back {
                        Ok(Some(b)) if blake3::hash(&b) == want => {
                            println!("lumis: kept {name}: {} bytes, encoded in {encode_ms} ms, written in {write_ms} ms, read back in {read_ms} ms, blake3 {} matches", bytes.len(), &want.to_hex()[..16]);
                            true
                        }
                        Ok(Some(b)) => {
                            println!("lumis: {name} NOT kept whole: wrote {} bytes (blake3 {}), read back {} bytes (blake3 {}); encoded in {encode_ms} ms, written in {write_ms} ms, read in {read_ms} ms", bytes.len(), &want.to_hex()[..16], b.len(), &blake3::hash(&b).to_hex()[..16]);
                            false
                        }
                        Ok(None) => {
                            println!("lumis: {name} NOT kept: written ({} bytes in {write_ms} ms), then absent on reading it back", bytes.len());
                            false
                        }
                        Err(e) => {
                            println!("lumis: {name} NOT kept: written ({} bytes in {write_ms} ms), then the read back failed: {e:?}", bytes.len());
                            false
                        }
                    }
                }
                Err(e) => {
                    println!("lumis: {name} ({} bytes, encoded in {encode_ms} ms) not kept: {e:?}", bytes.len());
                    false
                }
            },
            None => {
                println!("lumis: {name} ({} bytes) not kept: no vault this launch", bytes.len());
                false
            }
        };
        // A calibration map is not a photo: it never counts (as on Android).
        app::save_done(header, kept && !name.starts_with("cal_"));
    }
}

/// What the camera thread does with the window behind, said on the console as it changes.
#[derive(Clone, Copy, PartialEq)]
enum Stay {
    /// In front: Lumis as always.
    Front,
    /// Behind during a continuous save: capturing and saving on, nothing drawn.
    Continuous,
    /// Behind during a calibration: capturing on, nothing drawn.
    Calibrating,
    /// Behind with a save on its way: that save finished and kept, then Lumis ends.
    Finishing,
}

/// Whether Lumis stays behind for as long as it runs: a continuous save or a calibration (owner, 2026-11-17: "Basically if continuous save or cal is on"), the same bits lumis_core's integrator keeps the camera alive on with the UI quiet (lumis ferros-cal-behind).
/// A calibration whose finish was tapped holds CALIBRATING_BIT until its maps are written, so it stays thru its own save.
fn kept(flags: u64) -> bool {
    flags & (CONTINUOUS_SAVE_BIT | CALIBRATING_BIT) != 0
}

/// Whether a save the person asked for is still on its way, by the integrator's flags: a single save asked for and not yet begun (MANUAL_SAVE_BIT, which try_save starts from the image already made), or one encoding or encoded and not yet kept (CURRENTLY_SAVING, cleared by `keep_saves` once the vault has it).
/// An exposure still integrating with no save asked for is not on its way: nothing was asked to be kept.
fn in_flight(flags: u64) -> bool {
    flags & (MANUAL_SAVE_BIT | CURRENTLY_SAVING) != 0
}

/// The driver's CLOSE (docs/CAMERA.md, op 4): the view taken back, the stream stopped, the module powered down, and the driver ends.
/// Said by the camera thread (or before it reads any frame), so nothing reads the view after the driver takes it back.
fn close_camera(name: &str) {
    let mut r = [0u8; 4];
    if sys::call_live(CAMERA, &[camera::OP_CLOSE], &mut r) == sys::ERR {
        println!("lumis: camera {name}: the CLOSE was not answered; the driver closes when Lumis ends");
    } else {
        println!("lumis: camera {name}: closed");
    }
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
    if ui.slider_thickness > 0. && ui.exposure_time_ms > 0 {
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
    /// The development knobs from the spawn input (`DEV_WHOLE`, `DEV_COPY`, `DEV_SCRATCH`).
    knobs: u8,
    /// A display byte (Lumis's gamma-2 encoded light, 255 its white) as a 16-bit g2t darkness: the code VSF's encoder gives (atanh(code / 2^16)^2 = the light), Lumis's white at g2t's diffuse white.
    lut: [u32; 256],
    /// The preview's split on the UI thread (`Probe`).
    probe: Probe,
}

impl Lumis {
    fn new(cameras: Vec<camera::Camera>, vault: Option<Arc<kete::FlatStorage>>) -> Self {
        let mut lut = [0u32; 256];
        for (v, d) in lut.iter_mut().enumerate() {
            let light = (v as f32 / 255.) * (v as f32 / 255.);
            *d = G2t16::ONE - vsf::colour::convert::delinearize_gamma2tanh_bits_f32(light, 16);
        }
        Lumis { width: 0, height: 0, cameras, vault, screen: Screen::Menu, menu: None, menu_dirty: true, camera_name: String::new(), dev_opened: false, surface: None, opened: Arc::new(Mutex::new(None)), finger: None, down_sampled: false, pending_up: false, changed: false, parts: Vec::new(), knobs: dev_input().0, lut, probe: Probe::new() }
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

impl FluorApp<G2t16> for Lumis {
    type UserEvent = ();

    fn init(&mut self, _ctx: &mut Context) {}

    fn on_resize(&mut self, width: u32, height: u32, ctx: &mut Context) {
        let (w, h) = (width as usize, height as usize);
        if (w, h) == (self.width, self.height) {
            return;
        }
        // The frame is the panel, set once at start (ferros windows are the whole screen); Lumis is built for that size, as Android builds its menu and UI for the surface's.
        (self.width, self.height) = (w, h);
        self.surface = Some(PixelSurface::new(w, h));
        self.changed = true;
        self.menu = Some(app::menu_new(width, height, DENSITY_DPI, camera_infos(&self.cameras)));
        self.menu_dirty = true;
        ctx.window.request_redraw();
    }

    fn on_event(&mut self, event: &Event, ctx: &mut Context) -> EventResponse {
        if let Event::Focused(focused) = event {
            self.focus(*focused);
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
                self.open(0, None);
                return true;
            }
            Screen::Menu => {
                if self.menu_dirty {
                    self.menu_dirty = false;
                    if let Some(menu) = self.menu.as_mut() {
                        app::menu_draw(menu, surface, false);
                    }
                }
            }
            Screen::Opening => {}
            Screen::Camera(ui) => {
                use lumis_core::shared_memory::CAL_SHOW_RESULT_BIT;
                // What lumis_core's draw_screen last drew with, before this pass and after it: a field that moved is a full draw (the camera image and the controls whole).
                let before = (ui.previous_image_counter, ui.previous_exposure_start, ui.previous_histogram_counter, ui.previous_hold_present, ui.last_touch_valid, (ui.device_rotation, ui.painted_roll, ui.painted_pitch));
                let t = std::time::Instant::now();
                // The AoC's gravity, as Android's Lumis gets TYPE_GRAVITY (owner, 2026-11-18: "looks like we aren't relaying gravity data to lumis?"): the controls turn with the device, the level indicator follows, and a save's orientation tag is the way it is held.
                app::ui_frame(ui, surface, Buttons::default(), sys::gravity().unwrap_or(UPRIGHT), self.finger);
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
            println!("lumis: {} readouts listed", c.len());
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
        println!("lumis: the intake on the A510s (cores G#{CORES_LITTLE:04X}), the UI and saves on the A715s (G#{CORES_WORKERS:04X})");
    }
    let vault = open_vault();
    if let Some(v) = &vault {
        lumis_core::platform::set_config_store(Box::new(Vault(v.clone())));
    }
    let cameras = list();
    let caps = fluor::host::ferros::Caps { display: DISPLAY, touch: TOUCH, surface: SURFACE };
    match fluor::host::ferros::run_app(Lumis::new(cameras, vault), caps, 1.0, None) {
        Ok(()) => println!("lumis: closed"),
        Err(e) => println!("lumis: {e}"),
    }
}
