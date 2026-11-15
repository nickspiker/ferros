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
use rayon::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const DISPLAY: u64 = 1;
const TOUCH: u64 = 2;
const SURFACE: u64 = 3;
const IRA: u64 = 4;
const CAMERA: u64 = 7;

/// The binning Lumis asks the driver for when the open gave no view (a driver or kernel from before it): the largest frame a call can carry is 8 MiB (process.rs `REPLY_MAX`), so the main camera's whole readout (4080 x 3072 samples, 24 MiB) cannot cross and bin 2 (2040 x 1536, 6 MiB) is the finest a copy carries.
/// With the view (docs/CAMERA.md "The view"; owner, 2026-11-15: "why the ridiculously low camera resolution?") Lumis integrates and saves whole sensor frames, read in place.
const BIN: u32 = 2;

/// Rows unpacked per task when a frame is taken out of the view.
const ROWS_PER_TASK: usize = 1 << 4;

/// Where the camera service's frames come from.
enum Source {
    /// The view: whole frames read in place (`base` where this process reads it), the buffer held, the frame unpacked to 16 bits.
    View { base: usize, view: camera::View, held: u8, frame: Vec<u16> },
    /// Frames by copy at `BIN`, header and samples.
    Copy { buf: Vec<u8> },
}

/// A frame's time in the camera service, for the console every 64: waiting on the driver, unpacking out of the view, the integrator, the whole turn (each summed, microseconds).
#[derive(Default)]
struct Turns {
    wait: u64,
    unpack: u64,
    integrate: u64,
    since: Option<std::time::Instant>,
    n: u64,
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

impl Source {
    /// The next frame, its header and its samples (16 bits, little-endian, row by row), with the wait and the unpacking counted.
    fn next(&mut self, t: &mut Turns) -> Option<(camera::FrameHeader, &[u8])> {
        let t0 = std::time::Instant::now();
        match self {
            Source::View { base, view, held, frame } => {
                let mut r = [0u8; camera::FRAME_HEADER];
                let n = sys::call_live(CAMERA, &[camera::OP_MAP, *held], &mut r);
                let m = (n != sys::ERR).then(|| camera::parse_map(&r[..n as usize])).flatten()?;
                *held = m.buffer;
                let t1 = std::time::Instant::now();
                t.wait += (t1 - t0).as_micros() as u64;
                let (w, line) = (view.width, view.line);
                let src = *base + m.offset;
                frame.par_chunks_mut(w * ROWS_PER_TASK).enumerate().for_each_init(
                    || vec![0u8; line],
                    |tmp, (k, rows)| {
                        for (j, row) in rows.chunks_exact_mut(w).enumerate() {
                            copy_from_view(src + (k * ROWS_PER_TASK + j) * line, tmp);
                            camera::unpack_raw10(tmp, row);
                        }
                    },
                );
                t.unpack += t1.elapsed().as_micros() as u64;
                // SAFETY: the u16 samples' own bytes, little-endian on this target as the integrator reads them.
                let bytes = unsafe { core::slice::from_raw_parts(frame.as_ptr() as *const u8, frame.len() * 2) };
                Some((m.frame, bytes))
            }
            Source::Copy { buf } => {
                let n = sys::call_live(CAMERA, &[camera::OP_FRAME, BIN as u8], buf);
                t.wait += t0.elapsed().as_micros() as u64;
                let h = (n != sys::ERR).then(|| camera::parse_frame(&buf[..n as usize])).flatten()?;
                Some((h, &buf[camera::FRAME_HEADER..camera::FRAME_HEADER + h.samples_bytes()]))
            }
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

/// Android's gravity vector for a device held upright in portrait (m/s², +y toward the top edge): ferros hands a kist no motion sensor yet, so Lumis's UI stays upright and its level reads that.
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
/// The optical facts (sensor size, focal length, aperture) are not in the driver's list, so the menu draws no sensor or field of view for them.
fn camera_infos(cameras: &[camera::Camera]) -> Vec<CameraInfo> {
    cameras
        .iter()
        .enumerate()
        .filter_map(|(index, c)| {
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
                sensor_width: 0.,
                sensor_height: 0.,
                focal_lengths: Vec::new(),
                apertures: Vec::new(),
                min_focus_distance: if c.focus_max > c.focus_min { NEAREST_DIOPTERS as f32 } else { 0. },
                has_ois: false,
                // Shown as "RAW": full manual control of a raw stream.
                hardware_level: 1,
                sensor_orientation: c.rotation as i32 * 90,
                pixel_array_width: c.width,
                mode_count: 1,
                group_id: -1,
                max_res: false,
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
fn camera_service(cam: camera::Camera, calibration: Option<bool>, vault: Option<Arc<kete::FlatStorage>>, opened: Arc<Mutex<Option<Opened>>>) {
    let fail = |why: String| {
        println!("lumis: camera {}: {why}", cam.name);
        *opened.lock().unwrap() = Some(Opened::Failed(why));
    };
    // The open asks for the view (docs/CAMERA.md "The view"): whole sensor frames read in place. A driver or kernel that gives none answers a plain open, and frames come by copy at `BIN`.
    let mut r = [0u8; camera::VIEW_REPLY];
    let n = sys::call_live(CAMERA, &[camera::OP_OPEN, cam.id, camera::OPEN_VIEW], &mut r);
    match (n, r[0]) {
        (sys::ERR, _) => return fail("the camera driver did not answer the open".into()),
        (_, 0) => {}
        (_, 1) => return fail("the driver has no such camera".into()),
        (_, 2) => return fail("another kist has this camera open".into()),
        (_, k) => return fail(format!("the driver refused the open ({k})")),
    }
    // The view's token is kept in this process's memory for the life of the camera: the kernel answers its base from it.
    let mut src = match camera::parse_view(&r[..n as usize]) {
        Some(v) if v.format == camera::FORMAT_RAW10 => {
            let token = Box::leak(Box::new(v.token));
            let base = sys::dma(token);
            if base == sys::ERR {
                return fail("the view's token was refused".into());
            }
            println!("lumis: camera {}: the view, {} buffers of {} x {} RAW10 at {base:#x}", cam.name, v.buffers, v.width, v.height);
            Source::View { base: base as usize, view: v, held: camera::NONE, frame: vec![0u16; v.width * v.height] }
        }
        Some(v) => return fail(format!("the view's format {} is not one Lumis reads", v.format)),
        None => {
            println!("lumis: camera {}: no view; frames by copy at bin {BIN}", cam.name);
            // A frame's reply: the header and up to 8 MiB of samples (REPLY_MAX).
            Source::Copy { buf: vec![0u8; camera::FRAME_HEADER + (1 << 23)] }
        }
    };
    let mut turns = Turns::default();
    let Some((first, _)) = src.next(&mut turns) else {
        return fail("no first frame".into());
    };
    println!("lumis: camera {}: first frame, {} x {} samples (bin {}), exposure {} us, gain {}/1024", cam.name, first.width, first.height, first.bin, first.exposure_ns / 1000, first.gain);
    let iso = |gain: u32| gain as f64 * ISO_AT_UNITY / camera::GAIN_UNITY as f64;
    let (min_iso, max_iso) = (iso(cam.gain_min), iso(cam.gain_max));
    // The first open's seed (Android: the one-shot AE's metered values, or the calibration's extremes): the driver's own exposure and gain, as its first frame says.
    let (seed_iso, seed_shutter) = match calibration {
        Some(dark) => (max_iso, if dark { cam.exposure_max } else { cam.exposure_min } as f64),
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
    *opened.lock().unwrap() = Some(Opened::Memory { ptr: integrator.get_shared_memory_ptr() as usize, bytes: app::shared_memory_bytes(first.width, first.height) });
    let gain_of = |iso: i32| (iso as f64 * camera::GAIN_UNITY as f64 / ISO_AT_UNITY) as u64;
    let focus_of = |diopters: f32| cam.focus_min as u64 + ((diopters as f64 / NEAREST_DIOPTERS) * (cam.focus_max - cam.focus_min) as f64) as u64;
    // What the sensor was last told, so a setting goes to the driver only when it changes (as CameraProcessor.updateCameraSettings).
    let mut sent: (i32, i64, f32) = (-1, -1, -1.);
    let mut frames = 0u64;
    // The first frame again, now that the integrator exists: the next one (the first went to its sizing and seed).
    loop {
        let Some((head, samples)) = src.next(&mut turns) else {
            println!("lumis: camera {}: no frame (the driver refused or ended); Lumis ends, as Android's camera process does on a camera error", cam.name);
            lumis_core::platform::exit();
        };
        if head.width != first.width || head.height != first.height {
            println!("lumis: camera {}: a frame of {} x {} after {} x {}: the integrator is sized for the first; Lumis ends", cam.name, head.width, head.height, first.width, first.height);
            lumis_core::platform::exit();
        }
        let t_integrate = std::time::Instant::now();
        let (iso, shutter_ns, focus) = integrator.process_frame(samples, (iso(head.gain)) as i32, head.exposure_ns as i64, 0., false, head.width * 2);
        turns.integrate += t_integrate.elapsed().as_micros() as u64;
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
        frames += 1;
        turns.n += 1;
        if turns.n == 1 << 6 {
            let whole = turns.since.map_or(0, |s| s.elapsed().as_micros() as u64);
            println!("lumis: {frames} frames of {} x {} integrated; the last 64 every {} us: the driver's answer {} us, unpacking {} us, the integrator {} us", first.width, first.height, whole / 64, turns.wait / 64, turns.unpack / 64, turns.integrate / 64);
            turns = Turns { since: Some(std::time::Instant::now()), ..Turns::default() };
        } else if turns.since.is_none() {
            turns.since = Some(std::time::Instant::now());
        }
    }
}

/// Each save the integrator finished (Android: drainPendingSave, MediaStore), kept in the kist's vault under "save/" and its file name, then marked done.
fn keep_saves(vault: Option<&kete::FlatStorage>, header: &mut [u64]) {
    while let Some((bytes, name)) = lumis_core::get_pending_save_data() {
        let t = std::time::Instant::now();
        let kept = match vault {
            Some(v) => match v.write_device(&format!("save/{name}"), &bytes) {
                Ok(()) => true,
                Err(e) => {
                    println!("lumis: {name} ({} bytes) not kept: {e:?}", bytes.len());
                    false
                }
            },
            None => {
                println!("lumis: {name} ({} bytes) not kept: no vault this launch", bytes.len());
                false
            }
        };
        if kept {
            println!("lumis: kept {name}, {} bytes, into the vault in {} ms", bytes.len(), t.elapsed().as_millis());
        }
        // A calibration map is not a photo: it never counts (as on Android).
        app::save_done(header, kept && !name.starts_with("cal_"));
    }
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
    surface: Option<PixelSurface>,
    opened: Arc<Mutex<Option<Opened>>>,
    /// The finger, in frame pixels, and the camera UI's tap bookkeeping (UserInterface.kt: a tap shorter than a frame still reaches a draw as a down before its up).
    finger: Option<(f32, f32)>,
    down_sampled: bool,
    pending_up: bool,
    /// Lumis's frame as last shown, three bytes a pixel, and as fluor's pixels; the rows that differ since the last show.
    shown: Vec<u8>,
    pixels: Vec<G2t16>,
    changed: Option<(usize, usize)>,
    /// A display byte (Lumis's gamma-2 encoded light, 255 its white) as a 16-bit g2t darkness: the code VSF's encoder gives (atanh(code / 2^16)^2 = the light), Lumis's white at g2t's diffuse white.
    lut: [u32; 256],
}

impl Lumis {
    fn new(cameras: Vec<camera::Camera>, vault: Option<Arc<kete::FlatStorage>>) -> Self {
        let mut lut = [0u32; 256];
        for (v, d) in lut.iter_mut().enumerate() {
            let light = (v as f32 / 255.) * (v as f32 / 255.);
            *d = G2t16::ONE - vsf::colour::convert::delinearize_gamma2tanh_bits_f32(light, 16);
        }
        Lumis { width: 0, height: 0, cameras, vault, screen: Screen::Menu, menu: None, menu_dirty: true, surface: None, opened: Arc::new(Mutex::new(None)), finger: None, down_sampled: false, pending_up: false, shown: Vec::new(), pixels: Vec::new(), changed: None, lut }
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
        self.screen = Screen::Opening;
        let (vault, opened) = (self.vault.clone(), self.opened.clone());
        if std::thread::Builder::new().name("lumis-camera".into()).spawn(move || camera_service(cam, calibration, vault, opened)).is_err() {
            println!("lumis: no thread for the camera; back to the menu");
            self.screen = Screen::Menu;
        }
    }

    /// The rows of Lumis's frame that changed since the last show, into fluor's pixels (each row compared, then converted only if it differs).
    fn take_frame(&mut self) -> bool {
        let Some(surface) = &self.surface else { return false };
        let w = self.width;
        let (lo, hi) = (AtomicUsize::new(usize::MAX), AtomicUsize::new(0));
        let lut = &self.lut;
        self.shown
            .par_chunks_mut(w * 3)
            .zip(self.pixels.par_chunks_mut(w))
            .zip(surface.pixels().par_chunks(w * 3))
            .enumerate()
            .for_each(|(y, ((shown, px), src))| {
                if shown == src {
                    return;
                }
                shown.copy_from_slice(src);
                for (p, rgb) in px.iter_mut().zip(src.chunks_exact(3)) {
                    *p = G2t16::from_channels(G2t16::ONE, [lut[rgb[0] as usize], lut[rgb[1] as usize], lut[rgb[2] as usize]]);
                }
                lo.fetch_min(y, Ordering::Relaxed);
                hi.fetch_max(y + 1, Ordering::Relaxed);
            });
        let (lo, hi) = (lo.into_inner(), hi.into_inner());
        if lo >= hi {
            return false;
        }
        self.changed = Some(match self.changed {
            Some((a, b)) => (a.min(lo), b.max(hi)),
            None => (lo, hi),
        });
        true
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
        self.shown = vec![0u8; w * h * 3];
        // Black, as Lumis's zeroed frame is: a row Lumis never draws then reads the same in both.
        let black = G2t16::from_channels(G2t16::ONE, [self.lut[0]; 3]);
        self.pixels = vec![black; w * h];
        self.changed = Some((0, h));
        self.menu = Some(app::menu_new(width, height, DENSITY_DPI, camera_infos(&self.cameras)));
        self.menu_dirty = true;
        ctx.window.request_redraw();
    }

    fn on_event(&mut self, event: &Event, ctx: &mut Context) -> EventResponse {
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
                }
                Some(Opened::Failed(why)) => {
                    println!("lumis: back to the menu ({why})");
                    self.screen = Screen::Menu;
                    self.menu_dirty = true;
                }
                None => {}
            }
        }
        let Some(surface) = self.surface.as_ref() else { return false };
        match &mut self.screen {
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
                app::ui_frame(ui, surface, Buttons::default(), UPRIGHT, self.finger);
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
        let posted = surface.take_posted();
        posted && self.take_frame()
    }

    fn damage_rect(&mut self, viewport: fluor::geom::Viewport) -> Option<fluor::canvas::PixelRect> {
        let (y0, y1) = self.changed?;
        Some(fluor::canvas::PixelRect::new(0, y0, viewport.width_px as usize, y1))
    }

    fn render(&mut self, target: &mut [G2t16], _ctx: &mut Context) {
        // Lumis's frame is opaque everywhere, so the rows that changed are copied whole, nothing under them.
        let Some((y0, y1)) = self.changed.take() else { return };
        let w = self.width;
        target[y0 * w..y1 * w].copy_from_slice(&self.pixels[y0 * w..y1 * w]);
    }

    fn cursor_for(&self, _x: Coord, _y: Coord, _ctx: &Context) -> CursorIcon {
        CursorIcon::Default
    }

    fn opaque(&self) -> bool {
        true
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
        Ok(c) => {
            println!("lumis: {} cameras listed", c.len());
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
