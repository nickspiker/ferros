//! Cap-addressed command protocol.
//!
//! Every PT payload that represents a command follows this format:
//!
//!   `[cap: 32][op: 1][params...]`
//!
//! `cap` is a BLAKE3 credential — the same hash used by the vault
//! capability system. It addresses the target object AND proves access.
//!
//! In dev mode, well-known "dev caps" are BLAKE3 hashes of fixed strings.
//! The kernel recognizes them without validation. In prod mode, the host
//! authenticates and receives delegated caps — same wire format.
//!
//! Responses use the same cap field (echo back the credential) so the
//! caller can match responses to requests.

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

/// Compute a dev cap hash at runtime. In dev mode these are not validated,
/// but both sides must agree on the same hash for matching.
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
    /// Beam — read ring entries by generation offset.
    /// Params: [ring_id:1][mode:1][offset:4 BE][count:4 BE]
    /// ring_id: 0=kernel, 1=vault_root, 2=ledger, 3=state
    /// mode: 0=latest (~N offset), 1=absolute generation, 2=all
    pub const BEAM: &[u8] = b"ferros.dev.beam";
    /// Install — write sends signed kernel binary, exec persists to UFS + stem entry.
    /// Exec params: [size:4 LE][hash:32][sig:64] = 100 bytes
    pub const INSTALL: &[u8] = b"ferros.dev.install";
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
