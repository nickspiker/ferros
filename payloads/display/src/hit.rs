//! The hit map (owner, 2026-11-11; fluor's model, `HitId = u16`: "I mean, I already built it in fluor so sure?"; then "the hitmap is never even touched if the pixel is opaque. early out!"): a u16 a pixel, the owner of what the glass shows there, the glass's own size (8 MB on husky).
//!
//! - **Written by the compose, front to back.** Each publish writes the rows it composes (`row`), once (the first phase of a publish that carries several), and each pixel once: a row starts as one free run of columns, and each layer, frontmost first, claims the runs it owns, writes its id there and takes them out of the row (`Free`), so nothing behind it is looked at there. What no layer claimed is the ground's.
//! - **The layers, front to back:**
//!   - the system's places (`SYS` + op 41's code), as the console's worker leaves them at its own compose step (`bars_row`, from `bar_decode`): outside the app's circle every pixel (at the top the doorway by the camera 10 and else the nearest console circle's zone: the radios 0-4, USB 5, the clock 13, the battery 14, with 11 where none reaches; back 9 on the left of the bottom and home 8 on the right), and inside it the quick row's circles while the row stands open (32 + k). In full screen (`FULL`; owner, 2026-11-11, on the Pro: "helm should be functional but not visible in full screen, whilist console should be gone gone") only the helm's: back and home stay the system's, undrawn (so home's swipe over a full-screen app shows the controls again; WINDOWS.md "The helm"), and the console's rows are not read at all (`row`, one test a row), their pixels the app's;
//!   - behind the quick row while any of it shows on the glass (`SYS` + 12): everything inside the app's circle (never in full screen, where the row does not show);
//!   - with an app in front, its windows by the layer stack (`WINDOW` + surface): a window owns every pixel where it is not see-through, as fluor tags a widget by its shape; where no window is, nobody's;
//!   - on the home screen: the open circle's page (`REGION`), the map's circles front first (`CIRCLE` + their index in the scene, the crowd's included), each its disc; the plane under them all (`PLANE`).
//! - **Read by one lookup.** Op 27 ("is this point my window's") and op 41 ("what of the system's is here") read the pixel (`at`); the geometry they kept of their own (the windows' rectangles and clear bits, the console's circles, the quick row's, the doorway, the helm's halves) went with them; it lives on only where it also draws.
//! - **Seen in development** (owner: "a debug/devel checkbox for toggling on/off the hitmap on the fourth frame so we get three frames normal and fourth is always hitmap"; then on the Pro: "make sure that fourth frame gets updated completely so I can see the whole map proper with fresh colours per rotate"): the fourth temporal phase (`PHASE`) shows the map, a flat colour per owner, instead of the image, painted whole from the map each time it comes round to the layer, in a palette new to that showing (`on_layer`), so every owner is one flat colour across the glass and a change of ownership shows from one rotation to the next. The publishes never paint into it. Toggled by op 58 (the Developer page's checkbox, and two fingers swiped up from back and home together); never in a release build.

use super::*;
use core::sync::atomic::AtomicU32;

/// Nothing that answers a touch: the black inside the app's circle where no window is.
pub const NOBODY: u16 = 0;
/// The home screen's plane (the map's ground, the wallpaper).
pub const PLANE: u16 = 1;
/// The open circle's page (the map's region, COMPOSITOR.md).
pub const REGION: u16 = 2;
/// The system's places: op 41's code added (0-4 a radio's console circle and 5 USB, 13 the clock and 14 the battery (each its zone of the dash), 8 home, 9 back, 10 the doorway, 11 the top's other ground, 12 behind the quick row, 32 + k a quick circle).
pub const SYS: u16 = 1 << 8;
/// An app's window: its surface added.
pub const WINDOW: u16 = 1 << 9;
/// A circle of the map's plane: its index in the scene added.
pub const CIRCLE: u16 = 1 << 12;

/// The map, a u16 a pixel, rows of the screen's width.
static mut MAP: usize = 1;
/// The bars' ids (`bars_row`), a u16 a pixel of the bars' rows (`BAR_TOP` then `CHROME_BOT`, as `BAR_W`): front and back, swapped with the bars' decoded rows at a pass's top, so a publish reads the ids of the bars it composes.
static mut BARS: usize = 1;
static mut BARS_BACK: usize = 1;
/// The map's circles on the glass this frame, front first: (index in the scene, x, y, r, Q8 pixels), as the camera op draws them (`scene`).
static mut DISCS: [(u16, i64, i64, i64); 256] = [(0, 0, 0, 0); 256];
static mut NDISCS: usize = 0;
/// The open circle's ground (x, y, r, Q8 pixels) while a region is up.
static mut REGION_DISC: Option<(i64, i64, i64)> = None;
/// Whether any of the quick row shows on the glass (the bars on the screen hold some of it): everything inside the app's circle is behind it.
static mut BEHIND: bool = false;

/// The phase that shows the map in development.
pub const PHASE: usize = PHASES - 1;
/// Whether it does (op 58), and each change of it counted, so the render thread owes that phase the whole screen.
pub static VIEW: AtomicBool = AtomicBool::new(false);
pub static VIEW_GEN: AtomicU64 = AtomicU64::new(0);
static mut VIEW_SEEN: u64 = 0;
/// The palette's seed: a new one for every redraw of the map (`fresh`).
static SEED: AtomicU32 = AtomicU32::new(0);
/// Counter ticks the publishes spent writing the map since the last stats line, summed over their threads, and the rows written.
pub static TICKS: AtomicU64 = AtomicU64::new(0);
pub static ROWS: AtomicU64 = AtomicU64::new(0);

/// The map and the bars' ids, made at start (before the heap's mark). Returns their bytes.
pub fn alloc(width: usize, height: usize) -> (usize, usize) {
    let bars = unsafe { BAR_TOP + CHROME_BOT } * width;
    unsafe {
        MAP = alloc::vec![NOBODY; width * height].leak().as_mut_ptr() as usize;
        BARS = alloc::vec![NOBODY; bars].leak().as_mut_ptr() as usize;
        BARS_BACK = alloc::vec![NOBODY; bars].leak().as_mut_ptr() as usize;
    }
    (width * height * 2, 2 * bars * 2)
}

/// Who owns screen point (x, y): op 27's and op 41's one read. A point off the glass is nobody's.
pub fn at(x: i64, y: i64) -> u16 {
    let (w, h) = unsafe { SCREEN_WH };
    // WHY the bounds: the point is a client's message.
    if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
        return NOBODY;
    }
    unsafe { *(MAP as *const u16).add(y as usize * w as usize + x as usize) }
}

/// A row's columns no layer has claimed yet, [from, to) runs left to right: the map's early out. A layer claims a run (`claim`), writes its id over what of it is still free, and takes that out; the ground takes what is left (`rest`). The runs live in storage the caller gives (`slot_runs` for a publishing thread), so starting a row costs nothing.
pub struct Free<'a> {
    runs: &'a mut [(u16, u16)],
    n: usize,
}
/// The most runs a publish's row is cut into: each claim splits at most one run in two, and a row sees at most 8 runs of the bars' ids (the two sides outside the app's circle, the six quick circles), the open circle's ground and the scene's 256 circles: 265 claims and the first run, under 512.
const FREE_MAX: usize = 1 << 9;
/// Each publishing thread's runs (`draw::thread_slot`: the workers, then the render thread), so no row clears 2 KiB to start.
static mut RUNS: [[(u16, u16); FREE_MAX]; PUB_WORKERS + 1] = [[(0, 0); FREE_MAX]; PUB_WORKERS + 1];

/// This publishing thread's runs.
fn slot_runs() -> &'static mut [(u16, u16)] {
    unsafe { &mut (*(&raw mut RUNS))[draw::thread_slot()] }
}

impl<'a> Free<'a> {
    pub fn new(runs: &'a mut [(u16, u16)], a: usize, b: usize) -> Free<'a> {
        let mut f = Free { runs, n: 0 };
        f.add(a, b);
        f
    }
    /// One more run, right of every run already free.
    pub fn add(&mut self, a: usize, b: usize) {
        if a < b {
            self.runs[self.n] = (a as u16, b as u16);
            self.n += 1;
        }
    }
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }
    /// Columns [a, b) claimed: `put(from, to)` for each part of them still free, which then is not.
    pub fn claim(&mut self, a: usize, b: usize, mut put: impl FnMut(usize, usize)) {
        if a >= b || self.n == 0 {
            return;
        }
        let (a16, b16) = (a as u16, b as u16);
        // The runs it meets: from the first that ends past a to the last that starts before b.
        let i = self.runs[..self.n].iter().position(|r| r.1 > a16).unwrap_or(self.n);
        let mut j = i;
        while j < self.n && self.runs[j].0 < b16 {
            // The run and the claim's overlap (interval arithmetic, not a bound).
            let (lo, hi) = (self.runs[j].0.max(a16), self.runs[j].1.min(b16));
            put(lo as usize, hi as usize);
            j += 1;
        }
        if i == j {
            return;
        }
        // What is left of them: the first's part left of a, the last's right of b.
        let (left, right) = ((self.runs[i].0 < a16).then_some((self.runs[i].0, a16)), (self.runs[j - 1].1 > b16).then_some((b16, self.runs[j - 1].1)));
        let keep = left.is_some() as usize + right.is_some() as usize;
        let n2 = self.n - (j - i) + keep;
        self.runs.copy_within(j..self.n, i + keep);
        let mut k = i;
        for r in [left, right].into_iter().flatten() {
            self.runs[k] = r;
            k += 1;
        }
        self.n = n2;
    }
    /// Everything still free, to `put`.
    pub fn rest(&mut self, mut put: impl FnMut(usize, usize)) {
        for r in &self.runs[..self.n] {
            put(r.0 as usize, r.1 as usize);
        }
        self.n = 0;
    }
}

/// A disc's columns on row `y` (pixel centres inside it), clipped to the screen: f32 pixels, as the console's circles and the quick row's are laid out.
fn chord(cx: f32, cy: f32, r: f32, y: usize, width: usize) -> Option<(usize, usize)> {
    let dy = y as f32 + 0.5 - cy;
    let h2 = r * r - dy * dy;
    if h2 < 0. {
        return None;
    }
    let half = libm::sqrtf(h2);
    let (lo, hi) = (libm::ceilf(cx - half - 0.5), libm::floorf(cx + half - 0.5) + 1.);
    // WHY the bounds: a disc may reach past the screen's sides (the quick row's outer circles touch the glass; a corner circle sits at the edge); the map holds only the screen's columns.
    let lo = if lo < 0. { 0 } else { lo as usize };
    let hi = if hi > width as f32 { width } else if hi < 0. { 0 } else { hi as usize };
    (lo < hi).then_some((lo, hi))
}

/// The same for a disc in Q8 pixels (the scene's circles): f64, as a circle zoomed far past the screen squares past what f32 keeps to a pixel.
fn chord_q8(cx: i64, cy: i64, r: i64, y: usize, width: usize) -> Option<(usize, usize)> {
    let dy = (((y as i64) << 8) + 128 - cy) as f64;
    let r = r as f64;
    let h2 = r * r - dy * dy;
    if h2 < 0. {
        return None;
    }
    let half = libm::sqrt(h2);
    let (lo, hi) = (libm::ceil((cx as f64 - half - 128.) / 256.), libm::floor((cx as f64 + half - 128.) / 256.) + 1.);
    // WHY the bounds: as `chord`'s, and a zoomed circle reaches far past either side.
    let lo = if lo < 0. { 0 } else if lo > width as f64 { width } else { lo as usize };
    let hi = if hi > width as f64 { width } else if hi < 0. { 0 } else { hi as usize };
    (lo < hi).then_some((lo, hi))
}

/// The bars' ids for bar row `b` (`bar_decode`, on the console's worker, as it composes that row: the console's ids at its own compose step), into the back buffer. Front to back as the bars draw: outside the app's circle at the top the dash's zones (every pixel the nearest centre's: a console circle's, or the camera's, the doorway), back and home at the bottom; inside it the quick row's circles while it stands open as composed; nobody's elsewhere.
pub fn bars_row(b: usize, width: usize, height: usize) {
    let (top, bot) = unsafe { (BAR_TOP, CHROME_BOT) };
    let y = if b < top { b } else { height - bot + (b - top) };
    let out = unsafe { core::slice::from_raw_parts_mut((BARS_BACK as *mut u16).add(b * width), width) };
    let (cx, cy, rr) = circle(width, height);
    let (il, ir) = chord(cx, cy, rr, y, width).unwrap_or((0, 0));
    // The runs on the stack, a few: inside the circle the six quick circles cut at most 7, outside it the doorway and the dash's zones (at most 16 a row) from its 2.
    let (mut ins, mut outs) = ([(0u16, 0u16); 1 << 3], [(0u16, 0u16); 1 << 5]);
    let mut inside = Free::new(&mut ins, il, ir);
    if let Some(row) = quick::open_circles(width, height) {
        for (k, &(x, yc, r)) in row.iter().enumerate() {
            if let Some((a, z)) = chord(x, yc, r, y, width) {
                inside.claim(a, z, |a, z| out[a..z].fill(SYS + 32 + k as u16));
            }
        }
    }
    inside.rest(|a, z| out[a..z].fill(NOBODY));
    let mut outside = Free::new(&mut outs, 0, il);
    outside.add(ir, width);
    if y as f32 + 0.5 > height as f32 * 0.5 {
        // The helm's halves, whole (the arrow and the ring are drawn only there; nothing to aim at): home right of the middle, back left of it.
        outside.claim((width + 1) / 2, width, |a, z| out[a..z].fill(SYS + 8));
        outside.rest(|a, z| out[a..z].fill(SYS + 9));
        return;
    }
    // The dash's zones: every pixel of the chrome rows is the nearest centre's, a console circle's or the camera's (the doorway, 10, from which alone the quick row opens), a drawn circle's own pixels its own first (main.rs `dash_row`; owner 2026-11-11: "that should be a point and a dist from BT and GPS"); 11 only where no zone reaches (none on these screens).
    for &(a, z, id) in dash_row(y, width, height) {
        outside.claim(a as usize, z as usize, |a, z| out[a..z].fill(SYS + id));
    }
    outside.rest(|a, z| out[a..z].fill(SYS + 11));
}

/// The bars' ids swapped with the bars' decoded rows (the pass's top, where no publish reads them), and whether the quick row shows on the glass now: when that changed, everything inside the app's circle changed hands, and the whole map is written again (`refresh`).
pub fn bars_swap(width: usize, height: usize) {
    unsafe {
        core::mem::swap(&mut *(&raw mut BARS), &mut *(&raw mut BARS_BACK));
        let behind = !quick::EMPTY_FRONT;
        if behind != BEHIND {
            BEHIND = behind;
            refresh(width, height, draw::Rect::full(width, height));
        }
    }
}

/// The map's circles this frame (the camera op, before it draws them), each (index, x, y, r) in Q8 pixels, front first, written into the list in place (`discs`) and then counted (`scene`, with the open circle's ground while a region is up).
pub fn discs() -> &'static mut [(u16, i64, i64, i64); 256] {
    unsafe { &mut *(&raw mut DISCS) }
}
pub fn scene(n: usize, region: Option<(i64, i64, i64)>) {
    unsafe {
        NDISCS = n;
        REGION_DISC = region;
    }
}

/// The region ended (`region_off`): its page owns nothing now.
pub fn region_gone() {
    unsafe { REGION_DISC = None };
}

/// Row `y`'s map over columns [xa, xb), front to back (the module's doc), from the layers as this publish composes them: `app` with an app in front (its windows), else the home screen.
pub fn row(y: usize, xa: usize, xb: usize, width: usize, height: usize, app: bool) {
    let t0 = now();
    let out = unsafe { core::slice::from_raw_parts_mut((MAP as *mut u16).add(y * width), width) };
    let mut free = Free::new(slot_runs(), xa, xb);
    // Full screen (`FULL`; owner, 2026-11-11, on the Pro: "helm should be functional but not visible in full screen, whilist console should be gone gone"): the console's rows hold nothing of the system's, so the bars' ids are read on the helm's rows alone and the top falls to the windows, and nothing is behind the quick row, which does not show (`bar_line`). One test a row, never a pixel; every phase is published whole when `FULL` changes (op 43, `full_follow`), and with it every row of the map.
    let full = unsafe { FULL };
    // The system's, frontmost: runs of the bars' ids (every pixel outside the app's circle is one of them, but the console's in full screen).
    if let Some(b) = bar_row(y, height).filter(|&b| !full || b >= unsafe { BAR_TOP }) {
        let ids = unsafe { core::slice::from_raw_parts((BARS as *const u16).add(b * width), width) };
        let mut x = xa;
        while x < xb {
            let sys = ids[x] != NOBODY;
            let mut e = x + 1;
            while e < xb && (ids[e] != NOBODY) == sys {
                e += 1;
            }
            if sys {
                free.claim(x, e, |a, z| out[a..z].copy_from_slice(&ids[a..z]));
            }
            x = e;
        }
    }
    if unsafe { BEHIND } && !full {
        free.rest(|a, z| out[a..z].fill(SYS + 12));
    } else if app {
        windows(y, &mut free, out, width);
    } else {
        home(y, &mut free, out, width);
    }
    TICKS.fetch_add(now() - t0, Ordering::Relaxed);
    ROWS.fetch_add(1, Ordering::Relaxed);
}

/// The windows' share of a row: the layer stack's spans (`layout_build`), each chain frontmost first; a pixel is the first window's that is not see-through there.
fn windows(y: usize, free: &mut Free, out: &mut [u16], width: usize) {
    let bands = unsafe { core::slice::from_raw_parts(&raw const BANDS as *const Band, NBANDS) };
    let order = unsafe { *(&raw const ORDER) };
    let height = unsafe { SCREEN_WH.1 } as usize;
    if let Some(band) = bands.iter().find(|b| (y as u16) >= b.y0 && (y as u16) < b.y1) {
        for sp in band.spans() {
            let chain = sp.chain();
            free.claim(sp.x0 as usize, sp.x1 as usize, |a, z| {
                let Some(&first) = chain.first() else {
                    out[a..z].fill(NOBODY);
                    return;
                };
                let (k0, f0, m0) = order[first as usize];
                if m0 & OPAQUE != 0 && f0 != G2T16 {
                    out[a..z].fill(WINDOW + k0 as u16);
                    return;
                }
                for x in a..z {
                    out[x] = NOBODY;
                    for &i in chain {
                        let (k, f, _) = order[i as usize];
                        let p = surface(k as usize, width, height)[y * width + x];
                        // Not see-through: a G2T16 pixel with any α, a working one without bit 63 (`working`'s own test).
                        if if f == G2T16 { p >> 48 != 0 } else { p >> 63 == 0 } {
                            out[x] = WINDOW + k as u16;
                            break;
                        }
                    }
                }
            });
        }
    }
    free.rest(|a, z| out[a..z].fill(NOBODY));
}

/// The home screen's share of a row: a flight's frame is the map's plane throughout (its two images are the map's); else the open circle's page, then the map's circles front first, then the plane.
fn home(y: usize, free: &mut Free, out: &mut [u16], width: usize) {
    if unsafe { (*(&raw const FLIGHT_FRAME)).is_none() } {
        if let Some((x, yc, r)) = unsafe { REGION_DISC } {
            if let Some((a, z)) = chord_q8(x, yc, r, y, width) {
                free.claim(a, z, |a, z| out[a..z].fill(REGION));
            }
        }
        let discs = unsafe { &(&*(&raw const DISCS))[..NDISCS] };
        let py = ((y as i64) << 8) + 128;
        for &(i, x, yc, r) in discs {
            if free.is_empty() {
                break;
            }
            if (py - yc).abs() > r {
                continue;
            }
            if let Some((a, z)) = chord_q8(x, yc, r, y, width) {
                free.claim(a, z, |a, z| out[a..z].fill(CIRCLE + i));
            }
        }
    }
    free.rest(|a, z| out[a..z].fill(PLANE));
}

/// The map written again over `r` without composing anything (the quick row came or went: everything inside the app's circle changed hands), by stripes across the publish workers as a publish is.
pub fn refresh(width: usize, height: usize, r: draw::Rect) {
    let r = r.clip(width, height);
    if r.is_empty() {
        return;
    }
    stripes(2, 0, width, height, r);
}

/// One stripe of a refresh (`pub_take`, kind 2).
pub fn stripe(width: usize, height: usize, s: draw::Rect) {
    let app = unsafe { (*(&raw const APP)).is_some() };
    for y in s.y0 as usize..s.y1 as usize {
        row(y, s.x0 as usize, s.x1 as usize, width, height, app);
    }
}

/// The phase the layer showed last (`on_layer`): the view's frame is painted as its phase comes round, once a showing.
static mut LAST_ON_LAYER: usize = usize::MAX;

/// The layer is about to show phase `k` (`layer_fb`, every present and flip). The view's phase coming round, from another phase, is painted whole from the map in a new palette (owner, 2026-11-11, on the Pro: "Can we draw the map with the same ID and make sure that fourth frame gets updated completely so I can see the whole map proper with fresh colours per rotate?"). Until then each publish painted only its own rectangle, each in a palette of its own, so the same owner showed in a different colour inside every rectangle a publish had touched: the clock's, the console's, the helm's (the "blocky areas around updates" and the bottom-left rectangle the owner saw).
pub fn on_layer(k: usize) {
    let was = unsafe { core::mem::replace(&mut *(&raw mut LAST_ON_LAYER), k) };
    if k != PHASE || was == PHASE || !view() {
        return;
    }
    let t0 = now();
    SEED.fetch_add(1, Ordering::Relaxed);
    let (w, h) = unsafe { SCREEN_WH };
    stripes(3, 0, w as usize, h as usize, draw::Rect::full(w as usize, h as usize));
    PAINT_US.fetch_add(ferros_sys::us_since(t0), Ordering::Relaxed);
    PAINTS.fetch_add(1, Ordering::Relaxed);
}

/// The view's frames painted since the last stats line, and their µs.
pub static PAINTS: AtomicU64 = AtomicU64::new(0);
pub static PAINT_US: AtomicU64 = AtomicU64::new(0);

/// One stripe of the view's frame (`pub_take`, kind 3): each row from the map, a colour worked out once a run of one owner, then one wide copy into the phase (non-cacheable: written, never read).
pub fn paint_stripe(width: usize, s: draw::Rect) {
    let seed = SEED.load(Ordering::Relaxed);
    let dst = phase_slice(PHASE, width * unsafe { SCREEN_WH.1 } as usize);
    // The glass's widest row (husky's 1344) fits; the publish's own rows are the same size.
    let mut row = [0u32; 1344];
    for y in s.y0 as usize..s.y1 as usize {
        let ids = unsafe { core::slice::from_raw_parts((MAP as *const u16).add(y * width), width) };
        let (mut last, mut c) = (ids[0], colour(ids[0], seed));
        for x in 0..width {
            if ids[x] != last {
                last = ids[x];
                c = colour(last, seed);
            }
            row[x] = c;
        }
        dst[y * width..(y + 1) * width].copy_from_slice(&row[..width]);
    }
}

/// Whether the development view is on: never in a release build.
pub fn view() -> bool {
    DEV && VIEW.load(Ordering::Acquire)
}

/// Op 58 `hit view [what u8: 0 off, 1 on, 2 toggle, 3 ask]` → [0][on u8] (status 1 on a release build, which has no view): the receive thread's, answered at once; the render thread is woken to owe the phase its whole screen.
pub fn view_op(what: u8) -> [u8; 2] {
    if !DEV {
        return [1, 0];
    }
    let was = VIEW.load(Ordering::Acquire);
    let now = match what {
        0 => false,
        1 => true,
        2 => !was,
        _ => was,
    };
    if now != was {
        VIEW.store(now, Ordering::Release);
        VIEW_GEN.fetch_add(1, Ordering::AcqRel);
        ferros_sys::wake(&RING_TAIL, 1);
    }
    [0, now as u8]
}

/// At a pass's top: the view switched since the last pass, so its phase owes the whole screen (on: a publish into it writes every row of the map afresh before the view first comes round, `on_layer` painting it; off: the image back), and every block's compose-once bit is forgotten (the phase's blocks held the map, not the background).
pub fn view_step(width: usize, height: usize) {
    let g = VIEW_GEN.load(Ordering::Acquire);
    unsafe {
        if g != VIEW_SEEN {
            VIEW_SEEN = g;
            PHASE_DIRTY[PHASE] = draw::Rect::full(width, height);
            clean_reset_all();
            let mut l = Log { grants: PANIC_GRANTS, buf: [0; 256], n: 0 };
            l.s(if VIEW.load(Ordering::Acquire) { b"display: hit map shown in the fourth phase\n" } else { b"display: hit map hidden\n" });
            l.flush();
        }
    }
}

/// The bars' ids that row `b` changes, front (on the glass) against back (just decoded): the columns from the first differing id to the last, for the worker's changed rectangles (`CON_RECT`), so an owner that changes with no pixel changing is published, and so written into the map, all the same.
pub fn bars_changed(b: usize, width: usize) -> Option<(usize, usize)> {
    let (f, k) = unsafe { (core::slice::from_raw_parts((BARS as *const u16).add(b * width), width), core::slice::from_raw_parts((BARS_BACK as *const u16).add(b * width), width)) };
    let x0 = (0..width).find(|&x| f[x] != k[x])?;
    let x1 = (0..width).rev().find(|&x| f[x] != k[x]).map_or(x0, |x| x) + 1;
    Some((x0, x1))
}

/// An owner's colour in a palette: a hash of the id and the seed, one channel full so no owner reads as black, the others anywhere (10-bit codes, straight to the panel: a development view, not light).
fn colour(id: u16, seed: u32) -> u32 {
    let mut h = (id as u32 ^ seed.rotate_left(16)).wrapping_mul(0x9E37_79B9);
    h ^= h >> 15;
    h = h.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13;
    let mut c = [h & 1023, h >> 10 & 1023, h >> 20 & 1023];
    c[[0, 1, 2, 1][(h >> 30) as usize]] = 1023;
    // Every code halved (owner, 2026-11-11: "the colours should be roughly 18% grey in value ... it's just too bright across the board"): the full channel lands at 511, about 22% of white's light on this panel's ~2.2 power law, near the 18% grey he asked for; one shift is the knob.
    pack10(c[0] >> 1, c[1] >> 1, c[2] >> 1)
}
