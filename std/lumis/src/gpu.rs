//! Lumis's frames drawn on the GPU (the owner's GPU plan, 2026-11-20: "you render every px all at once thru the gpu when anything within that damage rectangle changes, that's it. Straight shot."; 2026-11-21: the GPU composes full frames): every pixel of a frame a job, straight into one of two surfaces of Lumis's own that fluor's host shows in turn (`FluorApp::own_surface`, a layer's mailbox). The menu is lumis_gpu's `menu_kernel` from its parts (the splash, the slots, the ink lumis_core draws as `ui::draw::set_ink` has it); the camera screen is Lumis's own frame (lumis_core's PixelSurface, drawn here in the block) thru `frame_kernel`, in place of the CPU's present.
//!
//! Lumis's part of the GPU block is its first 2^`UI_LOG` bytes: the queue, the kernels' words, the code table, the slots, the rows, the coverage, the splash, the ink and the camera screen's frame, each on its own place below; the recorder's kernel and frames after it (ferros_clip `gpu::set_base`). The block is grown to that before the queue opens, the two surfaces asked for before it too: a client's memory is its key to the GPU service (ferros_sys `gpu::Stamp::key`), so a block grown later (the recorder's) or a surface asked later is another client, and the queue is opened again for it (`Gpu::run`).

use ferros_sys::gpu::queue::{Job, Queue, E_CAMERA, S_DONE, S_LOST};
use ferros_sys::gpu::{ARENA_VA, PRELOAD_REGS64, PRELOAD_X, SURFACES_PA, SURFACES_VA};
use lumis_gpu::{frame, menu, stack, Compiled};

/// Lumis's part of the GPU block, 2^26 bytes (64 MiB): the Pro's menu's parts take 29 MiB of it, the camera screen's frame 12 more.
pub const UI_LOG: u32 = 26;
/// The queue at the block's start, 2^1 jobs deep (a frame's job at a time, and the next).
const QUEUE_AT: usize = 0;
const DEPTH_LOG2: u32 = 1;
/// The kernels' words (kernel k at CODE_AT + k 2^CODE_SLOT_LOG: the menu's, the frame's, the stack written and added), the code table, the slots' records (2^10 - 2^6 bytes after the table: two drawings of NUM_SLOTS rows and the empty one fit many times over), each pixel row's slot.
const CODE_AT: usize = 1 << 16;
const CODE_SLOT_LOG: u32 = 13;
/// The camera's integrator's shared memory (lumis_core `platform::set_shared_place`: its header, its image slots, the slitscan ring) from 2^28, the block grown to 2^29 for it: past the recorder's part (from 2^UI_LOG, the main's 137 MB), so a RAW clip and the stack never meet. The GPU stacks each frame into the plane in progress there (`Gpu::stack`).
pub const INTEGRATOR_AT: usize = 1 << 28;
const CAMERA_LOG: u32 = 29;
const LUT_AT: usize = 1 << 17;
const SLOTS_AT: usize = LUT_AT + (1 << 10);
const SLOTS_MOST: usize = ((1 << 18) - SLOTS_AT) / (menu::RECORD_WORDS * 4);
const ROWS_AT: usize = 1 << 18;
/// The coverage from 2^20, then the splash, the ink and the camera screen's frame, each on a MiB.
const COVER_AT: usize = 1 << 20;

/// Lumis holds the GPU's grants but no GPU service answers (`Gpu::open`): RAW video does not start (the recorder's GPU runs would wait for the service forever), and says why.
pub static NO_SERVICE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

fn mib(n: usize) -> usize {
    n.div_ceil(1 << 20) << 20
}

/// Which frame a job draws, for its line on the console.
#[derive(Clone, Copy)]
enum Kind {
    Menu = 0,
    Camera = 1,
    Stack = 2,
    StackAdd = 3,
}

/// Lumis's GPU: the queue on its part of the block, the two kernels, the two surfaces of Lumis's layer.
pub struct Gpu {
    service: u64,
    block: u64,
    /// The block's CPU address and its size when the queue was opened (a size since is another client).
    base: *mut u8,
    size: u64,
    q: Queue,
    /// Each kernel's handle in the queue and its recording: the menu's, the frame's, the stack written, the stack added.
    kernels: [u8; 4],
    compiled: [Compiled; 4],
    /// The two surfaces, each its base as Lumis's own `dma` of it (what the show names), and the one the next frame goes into.
    surfaces: [u64; 2],
    next: usize,
    /// The frame's size and the parts' places in the block.
    w: usize,
    h: usize,
    bg_at: usize,
    ink_at: usize,
    frame_at: usize,
    /// The splash is in the block (written once a menu).
    bg_written: bool,
    /// The slots as lumis_core gave them (`MainMenu::ink_frame`).
    slots: Vec<lumis_core::ui::draw::Slot>,
    /// The GPU's time of each kind's frames since its last line, counter ticks, and how many.
    ticks: [u64; 4],
    frames: [u64; 4],
    /// The camera screen's damage the pass before drew (rects x0, y0, x1, y1), which the next surface lacks: None, the whole frame (a menu frame, the first pass).
    drawn: Option<Vec<(usize, usize, usize, usize)>>,
}

// SAFETY: the block is this process's for its life (the kernel never takes a GPU block back); only the UI thread draws into it.
unsafe impl Send for Gpu {}

impl Gpu {
    /// The GPU, when Lumis holds its grants (ferros_clip `ferros::gpu_grants`: a kist's GPU slots, a spawn's --use gpu --dma layers), for a `w` x `h` frame: the block grown to Lumis's part, the surfaces asked for, the queue opened, the kernels recorded and given to it, the code table written. Err: why not, for the console.
    pub fn open(surface: u64, w: usize, h: usize, lut: &[u32; 256]) -> Result<Gpu, String> {
        let (service, block) = ferros_clip::ferros::gpu_grants().ok_or("no GPU grants")?;
        let (service, block) = (crate::sys::cap(service), crate::sys::cap(block));
        // The GPU service running at all, asked live first: the queue's calls (and the recorder's runs) wait for a server, and after a jump from Android ferros starts none, so Lumis sat at its open (the owner, 2026-11-22). A live call fails at once only when the service's process is gone (a busy one is waited for as ever); op 5, the service told this process's memory now, does nothing else before a queue.
        let mut r = [1u8; 2];
        if ferros_sys::call_live(service, &[5, 1], &mut r) == ferros_sys::ERR {
            NO_SERVICE.store(true, core::sync::atomic::Ordering::Relaxed);
            return Err("no GPU service running (ferros starts none after a jump from Android): the menu and the camera screen on the CPU, and no RAW video (its coder is the GPU's)".into());
        }
        let bg_at = COVER_AT + mib(w * h);
        let ink_at = bg_at + mib(w * h * 3);
        let frame_at = ink_at + mib(w * h * 3);
        if frame_at + mib(w * h * 3) > 1 << UI_LOG {
            return Err(format!("a A#{w} x A#{h} frame's parts pass Lumis's 2^A#{UI_LOG} bytes of the block"));
        }
        // The recorder's part after Lumis's: its kernel and frames from 2^UI_LOG.
        ferros_clip::gpu::set_base(1 << UI_LOG);
        let size = ferros_sys::gpu_block_ask(block, UI_LOG).ok_or("the GPU block did not grow")?;
        let mut t = [[0u8; 32]; 2];
        let mut surfaces = [0u64; 2];
        for k in 0..2 {
            surfaces[k] = ferros_sys::surface_ask(surface, &mut t[k]).ok_or("no surface of Lumis's own (at most four a process)")?;
        }
        let base = ferros_sys::dma(block) as *mut u8;
        let compiled = [lumis_gpu::menu_kernel(), lumis_gpu::frame_kernel(), lumis_gpu::stack_kernel(false), lumis_gpu::stack_kernel(true)];
        let (q, kernels) = Self::queue(service, block, base, &compiled)?;
        // SAFETY: the table's 2^10 bytes at LUT_AT, under Lumis's 2^UI_LOG of the block.
        unsafe { core::ptr::copy_nonoverlapping(lut.as_ptr() as *const u8, base.add(LUT_AT), 1 << 10) };
        *PLACE.lock().unwrap() = Some((block, base as usize));
        Ok(Gpu { service, block, base, size, q, kernels, compiled, surfaces, next: 0, w, h, bg_at, ink_at, frame_at, bg_written: false, slots: Vec::new(), ticks: [0; 4], frames: [0; 4], drawn: None })
    }

    /// The queue opened on the block as it is now and the kernels' words given to it.
    fn queue(service: u64, block: u64, base: *mut u8, compiled: &[Compiled; 4]) -> Result<(Queue, [u8; 4]), String> {
        let q = Queue::open(service, block, QUEUE_AT, DEPTH_LOG2).map_err(|s| format!("the GPU queue did not open (status G#{s:X})"))?;
        let mut kernels = [0u8; 4];
        for (k, c) in compiled.iter().enumerate() {
            let at = CODE_AT + (k << CODE_SLOT_LOG);
            // A kernel of a few hundred words (8 bytes each) in its 2^CODE_SLOT_LOG bytes.
            assert!(c.words.len() * 8 <= 1 << CODE_SLOT_LOG, "a kernel's words pass their room");
            // SAFETY: the words at their place, under LUT_AT.
            unsafe { core::ptr::copy_nonoverlapping(c.words.as_ptr() as *const u8, base.add(at), c.words.len() * 8) };
            let preload = PRELOAD_X | if c.regs64 { PRELOAD_REGS64 } else { 0 };
            kernels[k] = q.kernel(at, c.words.len(), preload).ok_or("the GPU service took no kernel")?;
        }
        Ok((q, kernels))
    }

    /// Whether the parts were laid out for a `w` x `h` frame.
    pub fn fits(&self, w: usize, h: usize) -> bool {
        (self.w, self.h) == (w, h)
    }

    /// Where Lumis's own frame lies in the block (`w` x `h` x 3 bytes): its PixelSurface's (lumis_core `PixelSurface::at`), which `camera` draws from.
    pub fn frame_bits(&self) -> *mut u8 {
        // SAFETY: frame_at is inside Lumis's part of the block (`open` checked).
        unsafe { self.base.add(self.frame_at) }
    }

    /// A new menu: its splash written again at the next frame.
    pub fn new_menu(&mut self) {
        self.bg_written = false;
    }

    /// The menu's parts now into the block (`fresh`: it changed since the last frame) and a frame of it drawn whole on the GPU into the next surface: that surface's base, for the show. Err: why not (the frame is then the CPU's).
    pub fn menu(&mut self, m: &mut lumis_core::ui_main_menu::MainMenu, fresh: bool) -> Result<u64, String> {
        let (w, h) = (self.w, self.h);
        if !self.bg_written {
            // SAFETY: the splash's w h 3 bytes at bg_at, its MiBs under ink_at.
            unsafe { core::ptr::copy_nonoverlapping(m.background().as_ptr(), self.base.add(self.bg_at), w * h * 3) };
            self.bg_written = true;
        }
        if fresh {
            // SAFETY: each part at its own place in Lumis's part of the block (`open` checked they fit), none overlapping another.
            let (ink, cover, rows) = unsafe { (core::slice::from_raw_parts_mut(self.base.add(self.ink_at), w * h * 3), core::slice::from_raw_parts_mut(self.base.add(COVER_AT), w * h), core::slice::from_raw_parts_mut(self.base.add(ROWS_AT) as *mut u32, h)) };
            m.ink_frame(ink, cover, rows, &mut self.slots);
            assert!(self.slots.len() < SLOTS_MOST, "the menu's slots pass their room in the block");
            // SAFETY: the records under ROWS_AT (SLOTS_MOST), record 0 the empty one.
            let records = unsafe { core::slice::from_raw_parts_mut(self.base.add(SLOTS_AT) as *mut u32, (self.slots.len() + 1) * menu::RECORD_WORDS) };
            records[..menu::RECORD_WORDS].fill(0);
            for (k, s) in self.slots.iter().enumerate() {
                let r = menu::record(s.x0 as u32, s.y0 as u32, s.x1 as u32, s.y1 as u32, s.big, s.glow, s.ch.map(|c| (c.k, c.f, c.colour)), s.fa);
                records[(k + 1) * menu::RECORD_WORDS..(k + 2) * menu::RECORD_WORDS].copy_from_slice(&r);
            }
        }
        let at = |o: usize| ARENA_VA + o as u64;
        let mut u = [0u64; menu::SLOT_COUNT as usize];
        u[menu::BG as usize] = at(self.bg_at);
        u[menu::INK as usize] = at(self.ink_at);
        u[menu::COVER as usize] = at(COVER_AT);
        u[menu::ROWS as usize] = at(ROWS_AT);
        u[menu::SLOTS as usize] = at(SLOTS_AT);
        u[menu::LUT as usize] = at(LUT_AT);
        u[menu::SIZE as usize] = w as u64 | ((w * h) as u64) << 32;
        u[menu::STRIDE as usize] = w as u64;
        // The menu fills both buffers in turn: the camera screen after it draws them whole.
        self.drawn = None;
        self.run(Kind::Menu, &mut u, (w * h) as u32)
    }

    /// The camera screen's frame as Lumis drew it (`frame_bits`) into the next surface on the GPU: `damage`, the rects the frame changed this pass (None: the whole frame), and the rects the pass before changed (that surface last showed the frame before it: the two buffers take turns); the rest of it is already this frame. Each rect its own job, not one box around them all (the Pro, 2026-11-22: the counters at the top and the exposure's progress at the bottom tick every frame, and the box around them was most of the screen, 2 to 4 ms of the GPU's every pass beside the camera's stack; capture before display, the owner's rule). A pixel in two of them (this pass's and the last's) is drawn twice, the same both times. Columns to whole quads of four (the frame kernel's). The surface's base, for the show.
    pub fn camera(&mut self, damage: Option<&[(usize, usize, usize, usize)]>) -> Result<u64, String> {
        let whole = [(0, 0, self.w, self.h)];
        let now = damage.unwrap_or(&whole).to_vec();
        let rects: Vec<_> = match self.drawn.take() {
            Some(before) => now.iter().chain(&before).map(|&(x0, y0, x1, y1)| (x0 & !3, y0, (x1 + 3) & !3, y1)).filter(|r| r.2 > r.0 && r.3 > r.1).collect(),
            None => whole.to_vec(),
        };
        self.drawn = Some(now);
        let k = self.next;
        for (j, &(x0, y0, x1, y1)) in rects.iter().enumerate() {
            let quads = (x1 - x0) / 4 * (y1 - y0);
            let mut u = [0u64; frame::SLOT_COUNT as usize];
            u[frame::DST as usize] = SURFACES_VA + (self.surfaces[k] - SURFACES_PA);
            u[frame::SRC as usize] = ARENA_VA + self.frame_at as u64;
            u[frame::LUT as usize] = ARENA_VA + LUT_AT as u64;
            u[frame::SIZE as usize] = ((x1 - x0) / 4) as u64 | (quads as u64) << 32;
            u[frame::ORIGIN as usize] = x0 as u64 | (y0 as u64) << 32;
            u[frame::STRIDE as usize] = self.w as u64 | (self.w as u64) << 32;
            self.exec(Kind::Camera, &u, quads as u32, j + 1 == rects.len())?;
        }
        self.next ^= 1;
        Ok(self.surfaces[k])
    }

    /// A job of `kind`'s kernel with uniforms `u` (its DST, slot 0 in both, set here to the next surface) over `invocations`, waited for: the surface it drew.
    fn run(&mut self, kind: Kind, u: &mut [u64], invocations: u32) -> Result<u64, String> {
        let k = self.next;
        u[0] = SURFACES_VA + (self.surfaces[k] - SURFACES_PA);
        self.exec(kind, u, invocations, true)?;
        self.next ^= 1;
        Ok(self.surfaces[k])
    }

    /// The camera's view mapped in this process's GPU space now (the GPU service's op 5): after each OPEN, at `CAMERA_VA` (a frame at its place in the view, the MAP header's +48); after a CLOSE, gone. None: the service did not answer.
    pub fn camera_mapped(&self) -> Option<bool> {
        self.q.refresh()
    }

    /// A camera frame stacked into the integrator's plane on the GPU (lumis_gpu `stack_kernel`): its RAW10 at `src` (`CAMERA_VA` plus its place in the view), `line` bytes a row, `width` x `height` samples, into the plane at `plane` bytes into the block (`INTEGRATOR_AT` on), written or added (`add`). Waited for.
    pub fn stack(&mut self, src: u64, line: usize, width: usize, height: usize, plane: usize, add: bool) -> Result<(), String> {
        let chunks = width / stack::CHUNK * height;
        let mut u = [0u64; stack::SLOT_COUNT as usize];
        u[stack::SRC as usize] = src;
        u[stack::PLANE as usize] = ARENA_VA + plane as u64;
        u[stack::SIZE as usize] = (width / stack::CHUNK) as u64 | (chunks as u64) << 32;
        u[stack::LINES as usize] = (line / 4) as u64 | (width as u64) << 32;
        self.exec(if add { Kind::StackAdd } else { Kind::Stack }, &u, chunks as u32, true)
    }

    /// A job of `kind`'s kernel with uniforms `u` over `invocations`, waited for; `ends_frame`: the last of its frame's jobs (the GPU time of all of them counted as one frame's).
    fn exec(&mut self, kind: Kind, u: &[u64], invocations: u32, ends_frame: bool) -> Result<(), String> {
        // The block grew since (the recorder's part for a clip, the integrator's for a camera): another client to the service, the queue opened again for it.
        let size = ferros_sys::dma_size(self.block, ferros_sys::GPU_BLOCK_MOST as u64).unwrap_or(0);
        if size != self.size {
            println!("lumis: the GPU block is G#{size:X} bytes now (G#{:X} at the queue's open): the queue opened again", self.size);
            (self.q, self.kernels) = Self::queue(self.service, self.block, self.base, &self.compiled)?;
            self.size = size;
        }
        let i = kind as usize;
        let mut uniforms = u.to_vec();
        uniforms.extend_from_slice(&self.compiled[i].consts);
        let groups = (invocations.div_ceil(1 << 4), 1);
        let n = self.q.submit(&Job { kernel: self.kernels[i], groups, uniforms: &uniforms }).ok_or("the GPU queue took no job")?;
        let mut e = self.q.wait(n);
        // A menu or camera screen job (neither reads the camera) lost because the camera's view went while a job was in flight (E_CAMERA: the service ends the queue at a CLOSE or a view given anew) did nothing wrong: the queue opened again and the job run once more, the GPU kept (the Pro, 2026-11-22: the camera screen's job lost at the tele's CLOSE for Slitscan turned Lumis's screen over to the CPU for the rest of the session). `close_camera` holds the GPU across its CLOSE so none is in flight; this is for any other way the view goes.
        if e.status == S_LOST && self.q.ended() == E_CAMERA && matches!(kind, Kind::Menu | Kind::Camera) {
            self.q.reopen();
            let n = self.q.submit(&Job { kernel: self.kernels[i], groups, uniforms: &uniforms }).ok_or("the GPU queue took no job")?;
            e = self.q.wait(n);
        }
        let name = ["the menu", "the camera screen", "the stack (a frame written)", "the stack (a frame added)"][i];
        if e.status != S_DONE {
            // The queue ended with it (a fault ends a queue): opened again for the next frame.
            let why = format!("{name}'s GPU job ended with status A#{} (the GPU address G#{:X}, fault G#{:X})", e.status, e.fault_address, e.fault_status);
            self.q.reopen();
            return Err(why);
        }
        self.ticks[i] += e.gpu_ticks().unwrap_or(0);
        self.frames[i] += ends_frame as u64;
        if self.frames[i] == 1 << 6 {
            println!("lumis: {name} on the GPU: {} us a frame on the GPU, the last {} frames", self.ticks[i] * 1_000_000 / (self.frames[i] * ferros_sys::freq()), self.frames[i]);
            (self.ticks[i], self.frames[i]) = (0, 0);
        }
        Ok(())
    }
}

/// Lumis's GPU, shared by the UI thread (the menu, the camera screen's frame) and the camera thread (the stack, the camera's view): None until `Gpu::open` (on the UI thread, with the first frame), and when there is none.
pub static SHARED: std::sync::Mutex<Option<Gpu>> = std::sync::Mutex::new(None);

/// The GPU block's grant and its CPU address, once the GPU is open: where the integrator's shared memory goes (`shared_place`).
static PLACE: std::sync::Mutex<Option<(u64, usize)>> = std::sync::Mutex::new(None);

/// lumis_core's `platform::set_shared_place`: the camera's integrator's `bytes` of shared memory at `INTEGRATOR_AT` in the GPU block (grown to 2^CAMERA_LOG for it) and zeroed, when the GPU is open and they fit under 2^CAMERA_LOG with the stack kernel's slack; None (the heap) otherwise: a readout too large (the 1:1 ones) is stacked on the CPU.
pub fn shared_place(bytes: usize) -> Option<*mut u64> {
    let (block, base) = (*PLACE.lock().unwrap())?;
    if INTEGRATOR_AT + bytes + stack::SLACK > 1 << CAMERA_LOG {
        return None;
    }
    let size = ferros_sys::gpu_block_ask(block, CAMERA_LOG)?;
    if size < 1 << CAMERA_LOG {
        return None;
    }
    let at = (base + INTEGRATOR_AT) as *mut u8;
    // SAFETY: the block is 2^CAMERA_LOG bytes at `base` (just grown), the process's for its life; the bytes from INTEGRATOR_AT are no one else's (the recorder's part ends under it, `INTEGRATOR_AT`), and the integrator they are for is the camera's only one (the one before it, of a camera since closed, never used again).
    unsafe { core::ptr::write_bytes(at, 0, bytes) };
    Some(at as *mut u64)
}

/// Where the integrator's plane at `plane` (`len` samples) lies in the block, in bytes from its start: Some when it is in the integrator's part (`shared_place` put it there) with the stack kernel's slack after it.
pub fn plane_in_block(plane: *const u16, len: usize) -> Option<usize> {
    let (_, base) = (*PLACE.lock().unwrap())?;
    let at = (plane as usize).checked_sub(base)?;
    (at >= INTEGRATOR_AT && at + len * 2 + stack::SLACK <= 1 << CAMERA_LOG).then_some(at)
}

/// The camera's view is mapped in Lumis's GPU space (`camera_changed` after an OPEN said so), so a frame can be stacked on the GPU from it.
pub static CAMERA_MAPPED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// A stack job failed: the stack is the CPU's for the rest of the process.
pub static STACK_OFF: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// The camera opened (`open`) or closed: the GPU service told (op 5), which maps the camera's view in Lumis's GPU space after an OPEN and takes it away after a CLOSE, and whether frames can be stacked from it now.
pub fn camera_changed(open: bool) {
    let mapped = SHARED.lock().unwrap().as_ref().and_then(|g| g.camera_mapped()) == Some(true);
    CAMERA_MAPPED.store(open && mapped, core::sync::atomic::Ordering::Relaxed);
    if open {
        println!("lumis: the camera's view {} in the GPU", if mapped { "mapped: frames stacked there" } else { "not mapped: frames stacked on the CPU" });
    }
}
