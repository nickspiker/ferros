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
