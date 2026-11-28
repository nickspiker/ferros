//! The hit map (owner, 2026-11-11; fluor's model, `HitId = u16`: "I mean, I already built it in fluor so sure?"; then "the hitmap is never even touched if the pixel is opaque. early out!"): a u16 a pixel, the owner of what the glass shows there, the glass's own size (8 MB on husky).
//!
//! - **Written by the compose, front to back.** Each publish writes the rows it composes (`row`), once (the first phase of a publish that carries several), and each pixel once: a row starts as one free run of columns, and each layer, frontmost first, claims the runs it owns, writes its id there and takes them out of the row (`Free`), so nothing behind it is looked at there. What no layer claimed is the ground's.
//! - **The layers, front to back:**
//!   - the system's places (`SYS` + op 41's code), as the console's worker leaves them at its own compose step (`bars_row`, from `bar_decode`): outside the app's circle every pixel (at the top the doorway by the camera 10 and else the nearest console circle's zone: the radios 0-4, USB 5, the clock 13, the battery 14, with 11 where none reaches; back 9 on the left of the bottom and deck 8 on the right), and inside it the quick row's circles while the row stands open (32 + k). In full screen (`FULL`; owner, 2026-11-11, on the Pro: "helm should be functional but not visible in full screen, whilist console should be gone gone") only the helm's: back and deck stay the system's, undrawn (so deck's swipe over a full-screen app shows the controls again; WINDOWS.md "The helm"), and the console's rows are not read at all (`row`, one test a row), their pixels the app's;
//!   - behind the quick row while any of it shows on the glass (`SYS` + 12): everything inside the app's circle (never in full screen, where the row does not show);
//!   - with an app in front, its windows by the layer stack (`WINDOW` + surface): a window owns every pixel where it is not see-through, as fluor tags a widget by its shape; where no window is, nobody's;
//!   - on the deck: the open circle's page (`REGION`), the map's circles front first (`CIRCLE` + their index in the scene, the crowd's included), each its disc; the plane under them all (`PLANE`).
//!   - the helm's wedges are the system's, frontmost everywhere, on the deck as over an app: the compose draws the bars in front of the scene (`bars_front`), so the map claims them first in the same walk, and a circle lying under deck or back is behind the button on the glass and to a touch alike (owner, 2026-11-17: "when a kist is under the back or home button the back or home button do not work when we're on deck"; then "as long as the topmost item gets drawn first and under is honored than any filled pixel will skip the hitmap completely"). From 2026-11-12 to 2026-11-17 the deck claimed the wedges behind the map's circles (owner, 2026-11-12, on the 8a: a tap on the settings circle in the bottom right was taken as deck's), a walk in another order than the pixels': the circle took a tap on a button drawn in front of it.
//! - **Read by one lookup.** Op 27 ("is this point my window's") and op 41 ("what of the system's is here") read the pixel (`at`); the geometry they kept of their own (the windows' rectangles and clear bits, the console's circles, the quick row's, the doorway, the helm's halves) went with them; it lives on only where it also draws.
//! - **Seen on the Developer page** (owner: "a debug/devel checkbox for toggling on/off the hitmap on the fourth frame so we get three frames normal and fourth is always hitmap"; then on the Pro: "make sure that fourth frame gets updated completely so I can see the whole map proper with fresh colours per rotate"): the fourth temporal phase (`PHASE`) shows the map, a flat colour per owner, instead of the image, painted whole from the map each time it comes round to the layer, in a palette new to that showing (`on_layer`), so every owner is one flat colour across the glass and a change of ownership shows from one rotation to the next. The publishes never paint into it. Toggled by op 58, from the Developer page's switch alone since 2026-11-18 (owner: "screenshot is the two finger slide", the gesture that switched it until then; the page is in every development build and in a release build after the twelve taps).

use super::*;
use core::sync::atomic::AtomicU32;

/// Nothing that answers a touch: the black inside the app's circle where no window is.
pub const NOBODY: u16 = 0;
/// The deck's plane (the map's ground, the wallpaper).
pub const PLANE: u16 = 1;
/// The open circle's page (the map's region, COMPOSITOR.md).
pub const REGION: u16 = 2;
/// The system's places: op 41's code added (0-4 a radio's console circle and 5 USB, 13 the clock and 14 the battery (each its zone of the dash), 8 deck, 9 back, 10 the doorway, 11 the top's other ground, 12 behind the quick row, 32 + k a quick circle).
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

/// The bars' ids for bar row `b` (`bar_decode`, on the console's worker, as it composes that row: the console's ids at its own compose step), into the back buffer. Front to back as the bars draw: outside the app's circle at the top the dash's zones (every pixel the nearest centre's: a console circle's, or the camera's, the doorway), back and deck at the bottom; inside it the quick row's circles while it stands open as composed; nobody's elsewhere.
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
        // The helm's halves, whole (the arrow and the ring are drawn only there; nothing to aim at): deck right of the middle, back left of it.
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

/// Row `y`'s map over columns [xa, xb), front to back (the module's doc), from the layers as this publish composes them: `app` with an app in front (its windows), else the deck.
pub fn row(y: usize, xa: usize, xb: usize, width: usize, height: usize, app: bool) {
    let t0 = now();
    let out = unsafe { core::slice::from_raw_parts_mut((MAP as *mut u16).add(y * width), width) };
    let mut free = Free::new(slot_runs(), xa, xb);
    // Full screen (`FULL`; owner, 2026-11-11, on the Pro: "helm should be functional but not visible in full screen, whilist console should be gone gone"): the console's rows hold nothing of the system's, so the bars' ids are read on the helm's rows alone and the top falls to the windows, and nothing is behind the quick row, which does not show (`bar_line`). One test a row, never a pixel; every phase is published whole when `FULL` changes (op 43, `full_follow`), and with it every row of the map.
    let full = unsafe { FULL };
    let behind = unsafe { BEHIND } && !full;
    // The system's, frontmost: runs of the bars' ids (every pixel outside the app's circle is one of them, but the console's in full screen), claimed before any layer behind them, as the compose puts the bars in front of the scene and the windows (`bars_front`). The helm's wedges among them, on the deck as over an app (WINDOWS.md "The helm"): a circle under deck or back is behind the button.
    if let Some(b) = bar_row(y, height).filter(|&b| !full || b >= unsafe { BAR_TOP }) {
        let ids = unsafe { core::slice::from_raw_parts((BARS as *const u16).add(b * width), width) };
        claim_ids(&mut free, out, ids, xa, xb, |id| id != NOBODY);
    }
    if behind {
        free.rest(|a, z| out[a..z].fill(SYS + 12));
    } else if app {
        windows(y, &mut free, out, width);
    } else {
        deck(y, &mut free, out, width);
    }
    TICKS.fetch_add(now() - t0, Ordering::Relaxed);
    ROWS.fetch_add(1, Ordering::Relaxed);
}

/// The runs of `ids` over [xa, xb) whose ids pass `take`, claimed with those ids.
fn claim_ids(free: &mut Free, out: &mut [u16], ids: &[u16], xa: usize, xb: usize, take: impl Fn(u16) -> bool) {
    let mut x = xa;
    while x < xb {
        let t = take(ids[x]);
        let mut e = x + 1;
        while e < xb && take(ids[e]) == t {
            e += 1;
        }
        if t {
            free.claim(x, e, |a, z| out[a..z].copy_from_slice(&ids[a..z]));
        }
        x = e;
    }
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
                        let p = window_pixels(k as usize, width, height)[src_row(k as usize, y, x) * width + x];
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

/// The deck's share of a row, what the bars in front of it left free (`row`): a flight's frame is the map's plane throughout (its two images are the map's); else the open circle's page, then the map's circles front first, then the plane.
fn deck(y: usize, free: &mut Free, out: &mut [u16], width: usize) {
    if unsafe { (*(&raw const FLIGHT_FRAME)).is_none() } {
        if let Some((x, yc, r)) = unsafe { REGION_DISC } {
            if let Some((a, z)) = chord_q8(x, yc, r, y, width) {
                free.claim(a, z, |a, z| out[a..z].fill(REGION));
            }
        }
        let discs = unsafe { &(&*(&raw const DISCS))[..NDISCS] };
        let py = ((y as i64) << 8) + 128;
        for &(i, x, yc, r) in discs {
            // Every column already claimed (the bars, the circles in front): nothing behind is looked at, the early out.
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
    // Each app's own ids from its plane where its window owns a pixel (docs/HITMAP.md), so every widget shows in a colour of its own.
    let planes = planes();
    for y in s.y0 as usize..s.y1 as usize {
        let ids = unsafe { core::slice::from_raw_parts((MAP as *const u16).add(y * width), width) };
        let k0 = key(ids[0], &planes, y * width);
        let (mut last, mut c) = (k0, colour(k0, seed));
        for x in 0..width {
            let k = key(ids[x], &planes, y * width + x);
            if k != last {
                last = k;
                c = colour(k, seed);
            }
            row[x] = c;
        }
        dst[y * width..(y + 1) * width].copy_from_slice(&row[..width]);
    }
}

/// Whether the view is on: the Developer page's switch (owner, 2026-11-18: the hit map moves to a switch there), on every build since, the page being the gate (always there in a development build, after the twelve taps in a release build).
pub fn view() -> bool {
    VIEW.load(Ordering::Acquire)
}

/// Op 58 `hit view [what u8: 0 off, 1 on, 2 toggle, 3 ask]` → [0][on u8]: the receive thread's, answered at once; the render thread is woken to owe the phase its whole screen. Its caller is checked there: the system's, or anyone on a development build.
pub fn view_op(what: u8) -> [u8; 2] {
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
fn colour(id: u32, seed: u32) -> u32 {
    let mut h = (id ^ seed.rotate_left(16)).wrapping_mul(0x9E37_79B9);
    h ^= h >> 15;
    h = h.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13;
    let mut c = [h & 1023, h >> 10 & 1023, h >> 20 & 1023];
    c[[0, 1, 2, 1][(h >> 30) as usize]] = 1023;
    // Every code halved (owner, 2026-11-11: "the colours should be roughly 18% grey in value ... it's just too bright across the board"): the full channel lands at 511, about 22% of white's light on this panel's ~2.2 power law, near the 18% grey he asked for; one shift is the knob.
    pack10(c[0] >> 1, c[1] >> 1, c[2] >> 1)
}

// ---------------------------------------------------------------------------
// The apps' planes (docs/HITMAP.md): each app's own hit ids beside the display's, a u16 a pixel in screen coordinates.
// ---------------------------------------------------------------------------

/// A plane's run in the display's pool: a head (its first u32 the plane that goes with the frame shown, 0 or 1), then two planes of the panel's size, so the app writes one while the display reads the other.
pub const PLANE_HEAD: usize = 64;
/// Ids from the apps' planes, as the list (op 89) and the view name them: the display's own ids keep theirs, and an app's (window, id) pair is handed one from here up as first seen (`Table`).
pub const APP_IDS: u16 = 1 << 13;

/// Surface k's plane: its address (0 none), the holder generation it was given under (it counts only while that holder holds the surface), and the view id the app holds it by.
static PLANE_AT: [AtomicU64; SURFACES] = [const { AtomicU64::new(0) }; SURFACES];
static PLANE_GEN: [AtomicU64; SURFACES] = [const { AtomicU64::new(0) }; SURFACES];
static PLANE_ID: [AtomicU64; SURFACES] = [const { AtomicU64::new(0) }; SURFACES];
/// The display's pool (`ferros_sys::share`): its address and its size as a log, grown in place as surfaces ask.
static POOL_AT: AtomicU64 = AtomicU64::new(0);
static POOL_LOG: AtomicU32 = AtomicU32::new(0);

/// One plane's bytes: the panel's pixels, a u16 each, on a cache line.
fn plane_bytes(w: usize, h: usize) -> usize {
    (w * h * 2 + 63) & !63
}

/// The run a plane takes, as a log: the head and two planes, rounded up to a power of two (the pool gives runs aligned to their size). Proof of the size: it is the panel's, so it never binds anything the panel can show.
fn run_log(w: usize, h: usize) -> u32 {
    (PLANE_HEAD + 2 * plane_bytes(w, h)).next_power_of_two().trailing_zeros()
}

/// Surface k's plane given read-write to the caller of `tok`, the app that holds the surface (op 88): its run at k runs into the pool, the pool grown to hold it, cleared first so the app reads no ids of the surface's last holder. (view id, plane bytes) or the refusal byte.
pub fn plane_give(k: usize, tok: &[u8; 32], grants: u64) -> Result<(u64, usize), u8> {
    let (w, h) = unsafe { SCREEN_WH };
    let (w, h) = (w as usize, h as usize);
    let log = run_log(w, h);
    let off = (k as u64) << log;
    // The pool holds runs 0..=k: k + 1 runs, a power of two of them.
    let want = log + ((k + 1) as u64).next_power_of_two().trailing_zeros();
    if POOL_LOG.load(Ordering::Acquire) < want {
        // The pool's handle slot: the first pair past the display's grants (its count at the grants page's G#200), as ip's.
        let slot = unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + grants + 0x200) as *const u32) } as u64;
        let Some(a) = ferros_sys::share::pool(want, slot, None) else { return Err(4) };
        POOL_AT.store(a, Ordering::Release);
        POOL_LOG.store(want, Ordering::Release);
    }
    // The painters stop reading the old plane before it is cleared; an earlier holder's view is taken back (gone already when that holder ended).
    PLANE_AT[k].store(0, Ordering::Release);
    let old = PLANE_ID[k].swap(0, Ordering::AcqRel);
    if old != 0 {
        ferros_sys::share::take(old);
    }
    let at = POOL_AT.load(Ordering::Acquire) + off;
    // SAFETY: [at, at + 2^log) is run k of the display's own pool (grown above to hold it); no painter reads it while PLANE_AT[k] is 0.
    unsafe { core::ptr::write_bytes(at as *mut u8, 0, PLANE_HEAD + 2 * plane_bytes(w, h)) };
    let Some(id) = ferros_sys::share::give(tok, off, log) else { return Err(4) };
    PLANE_ID[k].store(id, Ordering::Release);
    PLANE_GEN[k].store(live_gen(k) as u64, Ordering::Release);
    PLANE_AT[k].store(at, Ordering::Release);
    Ok((id, plane_bytes(w, h)))
}

/// Surface k's plane shown now: the u16 ids of the frame on the glass, in screen coordinates. None without one, or when the surface has changed hands since it was given.
fn plane(k: usize) -> Option<*const u16> {
    let at = PLANE_AT[k].load(Ordering::Acquire);
    let g = live_gen(k) as u64;
    if at == 0 || g == 0 || PLANE_GEN[k].load(Ordering::Acquire) != g {
        return None;
    }
    let (w, h) = unsafe { SCREEN_WH };
    // SAFETY: the head's first word, which the app writes (0 or 1); any other value it might write picks a plane all the same.
    let shown = unsafe { core::ptr::read_volatile(at as *const u32) } as usize & 1;
    Some((at as usize + PLANE_HEAD + shown * plane_bytes(w as usize, h as usize)) as *const u16)
}

/// The key of what pixel (x, y) is, for the view and the list: an app's own id where its window owns the pixel and its plane names one there (bit 24, the surface, the id), else the display's id.
#[inline]
fn key(id: u16, planes: &[Option<*const u16>; SURFACES], i: usize) -> u32 {
    let k = id.wrapping_sub(WINDOW) as usize;
    if k < SURFACES {
        if let Some(p) = planes[k] {
            // SAFETY: i is a pixel of the screen (the callers walk its rows), and the plane is a screen of u16.
            let local = unsafe { core::ptr::read_volatile(p.add(i)) };
            if local != 0 {
                return 1 << 24 | (k as u32) << 16 | local as u32;
            }
        }
    }
    id as u32
}

/// Every surface's plane shown now (`plane`), looked up once a stripe or a list.
fn planes() -> [Option<*const u16>; SURFACES] {
    core::array::from_fn(plane)
}

/// The table of app ids (owner, 2026-11-27: "a u16 and just do our own bookkeeping ... a teency hash map with collision ness"): an app's (surface, its id, the holder generation) to the id the list names it by, from `APP_IDS` up, open addressing with linear probing, doubled when half full. Its entries go when their surface changes hands (`sweep`). The receive thread's alone.
struct Table {
    /// Each slot: 0 free, else bit 63, the generation (bits 32-39), the key (bits 0-23), the global id (bits 40-55).
    slots: alloc::vec::Vec<u64>,
    used: usize,
    next: u32,
}
static mut TABLE: Table = Table { slots: alloc::vec::Vec::new(), used: 0, next: APP_IDS as u32 };

impl Table {
    fn hash(key: u32) -> usize {
        (key.wrapping_mul(0x9E37_79B9) >> 7) as usize
    }

    /// The global id of `key` (bit 24 clear: the surface and the app's id) under holder generation `g`, handed out when first seen; None when every u16 past `APP_IDS` is in use (a refusal, said in the reply).
    fn id(&mut self, key: u32, g: u8) -> Option<u16> {
        if self.slots.is_empty() || 2 * (self.used + 1) > self.slots.len() {
            let n = (self.slots.len() * 2).max(1 << 8);
            let old = core::mem::replace(&mut self.slots, alloc::vec![0; n]);
            self.used = 0;
            for e in old.into_iter().filter(|&e| e != 0) {
                self.put(e);
            }
        }
        let mask = self.slots.len() - 1;
        let mut i = Self::hash(key) & mask;
        loop {
            let e = self.slots[i];
            if e == 0 {
                break;
            }
            if e as u32 & 0xFF_FFFF == key && (e >> 32) as u8 == g {
                return Some((e >> 40) as u16);
            }
            i = (i + 1) & mask;
        }
        // WHY: the global ids are a u16 (owner); the ones past APP_IDS are all handed out only when 57344 (window, id) pairs are on the glass at once.
        if self.next > u16::MAX as u32 {
            return None;
        }
        let gid = self.next as u16;
        self.next += 1;
        self.put(1 << 63 | (gid as u64) << 40 | (g as u64) << 32 | key as u64);
        Some(gid)
    }

    fn put(&mut self, e: u64) {
        let mask = self.slots.len() - 1;
        let mut i = Self::hash(e as u32 & 0xFF_FFFF) & mask;
        while self.slots[i] != 0 {
            i = (i + 1) & mask;
        }
        self.slots[i] = e;
        self.used += 1;
    }

    /// The entries of surfaces whose holder has changed since they were made: gone, the table rebuilt from the rest; with none left at all, the ids start again from `APP_IDS`.
    fn sweep(&mut self) {
        let keep: alloc::vec::Vec<u64> = self.slots.iter().copied().filter(|&e| e != 0 && (e >> 32) as u8 == live_gen(((e >> 16) & 0xFF) as usize) && live_gen(((e >> 16) & 0xFF) as usize) != 0).collect();
        self.slots = alloc::vec![0; self.slots.len().max(1 << 8)];
        self.used = 0;
        if keep.is_empty() {
            self.next = APP_IDS as u32;
        }
        for e in keep {
            self.put(e);
        }
    }
}

/// Op 89 `hits` (docs/HITMAP.md "Accessibility and tests"): everything on the glass that a touch can land on, one entry each: [0][count u32] then per entry 18 bytes, all little-endian: its id (u16: the display's own, or an app's from the table), its surface (u8, G#FF for the display's own), a 0 byte, the app's own id (u16, 0 for the display's), its bounding box (x0, y0, x1, y1, each u16, x1 and y1 past the last pixel), and a pixel inside it (x, y, u16 each: the middle one of its pixels in row order, so a tap there lands on it whatever its shape).
/// Two walks of the screen: the first counts each one's pixels and box, the second finds each one's middle pixel. The receive thread's, on demand: nothing runs while nobody asks.
pub fn list() -> alloc::vec::Vec<u8> {
    let (w, h) = unsafe { SCREEN_WH };
    let (w, h) = (w as usize, h as usize);
    let planes = planes();
    let table = unsafe { &mut *(&raw mut TABLE) };
    table.sweep();
    // (key, count, x0, y0, x1, y1, middle index, seen in the second walk, middle pixel)
    let mut seen: alloc::vec::Vec<(u32, u32, u16, u16, u16, u16, u32, u32, u32)> = alloc::vec::Vec::new();
    let mut index: alloc::collections::BTreeMap<u32, usize> = alloc::collections::BTreeMap::new();
    let map = unsafe { MAP } as *const u16;
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            // SAFETY: i is a pixel of the screen, the map its size.
            let k = key(unsafe { *map.add(i) }, &planes, i);
            if k == NOBODY as u32 {
                continue;
            }
            let at = *index.entry(k).or_insert_with(|| {
                seen.push((k, 0, x as u16, y as u16, x as u16, y as u16, 0, 0, 0));
                seen.len() - 1
            });
            let e = &mut seen[at];
            e.1 += 1;
            e.2 = e.2.min(x as u16);
            e.3 = e.3.min(y as u16);
            e.4 = e.4.max(x as u16 + 1);
            e.5 = e.5.max(y as u16 + 1);
        }
    }
    for e in seen.iter_mut() {
        e.6 = e.1 / 2;
    }
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let k = key(unsafe { *map.add(i) }, &planes, i);
            if let Some(&at) = index.get(&k) {
                let e = &mut seen[at];
                if e.7 == e.6 {
                    e.8 = i as u32;
                }
                e.7 += 1;
            }
        }
    }
    let mut out = alloc::vec![0u8, 0, 0, 0, 0];
    let mut n = 0u32;
    for e in &seen {
        let (gid, surface, local) = if e.0 >> 24 == 1 {
            let k = ((e.0 >> 16) & 0xFF) as usize;
            match table.id(e.0 & 0xFF_FFFF, live_gen(k)) {
                Some(g) => (g, k as u8, e.0 as u16),
                None => continue,
            }
        } else {
            (e.0 as u16, 0xFF, 0)
        };
        out.extend_from_slice(&gid.to_le_bytes());
        out.push(surface);
        out.push(0);
        out.extend_from_slice(&local.to_le_bytes());
        for v in [e.2, e.3, e.4, e.5, (e.8 as usize % w) as u16, (e.8 as usize / w) as u16] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        n += 1;
    }
    out[1..5].copy_from_slice(&n.to_le_bytes());
    out
}
