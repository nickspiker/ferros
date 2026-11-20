//! The kist's side of the hold (docs/STORE.md "The calls"; docs/HOLD-API.md): what a kist calls to keep things for the person, find them, read their bodies by range and remove them.
//!
//! Every call goes thru the kist's `items` grant (ferros_kist ITEMS, slot by its place among the capsule's wants): `SYS_STORE` x1 bit 7, which the kernel forwards to the vault process with the person's items domain and the kist's stamp (process.rs `items_forward`), so the vault never takes the kist's word for who it is or what it may open.
//! A denied right is an empty slot: every call on it is refused, never answered with made-up items.
//!
//! The calls are the same over any `Transport`: the device's syscall (`Sys`), or a host test's straight call into `serve::Server`.

use alloc::vec::Vec;

use crate::{wire, Card, Error, Find, Found, Proposed, Result};

/// The syscall's answers for a refusal and an absence (ferros_sys `ERR`, `NOTHING`).
pub const ERR: u64 = u64::MAX;
pub const NOTHING: u64 = u64::MAX - 1;

/// Where a call goes.
pub trait Transport {
    /// One call (`wire::call`'s bytes) with the reply's room: the value; the reply's bytes land in `reply`.
    fn call(&mut self, call: &[u8], reply: &mut [u8]) -> u64;
    /// Whole blocks at `block` of a clip's extent, written thru the extent's own capability (SYS_BLOCK): 0, or ERR. A transport with no such path refuses, and the clip is written thru the vault.
    fn block_write(&mut self, _cap: &[u8; 32], _block: u64, _bytes: &[u8]) -> u64 {
        ERR
    }
}

/// A body being kept: the vault's token for the making, the caller's alone.
pub struct Body {
    token: [u8; 32],
}

/// A clip being recorded into its single-copy extent: the item's handle (the card is placed at `make_clip`, so the clip is found while it records), the blocks reserved, and the extent's own capability when the kernel gave one (its writes then go straight to the flash).
pub struct Clip {
    pub handle: [u8; 32],
    pub blocks: u64,
    cap: Option<[u8; 32]>,
}

impl Clip {
    /// Whether the clip's blocks are written straight to the flash (the extent's own capability) or thru the vault process.
    pub fn direct(&self) -> bool {
        self.cap.is_some()
    }
}

/// The hold, as a kist reaches it.
pub struct Hold<T: Transport> {
    pub t: T,
    /// Eagle time now, in oscillations: the documents a call carries are stamped with it.
    clock: fn() -> i64,
}

fn answer(v: u64) -> Result<u64> {
    match v {
        ERR => Err(Error::Refused),
        NOTHING => Err(Error::NotFound),
        v => Ok(v),
    }
}

impl<T: Transport> Hold<T> {
    pub fn new(t: T, clock: fn() -> i64) -> Hold<T> {
        Hold { t, clock }
    }

    fn call(&mut self, op: u8, handle: &[u8; 32], offset: u64, payload: &[u8], reply: &mut [u8]) -> Result<u64> {
        let c = wire::call(op, handle, offset, reply.len() as u32, payload);
        answer(self.t.call(&c, reply))
    }

    /// A card document answered into a room of `CARD_DOC_MAX`.
    fn card_call(&mut self, op: u8, handle: &[u8; 32], payload: &[u8]) -> Result<([u8; 32], Card)> {
        let mut room = alloc::vec![0u8; wire::CARD_DOC_MAX];
        let n = self.call(op, handle, 0, payload, &mut room)? as usize;
        // WHY: the value is the vault's; a card document past its room is not one this build reads.
        wire::parse_card_doc(room.get(..n).ok_or(Error::TooLarge)?)
    }

    /// A new item of a kind the kist declares it makes: its body follows (`write`, then `finish`).
    pub fn make(&mut self, p: &Proposed) -> Result<Body> {
        let doc = wire::propose(p, (self.clock)())?;
        let mut token = [0u8; 32];
        self.call(wire::OP_MAKE, &[0; 32], 0, &doc, &mut token)?;
        Ok(Body { token })
    }

    /// The body's next bytes, any length (sent a transfer at a time).
    pub fn write(&mut self, b: &Body, bytes: &[u8]) -> Result<()> {
        for chunk in bytes.chunks(wire::TRANSFER) {
            self.call(wire::OP_WRITE, &b.token, 0, chunk, &mut [])?;
        }
        Ok(())
    }

    /// The body committed and the card placed: the item's handle and its card.
    pub fn finish(&mut self, b: Body) -> Result<([u8; 32], Card)> {
        self.card_call(wire::OP_FINISH, &b.token, &[])
    }

    /// `make`, `write` and `finish` for a body in hand (a still, a list, a contact).
    pub fn keep(&mut self, p: &Proposed, body: &[u8]) -> Result<([u8; 32], Card)> {
        let b = self.make(p)?;
        self.write(&b, body)?;
        self.finish(b)
    }

    /// Keep an item again in place (no versions: the new body replaces the old at `finish`).
    pub fn keep_again(&mut self, handle: &[u8; 32]) -> Result<Body> {
        let mut token = [0u8; 32];
        self.call(wire::OP_KEEP_AGAIN, handle, 0, &[], &mut token)?;
        Ok(Body { token })
    }

    /// A find: a page of matching items, each with this kist's handle. The answer's room grows once to its length when the first was short.
    pub fn find(&mut self, f: &Find) -> Result<Found> {
        let doc = wire::find(f, (self.clock)())?;
        let mut room = alloc::vec![0u8; 1 << 16];
        let mut n = self.call(wire::OP_FIND, &[0; 32], 0, &doc, &mut room)? as usize;
        if n > room.len() {
            room.resize(n, 0);
            n = self.call(wire::OP_FIND, &[0; 32], 0, &doc, &mut room)? as usize;
        }
        // WHY: the value is the vault's; an answer that grew past its room between the two asks is refused, not read short.
        wire::parse_found(room.get(..n).ok_or(Error::TooLarge)?)
    }

    pub fn card(&mut self, handle: &[u8; 32]) -> Result<Card> {
        Ok(self.card_call(wire::OP_CARD, handle, &[])?.1)
    }

    /// Body bytes from `offset` into `buf`, a transfer a call: how many came (fewer at the body's end) and the body's size. A RAW clip reads the same way: the vault reads its blocks.
    pub fn read(&mut self, handle: &[u8; 32], offset: u64, buf: &mut [u8]) -> Result<(usize, u64)> {
        let mut done = 0;
        let mut size = 0;
        while done < buf.len() {
            // WHY: one transfer a call; the last call asks only for what is left.
            let k = (buf.len() - done).min(wire::TRANSFER);
            size = self.call(wire::OP_READ, handle, offset + done as u64, &[], &mut buf[done..done + k])?;
            // The bytes that came: up to the body's end.
            let came = k.min(size.saturating_sub(offset + done as u64) as usize); // WHY/PROOF: an offset past the end reads nothing (the vault answers the size and no bytes), never a wrapped count.
            done += came;
            if came < k {
                break;
            }
        }
        Ok((done, size))
    }

    /// One usage call sizing `kists` (each its developer and option; at most `wire::USAGE_KISTS`).
    pub fn usage_of(&mut self, kists: &[([u8; 32], [u8; 32])]) -> Result<wire::Usage> {
        let mut payload = Vec::with_capacity(kists.len() << 6);
        for (d, o) in kists {
            payload.extend_from_slice(d);
            payload.extend_from_slice(o);
        }
        let mut room = alloc::vec![0u8; 1 << 14];
        let mut n = self.call(wire::OP_USAGE, &[0; 32], 0, &payload, &mut room)? as usize;
        if n > room.len() {
            room.resize(n, 0);
            n = self.call(wire::OP_USAGE, &[0; 32], 0, &payload, &mut room)? as usize;
        }
        // WHY: the value is the vault's; an answer that grew past its room between the two asks is refused, not read short.
        wire::parse_usage(room.get(..n).ok_or(Error::TooLarge)?)
    }

    /// What is on the device, read only: the hold's totals by kind, the clips area, and every stowed kist with its own domain's size (one call to learn the kists, then one a `wire::USAGE_KISTS` of them to size them).
    pub fn usage(&mut self) -> Result<wire::Usage> {
        let mut u = self.usage_of(&[])?;
        let ids: Vec<([u8; 32], [u8; 32])> = u.kists.iter().map(|k| (k.developer, k.option)).collect();
        for batch in ids.chunks(wire::USAGE_KISTS) {
            let sized = self.usage_of(batch)?;
            for s in sized.kists.into_iter().filter(|s| s.bytes.is_some()) {
                if let Some(k) = u.kists.iter_mut().find(|k| k.developer == s.developer && k.option == s.option) {
                    *k = s;
                }
            }
        }
        Ok(u)
    }

    /// Removed, gone at once: its card, its body, a clip's extent. No bin.
    pub fn remove(&mut self, handle: &[u8; 32]) -> Result<()> {
        self.call(wire::OP_REMOVE, handle, 0, &[], &mut []).map(|_| ())
    }

    /// A new item whose body is a single-copy extent of `blocks` 4 KiB blocks (a RAW clip: written once at the flash's rate, never mirrored): its card placed at once.
    pub fn make_clip(&mut self, p: &Proposed, blocks: u64) -> Result<Clip> {
        let doc = wire::propose(p, (self.clock)())?;
        let mut r = [0u8; 64];
        let got = self.call(wire::OP_MAKE_EXTENT, &[0; 32], blocks, &doc, &mut r)?;
        let handle: [u8; 32] = r[..32].try_into().unwrap();
        let cap: [u8; 32] = r[32..].try_into().unwrap();
        Ok(Clip { handle, blocks: got, cap: cap.iter().any(|&b| b != 0).then_some(cap) })
    }

    /// Whole blocks of the clip at `block`, at most one transfer (1 MiB): straight to the flash thru its capability, or thru the vault.
    pub fn write_clip(&mut self, c: &Clip, block: u64, bytes: &[u8]) -> Result<()> {
        match &c.cap {
            Some(cap) => answer(self.t.block_write(cap, block, bytes)).map(|_| ()),
            None => self.call(wire::OP_WRITE_EXTENT, &c.handle, block, bytes, &mut []).map(|_| ()),
        }
    }

    /// The clip sealed at `len` bytes with its BLAKE3 (the recorder's seal): its tail given back, its card as it now stands.
    pub fn seal_clip(&mut self, c: Clip, len: u64, hash: &[u8; 32]) -> Result<Card> {
        let mut p = Vec::with_capacity(40);
        p.extend_from_slice(&len.to_le_bytes());
        p.extend_from_slice(hash);
        Ok(self.card_call(wire::OP_FINISH_EXTENT, &c.handle, &p)?.1)
    }
}

/// The device's transport: the `items` grant's token (its offset in the arc: ferros_sys `cap(grants, slot)`, or `0x204 + 32 * slot` in a std kist, whose grants page is at the arc's base).
#[cfg(all(target_arch = "aarch64", any(target_os = "none", target_os = "ferros")))]
pub struct Sys {
    pub cap: u64,
}

#[cfg(all(target_arch = "aarch64", any(target_os = "none", target_os = "ferros")))]
mod sys {
    /// SYS_STORE, its hold bit (process.rs STORE_ITEMS), SYS_BLOCK.
    const STORE: u64 = 13;
    const STORE_ITEMS: u64 = 1 << 7;
    const BLOCK: u64 = 29;

    #[inline(always)]
    fn svc(n: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
        let r: u64;
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        unsafe { core::arch::asm!("svc #0", inlateout("x0") a0 => r, in("x1") a1, in("x2") a2, in("x3") a3, in("x4") a4, in("x8") n, options(nostack)) };
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        r
    }

    /// A buffer's offset in the arc (x28 holds its base, the kernel's, never written by compiled code on ferros's targets).
    #[inline(always)]
    fn off<T: ?Sized>(p: *const T) -> u64 {
        let b: u64;
        unsafe { core::arch::asm!("mov {}, x28", out(reg) b, options(nomem, nostack, preserves_flags)) };
        (p as *const u8 as u64).wrapping_sub(b)
    }

    impl super::Transport for super::Sys {
        fn call(&mut self, call: &[u8], reply: &mut [u8]) -> u64 {
            // The room in bits 8-31: every room `Hold` hands is at most a transfer or a find's answer, well under 2^24.
            svc(STORE, self.cap, STORE_ITEMS | (reply.len() as u64) << 8, off(call.as_ptr()), call.len() as u64, off(reply.as_mut_ptr()))
        }
        fn block_write(&mut self, cap: &[u8; 32], block: u64, bytes: &[u8]) -> u64 {
            svc(BLOCK, off(cap.as_ptr()), 1 | ((bytes.len() as u64 >> 12) & 0xFFFF) << 8, block, off(bytes.as_ptr()), 0)
        }
    }
}
