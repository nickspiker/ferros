//! The recorder on ferros: a clip's single-copy extent in the vault (`SYS_STORE` x1 bit 6, process.rs `extent_forward`; payloads/vault `extent`), and pinning a thread.
//! The syscall inlined, as kete's and Lumis's are (ferros_sys is no_std and not published).

const STORE: u64 = 13;
const PIN: u64 = 20;
const ERR: u64 = u64::MAX;
const NOTHING: u64 = u64::MAX - 1;
/// SYS_STORE x1 bits: a listing (the probe), the name a 32-byte address, the extent call.
const LIST: u64 = 16;
const AT: u64 = 2;
const ADDR: u64 = 32;
const EXTENT: u64 = 1 << 6;
/// The extent ops (process.rs `EOP_*`).
const EOP_OPEN: u64 = 0;
const EOP_WRITE: u64 = 1;
const EOP_SEAL: u64 = 2;
const EOP_READ: u64 = 3;
const EOP_INFO: u64 = 4;
const EOP_DELETE: u64 = 5;
/// An extent's record as the vault gives it (process.rs `EXTENT_INFO`).
const INFO: usize = 8 + 8 + 1 + 32 + 8 + 8;

#[inline(always)]
fn svc(n: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
    let r: u64;
    unsafe { core::arch::asm!("svc #0", inlateout("x0") a0 => r, in("x1") a1, in("x2") a2, in("x3") a3, in("x4") a4, in("x8") n, options(nostack)) };
    r
}

/// The arc base (x28, the kernel's, never written by compiled code on this target).
#[inline(always)]
fn arc_base() -> u64 {
    let b: u64;
    unsafe { core::arch::asm!("mov {}, x28", out(reg) b, options(nomem, nostack, preserves_flags)) };
    b
}

#[inline(always)]
fn off<T: ?Sized>(p: *const T) -> u64 {
    (p as *const u8 as u64).wrapping_sub(arc_base())
}

fn grant_count() -> u64 {
    unsafe { ((arc_base() + 0x200) as *const u32).read_volatile() as u64 }
}

fn cap(slot: u64) -> u64 {
    0x204 + 32 * slot
}

/// The cores this thread may run on from now (husky: A510s G#000F, A715s G#00F0, the X3 G#0100).
pub fn pin(mask: u64) {
    svc(PIN, mask, 0, 0, 0, 0);
}

/// The first grant a store listing answers on: the process's vault scope or kist domain (as kete finds it).
pub fn store_slot() -> Option<u64> {
    let mut probe = [0u8; 8];
    (1..grant_count().min(64)).find(|&s| svc(STORE, cap(s), ADDR | LIST | AT | 8 << 8, off(b"".as_ptr()), 0, off(probe.as_mut_ptr())) != ERR)
}

/// An extent's record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Info {
    pub blocks: u64,
    pub bytes: u64,
    pub sealed: bool,
    pub hash: [u8; 32],
    pub at: i64,
    /// Where it lies in the clips area (blocks): for the bench's pull of a clip by its blocks.
    pub first: u64,
}


/// The recording space, read only, thru the store grant in `slot` (a vault and kernel from 2026-11-19 on; None from an older one).
pub fn space(slot: u64) -> Option<crate::Space> {
    const EOP_SPACE: u64 = 6;
    let name = b"space";
    let mut b = [0u8; 40];
    match svc(STORE, cap(slot), EXTENT | EOP_SPACE | 40 << 8, off(name.as_ptr()), name.len() as u64, off(b.as_mut_ptr())) {
        ERR | NOTHING => None,
        _ => Some(crate::Space::from_reply(&b)),
    }
}

/// A named extent of the vault, on the store grant in `slot`.
pub struct Extent {
    slot: u64,
    name: Vec<u8>,
    /// The extent's own SYS_BLOCK capability, when the vault gave one at the open (a kernel and vault from 2026-11-19 on): writes go straight to the flash, not thru the vault process.
    own: Option<Box<[u8; 32]>>,
}

const BLOCK: u64 = 29;

impl Extent {
    /// The extent named `name` (printable ASCII, at most 96 bytes, as a value's name).
    pub fn new(slot: u64, name: &str) -> Extent {
        Extent { slot, name: name.as_bytes().to_vec(), own: None }
    }
    fn call(&self, op: u64, block: u64, buf: *const u8, len: usize) -> u64 {
        svc(STORE, cap(self.slot), EXTENT | op | (len as u64) << 8 | block << 32, off(self.name.as_ptr()), self.name.len() as u64, off(buf))
    }
    /// Reserve `blocks` (a fresh extent under the name; the old one, if any, goes): the blocks reserved.
    pub fn open(&mut self, blocks: u64) -> Result<u64, String> {
        let mut b = [0u8; 32];
        b[..8].copy_from_slice(&blocks.to_le_bytes());
        match self.call(EOP_OPEN, 0, b.as_ptr(), 32) {
            ERR => Err(format!("extent open of A#{blocks} blocks refused (no stretch that long free, or no vault process with a clips window)")),
            n => {
                // The reply's 32 bytes: the extent's own capability; a vault that gives none answers no bytes, and past the count they stay zeros.
                self.own = b[8..].iter().any(|&x| x != 0).then(|| Box::new(b));
                Ok(n)
            }
        }
    }
    /// Whether writes go straight to the flash (the extent's own capability) or thru the vault process.
    pub fn direct(&self) -> bool {
        self.own.is_some()
    }
    /// Whole blocks at `block`, at most 1 MiB.
    pub fn write(&self, block: u64, bytes: &[u8]) -> Result<(), String> {
        let r = match &self.own {
            Some(t) => svc(BLOCK, off(t.as_ptr()), 1 | ((bytes.len() as u64 >> 12) & 0xFFFF) << 8, block, off(bytes.as_ptr()), 0),
            None => self.call(EOP_WRITE, block, bytes.as_ptr(), bytes.len()),
        };
        match r {
            0 => Ok(()),
            r => Err(format!("extent write of A#{} bytes at block A#{block} refused (G#{r:X})", bytes.len())),
        }
    }
    /// Whole blocks from `block` into `buf`, at most 1 MiB.
    pub fn read(&self, block: u64, buf: &mut [u8]) -> Result<(), String> {
        match self.call(EOP_READ, block, buf.as_mut_ptr(), buf.len()) {
            ERR | NOTHING => Err(format!("extent read of A#{} bytes at block A#{block} refused", buf.len())),
            _ => Ok(()),
        }
    }
    pub fn seal(&self, len: u64, hash: &[u8; 32]) -> Result<(), String> {
        let mut b = [0u8; 40];
        b[..8].copy_from_slice(&len.to_le_bytes());
        b[8..].copy_from_slice(hash);
        match self.call(EOP_SEAL, 0, b.as_ptr(), 40) {
            0 => Ok(()),
            _ => Err(format!("extent seal of A#{len} bytes refused")),
        }
    }
    pub fn info(&self) -> Option<Info> {
        let mut b = [0u8; INFO];
        match self.call(EOP_INFO, 0, b.as_mut_ptr(), INFO) {
            ERR | NOTHING => None,
            _ => {
                let u = |k: usize| u64::from_le_bytes(b[k..k + 8].try_into().unwrap());
                Some(Info { blocks: u(0), bytes: u(8), sealed: b[16] != 0, hash: b[17..49].try_into().unwrap(), at: u(49) as i64, first: u(57) })
            }
        }
    }
    pub fn delete(&self) -> bool {
        self.call(EOP_DELETE, 0, core::ptr::null(), 0) == 0
    }
}

/// The recorder's sink: the clip's extent.
pub struct ExtentSink(pub Extent);

impl crate::Sink for ExtentSink {
    fn open(&mut self, blocks: u64) -> Result<u64, String> {
        self.0.open(blocks)
    }
    fn write(&mut self, block: u64, bytes: &[u8]) -> Result<(), String> {
        self.0.write(block, bytes)
    }
    fn seal(&mut self, len: u64, hash: [u8; 32]) -> Result<(), String> {
        self.0.seal(len, &hash)
    }
}

/// An extent read as a VSF byte source (vsf `container::ByteSource`), a block run at a time: for reading a clip back on the device (vsf `clip::Reader`).
pub struct ExtentSource {
    pub extent: Extent,
    pub len: u64,
}

impl vsf::container::ByteSource for ExtentSource {
    fn len(&self) -> u64 {
        self.len
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), vsf::container::SourceError> {
        // WHY: the offset and length are the reader's, from the clip's bytes; past the clip is outside the source.
        if offset.checked_add(buf.len() as u64).is_none_or(|e| e > self.len) {
            return Err(vsf::container::SourceError::OutOfRange { offset, len: buf.len() as u64, size: self.len });
        }
        let (first, last) = (offset >> 12, (offset + buf.len() as u64).div_ceil(1 << 12));
        let mut tmp = vec![0u8; ((last - first) as usize) << 12];
        for (k, chunk) in tmp.chunks_mut(crate::WRITE_MAX).enumerate() {
            self.extent.read(first + ((k * crate::WRITE_MAX) >> 12) as u64, chunk).map_err(vsf::container::SourceError::Io)?;
        }
        let at = (offset & 4095) as usize;
        buf.copy_from_slice(&tmp[at..at + buf.len()]);
        Ok(())
    }
}

const CALL: u64 = 1;
const DMA: u64 = 7;
/// The client's GPU block in the GPU's own view (ferros_sys `gpu::ARENA_VA`).
const ARENA_VA: u64 = 0x8000_0000;

/// The band coder's first half on the GPU (crate::gpu): the GPU service's grant and the process's GPU block (a spawn's `--use gpu --dma layers`, a kist's ferros_sys `gpu::kist_slots`), the host-built kernel uploaded once, then a frame copied in and run.
pub struct GpuDevice {
    service: u64,
    block: *mut u8,
    layout: crate::gpu::Layout,
    kernel: &'static crate::gpu::Prebuilt,
    height: usize,
}

// SAFETY: the block is this process's for its life (the kernel never takes a GPU block back), and only the recorder's frame call writes it, while the coders read it only between a run's end and the frame call's return.
unsafe impl Send for GpuDevice {}
unsafe impl Sync for GpuDevice {}

impl GpuDevice {
    /// The GPU service in grant `service`, the GPU block in grant `block`, for frames of `width` x `height` samples `stride` bytes a row: the block grown to the layout and the kernel's words written to it.
    pub fn open(service: u64, block: u64, width: usize, height: usize, stride: usize) -> Result<GpuDevice, String> {
        let kernel = crate::gpu::prebuilt(width, stride).ok_or_else(|| format!("no GPU kernel built for A#{width} samples on A#{stride} bytes"))?;
        let layout = crate::gpu::Layout::of(height, stride);
        let size = svc(DMA, cap(block), 2, layout.log as u64, 0, 0);
        // WHY: the kernel answers ERR for a grant that is no GPU block or a frame pool short of the size, and a kernel from before 2026-11-13 answers the base instead (past any block's size).
        if size == ERR || size > 1 << 29 || size < 1 << layout.log {
            return Err(format!("the GPU block in grant A#{block} did not grow to 2^A#{} bytes (G#{size:X})", layout.log));
        }
        let base = svc(DMA, cap(block), 0, 0, 0, 0);
        if base == ERR {
            return Err(format!("grant A#{block} is no DMA region"));
        }
        let d = GpuDevice { service, block: base as *mut u8, layout, kernel, height };
        // SAFETY: the block is `size` bytes at `base`, the words far under the frame's MiB.
        unsafe { core::ptr::copy_nonoverlapping(kernel.words.as_ptr() as *const u8, d.block, kernel.words.len() * 8) };
        Ok(d)
    }

    /// The frame's rows copied into the block and the kernel run over them; the GPU's time, microseconds, as the service measured it.
    pub fn run(&mut self, rows: &[u8]) -> Result<u32, String> {
        let n = rows.len().min(self.height * self.kernel.stride);
        // SAFETY: the layout gives the frame `height * stride` bytes and more at `frame`.
        unsafe { core::ptr::copy_nonoverlapping(rows.as_ptr(), self.block.add(self.layout.frame), n) };
        self.go()
    }

    /// The kernel run over the frame as it lies in the block now.
    pub fn go(&mut self) -> Result<u32, String> {
        let invocations = (self.height << crate::gpu::BLOCKS_LOG2) as u32;
        let mut msg = [0u8; 18 + 8 * 64];
        msg[0] = 1;
        let preload: u16 = 0x1000 | if self.kernel.regs64 { 0x8000 } else { 0 };
        msg[1..3].copy_from_slice(&preload.to_le_bytes());
        msg[7..9].copy_from_slice(&(self.kernel.words.len() as u16).to_le_bytes());
        msg[9..13].copy_from_slice(&(invocations >> 4).to_le_bytes());
        msg[13..17].copy_from_slice(&1u32.to_le_bytes());
        let mut u = vec![ARENA_VA + self.layout.codes as u64, ARENA_VA + self.layout.frame as u64, ARENA_VA + self.layout.info as u64];
        u.extend_from_slice(self.kernel.consts);
        msg[17] = u.len() as u8;
        for (i, v) in u.iter().enumerate() {
            msg[18 + 8 * i..26 + 8 * i].copy_from_slice(&v.to_le_bytes());
        }
        let mut reply = [0u8; 17];
        let len = 18 + 8 * u.len();
        let r = svc(CALL, cap(self.service), off(msg.as_ptr()), len as u64, off(reply.as_mut_ptr()), reply.len() as u64);
        if r == ERR {
            return Err("the GPU service refused the call".into());
        }
        let us = u32::from_le_bytes(reply[1..5].try_into().unwrap());
        match reply[0] {
            0 => Ok(us),
            s => Err(format!("the GPU run ended with status A#{s} (reply G#{})", reply.iter().map(|b| format!("{b:02X}")).collect::<String>())),
        }
    }

    /// The frame as it lies in the block.
    pub fn frame(&self) -> &[u8] {
        // SAFETY: the layout's frame.
        unsafe { core::slice::from_raw_parts(self.block.add(self.layout.frame), self.height * self.kernel.stride) }
    }

    /// Every block's facts, `1 << BLOCKS_LOG2` a row, after a run.
    pub fn info(&self) -> &[u32] {
        // SAFETY: the layout's facts, written by the last run.
        unsafe { core::slice::from_raw_parts(self.block.add(self.layout.info) as *const u32, self.height << crate::gpu::BLOCKS_LOG2) }
    }

    /// Every block's body, 2^`BODY_LOG2` words a block, after a run.
    pub fn bodies(&self) -> &[u32] {
        // SAFETY: the layout's bodies, written by the last run.
        unsafe { core::slice::from_raw_parts(self.block.add(self.layout.codes) as *const u32, self.height << crate::gpu::BLOCKS_LOG2 << crate::gpu::BODY_LOG2) }
    }
}
