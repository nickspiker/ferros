//! Lumis's frames drawn on the GPU (the owner's GPU plan, 2026-11-20: "you render every px all at once thru the gpu when anything within that damage rectangle changes, that's it. Straight shot."): the menu first, every pixel of it a job of lumis_gpu's `menu_kernel` from its parts (the splash, the slots, the ink lumis_core draws as `ui::draw::set_ink` has it), straight into one of two surfaces of Lumis's own that fluor's host shows in turn (`FluorApp::own_surface`, a layer's mailbox).
//!
//! Lumis's part of the GPU block is its first 2^`UI_LOG` bytes: the queue, the kernel's words, the code table, the slots, the rows, the coverage, the splash and the ink, each on its own place below; the recorder's kernel and frames after it (ferros_clip `gpu::set_base`). The block is grown to that before the queue opens, the two surfaces asked for before it too: a client's memory is its key to the GPU service (ferros_sys `gpu::Stamp::key`), so a block grown later (the recorder's) or a surface asked later is another client, and the queue is opened again for it (`Gpu::frame`).

use ferros_sys::gpu::queue::{Job, Queue, S_DONE};
use ferros_sys::gpu::{ARENA_VA, PRELOAD_REGS64, PRELOAD_X, SURFACES_PA, SURFACES_VA};
use lumis_gpu::menu;

/// Lumis's part of the GPU block, 2^25 bytes (32 MiB): the Pro's menu's parts take 29 MiB of it.
pub const UI_LOG: u32 = 25;
/// The queue at the block's start, 2^1 jobs deep (a frame's job at a time, and the next).
const QUEUE_AT: usize = 0;
const DEPTH_LOG2: u32 = 1;
/// The kernel's words, the code table, the slots' records (2^10 - 2^6 bytes after the table, 15 records: two drawings of NUM_SLOTS rows and the empty one), each pixel row's slot.
const CODE_AT: usize = 1 << 16;
const LUT_AT: usize = 1 << 17;
const SLOTS_AT: usize = LUT_AT + (1 << 10);
const SLOTS_MOST: usize = ((1 << 18) - SLOTS_AT) / (menu::RECORD_WORDS * 4);
const ROWS_AT: usize = 1 << 18;
/// The coverage from 2^20, then the splash and the ink, each on a MiB.
const COVER_AT: usize = 1 << 20;

fn mib(n: usize) -> usize {
    n.div_ceil(1 << 20) << 20
}

/// Lumis's GPU: the queue on its part of the block, the menu kernel, the two surfaces of the menu's layer.
pub struct Gpu {
    service: u64,
    block: u64,
    /// The block's CPU address and its size when the queue was opened (a size since is another client).
    base: *mut u8,
    size: u64,
    q: Queue,
    kernel: u8,
    compiled: lumis_gpu::Compiled,
    /// The two surfaces, each its base as Lumis's own `dma` of it (what the show names), and the one the next frame goes into.
    surfaces: [u64; 2],
    next: usize,
    /// The frame's size and the parts' places in the block.
    w: usize,
    h: usize,
    bg_at: usize,
    ink_at: usize,
    /// The splash is in the block (written once a menu).
    bg_written: bool,
    /// The slots as lumis_core gave them (`MainMenu::ink_frame`).
    slots: Vec<lumis_core::ui::draw::Slot>,
    /// The GPU's time of the frames since the last line, counter ticks, and how many.
    ticks: u64,
    frames: u64,
}

// SAFETY: the block is this process's for its life (the kernel never takes a GPU block back); only the UI thread draws into it.
unsafe impl Send for Gpu {}

impl Gpu {
    /// The GPU, when Lumis holds its grants (ferros_clip `ferros::gpu_grants`: a kist's GPU slots, a spawn's --use gpu --dma layers), for a `w` x `h` frame: the block grown to Lumis's part, the surfaces asked for, the queue opened, the menu kernel recorded and given to it, the code table written. Err: why not, for the console.
    pub fn open(surface: u64, w: usize, h: usize, lut: &[u32; 256]) -> Result<Gpu, String> {
        let (service, block) = ferros_clip::ferros::gpu_grants().ok_or("no GPU grants")?;
        let (service, block) = (crate::sys::cap(service), crate::sys::cap(block));
        let bg_at = COVER_AT + mib(w * h);
        let ink_at = bg_at + mib(w * h * 3);
        if ink_at + mib(w * h * 3) > 1 << UI_LOG {
            return Err(format!("a A#{w} x A#{h} menu's parts pass Lumis's 2^A#{UI_LOG} bytes of the block"));
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
        let compiled = lumis_gpu::menu_kernel();
        let (q, kernel) = Self::queue(service, block, base, &compiled)?;
        // SAFETY: the table's 2^10 bytes at LUT_AT, under Lumis's 2^UI_LOG of the block.
        unsafe { core::ptr::copy_nonoverlapping(lut.as_ptr() as *const u8, base.add(LUT_AT), 1 << 10) };
        Ok(Gpu { service, block, base, size, q, kernel, compiled, surfaces, next: 0, w, h, bg_at, ink_at, bg_written: false, slots: Vec::new(), ticks: 0, frames: 0 })
    }

    /// The queue opened on the block as it is now and the kernel's words given to it.
    fn queue(service: u64, block: u64, base: *mut u8, compiled: &lumis_gpu::Compiled) -> Result<(Queue, u8), String> {
        let q = Queue::open(service, block, QUEUE_AT, DEPTH_LOG2).map_err(|s| format!("the GPU queue did not open (status G#{s:X})"))?;
        // SAFETY: the words at CODE_AT, under LUT_AT (the menu kernel is a few hundred words, 8 bytes each).
        assert!(compiled.words.len() * 8 <= LUT_AT - CODE_AT, "the menu kernel's words pass their room");
        unsafe { core::ptr::copy_nonoverlapping(compiled.words.as_ptr() as *const u8, base.add(CODE_AT), compiled.words.len() * 8) };
        let preload = PRELOAD_X | if compiled.regs64 { PRELOAD_REGS64 } else { 0 };
        let kernel = q.kernel(CODE_AT, compiled.words.len(), preload).ok_or("the GPU service took no kernel")?;
        Ok((q, kernel))
    }

    /// Whether the parts were laid out for a `w` x `h` frame.
    pub fn fits(&self, w: usize, h: usize) -> bool {
        (self.w, self.h) == (w, h)
    }

    /// A new menu: its splash written again at the next frame.
    pub fn new_menu(&mut self) {
        self.bg_written = false;
    }

    /// The menu's parts now into the block (`fresh`: it changed since the last frame) and a frame of it drawn whole on the GPU into the next surface: that surface's base, for the show. Err: why not (the frame is then the CPU's).
    pub fn frame(&mut self, m: &mut lumis_core::ui_main_menu::MainMenu, fresh: bool) -> Result<u64, String> {
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
        // The recorder grew the block since (ferros_clip, a clip on the GPU): another client to the service, the queue opened again for it.
        let size = ferros_sys::dma_size(self.block, ferros_sys::GPU_BLOCK_MOST as u64).unwrap_or(0);
        if size != self.size {
            println!("lumis: the GPU block is G#{size:X} bytes now (G#{:X} at the queue's open): the queue opened again", self.size);
            (self.q, self.kernel) = Self::queue(self.service, self.block, self.base, &self.compiled)?;
            self.size = size;
        }
        let k = self.next;
        let dst = SURFACES_VA + (self.surfaces[k] - SURFACES_PA);
        let at = |o: usize| ARENA_VA + o as u64;
        let mut u = [0u64; menu::SLOT_COUNT as usize];
        u[menu::DST as usize] = dst;
        u[menu::BG as usize] = at(self.bg_at);
        u[menu::INK as usize] = at(self.ink_at);
        u[menu::COVER as usize] = at(COVER_AT);
        u[menu::ROWS as usize] = at(ROWS_AT);
        u[menu::SLOTS as usize] = at(SLOTS_AT);
        u[menu::LUT as usize] = at(LUT_AT);
        u[menu::SIZE as usize] = w as u64 | ((w * h) as u64) << 32;
        u[menu::STRIDE as usize] = w as u64;
        let mut uniforms = u.to_vec();
        uniforms.extend_from_slice(&self.compiled.consts);
        let groups = ((w * h).div_ceil(1 << 4) as u32, 1);
        let n = self.q.submit(&Job { kernel: self.kernel, groups, uniforms: &uniforms }).ok_or("the GPU queue took no job")?;
        let e = self.q.wait(n);
        if e.status != S_DONE {
            // The queue ended with it (a fault ends a queue): opened again for the next frame.
            let why = format!("the menu's GPU job ended with status A#{} (the GPU address G#{:X}, fault G#{:X})", e.status, e.fault_address, e.fault_status);
            self.q.reopen();
            return Err(why);
        }
        self.ticks += e.gpu_ticks().unwrap_or(0);
        self.frames += 1;
        if self.frames == 1 << 6 {
            println!("lumis: the menu on the GPU: {} us a frame on the GPU, the last {} frames", self.ticks * 1_000_000 / (self.frames * ferros_sys::freq()), self.frames);
            (self.ticks, self.frames) = (0, 0);
        }
        self.next ^= 1;
        Ok(self.surfaces[k])
    }
}
