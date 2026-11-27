//! Cap-addressed command protocol.
//!
//! Every PT payload that represents a command follows this format:
//!
//!   `[cap: 32][op: 1][params...]`
//!
//! `cap` is a BLAKE3 credential — the same hash used by the vault capability system. It addresses the target object AND proves access.
//!
//! In dev mode, well-known "dev caps" are BLAKE3 hashes of fixed strings. The kernel recognizes them without validation. In prod mode, the host authenticates and receives delegated caps — same wire format.
//!
//! Responses use the same cap field (echo back the credential) so the caller can match responses to requests.

/// Minimum command size: 32-byte cap + 1-byte op.
pub const CMD_MIN_SIZE: usize = 33;

/// Operation codes — mirror CapabilityLevel semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Op {
    /// Read the addressed object.
    Read = 0x01,
    /// Write to the addressed object.
    Write = 0x02,
    /// Execute/invoke the addressed object.
    Exec = 0x03,
}

impl Op {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0x01 => Some(Self::Read),
            0x02 => Some(Self::Write),
            0x03 => Some(Self::Exec),
            _ => None,
        }
    }
}

/// A decoded command.
#[derive(Clone, Debug)]
pub struct Command<'a> {
    /// The 32-byte capability credential (BLAKE3 hash).
    pub cap: [u8; 32],
    /// The operation to perform.
    pub op: Op,
    /// Any remaining bytes after cap+op (operation-specific params).
    pub params: &'a [u8],
}

/// Parse a command from a PT payload.
pub fn parse(buf: &[u8]) -> Option<Command<'_>> {
    if buf.len() < CMD_MIN_SIZE {
        return None;
    }
    let mut cap = [0u8; 32];
    cap.copy_from_slice(&buf[..32]);
    let op = Op::from_u8(buf[32])?;
    let params = &buf[CMD_MIN_SIZE..];
    Some(Command { cap, op, params })
}

/// Encode a command into a buffer. Returns bytes written.
pub fn encode(buf: &mut [u8], cap: &[u8; 32], op: Op, params: &[u8]) -> usize {
    let total = CMD_MIN_SIZE + params.len();
    if buf.len() < total {
        return 0;
    }
    buf[..32].copy_from_slice(cap);
    buf[32] = op as u8;
    buf[CMD_MIN_SIZE..total].copy_from_slice(params);
    total
}

// ---------------------------------------------------------------------------
// Dev caps — well-known BLAKE3 hashes for development mode
// ---------------------------------------------------------------------------

/// Compute a dev cap hash at runtime. In dev mode these are not validated, but both sides must agree on the same hash for matching.
///
/// Uses BLAKE3 of the string directly.
pub fn dev_cap(name: &[u8]) -> [u8; 32] {
    *blake3::hash(name).as_bytes()
}

/// Well-known dev cap names.
pub mod caps {
    /// Boot diagnostics — read boot log, probe hardware.
    pub const DIAG: &[u8] = b"ferros.dev.diag";
    /// Memory — read/write MMIO regions.
    pub const MEM: &[u8] = b"ferros.dev.mem";
    /// Vault — get/put objects.
    pub const STORE: &[u8] = b"ferros.dev.store";
    /// USB — device info, endpoint status.
    pub const USB: &[u8] = b"ferros.dev.usb";
    /// Reboot — exec triggers reboot, params select mode.
    pub const REBOOT: &[u8] = b"ferros.dev.reboot";
    /// Reload — write sends new kernel binary, exec jumps to it.
    pub const RELOAD: &[u8] = b"ferros.dev.reload";
    /// Run — write stages an arbitrary payload blob, exec CALLS it (and returns, unlike RELOAD's one-way jump). Payload ABI: `extern "C" fn(in_ptr: *const u8, in_len: usize, out_ptr: *mut u8, out_cap: usize) -> u64`, entry at blob offset 0, PC-relative code only. Exec params become the payload's input; the response is [ret: 8 LE][out bytes].
    pub const RUN: &[u8] = b"ferros.dev.run";
    /// Spawn — exec runs the payload staged through RUN as a process: EL0, in its own arc, holding only a console capability (ferros_kernel process.rs). Exec params become the process's input; the response is [exit: 8 LE][what it sent its console].
    pub const SPAWN: &[u8] = b"ferros.dev.spawn";
    /// Seal (install) a masked capsule: exec runs the capsule staged through RUN through the gate and the sandbox checker once, and its seal (a keyed hash under the OS-only key) joins the vault's `sealed` list; spawn then requires that seal and does not re-check (SANDBOX.md). Response [0][seal: 32] or ERR:….
    pub const SEAL: &[u8] = b"ferros.dev.seal";
    /// Beam — read ring entries by generation offset. Params: [ring_id:1][mode:1][offset:4 BE][count:4 BE] ring_id: 0=kernel, 1=vault_root, 2=ledger, 3=state mode: 0=latest (~N offset), 1=absolute generation, 2=all
    pub const BEAM: &[u8] = b"ferros.dev.beam";
    /// Display — write renders the params (text, '\n'-separated) on the panel through the adopted DECON pipeline; the response is a register report.
    pub const DISPLAY: &[u8] = b"ferros.dev.display";
    /// Install — write sends signed kernel binary, exec persists to UFS + stem entry. Exec params: [size:4 LE][hash:32][sig:64] = 100 bytes
    pub const INSTALL: &[u8] = b"ferros.dev.install";
    /// Offer a kist to stow (docs/KISTS.md "Stowing", docs/USB.md "Offering a kist"; owner, 2026-11-11: "Just seems to me that we should just make a proper checked path and start using it"). Not a development door: a production build answers it too, and only while a person is present (attested, so the vault's identity half is open). Its hash is `dev_cap`'s (blake3 of the name; the name says it is no `ferros.dev.*`). The device's stower checks it exactly as a kist from the directory, then the person says Stow or Cancel on the glass; nothing is in their list before Stow. Ops (ferros_kist::offer for the states and reasons):
    /// - Write [capsule 32][offset u64 LE][more u8][bytes]: a part of the capsule (at most `ferros_kist::offer::PART`), in order from offset 0, kept in the device's `capsules.` under its blake3 → [0] kept, or [a nonzero status][words].
    /// - Exec [capsule 32][length u64 LE]: offer it, every part in → [0] offered, or [a nonzero status][words] (nobody present, another offer waiting on the person, not carried in whole, no stower).
    /// - Read [capsule 32][the state last seen u8]: its state, held until it is not the one last seen or for `ferros_kist::offer::HOLD_SECONDS` → [state][why][flags].
    pub const OFFER: &[u8] = b"ferros.kist.offer";
    /// The end of a command the device answered `held::STILL` (docs/USB.md "Long commands"): Read, no parameters → that command's own answer once its work ends, or `STILL` again at the end of a lease, or `NOTHING` when no command is held. Answered by a production build too: it tells only the end of the host's own last command.
    pub const HELD: &[u8] = b"ferros.pt.held";
}

/// A command whose work outlives one answer (docs/USB.md "Long commands"; 2026-11-13, the 8a: the last part of Photon's 92 MB capsule committed the whole value on a busy vault and ran past the USB driver's 16 s, which gives up on a command the kernel never answers).
/// The device answers it `STILL` at the end of a lease and carries the work on; the host then reads its end with `caps::HELD`, a read the device holds until the work ends (its answer then the command's own) or another lease ends (`STILL` again).
/// No timer on the host and no polling: each wait is a held call answered at a change, and the lease is the USB driver's own deadline.
pub mod held {
    /// The status byte of a command still at work: ask `caps::HELD` for its end.
    pub const STILL: u8 = 6;
    /// The status byte of a held read with no command held.
    pub const NOTHING: u8 = 7;
    /// How long the device keeps a command's answer before it says `STILL`, in seconds: inside the USB driver's 16 s (payloads/usb), which ends a command the kernel never answers.
    pub const LEASE_SECONDS: u64 = 1 << 3;
}

/// Ring identifiers for the beam command.
pub mod ring_id {
    pub const KERNEL: u8 = 0;
    pub const VAULT_ROOT: u8 = 1;
    pub const LEDGER: u8 = 2;
    pub const STATE: u8 = 3;
}

/// Beam mode: how to interpret the offset parameter.
pub mod beam_mode {
    /// Latest minus offset (~N). offset=0 = newest entry.
    pub const LATEST: u8 = 0;
    /// Absolute generation number.
    pub const ABSOLUTE: u8 = 1;
    /// Dump all entries (dev mode only).
    pub const ALL: u8 = 2;
}
