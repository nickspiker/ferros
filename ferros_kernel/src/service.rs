//! The ferros USB service: Photon Transport on the device's DWC3, shared by both ways ferros boots on husky.
//!
//! - **Handoff** (slot a, chainloaded from the patched Linux): UFS is live and the ira-rooted vault is open, so STORE works; every exit warm-resets back into Android.
//! - **Cold** (slot b, straight from ABL): no Linux; ferros powers the flash back on and brings UFS up itself (cold_ufs.rs), so the vault opens here too. Exits other than a plain host REBOOT go to fastboot, since a plain reset would just boot ferros again (slot marking via devinfo is not implemented yet).
//!
//! Both paths: the USB DMA master is put on physical addresses (HSI0 S2MPU bypassed, every USB SYSMMU VM context disabled), the eUSB2 PHY + DWC3 are (re)initialised with the enumeration watchdog, and PT serves DIAG / RUN / RELOAD / MEM / DISPLAY / STORE / REBOOT. Exits leave a ramoops report (exit reason in the crumb byte): REBOOT from the host, 64 s without enumerating, 512 s without a PT command, a 16 s mid-transfer stall, or both volume buttons; faults and hangs are caught by the fault sink and the watchdog.
//!
//! RELOAD: the image is statically linked with absolute addresses in its data, so every generation must run at its link base. The new image is staged at `Boot::stage`, a tiny copy stub is placed at `Boot::stub`, and the stub copies the image over the link base and jumps to it; x0 carries the generation (bit 0 = reloaded).

use core::ptr::{read_volatile, write_volatile};

use crate::platform::{self, append_hex, cntfrq, cntpct, open_usb_dma_path, queue_pt_response};
use crate::vault_boot::{report_vault, ShimStore, VaultProbe};

pub const DWC3: usize = 0x1121_0000;

/// Which way ferros got here; decides the DIAG banner and where exits go.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BootKind {
    /// Chainloaded from the patched Linux on slot a (runtime trigger): exits warm-reset back into Android.
    #[cfg_attr(not(shim_handoff), allow(dead_code))]
    Handoff,
    /// Booted straight from ABL on slot b: exits other than a plain host REBOOT go to fastboot.
    Cold,
}

/// Everything the service needs from the boot path that brought ferros up.
pub struct Boot {
    pub kind: BootKind,
    /// Address the image is linked at (absolute pointers in its data assume it); RELOAD's copy stub lands every generation here.
    pub link_base: usize,
    /// RELOAD/RUN staging slot, clear of this image, its stack and its heap.
    pub stage: usize,
    /// Where the RELOAD copy stub runs from (clear of both the image and the staging slot).
    pub stub: usize,
    /// x0 at entry: 0 or a DTB pointer from the loader, or ((generation << 1) | 1) from a RELOAD.
    pub x0: u64,
    pub probe: Option<VaultProbe>,
    pub store: Option<ShimStore>,
    /// The previous session's report block, read before `arm_safety` cleared it (survives warm resets).
    pub prev: Option<[u64; 32]>,
    /// Boot-path specific DIAG lines.
    pub extras: alloc::vec::Vec<(&'static [u8], u32)>,
}

/// What `reboot_to_bootloader` returned when a fastboot request fell through (0 = never tried); reported in r[23].
static FASTBOOT_FAIL: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Largest RELOAD/RUN image the staging slot takes.
const STAGE_MAX: usize = 4 << 20;
/// The largest value the vault takes (3 MiB): sealing a value and reading it back each need about twice its size in one piece of the kernel heap. The largest capsule today is the display server's, 2.8 MB. Larger things (std images) go through the streamed spawn, not the vault, until the vault stores in pieces.
pub const STORE_VALUE_MAX: usize = 3 << 20;

/// Exit reasons (crumb byte in the ramoops report, kernel prints it as the low byte of `mbr_sig`).
const EXIT_REBOOT: u8 = 0xB0;
const EXIT_FASTBOOT: u8 = 0xB3;

/// The session's command counters for DIAG and the exit report.
#[derive(Default)]
struct PtStats {
    /// Commands the USB process handed over (ptlink.rs).
    proc_cmds: u32,
    /// The last command parsed: cap bytes 0..3 (LE) | op << 56 (DIAG PREV_PT_CMD).
    last_cmd: u64,
}


// Reload copy stub. Position-independent; runs from `Boot::stub` after being copied there, so it can overwrite the image that called it.
// x0 = source (`Boot::stage`), x1 = destination (`Boot::link_base`), x2 = byte count (multiple of 8, nonzero), x3 = x0 for the new image.
// MMU is off at EL2 after the handoff, so data stores go straight to DRAM; only the I-side needs invalidating.
core::arch::global_asm!(
    ".section .text.reload_stub, \"ax\"",
    ".global ferros_reload_stub",
    ".global ferros_reload_stub_end",
    ".balign 8",
    "ferros_reload_stub:",
    "    mov x5, x1",
    "1:  ldr x4, [x0], #8",
    "    str x4, [x1], #8",
    "    subs x2, x2, #8",
    "    b.hi 1b",
    "    dsb sy",
    "    ic iallu",
    "    dsb sy",
    "    isb",
    "    mov x0, x3",
    "    br x5",
    "ferros_reload_stub_end:",
);

unsafe extern "C" {
    pub(crate) static ferros_reload_stub: u8;
    pub(crate) static ferros_reload_stub_end: u8;
}

/// Report the vault and the session, then reset. The kernel dumps the whole 256-byte block as r[00..31]; this session's words: r[22] the commands the USB process handed over, r[23] what `reboot_to_bootloader` returned, r[24] the last command (cap bytes 0..3 | op << 56), r[27] the processes alive.
fn leave(b: &Boot, exit: u8, st: &PtStats) -> ! {
    report(b, exit);
    let w = |idx: usize, v: u64| unsafe { write_volatile((0xFD60_0000usize + idx * 8) as *mut u64, v) };
    w(22, st.proc_cmds as u64);
    w(23, FASTBOOT_FAIL.load(core::sync::atomic::Ordering::Relaxed));
    w(24, st.last_cmd);
    w(27, crate::process::alive() as u64);
    unsafe {
        ferros_hal::mmio::cache_clean(0xFD60_0000, 0x100);
        // The USB process's controller stopped, so the host sees a disconnect rather than a dead device.
        let dctl = read_volatile((DWC3 + 0xC704) as *const u32);
        write_volatile((DWC3 + 0xC704) as *mut u32, dctl & !(1 << 31));
    }
    // Cold boot: a plain reset would just boot ferros again, so anything but an explicit host REBOOT goes to fastboot for the host to decide (falls back to a plain reset if the EL3 call is refused).
    if b.kind == BootKind::Cold && exit != EXIT_REBOOT {
        let _ = platform::reboot_to_bootloader();
    }
    platform::psci_reset()
}

/// Write this session's report: the full vault report when a vault was probed, the minimal one otherwise.
fn report(b: &Boot, exit: u8) {
    match &b.probe {
        Some(p) => report_vault(p, exit),
        None => platform::report_exit(exit),
    }
}

/// `ferros.dev.store` on the ira-keyed vault (every set/remove is sealed by commit_root, so it survives a reset). Responses start with a status byte: 0 ok, 1 key not found, 2 vault error, 3 bad request, 4 no vault open, 5 a value over STORE_VALUE_MAX.
/// Write [keylen:2 BE][key][value] -> [0][object hash:32]. Read [key] -> [0][value]; an empty key lists every key, '\n'-separated. Exec [key] -> remove -> [0] or [1].
fn store_command(store: Option<&mut ShimStore>, op: ferros_pt::Op, params: &[u8]) -> alloc::vec::Vec<u8> {
    let store = match store {
        Some(s) => s,
        None => return alloc::vec![4],
    };
    let key_of = |b: &[u8]| core::str::from_utf8(b).ok().map(alloc::string::String::from);
    match op {
        ferros_pt::Op::Write => {
            if params.len() < 2 {
                return alloc::vec![3];
            }
            let klen = u16::from_be_bytes([params[0], params[1]]) as usize;
            if params.len() < 2 + klen {
                return alloc::vec![3];
            }
            let key = match key_of(&params[2..2 + klen]) {
                Some(k) if !k.is_empty() => k,
                _ => return alloc::vec![3],
            };
            let len = params.len() - 2 - klen;
            // A value larger than STORE_VALUE_MAX is refused (status 5): sealing it, and reading it back, each take about twice its size in one piece of the 22 MiB kernel heap, and 4 and 5 MiB values ran it out and panicked the kernel (2026-10-31).
            if len > STORE_VALUE_MAX {
                crate::flashlog::line(&alloc::format!("store set {key:?} {len} bytes refused: over {STORE_VALUE_MAX}"));
                return alloc::vec![5];
            }
            crate::flashlog::line(&alloc::format!("store set {key:?} {len} bytes"));
            crate::process::note_old_put(&key);
            match store.set(&key, &params[2 + klen..]) {
                Ok(h) => {
                    // The settings document set from the bridge applies at once, as a change on the device does (SYS_SET_PREF): every live process sees it on its next frame.
                    if key == crate::settings::KEY {
                        let _ = crate::settings::apply(&params[2 + klen..], true);
                    }
                    // A new zones document (tools/tzvsf): read again at the service loop's next pass.
                    if key == "zones" {
                        crate::zone::table_stale();
                    }
                    crate::flashlog::line(&alloc::format!("store set {key:?} ok: {}", store.describe()));
                    let mut r = alloc::vec![0u8];
                    r.extend_from_slice(&h);
                    r
                }
                Err(e) => {
                    crate::flashlog::line(&alloc::format!("store set {key:?} FAILED: {e:?}"));
                    // Status 2 followed by the vault's own error text, so the bridge can show it.
                    let mut r = alloc::vec![2u8];
                    r.extend_from_slice(alloc::format!("{e:?}").as_bytes());
                    r
                }
            }
        }
        ferros_pt::Op::Read => {
            let key = match key_of(params) {
                Some(k) => k,
                None => return alloc::vec![3],
            };
            if key.is_empty() {
                return match store.keys() {
                    Ok(keys) => {
                        let mut r = alloc::vec![0u8];
                        for k in keys {
                            r.extend_from_slice(k.as_bytes());
                            r.push(b'\n');
                        }
                        r
                    }
                    Err(_) => alloc::vec![2],
                };
            }
            match store.get_by_key(&key) {
                Ok(Some(v)) => {
                    let mut r = alloc::vec![0u8];
                    r.extend_from_slice(&v);
                    r
                }
                Ok(None) => alloc::vec![1],
                Err(e) => {
                    crate::flashlog::line(&alloc::format!("store get {key:?} FAILED: {e:?}"));
                    let mut r = alloc::vec![2u8];
                    r.extend_from_slice(alloc::format!("{e:?}").as_bytes());
                    r
                }
            }
        }
        ferros_pt::Op::Exec => match key_of(params) {
            Some(k) if !k.is_empty() => {
                let r = store.remove_key(&k);
                crate::flashlog::line(&alloc::format!("store remove {k:?}: {r:?}"));
                match r {
                    Ok(true) => alloc::vec![0],
                    Ok(false) => alloc::vec![1],
                    Err(_) => alloc::vec![2],
                }
            }
            _ => alloc::vec![3],
        },
    }
}

/// A bridge `store` command the vault process answers (VAULT-OUT.md: the names it holds), through the kernel's own channel (process.rs `vault_call`), a transfer a call. Its PT reply waits until the last call is answered: the USB process polls for a reply that is not ready yet (ptlink.rs).
#[cfg(not(ferros_production))]
enum VaultJob {
    /// A value read whole: its full name, what has come so far.
    Get { full: alloc::string::String, got: alloc::vec::Vec<u8> },
    /// A value written in pieces from the command's parameters (the command kept beside the job: `vault_cmd` in `run`): its full name (its scope's name, when `addr` names it by a 32-byte address), where the value starts in them, the next offset into it; and for a value larger than one command carries (an installed kist's capsule), where this command's part starts in the value and whether more commands follow.
    Put { full: alloc::string::String, addr: Option<[u8; 32]>, from: usize, at: usize, base: u64, more: bool },
    Remove { full: alloc::string::String },
    /// Every name: the kernel's own vault's first (already in `out`, after its status byte), then each scope's from the vault process in turn, `scope` the one being read, `at` into its listing, `part` its listing so far.
    List { out: alloc::vec::Vec<u8>, scope: usize, at: u64, part: alloc::vec::Vec<u8> },
}

/// One transfer of the kernel's channel (process.rs STORE_MAX, the vault process's one-transfer bound).
#[cfg(not(ferros_production))]
const VAULT_PIECE: usize = crate::process::STORE_MAX;

/// A bridge `store` command for the vault process: Some(Ok(the job, its first call made)) when the vault process serves and the name is one it holds (a scope's, not one the kernel keeps), or the listing; Some(Err(the PT reply)) when the call could not be made; None: the kernel's vault answers, as before.
#[cfg(not(ferros_production))]
fn vault_job_start(store: &mut Option<ShimStore>, op: ferros_pt::Op, params: &[u8]) -> Option<Result<VaultJob, alloc::vec::Vec<u8>>> {
    if !crate::process::vault_serving() {
        return None;
    }
    let name = |b: &[u8]| core::str::from_utf8(b).ok().filter(|k| crate::process::vault_name(k).is_some()).map(alloc::string::String::from);
    let job = match op {
        ferros_pt::Op::Write => {
            // [key length u16 BE][key][value]. Bit 15 of the length: a part of a larger value, [offset u64 LE][more u8] after the key. Bit 14: the key is a scope's prefix and a 32-byte address (an installed kist's capsule, by its blake3).
            let k = u16::from_be_bytes([*params.first()?, *params.get(1)?]);
            let (part, raw, klen) = (k & 1 << 15 != 0, k & 1 << 14 != 0, (k & 0x3FFF) as usize);
            let key = params.get(2..2 + klen)?;
            let (full, addr) = if raw {
                let (prefix, a) = key.split_at(klen.checked_sub(32)?);
                let sc = crate::process::STORES.iter().find(|s| s.prefix.as_bytes() == prefix)?;
                (alloc::string::String::from(sc.name), Some(<[u8; 32]>::try_from(a).ok()?))
            } else {
                (name(key)?, None)
            };
            let (base, more, from) = if part {
                let o = 2 + klen;
                (u64::from_le_bytes(params.get(o..o + 8)?.try_into().ok()?), *params.get(o + 8)? != 0, o + 9)
            } else {
                (0, false, 2 + klen)
            };
            VaultJob::Put { full, addr, from, at: 0, base, more }
        }
        ferros_pt::Op::Read if params.is_empty() => {
            let mut out = alloc::vec![0u8];
            match crate::process::with_lock(|| store.as_mut().map(|s| s.keys())) {
                Some(Ok(keys)) => {
                    for k in keys {
                        out.extend_from_slice(k.as_bytes());
                        out.push(b'\n');
                    }
                }
                Some(Err(_)) => return Some(Err(alloc::vec![2])),
                None => {}
            }
            VaultJob::List { out, scope: 0, at: 0, part: alloc::vec::Vec::new() }
        }
        ferros_pt::Op::Read => VaultJob::Get { full: name(params)?, got: alloc::vec::Vec::new() },
        ferros_pt::Op::Exec => VaultJob::Remove { full: name(params)? },
    };
    Some(vault_job_call(&job, params).map(|_| job))
}

/// The job's next call to the vault process; Err(the PT reply) when it cannot be made.
#[cfg(not(ferros_production))]
fn vault_job_call(job: &VaultJob, params: &[u8]) -> Result<(), alloc::vec::Vec<u8>> {
    use crate::process::{vault_stamp, VOP_DELETE, VOP_GET, VOP_SET};
    let mut rest = alloc::vec::Vec::with_capacity(16 + VAULT_PIECE);
    let msg = match job {
        VaultJob::Get { full, got } => {
            rest.extend_from_slice(&(got.len() as u64).to_le_bytes());
            rest.extend_from_slice(&(VAULT_PIECE as u32).to_le_bytes());
            vault_stamp(VOP_GET, full, &rest)
        }
        VaultJob::Put { full, addr, from, at, base, more } => {
            let value = &params[*from..];
            let end = (at + VAULT_PIECE).min(value.len());
            rest.extend_from_slice(&(base + *at as u64).to_le_bytes());
            rest.push((end < value.len() || *more) as u8);
            rest.extend_from_slice(&value[*at..end]);
            match addr {
                Some(a) => crate::process::vault_stamp_addr(VOP_SET, full, a, &rest),
                None => vault_stamp(VOP_SET, full, &rest),
            }
        }
        VaultJob::Remove { full } => vault_stamp(VOP_DELETE, full, &[]),
        VaultJob::List { scope, at, .. } => crate::process::vault_stamp_list(*scope, *at, VAULT_PIECE as u32).map(|(m, _)| m),
    };
    rest.fill(0);
    let Some(msg) = msg else {
        let mut r = alloc::vec![2u8];
        r.extend_from_slice(b"locked: the person's entries open only while they are present");
        return Err(r);
    };
    if crate::process::vault_call(msg) {
        Ok(())
    } else {
        let mut r = alloc::vec![4u8];
        r.extend_from_slice(b"the vault process is gone, or the kernel's channel to it is busy");
        Err(r)
    }
}

/// One step of a job: the answer to its last call, if it has come, then its next call. Some(the PT reply) when it is done. A get or a remove the vault process does not find goes to the kernel's vault, where a name not yet moved still is.
#[cfg(not(ferros_production))]
fn vault_job_step(job: &mut VaultJob, params: &[u8], store: &mut Option<ShimStore>) -> Option<alloc::vec::Vec<u8>> {
    use crate::process::{ERR, NOTHING};
    let (value, bytes) = match crate::process::vault_answer()? {
        Ok(a) => a,
        Err(()) => return Some(alloc::vec![4]),
    };
    match job {
        VaultJob::Get { full, got } => match value {
            NOTHING => return Some(crate::process::with_lock(|| store_command(store.as_mut(), ferros_pt::Op::Read, full.as_bytes()))),
            ERR => return Some(alloc::vec![2]),
            // The reply is built whole in the kernel's heap, and copied once more as it goes out: held to the limit the kernel's vault keeps for the same reason (STORE_VALUE_MAX).
            whole if whole as usize > STORE_VALUE_MAX => return Some(alloc::vec![5]),
            whole => {
                if got.is_empty() {
                    got.reserve_exact(whole as usize);
                }
                // A piece that brings nothing while the value is not whole would ask for the same piece for ever: the vault's error.
                if bytes.is_empty() && (got.len() as u64) < whole {
                    return Some(alloc::vec![2]);
                }
                got.extend_from_slice(&bytes);
                if got.len() as u64 >= whole {
                    let mut r = alloc::vec::Vec::with_capacity(1 + got.len());
                    r.push(0);
                    r.extend_from_slice(got);
                    got.fill(0);
                    return Some(r);
                }
            }
        },
        VaultJob::Put { from, at, .. } => {
            if value != 0 {
                return Some(alloc::vec![2]);
            }
            let len = params.len() - *from;
            *at = (*at + VAULT_PIECE).min(len);
            if *at >= len {
                return Some(alloc::vec![0]);
            }
        }
        VaultJob::Remove { full } => {
            return Some(match value {
                0 => alloc::vec![0],
                NOTHING => crate::process::with_lock(|| store_command(store.as_mut(), ferros_pt::Op::Exec, full.as_bytes())),
                _ => alloc::vec![2],
            });
        }
        VaultJob::List { out, scope, at, part } => {
            if value == ERR {
                let mut r = alloc::vec![2u8];
                r.extend_from_slice(alloc::format!("listing scope {} failed", crate::process::STORES[*scope].name).as_bytes());
                return Some(r);
            }
            if bytes.is_empty() && (part.len() as u64) < value {
                return Some(alloc::vec![2]);
            }
            part.extend_from_slice(&bytes);
            *at = part.len() as u64;
            if *at >= value {
                // The scope's listing whole: each name made whole with its scope's prefix.
                let prefix = crate::process::STORES[*scope].prefix;
                for line in part.split(|&b| b == b'\n').filter(|l| !l.is_empty()) {
                    out.extend_from_slice(prefix.as_bytes());
                    out.extend_from_slice(line);
                    out.push(b'\n');
                }
                part.clear();
                *at = 0;
                *scope += 1;
                if *scope == crate::process::STORES.len() {
                    return Some(core::mem::take(out));
                }
            }
        }
    };
    vault_job_call(job, params).err()
}

/// A kist's capsule streamed from the vault process through the kernel's channel a transfer a call: the first transfers into `head` until the code's header is in (`kist_need` sizes the arc from it, masked or native as its code is), then straight into the arc at the code offset, as the bridge's streamed spawn lands its pieces. For a launch init asked for (process.rs `kist_request`), `spawn_kist` then checks it (a masked kist against its seal in `sealed`) and builds the process. For an install the installer asked for (`install_request`), the checker and the seal follow, a stage a pass, each told to the installer (`install_tell`).
struct KistJob {
    why: KistWhy,
    capsule: [u8; 32],
    /// The `sealed` list as the job began (32-byte seals): what a masked kist's launch is checked against.
    sealed: alloc::vec::Vec<u8>,
    total: usize,
    head: alloc::vec::Vec<u8>,
    arc: Option<crate::process::Arc>,
    got: usize,
    t0: u64,
    /// Its first bytes are in and its arc waits for one that is coming back (`kist_arc`).
    waiting: bool,
    /// An install read whole and checked, waiting for its seal on the next pass: the gate's word on it, its modes and its code's span.
    checked: Option<(crate::oracle::Located, crate::oracle::Modes, usize, usize)>,
    /// An install read whole, waiting for the checker on the next pass.
    read: bool,
    /// An install refused and already told why.
    told: bool,
}

enum KistWhy {
    Launch(crate::process::KistLaunch),
    Install(crate::process::KistInstall),
}

/// What a kist job came to: a launch built, or an install's seal for the `sealed` list.
enum KistDone {
    Launched,
    Sealed([u8; 32]),
}

use crate::process::KistFault;
use ferros_kist::launch::why;

/// The kist job's next read (from `got`); false when the channel would not take it.
fn kist_call(job: &KistJob) -> bool {
    let mut rest = [0u8; 12];
    rest[..8].copy_from_slice(&(job.got as u64).to_le_bytes());
    rest[8..].copy_from_slice(&(crate::process::STORE_MAX as u32).to_le_bytes());
    match crate::process::vault_stamp_addr(crate::process::VOP_GET, "capsules", &job.capsule, &rest) {
        Some(msg) => crate::process::vault_call(msg),
        None => false,
    }
}

/// The kist's arc, sized from its first bytes (`kist_need`), masked when its code is (the guard above it), with those bytes copied in. Ok(false): the pool has no room yet while a process ends (a relaunch's own predecessor still leaving a core, its arc coming back at its next trap), so the job waits for that, not for a time; Err when there is no room and nothing is coming back.
fn kist_arc(job: &mut KistJob) -> Result<bool, KistFault> {
    let (need, ceiling) = crate::process::kist_need(&job.head, job.total).map_err(|e| (why::READ, e))?;
    let masked = crate::process::kist_masked(&job.head, job.total);
    let Some(arc) = crate::process::with_lock(|| crate::process::arc_alloc_room(need, ceiling, masked)) else {
        return if crate::process::any_dying() { Ok(false) } else { Err((why::ROOM, "the arc pool has no room for the kist")) };
    };
    if crate::process::CODE + job.total > arc.size {
        crate::process::with_lock(|| crate::process::arc_free(arc));
        return Err((why::OTHER, "the kist's arc is smaller than its capsule"));
    }
    unsafe { core::ptr::copy_nonoverlapping(job.head.as_ptr(), (arc.base + crate::process::CODE) as *mut u8, job.head.len()) };
    job.arc = Some(arc);
    job.head = alloc::vec::Vec::new();
    Ok(true)
}

/// An install told a stage, if it is one.
fn kist_tell(job: &KistJob, stage: u64, value: u64) {
    if let KistWhy::Install(i) = &job.why {
        crate::process::install_tell(i, stage, value);
    }
}

/// The kist job after its last bytes or its arc: a launch's process built once the capsule is whole; an install told it is read and its checker due on the next pass; else the next read.
fn kist_next(job: &mut KistJob) -> Option<Result<KistDone, KistFault>> {
    if job.got >= job.total {
        let Some(arc) = job.arc.take() else { return Some(Err((why::READ, "the capsule ended before its code"))) };
        return match &job.why {
            KistWhy::Launch(l) => Some(crate::process::spawn_kist(l, arc, job.total, &job.sealed).map(|()| KistDone::Launched)),
            KistWhy::Install(_) => {
                job.arc = Some(arc);
                job.read = true;
                kist_tell(job, crate::process::INSTALL_CHECKING, 0);
                // The checker on the next pass, so the installer shows the stage before the work: the loop goes round at once (no wait for an event).
                unsafe { core::arch::asm!("sev") };
                None
            }
        };
    }
    if !kist_call(job) {
        return Some(Err((why::READ, "the kernel's channel to the vault process would not take the next read")));
    }
    None
}

/// An install's stage after its reading, one a pass: the checker (SANDBOX.md: once, at install), then the seal. Done: its seal, for the service loop to keep in `sealed`.
fn install_step(job: &mut KistJob) -> Option<Result<KistDone, KistFault>> {
    let arc = job.arc?;
    let code = match job.checked.take() {
        None => match crate::process::install_check(&arc, job.total, &job.capsule) {
            Ok(c) => {
                job.checked = Some(c);
                kist_tell(job, crate::process::INSTALL_SEALING, 0);
                unsafe { core::arch::asm!("sev") };
                return None;
            }
            Err(c) => c,
        },
        Some((g, m, co, cl)) => match crate::process::install_seal(&arc, &g, &m, co, cl) {
            Ok(seal) => {
                job.arc = None;
                crate::process::with_lock(|| crate::process::arc_free(arc));
                return Some(Ok(KistDone::Sealed(seal)));
            }
            Err(c) => c,
        },
    };
    kist_tell(job, crate::process::INSTALL_REFUSED, code);
    job.told = true;
    Some(Err((why::CHECKS, "refused (the reason is on the console)")))
}

/// One step of the kist job: waiting for its arc, an install's next stage once it is read, or the answer to its last read if it has come, then its next read. Some(the outcome) once it is done; the arc is the process's or back in the pool by then.
fn kist_step(job: &mut KistJob) -> Option<Result<KistDone, KistFault>> {
    use crate::process::{ERR, NOTHING};
    let r = (|| {
        if job.read {
            return install_step(job);
        }
        if job.waiting {
            return match kist_arc(job) {
                Ok(true) => {
                    job.waiting = false;
                    kist_next(job)
                }
                Ok(false) => None,
                Err(e) => Some(Err(e)),
            };
        }
        let (value, bytes) = match crate::process::vault_answer()? {
            Ok(a) => a,
            Err(()) => return Some(Err((why::READ, "the vault process ended while the capsule was read"))),
        };
        match value {
            NOTHING => return Some(Err((why::NOT_HERE, "no such capsule in the vault (install it first)"))),
            ERR => return Some(Err((why::READ, "the vault process could not read the capsule"))),
            _ => {}
        }
        // WHY the bounds: the length is the vault's word; a capsule of nothing, or past the gigabyte the arc pool can hold, is refused before any arc is asked for.
        if value == 0 || value > 1 << 30 || (job.total != 0 && value as usize != job.total) {
            return Some(Err((why::READ, "the capsule's length is not one an arc can hold, or it changed while it was read")));
        }
        job.total = value as usize;
        // WHY: the piece is the vault's; past the capsule's length it would land past the staging the arc was sized for, and nothing at all while the capsule is not whole would ask for the same piece for ever.
        if job.got + bytes.len() > job.total || (bytes.is_empty() && job.got < job.total) {
            return Some(Err((why::READ, "the vault process answered past the capsule's end, or nothing")));
        }
        let before = job.got;
        match job.arc {
            None => {
                job.head.extend_from_slice(&bytes);
                job.got += bytes.len();
                if job.head.len() >= job.total.min(1 << 16) {
                    match kist_arc(job) {
                        Ok(true) => {}
                        Ok(false) => {
                            job.waiting = true;
                            return None;
                        }
                        Err(e) => return Some(Err(e)),
                    }
                }
            }
            Some(arc) => {
                unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), (arc.base + crate::process::CODE + job.got) as *mut u8, bytes.len()) };
                job.got += bytes.len();
            }
        }
        // The installer's progress bar: told at each mebibyte read, not each transfer.
        if before >> 20 != job.got >> 20 {
            kist_tell(job, crate::process::INSTALL_READING, job.got as u64);
        }
        kist_next(job)
    })();
    if r.as_ref().is_some_and(|x| x.is_err()) {
        if let Some(arc) = job.arc.take() {
            crate::process::with_lock(|| crate::process::arc_free(arc));
        }
    }
    r
}

/// The boot screen: boot path, generation, vault state + root, and how to get out.
fn status_text(kind: BootKind, generation: u64, probe: Option<&VaultProbe>) -> alloc::vec::Vec<u8> {
    let mut t = alloc::vec::Vec::new();
    let hex = b"0123456789ABCDEF";
    t.extend_from_slice(match kind {
        BootKind::Handoff => b"ferros\n\nchainloaded from Linux, generation G#".as_slice(),
        BootKind::Cold => b"ferros\n\nbooted from ABL, generation G#",
    });
    for i in (0..4).rev() {
        t.push(hex[((generation >> (i * 4)) & 0xF) as usize]);
    }
    let p = match probe {
        Some(p) => p,
        None => {
            t.extend_from_slice(b"\n\nufs: no vault probe\n\nusb: serving Photon Transport\n     1209:4665\n\nboth volume keys\n= power off\n");
            return t;
        }
    };
    t.extend_from_slice(b"\n\nufs: ");
    t.extend_from_slice(if p.link_up { b"link up" } else { b"LINK DOWN" });
    t.extend_from_slice(b"\nvault: ");
    t.extend_from_slice(if p.verified == 1 { b"opened + verified".as_slice() } else if p.sealed == 1 { b"genesis sealed" } else if p.opened == 1 { b"opened, NOT verified" } else { b"NOT OPENED" });
    t.extend_from_slice(b"\nroot: ");
    for &b in &p.root16[..8] {
        t.push(hex[(b >> 4) as usize]);
        t.push(hex[(b & 0xF) as usize]);
    }
    t.extend_from_slice(b"\nwairua: ");
    t.extend_from_slice(match crate::wairua::peek() { None => b"not drawn yet".as_slice(), Some((true, _)) => b"live", Some((false, _)) => b"KOHI FAILED" });
    t.extend_from_slice(b"\n\nusb: serving Photon Transport\n     1209:4665\n\nboth volume keys\n= power off\n");
    t
}

/// Shim entry with the USB service: adopt UFS, arm the safety nets, probe the vault, then serve PT over USB until told to leave.
/// The service loop's vault, lent to `SYS_SPAWN` (process.rs `CAPSULES`) for reading capsules by key: `run` never returns, so the pointer stays good for the rest of the boot.
static mut SPAWN_STORE: *mut Option<ShimStore> = core::ptr::null_mut();

/// Android from within ferros, behind an attest (owner, 2026-11-01 and 2026-11-04: "launch Android from within ferros so it's gated behind an attest", from the home screen). The device's opt-in, the vault's device-class `android.allowed` (one byte, 1), as the service loop read it at start: 2 allowed, 1 not, 0 not read yet (SYS_FACTS 82, so the map shows the Android circle only on a device that allows it). Written by the opt-in (the disclaimer, next) and for now from the host.
pub static ANDROID_ALLOWED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// A launch the `android` switch accepted, for the service loop's next pass.
static ANDROID_REQ: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// A completed departure released the identity (SYS_UNLOCK op 4): the service loop restarts the device, which comes back at the attest screen (AUTH.md's "no way back to the attest screen" gap, closed the plain way).
static RELEASE_RESET: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// The drops of the released person's domains in the vault process (process.rs `vault_stamp_drops`), each a stamp carrying that domain's keys, waiting for the service loop to send them before the restart. Under the kernel lock (filled in a syscall, taken by the loop).
static mut RELEASE_DROPS: alloc::vec::Vec<alloc::vec::Vec<u8>> = alloc::vec::Vec::new();

/// The `android` switch (process.rs POWERS): accepted only on a device that allows Android and while a person is present (the vault's identity class unlocked: the kernel's own state, set by an attest through SYS_UNLOCK, never a process's word). Nothing in the base OS holds it (owner, 2026-11-04: nothing is built in but system settings): it is for an Android launcher a person installs, and this check is the lock whoever holds it. The service loop then does the gate's Android path: ANDROID_ONCE, slot a, the reset with its scrub (`diag android`'s sequence).
pub fn android_request(on: bool) -> bool {
    let present = unsafe { (*(&raw mut SPAWN_STORE)).as_mut() }.and_then(|s| s.as_mut()).is_some_and(|s| s.unlocked());
    let ok = on && present && ANDROID_ALLOWED.load(core::sync::atomic::Ordering::Relaxed) == 2;
    crate::flashlog::line(if ok { "android: launch accepted (attested, allowed)" } else if !present { "android: launch refused: no one has attested" } else { "android: launch refused: this device does not allow Android" });
    if ok {
        ANDROID_REQ.store(true, core::sync::atomic::Ordering::Release);
    }
    ok
}

/// `SYS_SET_PREF`'s persistence: a preference into the device vault under its key (SETTINGS.md).
fn store_pref(index: usize, value: u8) -> bool {
    if !crate::settings::FIELDS.iter().any(|&(_, i)| i == index) {
        return false;
    }
    let Some(doc) = crate::settings::document(Some((index, value))) else { return false };
    unsafe { (*(&raw mut SPAWN_STORE)).as_mut().and_then(|s| s.as_mut()).map(|s| s.set(crate::settings::KEY, &doc).is_ok()).unwrap_or(false) }
}

/// The settings document as it stands (the radio knobs and the zone with it: SYS_CONTROL), into the vault.
fn store_document() -> bool {
    let Some(doc) = crate::settings::document(None) else { return false };
    unsafe { (*(&raw mut SPAWN_STORE)).as_mut().and_then(|s| s.as_mut()).map(|s| s.set(crate::settings::KEY, &doc).is_ok()).unwrap_or(false) }
}

/// `SYS_STORE`'s vault: the kernel has already checked the capability and prefixed the key with its scope.
fn store_scoped(key: &str, value: Option<&[u8]>) -> Result<Option<alloc::vec::Vec<u8>>, ()> {
    let st = unsafe { (*(&raw mut SPAWN_STORE)).as_mut() }.and_then(|s| s.as_mut()).ok_or(())?;
    match value {
        Some(v) => {
            crate::flashlog::line(&alloc::format!("store set {key:?} {} bytes (process)", v.len()));
            // Written here while no vault process serves (it is opening, or not started): it moves there later.
            crate::process::note_old_put(key);
            st.set(key, v).map(|_| None).map_err(|_| ())
        }
        None => st.get_by_key(key).map_err(|_| ()),
    }
}

/// The base of a class's vault domains (process.rs `domain_keys`, VAULT-OUT.md): the device's from the ira, derived once; the person's from their unlock, None while locked.
pub(crate) fn domain_base(identity: bool) -> Option<[u8; 32]> {
    if identity {
        return unsafe { (*(&raw mut SPAWN_STORE)).as_mut() }.and_then(|s| s.as_mut())?.identity_domain();
    }
    let base = unsafe { &mut *(&raw mut DEVICE_DOMAIN) };
    if base.is_none() {
        *base = crate::ira::derive(DEVICE_DOMAIN_CONTEXT);
    }
    *base
}
const DEVICE_DOMAIN_CONTEXT: &str = "ferros 2026-11-07 device vault domain";
static mut DEVICE_DOMAIN: Option<[u8; 32]> = None;

/// Before any reset the kernel orders: the identity's key dropped (AUTH.md: it locks when power goes or the device switches OS, and a warm reset keeps DRAM).
fn lock_before_reset() {
    if let Some(st) = unsafe { (*(&raw mut SPAWN_STORE)).as_mut() }.and_then(|s| s.as_mut()) {
        st.lock();
    }
}

/// The identity class's lock (AUTH.md): op 0 asks (bit 0 unlocked, bit 1 bound), 1 unlocks from a seed (the number of entries the identity has on this device, or NOTHING: the device is bound to another identity), 2 locks, 3 binds the device to the unlocked identity, 4 releases it (every identity entry deleted).
fn store_unlock(op: u8, seed: Option<&[u8; 32]>) -> Result<u64, ()> {
    let st = unsafe { (*(&raw mut SPAWN_STORE)).as_mut() }.and_then(|s| s.as_mut()).ok_or(())?;
    match (op, seed) {
        (1, Some(s)) => match st.unlock(s, crate::process::pref(crate::process::PREF_UNLOCK_AT_BOOT) == 1) {
            Ok(()) => {
                crate::flashlog::line("identity unlocked");
                Ok(st.identity_entries() as u64)
            }
            Err(crate::kvault::KvError::Belongs) => {
                crate::flashlog::line("identity refused: this device is bound to another identity");
                Ok(crate::process::NOTHING)
            }
            Err(e) => {
                crate::flashlog::line(&alloc::format!("identity unlock failed: {e:?}"));
                Err(())
            }
        },
        (2, _) => {
            st.lock();
            crate::flashlog::line("identity locked");
            Ok(0)
        }
        (3, _) => {
            st.bind().map_err(|_| ())?;
            crate::flashlog::line("identity bound: this device now opens only for it");
            Ok(0)
        }
        (4, _) => {
            // The person's domains in the vault process, stamped while their keys still exist (release locks them); the service loop sends the drops before its restart.
            let mut drops = crate::process::vault_stamp_drops();
            let n = match st.release() {
                Ok(n) => n,
                Err(_) => {
                    for d in drops.iter_mut() {
                        d.fill(0);
                    }
                    return Err(());
                }
            };
            crate::flashlog::line(&alloc::format!("identity released: {n} entries deleted from the kernel's vault, the device-only secret renewed; {} of the person's domains to drop in the vault process", drops.len()));
            // The person's zone goes with them (zone.rs): this device's own choice rules again.
            let _ = crate::zone::set_identity(None, || crate::settings::document(None).is_some_and(|d| st.set(crate::settings::KEY, &d).is_ok()));
            unsafe { (*(&raw mut RELEASE_DROPS)).append(&mut drops) };
            RELEASE_RESET.store(true, core::sync::atomic::Ordering::Release);
            Ok(0)
        }
        _ => Ok(st.unlocked() as u64 | (st.bound().unwrap_or(false) as u64) << 1),
    }
}


/// The identity's own entries on this device as one VSF document, for the identity service to seal into its fleet before a departure (SYS_UNLOCK op 5; the Fleet page's "save first"): section `identity_export`, one `entry` field per vault name, its values the name (`d`) and the stored bytes (`v'r'`). None while locked.
fn store_export() -> Option<alloc::vec::Vec<u8>> {
    let st = unsafe { (*(&raw mut SPAWN_STORE)).as_mut() }.and_then(|s| s.as_mut())?;
    let entries = st.identity_export().ok()?;
    let mut section = vsf::VsfSection::new("identity_export");
    for (name, value) in &entries {
        // A name is a `d`, ASCII: one that is not cannot be carried, and the export fails whole rather than leaving it behind unsaid.
        let Ok(n) = vsf::Ascii::new(name.clone()) else {
            crate::flashlog::line(&alloc::format!("identity export refused: a name that is not ASCII ({} bytes)", name.len()));
            return None;
        };
        section.add_field_multi("entry", alloc::vec![vsf::VsfType::d(n), vsf::VsfType::v(b'r', value.clone())]);
    }
    crate::flashlog::line(&alloc::format!("identity export: {} entries", entries.len()));
    vsf::VsfBuilder::new().creation_time_oscillations(crate::settings::eagle_now()).provenance_only().add_section_direct(section).build().ok()
}
fn store_remove(key: &str) -> Result<bool, ()> {
    let st = unsafe { (*(&raw mut SPAWN_STORE)).as_mut() }.and_then(|s| s.as_mut()).ok_or(())?;
    crate::flashlog::line(&alloc::format!("store remove {key:?} (process)"));
    st.remove_key(key).map_err(|_| ())
}

fn store_keys() -> Result<alloc::vec::Vec<alloc::string::String>, ()> {
    let st = unsafe { (*(&raw mut SPAWN_STORE)).as_mut() }.and_then(|s| s.as_mut()).ok_or(())?;
    st.keys().map_err(|_| ())
}

fn store_addr(prefix: &str, addr: &[u8; 32], op: crate::process::AddrOp) -> Result<Option<alloc::vec::Vec<u8>>, ()> {
    use crate::process::AddrOp;
    let st = unsafe { (*(&raw mut SPAWN_STORE)).as_mut() }.and_then(|s| s.as_mut()).ok_or(())?;
    match op {
        AddrOp::Get => st.get_addr(prefix, addr).map_err(|_| ()),
        AddrOp::Set(v) => st.set_addr(prefix, addr, v).map(|_| None).map_err(|_| ()),
        AddrOp::Remove => st.remove_addr(prefix, addr).map(|was| was.then_some(alloc::vec::Vec::new())).map_err(|_| ()),
        AddrOp::List => st.addrs(prefix).map(Some).map_err(|_| ()),
    }
}

fn read_capsule(key: &[u8]) -> Option<alloc::vec::Vec<u8>> {
    let k = core::str::from_utf8(key).ok()?;
    match unsafe { (*(&raw mut SPAWN_STORE)).as_mut()?.as_mut()?.get_by_key(k) } {
        Ok(v) => v,
        // A read that failed is not an absent key: say which (2026-10-24, touch "not in the vault" at boot, found by a restart minutes later).
        Err(e) => {
            crate::flashlog::line(&alloc::format!("spawn: reading {k:?} failed: {e:?}"));
            None
        }
    }
}

/// The boot checklist's process rows, from what a spawned process did in its first slice (the host's spawn over USB, or init at boot — which marked nothing until 2026-10-29, so a boot without a host left them red): an arc and a spawn, EL0 reached unless it faulted at entry (exit G#FA…), a syscall and its capabilities once it said something, IPC once a call came back; and the first process itself.
/// A kernel lock call site (process.rs `site_word`) as `serv* line A#1234`: the four bytes of the file's name it keeps, then the line.
fn site_text(w: u64) -> alloc::string::String {
    let tag: alloc::string::String = ((w >> 24) as u32).to_le_bytes().iter().map(|&c| if c.is_ascii_graphic() { c as char } else { '?' }).collect();
    alloc::format!("{tag}* line A#{}", w & 0xFF_FFFF)
}

/// DIAG's decoded lines for the lock and the stall catcher: the longest hold since boot and its site, the previous boot's last holder (note slot 0), and the stall records (platform::stall_check), this boot's and the previous boot's. Addresses are the kernel's own (`aarch64-linux-gnu-addr2line -e target/aarch64-unknown-none/release/ferros_kernel`) or, at EL0 (SPSR mode 0), the process's.
fn stall_lines(freq: u64) -> alloc::string::String {
    use core::sync::atomic::Ordering::Relaxed;
    let mut s = alloc::format!("lock: held longest A#{} us, taken at {}\n", crate::process::LOCK_HOLD_MAX.load(Relaxed) * 1_000_000 / freq, site_text(crate::process::LOCK_HOLD_SITE.load(Relaxed)));
    let prev_lock = platform::note_read(8);
    if prev_lock & 0xFF != 0 {
        s.push_str(&alloc::format!("lock at the previous boot's end: held by core A#{}, taken at {}\n", (prev_lock & 0xFF) - 1, site_text(prev_lock >> 8)));
    }
    for (prev, what) in [(false, "this boot"), (true, "the previous boot")] {
        let w = platform::stall_read(prev);
        if w[0] & 0xFF == 0 {
            continue;
        }
        let holder = (w[0] >> 40) & 0xFF;
        let lock = if holder == 0 { alloc::string::String::from("the lock free") } else { alloc::format!("the lock held by core A#{} from {}", holder - 1, site_text(w[4] >> 8)) };
        let at = if w[1] == 0 { alloc::string::String::from("it did not answer (interrupts masked)") } else { alloc::format!("ELR G#{:X} LR G#{:X} SPSR G#{:X}", w[1], w[2], w[3]) };
        s.push_str(&alloc::format!("stall ({what}, A#{} caught): core A#{} silent A#{} ms after the last pet, caught by core A#{}, {lock}; {at}\n", w[0] >> 48, (w[0] & 0xFF) - 1, (w[0] >> 16) & 0xFF_FFFF, (w[0] >> 8) & 0xFF));
    }
    s
}

fn mark_ran(o: &crate::process::Outcome) {
    use crate::checklist::{mark, Item};
    mark(Item::Arcs);
    mark(Item::Spawn);
    if o.exit >> 56 != 0xFA {
        mark(Item::El0);
        mark(Item::FirstProcess);
    }
    if !o.console.is_empty() {
        mark(Item::Syscall);
        mark(Item::Caps);
    }
    if o.round_trips > 0 {
        mark(Item::Ipc);
    }
}

/// What init holds: everything the shell's three processes need, in the slot order payloads/init expects (its header). The kernel grants it once; init hands each process its share.
fn home_grants() -> alloc::vec::Vec<crate::process::Grant<'static>> {
    use crate::process::Grant::*;
    alloc::vec![
        Serve(b"display"),
        Hardware(b"decon0-main"), Hardware(b"decon0-win"), Hardware(b"decon0-wincon"), Hardware(b"dpu-dma0"), Hardware(b"dpp0"), Hardware(b"gpio-gpp0"),
        Dma(b"display-fb"), Dma(b"display-heap"),
        Hardware(b"dsim0"), Hardware(b"dphy0"), Hardware(b"sramc0"), Hardware(b"hdrcomm0"),
        Serve(b"touch"), Hardware(b"usi2-spi"), Hardware(b"gpio-gpn0"),
        Use(b"display"), Use(b"touch"),
        Settings,
        Store(b"map"),
        Dma(b"display-phases"),
        Dma(b"surfaces"),
        // 23 the touch controller's interrupt line, 25 DECON0's frame-start (KERNEL.md "Interrupts to user space"). 24 was its frame-done, which no process took: since 2026-11-05 `use gpu`, a client's (the attest screen's).
        Irq(b"touch-int"), Use(b"gpu"), Irq(b"decon0-frame-start"),
        // 26 the display's sprite cache, 27 its canvas.
        Dma(b"display-sprites"),
        Dma(b"display-canvas"),
        // The network (WIFI.md): 28 serve wlan; 29-40 the WiFi driver's hardware (PCIe ch1's blocks, the HSI2 and far-alive GPIO banks, CMU_HSI2, the PMU through EL3); 41 entropy (the driver's firmware seed, the IP stack's choices); 42 the driver's DMA region; 43 the vault scope `wifi.`; 44 use wlan; 45 serve udp.
        Serve(b"wlan"),
        Hardware(b"pcie1-elbi"), Hardware(b"pcie1-soc"), Hardware(b"pcie1-pcs"), Hardware(b"pcie1-udbg"), Hardware(b"pcie1-phy"), Hardware(b"pcie1-dbi"), Hardware(b"pcie1-cfg"), Hardware(b"pcie1-mem"),
        Hardware(b"gpio-hsi2"), Hardware(b"gpio-far-alive"), Hardware(b"cmu-hsi2"), Hardware(b"pmu"),
        Entropy,
        Dma(b"pcie1-dma"),
        Store(b"wifi"),
        Use(b"wlan"),
        Serve(b"udp"),
        // fgtw, the OS's identity service (payloads/fgtw; part of attest, not the Photon app): 46 serve fgtw, 47 use fgtw (the attest screen's: the verdict on an identity, the mint), 48 the vault scope `fgtw.`, 49 the ira-derived key, 50 use udp (the service's).
        Serve(b"fgtw"),
        Use(b"fgtw"),
        Store(b"fgtw"),
        Ira,
        Use(b"udp"),
        // 51 the attest screen's vault scope `ahau.` (payloads/ahau).
        Store(b"ahau"),
        // 52 the cellular link, the IP process's second (served by the modem driver, payloads/modem).
        Use(b"rmnet"),
        // The modem driver (payloads/modem): 53 serve rmnet; 54-68 its hardware (PCIe ch0, HSI1, the alive GPIO bank, the PMU through EL3); 69-71 its shared-memory windows; 72-76 the partitions it reads (its image, NV, handover signature, carrier configs).
        Serve(b"rmnet"),
        Hardware(b"pcie0-elbi"), Hardware(b"pcie0-soc"), Hardware(b"pcie0-pcs"), Hardware(b"pcie0-udbg"), Hardware(b"pcie0-phy"), Hardware(b"pcie0-dbi"), Hardware(b"pcie0-cfg"), Hardware(b"pcie0-mem"),
        Hardware(b"cmu-hsi1"), Hardware(b"hsi1-sysreg"), Hardware(b"gpio-hsi1"), Hardware(b"sysmmu-hsi1"), Hardware(b"s2mpu-hsi1"), Hardware(b"gpio-alive"),
        // 68: was the PMU a second time (the modem takes slot 40's, the same grant, since 2026-11-05); now the layer arena (GPU.md "Layers"), for the GPU service and its clients. The grants page holds 108 slots (process.rs PREFS at G#FC0), so slots are reused, never appended past 108.
        Dma(b"layers"),
        Dma(b"modem-pktproc"), Dma(b"modem-ipc"), Dma(b"modem-msi"),
        // 72 and 76, the modem and vendor partitions of the slot Android runs from: akita's is slot b (its Bluetooth patch read from there, 2026-11-09), husky's slot a.
        Part(if crate::board::akita() { b"modem_b" } else { b"modem_a" }), Part(b"efs"), Part(b"persist"), Part(b"modem_userdata"), Part(if crate::board::akita() { b"vendor_b" } else { b"vendor_a" }),
        // The NFC driver (payloads/nfc, docs/NFC.md): 77 serve nfc, 78 hsi2c_0, 79 its power; its interrupt line is read in gpio-alive (67). 80 use nfc (Photon's, the tap's secret).
        Serve(b"nfc"), Hardware(b"hsi2c0"), Power(b"nfc"), Use(b"nfc"),
        // The Bluetooth driver (payloads/bt, docs/BT.md): 81 serve bt, 82 the APM UART, 83 its power; the patch from vendor_a (76), entropy (41). 84 use bt (Photon's, the beacon).
        Serve(b"bt"), Hardware(b"apm-usi1-uart"), Power(b"bt"), Use(b"bt"),
        // 85 the identity unlock right (AUTH.md): the attest screen unlocks the person's space with it; init asks it whether a person is present before it starts what reads that space.
        Unlock,
        // The USB driver (payloads/usb, USB.md): 86 the controller's registers, 87 its DMA memory, 88 its interrupt, 89 the kernel's PT commands, 90 the `usb` power switch (the controller from the kernel).
        Dma(b"usb-regs"), Dma(b"usb-dma"), Irq(b"usb-dwc3"), Pt, Power(b"usb"),
        // 91 the touch controller's power (clocks, rails, reset: touch.rs `power`); the touch process brings the controller up with it.
        Power(b"touch"),
        // The sensors process (payloads/sensors, KERNEL-OUT.md move 4): 92 the AoC's IPC block, 93 its mailboxes, 94 the vault scope `aoc.` (the registry scripts), 95 the Sensors right (its published block), 96 the `sensors` switch (the AoC's start).
        Dma(b"aoc-ipc"), Hardware(b"aoc-mbox"), Store(b"aoc"), Sensors, Power(b"sensors"),
        // The display server's modeset (KERNEL-OUT.md decision 6): 97 DECON0's sub block (the DSC encoders, the DSIM interface select), 98 its DQE, 99 the DQE's CGC, 100 the DPU sysreg (the D-PHY's reset).
        Hardware(b"decon0-sub"), Hardware(b"dqe0"), Hardware(b"dqe0-cgc"), Hardware(b"dpu-sysreg"),
        // 101 the `dpu` switch (dpu_power.rs): the display server powers the DPU up from off (DISPLAY-DESIGN.md §8).
        Power(b"dpu"),
        // 102 the `panel` switch (panel_power.rs: VCI and the reset pulse), 103 `panel-vddd` (VDDD after the init set): the panel from off (§8, state B).
        Power(b"panel"), Power(b"panel-vddd"),
        // Kists (KISTS.md, 2026-11-08): 104 the right to launch them (each right a kist is granted is made by the kernel from it: init holds no other grant for them), 105 serve `launch` (the map asks init to launch one), 106 use `launch` (the map's), 107 the vault scope `kists.` (the person's list and answers: init reads it, the map shows it and writes the answers). These were the GPU service's until it got its own list (`gpu_grants`), started by the kernel as the vault and GPS are: the page holds 108 slots.
        Kists, Serve(b"launch"), Use(b"launch"), Store(b"kists"),
        // 108 use `kists`, the installer's endpoint (payloads/kists, which the kernel starts with its own list, `kists_grants`): the map's Kists page installs, updates and uninstalls through it. The last slot the page holds (0 the console, 1 to 108).
        Use(b"kists"),
        // The `android` switch (android_request) is in no one's list: the base OS has no Android launcher (owner, 2026-11-04: nothing built in but system settings). It goes to a launcher a person installs, once installing exists.
    ]
}

/// The person's wallpaper seed from their last unlock (kvault `unlock`), waiting for the service loop to start wallgen with it.
static mut WALLGEN_SEED: Option<[u8; 32]> = None;

pub fn wallgen_after_unlock(seed: [u8; 32]) {
    unsafe { WALLGEN_SEED = Some(seed) };
}

/// wallgen (payloads/wallgen) with the person's wallpaper seed as its input, from its capsule in the kernel's vault: it makes their wallpaper once on this device and keeps it in their space, or ends at once if it already has. 1 the vault scope `map.`, 2 use display. Started from the service loop, after an unlock (an unlock is a syscall, under the kernel's lock: no spawn there).
fn spawn_wallgen(store: &mut Option<ShimStore>, seed: &[u8; 32]) {
    // Under the kernel lock, as every UFS command must be: the vault process drives the same controller from the workers (SYS_BLOCK), and a read outside the lock collided with its moves at an attest and wedged the link (2026-11-07).
    let Some(capsule) = crate::process::with_lock(|| store.as_mut().and_then(|s| s.get_by_key("wallgen").ok().flatten())) else {
        crate::process::push_console(b"home: no wallgen capsule in the vault; the display draws the wallpaper at every attest\n");
        return;
    };
    let sealed = crate::process::with_lock(|| store.as_mut().and_then(|s| s.get_by_key("sealed").ok().flatten())).unwrap_or_default();
    use crate::process::Grant::*;
    match crate::process::spawn_capsule(b"wallgen", &capsule, seed, &[Store(b"map"), Use(b"display")], &sealed) {
        Ok(o) => {
            mark_ran(&o);
            crate::process::push_console(&o.console);
        }
        Err(e) => crate::process::push_console(alloc::format!("home: wallgen refused: {e}\n").as_bytes()),
    }
}

/// The vault process's capsule the kernel image carries (FERROS_VAULT_CAPSULE at build; VAULT-OUT.md decision 0), empty when the build carried none. The boot starts the vault from it first; a restart from the stored one first (the development loop pushes one).
#[cfg(ferros_vault_capsule)]
static VAULT_CARRIED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/vault.capsule.vsf"));
#[cfg(not(ferros_vault_capsule))]
static VAULT_CARRIED: &[u8] = &[];

/// The vault process's capabilities (payloads/vault: 1 serve `vault`, 2 the `vault` block window, 3 entropy, 4 the vault right). Not init's to hand out: the kernel starts it before init (VAULT-OUT.md, decision 0), and init's grants page is full.
fn vault_grants() -> alloc::vec::Vec<crate::process::Grant<'static>> {
    use crate::process::Grant::*;
    alloc::vec![Serve(b"vault"), Blocks(b"vault"), Entropy, Vault]
}

/// The GPS driver's grants in its slot order (payloads/gps): 1 serve gps, 2 the SPI port, 3 gpio-far-alive, 4 gpio-gpp0, 5 gpio-hsi2, 6 the vault scope `gps.`, 7 the power switch, 8 HOST_REQ's interrupt. Its own list, as the vault's: init's page is full at 108, and the kernel starts this driver itself, only once a person has attested (radios at attest, GPS locked until then).
fn gps_grants() -> alloc::vec::Vec<crate::process::Grant<'static>> {
    use crate::process::Grant::*;
    alloc::vec![Serve(b"gps"), Hardware(b"usi3-spi"), Hardware(b"gpio-far-alive"), Hardware(b"gpio-gpp0"), Hardware(b"gpio-hsi2"), Store(b"gps"), Power(b"gps"), Irq(b"gps-host-req")]
}

/// The GPU service's grants in its slot order (payloads/gpu): 1 its registers, 2 its memory, 3 the vault scope `gpu.` (its firmware), 4 its job interrupt, 5 the layer arena, 6 serve gpu. Its own list since 2026-11-08, as the vault's and the GPS driver's: init's page gave its five slots to kists, and the kernel starts the GPU service at home, before init (so before the attest screen, its first client).
fn gpu_grants() -> alloc::vec::Vec<crate::process::Grant<'static>> {
    use crate::process::Grant::*;
    alloc::vec![Dma(b"gpu-regs"), Dma(b"gpu-mem"), Store(b"gpu"), Irq(b"gpu-job"), Dma(b"layers"), Serve(b"gpu")]
}

/// The installer's grants in its slot order (payloads/kists; docs/KISTS.md "Install"): 1 use udp (the network, plain HTTP), 2 the vault scope `capsules.` (it stores what it fetched, by content address), 3 the vault scope `kists.` (the person's list), 4 serve `kists` (the map's Kists page), 5 use fgtw (the identity service's doorbell: a restore follows the person's space), 6 the Install right (the kernel checks and seals what it stored). Its own list, as the GPS driver's: init's page is full, and the kernel starts it once a person has attested (the list is theirs).
fn kists_grants() -> alloc::vec::Vec<crate::process::Grant<'static>> {
    use crate::process::Grant::*;
    alloc::vec![Use(b"udp"), Store(b"capsules"), Store(b"kists"), Serve(b"kists"), Use(b"fgtw"), Install]
}

/// Each shell process's capabilities, as init hands them out (payloads/init: the slots of `home_grants`, 1-based), for restarting one from the vault without a reboot.
fn shell_grants(name: &[u8]) -> Option<alloc::vec::Vec<crate::process::Grant<'static>>> {
    if name == b"vault" {
        return Some(vault_grants());
    }
    if name == b"kists" {
        return Some(kists_grants());
    }
    if name == b"gps" {
        return Some(gps_grants());
    }
    if name == b"gpu" {
        return Some(gpu_grants());
    }
    let slots: &[usize] = match name {
        // The same slots init hands out (payloads/init): a restarted server must see exactly what a booted one does (the display's vsync thread had no interrupt grant after a restart and never started, 2026-10-23).
        b"display" => &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 21, 22, 25, 26, 27, 97, 98, 99, 100, 101, 102, 103, 20],
        b"touch" => &[14, 15, 16, 23, 91],
        // 1 use display, 2 use touch, 3 settings, 4 the vault scope `map.`, 5 use fgtw (the Fleet page), 6 the vault scope `kists.` (the person's kists and answers), 7 use launch, 8 use kists (the installer: the Kists page) (payloads/map).
        b"map" => &[17, 18, 19, 20, 47, 107, 106, 108],
        b"pcie1" => &[29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 28, 43, 49, 83],
        b"ip" => &[44, 45, 41, 52],
        // The modem driver: serve rmnet, its hardware, windows and partitions (payloads/modem's slot order).
        b"modem" => &[53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 40, 69, 70, 71, 72, 73, 74, 75, 76],
        // 1 use udp, 2 the vault scope, 3 the ira key, 4 serve fgtw, 5 entropy (the fleet-key minting), 6 use nfc, 7 use bt, 8 the identity unlock right, 9 the vault scope `map.`, 10 the vault scope `kists.` (payloads/fgtw; as init gives them: without 9 and 10 a restarted fgtw could not join the identity space, "the person's list is not readable", 2026-11-10).
        b"fgtw" => &[50, 48, 49, 46, 41, 80, 84, 85, 20, 107],
        // 1 serve nfc, 2 hsi2c_0, 3 gpio-alive, 4 its power (payloads/nfc).
        b"nfc" => &[77, 78, 67, 79],
        // 1 serve bt, 2 the APM UART, 3 vendor_a, 4 its power, 5 entropy (payloads/bt).
        b"bt" => &[81, 82, 76, 83, 41],
        // 1 use display, 2 use touch, 3 a surface, 4 the vault scope `ahau.`, 5 the vault scope `fgtw.`, 6 the ira key, 7 settings (the layout switch), 8 use fgtw (the identity service's verdict and the mint), 9 the identity unlock right, 10 a second surface, the keyboard's back buffer, 11 use gpu, 12 the layer arena, 13 use wlan (the network page) (payloads/ahau; as init gives them: without 13 a restarted attest screen's network page said "The WiFi is starting." forever, 2026-11-10).
        b"ahau" => &[17, 18, 22, 51, 48, 49, 19, 47, 85, 22, 24, 68, 44],
        // 1 the controller's registers, 2 its DMA memory, 3 its interrupt, 4 the kernel's PT commands, 5 the `usb` power switch (payloads/usb).
        b"usb" => &[86, 87, 88, 89, 90],
        // 1 the AoC's IPC block, 2 its mailboxes, 3 the vault scope `aoc.`, 4 the Sensors right, 5 the `sensors` switch (payloads/sensors).
        b"sensors" => &[92, 93, 94, 95, 96],
        _ => return None,
    };
    let all = home_grants();
    // The attest screen's two surfaces as init's spawn hands them out (SYS_SPAWN): each a free surface of the four, its own. Passed as the whole region, a restarted attest screen drew into surface 0 whoever held it, and its back buffer was its own front (2026-11-01: every board redrew in place, black between). Not for the display server, which takes the region's base (surface 0 at boot).
    let own = name == b"ahau";
    Some(slots.iter().filter_map(|&s| all.get(s - 1).copied()).map(|g| match g {
        crate::process::Grant::Dma(b"surfaces") if own => crate::process::Grant::Dma(b"surface"),
        g => g,
    }).collect())
}

/// `diag restart <name>…`: each named shell process killed and started again from its capsule in the vault, in the order given (display before map: the map hands a fresh display server its scene at start). The development loop for the shell — build, `store put`, restart — without a reboot.
fn restart(store: &mut Option<ShimStore>, names: &[u8], rep: &mut alloc::vec::Vec<u8>) {
    // Vault reads under the kernel lock (the vault's one owner at a time); the spawns below take it themselves.
    let sealed = crate::process::with_lock(|| store.as_mut().and_then(|s| s.get_by_key("sealed").ok().flatten())).unwrap_or_default();
    for name in names.split(|&b| b == b' ').filter(|n| !n.is_empty()) {
        let Some(grants) = shell_grants(name) else {
            rep.extend_from_slice(b"not a shell process: ");
            rep.extend_from_slice(name);
            rep.push(b'\n');
            continue;
        };
        // What reads the person's space starts only when init's `wait_person` would start it (owner, 2026-11-04: "Attest should ONLY load attest", and "if it's not attached to an identity, not usable"): fgtw once a person has attested, the map once the device is also bound to that identity. A restart does not get around either. A vault that cannot say whether it is bound counts as not bound.
        let person = store.as_mut().is_some_and(|s| s.unlocked());
        // Under the kernel lock: `bound` reads the vault (a UFS command), and processes run on the other cores.
        let attached = person && crate::process::with_lock(|| store.as_mut().is_some_and(|s| s.bound().unwrap_or(false)));
        let refused: Option<&[u8]> = match name {
            b"fgtw" if !person => Some(b"not started, nobody has attested: "),
            b"gps" if !person => Some(b"not started, GPS is locked until a person attests: "),
            b"kists" if !person => Some(b"not started, nobody has attested (the installer keeps a person's list): "),
            b"map" if !attached => Some(b"not started, this device is not bound to an attested identity: "),
            _ => None,
        };
        if let Some(why) = refused {
            rep.extend_from_slice(why);
            rep.extend_from_slice(name);
            rep.push(b'\n');
            continue;
        }
        // Each step in the flash log and the crumb (G#51 killing, G#52 reading, G#53 spawning, G#54 done — G#70+ are the syscalls; 2026-10-25: a reset followed a display restart with nothing recorded after the vault took the new build).
        let nm = core::str::from_utf8(name).unwrap_or("?");
        crate::flashlog::line(&alloc::format!("restart {nm}: killing"));
        platform::crumb(0x51);
        let killed = crate::process::kill(name);
        // What it left switched on goes off before its successor starts, so the new process finds it off rather than switched off under it.
        crate::process::power_offs_apply();
        crate::flashlog::line(&alloc::format!("restart {nm}: {} ; reading the capsule", if killed { "killed" } else { "was not running" }));
        platform::crumb(0x52);
        // The vault restarts from the stored capsule (the development loop pushes one), else from the one the kernel image carries.
        let stored = crate::process::with_lock(|| store.as_mut().and_then(|s| s.get_by_key(core::str::from_utf8(name).unwrap_or("")).ok().flatten()));
        let Some(capsule) = stored.or_else(|| (name == b"vault" && !VAULT_CARRIED.is_empty()).then(|| VAULT_CARRIED.to_vec())) else {
            rep.extend_from_slice(b"no capsule in the vault for ");
            rep.extend_from_slice(name);
            rep.push(b'\n');
            continue;
        };
        crate::flashlog::line(&alloc::format!("restart {nm}: capsule {} bytes; spawning", capsule.len()));
        platform::crumb(0x53);
        match crate::process::spawn_capsule(name, &capsule, b"", &grants, &sealed) {
            Ok(o) => {
                crate::process::push_console(&o.console);
                rep.extend_from_slice(alloc::format!("{} {}\n", core::str::from_utf8(name).unwrap_or("?"), if killed { "restarted" } else { "started" }).as_bytes());
                crate::flashlog::line(&alloc::format!("restart {nm}: started"));
            }
            Err(e) => {
                rep.extend_from_slice(alloc::format!("{}: {e}\n", core::str::from_utf8(name).unwrap_or("?")).as_bytes());
                crate::flashlog::line(&alloc::format!("restart {nm}: {e}"));
            }
        }
        platform::crumb(0x54);
    }
}

/// The ring has completed: bring the touch controller up (the kernel's part, as `diag touch up` does) and start the vault's `init`, which starts the shell. Its console goes to `diag console`.
/// Returns false when the shell is off this boot (`home.off`).
fn start_home(store: &mut Option<ShimStore>) -> bool {
    crate::checklist::HOME.store(cntpct(), core::sync::atomic::Ordering::Relaxed);
    // FERROS_RESCUE (2026-11-03): the vault's usb, pcie1 and bt capsules replaced by the builds this kernel carries, whatever is stored (a rescue kernel is flashed only on purpose, and a broken build of either kind leaves the bridge dead). A masked build of a driver that reaches its registers by raw pointers dies at once, and with usb dead the bridge cannot put the capsule back: the chicken-and-egg this switch exists for. Flash it once, let it mend the vault, flash the ordinary kernel again.
    #[cfg(ferros_rescue)]
    if let Some(s) = store.as_mut() {
        for (name, bytes) in [("usb", include_bytes!("../../target/rescue-usb.capsule.vsf") as &[u8]), ("pcie1", include_bytes!("../../target/rescue-pcie1.capsule.vsf") as &[u8]), ("bt", include_bytes!("../../target/rescue-bt.capsule.vsf") as &[u8])] {
            let r = s.set(name, bytes);
            crate::process::push_console(alloc::format!("rescue: the vault's {name} replaced by the kernel's known-good capsule ({})\n", if r.is_ok() { "ok" } else { "failed" }).as_bytes());
        }
    }
    // akita (board.rs) without `init` in its vault (its first boots, or no vault): the USB process alone, from the native capsule this kernel carries, so the bridge reaches the device and can store the shell. With `init` stored, akita takes husky's path below (since 2026-11-08: its vault and ira since 2026-11-06; init leaves out the drivers whose hardware is not husky's there, payloads/init `unfit`).
    let akita_bare = crate::board::akita() && crate::process::with_lock(|| store.as_mut().and_then(|s| s.get_by_key("init").ok().flatten())).is_none();
    if akita_bare {
        #[cfg(ferros_usb_capsule)]
        // Without a vault there is no ira and so no oracle secret: the capsule is checked by its signature and runs without an ihi (spawn_capsule would refuse it: "no ira: no system secret, no ihi", the first akita boots, 2026-11-06).
        match shell_grants(b"usb").map(|g| crate::process::spawn_capsule_no_ihi(b"usb", include_bytes!("../../target/akita-usb.capsule.vsf"), b"", &g)) {
            Some(Ok(o)) => {
                mark_ran(&o);
                crate::process::push_console(b"home: akita, the usb process from the kernel's capsule\n");
                crate::checklist::dot(0);
                crate::board::probe(7);
                crate::board::probe_starve(9);
                crate::process::push_console(&o.console);
            }
            Some(Err(e)) => {
                crate::process::push_console(b"home: akita, usb refused: ");
                crate::board::probe(71);
                crate::process::push_console(e.as_bytes());
                crate::process::push_console(b"\n");
            }
            None => {
                crate::process::push_console(b"home: akita, no usb grants\n");
                crate::board::probe(72);
            }
        }
        #[cfg(not(ferros_usb_capsule))]
        crate::process::push_console(b"home: akita, but this kernel carries no usb capsule (target/akita-usb.capsule.vsf)\n");
        return true;
    }
    // `home.off` in the vault (any value): no shell this boot, so the panel is the bridge's alone — a measurement session (tools/display-profile.sh) must not have the map redraw because an instrument rests on the glass. Removed with `store rm home.off`.
    // Under the kernel lock, as every UFS command (nothing else runs yet, but the rule has no exceptions).
    if crate::process::with_lock(|| store.as_mut().and_then(|s| s.get_by_key("home.off").ok().flatten())).is_some() {
        crate::process::push_console(b"home: off (vault home.off): no shell this boot\n");
        return false;
    }
    let Some(capsule) = crate::process::with_lock(|| store.as_mut().and_then(|s| s.get_by_key("init").ok().flatten())) else {
        crate::process::push_console(b"home: no init capsule in the vault\n");
        return true;
    };
    let sealed = crate::process::with_lock(|| store.as_mut().and_then(|s| s.get_by_key("sealed").ok().flatten())).unwrap_or_default();
    // The vault process first (VAULT-OUT.md, decision 0), so it is serving when the processes init starts ask for what they keep; until it is, the kernel's vault answers them. From the capsule the kernel image carries (FERROS_VAULT_CAPSULE at build), else the `vault` entry of the kernel's own vault (kept by the kernel: never forwarded, never moved). Neither: every store call is the kernel's vault's, as before.
    let mut vault_up = false;
    if !VAULT_CARRIED.is_empty() {
        match crate::process::spawn_capsule(b"vault", VAULT_CARRIED, b"", &vault_grants(), &sealed) {
            Ok(o) => {
                mark_ran(&o);
                crate::process::push_console(&o.console);
                crate::process::push_console(b"home: vault from the kernel image\n");
                vault_up = true;
            }
            Err(e) => crate::process::push_console(alloc::format!("home: the kernel image's vault refused: {e}; the stored one next\n").as_bytes()),
        }
    }
    if !vault_up {
        if let Some(vc) = crate::process::with_lock(|| store.as_mut().and_then(|s| s.get_by_key("vault").ok().flatten())) {
            match crate::process::spawn_capsule(b"vault", &vc, b"", &vault_grants(), &sealed) {
                Ok(o) => {
                    mark_ran(&o);
                    crate::process::push_console(&o.console);
                }
                Err(e) => crate::process::push_console(alloc::format!("home: vault refused: {e}\n").as_bytes()),
            }
        }
    }
    // The GPU service, from its capsule in the vault, with its own grants (`gpu_grants`): before init, so it serves before the attest screen asks it. Not after a jump from Android: Android ran the GPU, and the attest screen draws on the CPU without it (init skipped it there too).
    if !platform::jumped() {
        match crate::process::with_lock(|| store.as_mut().and_then(|s| s.get_by_key("gpu").ok().flatten())) {
            Some(gc) => match crate::process::spawn_capsule(b"gpu", &gc, b"", &gpu_grants(), &sealed) {
                Ok(o) => {
                    mark_ran(&o);
                    crate::process::push_console(&o.console);
                }
                Err(e) => crate::process::push_console(alloc::format!("home: gpu refused: {e}\n").as_bytes()),
            },
            None => crate::process::push_console(b"home: no gpu capsule in the vault\n"),
        }
    }
    // The pipe stays at ABL's 8 bpc here: the display server takes it to the 10 bpc blend at its start (payloads/display modeset.rs; KERNEL-OUT.md decision 6, 2026-11-01).
    // The touch controller is the touch process's to bring up (its `touch` power switch, then the ready event and scan mode: payloads/touch; KERNEL-OUT.md move 1).
    match crate::process::spawn_capsule(b"init", &capsule, b"", &home_grants(), &sealed) {
        Ok(o) => {
            mark_ran(&o);
            // The processes init started are its spawn syscalls, each handing out capabilities.
            if crate::process::alive() > 0 {
                crate::checklist::mark(crate::checklist::Item::Syscall);
                crate::checklist::mark(crate::checklist::Item::Caps);
            }
            crate::process::push_console(&o.console)
        }
        Err(e) => {
            crate::process::push_console(b"home: init refused: ");
            crate::process::push_console(e.as_bytes());
            crate::process::push_console(b"\n");
        }
    }
    crate::checklist::at("home: init ran");
    true
}

/// The USB link as of the service loop's last pass, for the facts.
static USB_LINK: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// The last battery sample and when it was taken: the gauge is read at most every two seconds, however often a dashboard asks.
static mut GAUGE: ([u64; 4], u64) = ([crate::process::FACT_UNKNOWN; 4], 0);

/// SYS_FACTS (process.rs): 0 press to home µs; 1 battery %; 2 battery temperature, hundredths of a degree C (i64); 3 cell voltage µV; 4 battery current µA, positive = charging (i64); 5 USB link up (0/1); 6 processes alive; 7 keys in the vault; 8 the wall clock's base — unix seconds × counter frequency − the counter, so unix time = (base + counter) / frequency (rtc.rs; unknown until the PMIC's clock has been read); 9 the time zone, seconds east of UTC (i64); 31 how this boot began (the hv gate's answer, 6 = the jump from Android, 0 = not through the hv); the rest unknown.
fn facts() -> [u64; crate::process::FACT_COUNT] {
    use core::sync::atomic::Ordering::Relaxed;
    let mut v = [crate::process::FACT_UNKNOWN; crate::process::FACT_COUNT];
    let f = cntfrq();
    let (press, home) = (crate::checklist::PRESS.load(Relaxed), crate::checklist::HOME.load(Relaxed));
    if press != 0 && home > press {
        v[0] = (home - press) * 1_000_000 / f;
    }
    // The gauge as the service loop last sampled it (gauge_poll): never I2C here — this runs inside a syscall under the kernel lock, on whichever core called, and a gauge read that hung there held the lock and froze the X3 (2026-10-23, twice).
    unsafe {
        let g = &*(&raw const GAUGE);
        v[1..5].copy_from_slice(&g.0);
    }
    v[5] = USB_LINK.load(Relaxed) as u64;
    // Inside a syscall the kernel lock is held: the lock-free count, and the vault's key count as the service loop last read it (the vault is the service loop's, on its own core).
    v[6] = crate::process::alive_locked() as u64;
    let keys = VAULT_KEYS.load(Relaxed);
    if keys != u64::MAX {
        v[7] = keys;
    }
    let base = crate::rtc::BASE.load(Relaxed);
    if base != 0 {
        v[8] = base;
    }
    v[9] = crate::rtc::TZ.load(Relaxed) as u64;
    let hottest = THERMAL.iter().map(|t| t.load(Relaxed)).max().unwrap_or(0);
    if hottest != 0 {
        v[10] = hottest as u64;
    }
    v[11] = (THROTTLED_FROM.load(Relaxed) != 0) as u64;
    // How this boot began: the hv gate's answer (ready.rs GATE_TELEMETRY low byte: 1 power, 2 timer, 3 volume down, 4 recovery refused, 5 the chord's reset, 6 the jump from a running Android), 0 when not through the hv. A driver whose hardware Android may have left running reads it (the modem, after a jump).
    v[31] = crate::ready::GATE_TELEMETRY.load(Relaxed) & 0xFF;
    // 30: whether the DPU's power domains are on (2) or down (1: Android's DECON hibernation at a jump, or ferros's own `dpu off`; never touch DECON or DSIM then, the EL3 poison rule in display.rs, until the `dpu` switch has powered it). Read through EL3, which is safe.
    v[30] = 1 + crate::display::dpu_on() as u64;
    // 32: the radios' drivers running, a bit each (the corners' circles, WINDOWS.md): 0 modem (cellular), 1 pcie1 (WiFi), 2 bt, 3 nfc, 4 gps, 5 ip.
    v[32] = crate::process::named_alive_locked(&[b"modem", b"pcie1", b"bt", b"nfc", b"gps", b"ip"]);
    // 83: whether a person is present (an identity unlocked: 2 yes, 1 no; kvault PERSON), so the radios' drivers can refuse what only a person may start while locked (AUTH.md "Radios while locked").
    v[83] = 1 + crate::kvault::PERSON.load(core::sync::atomic::Ordering::Acquire) as u64;
    // 85: GPS's claims (process.rs GPS_CLAIMS, SYS_CLAIM): bits 0-15 how many processes are using it now, bits 16 and up a count of every change. The console draws satellites only while the driver runs, which is while this is not zero; the settings pages read who is using it again when it moves.
    v[85] = crate::process::GPS_CLAIMS.load(core::sync::atomic::Ordering::Acquire);
    // 84: which board (ferros_kernel board.rs): 1 husky (Pixel 8 Pro), 2 akita (Pixel 8a). For the drivers whose part differs between them (the touch controller).
    v[84] = 1 + crate::board::akita() as u64;
    // 82: whether this device allows Android from within ferros (ANDROID_ALLOWED: 2 yes, 1 no).
    v[82] = match ANDROID_ALLOWED.load(Relaxed) { 0 => crate::process::FACT_UNKNOWN, s => s };
    // 81: the panel's VCI (panel_power.rs): 2 on, 1 off, unknown until the service loop has read it.
    v[81] = match crate::panel_power::VCI.load(Relaxed) { 0 => crate::process::FACT_UNKNOWN, s => s };
    // 33-80: those drivers' own measures (SYS_STATUS), eight tagged words each, in the same order, then the usb driver's.
    for i in 0..6 {
        for w in 0..8 {
            v[33 + 8 * i + w] = crate::process::STATUS[i][w].load(Relaxed);
        }
    }
    // The sensors process's block when it publishes one (sensorlink.rs, KERNEL-OUT.md move 4); with none, the sensor words stay unknown (the kernel has no USF of its own since 2026-11-01).
    if crate::sensorlink::live() {
        use crate::sensorlink::{word, GRAVITY_XY, GRAVITY_Z, PAIR, PROX, SPECTRAL};
        let w = |i: usize| word(i).unwrap_or(0);
        if w(GRAVITY_Z) >> 32 != 0 {
            v[12] = w(GRAVITY_XY);
            v[13] = w(GRAVITY_Z);
            if w(SPECTRAL + 6) != 0 {
                for i in 0..7 {
                    v[20 + i] = w(SPECTRAL + i);
                }
            }
            if w(PROX + 2) >> 32 != 0 {
                for i in 0..3 {
                    v[27 + i] = w(PROX + i);
                }
            }
            if w(PAIR + 5) != 0 {
                for i in 0..6 {
                    v[14 + i] = w(PAIR + i);
                }
            }
        }
        return v;
    }
    v
}

/// The AoC's start as background work of the service loop, a step a pass, so the X3 never stops serving and drawing for the 1.4 s it takes whole: the rails, the firmware out of the vault a 2 MiB piece at a time, GSA's load and the Trusty start, then polling until the firmware is online. Asked by the `sensors` power switch (sensorlink.rs `START`): the sensors process, which init starts, waits for the AoC online and then speaks USF itself. Nothing after online is the kernel's (decision 2, 2026-11-01).
enum SensorBoot {
    Wait,
    Pieces(crate::aoc::Loader),
    Online(u64),
    Done,
}

/// The AoC starts no sooner than this long after the service loop does: the home screen first.
const SENSORS_AFTER_S: u64 = 4;

/// One step of the AoC's start; what it said goes to the flash log.
fn sensor_step(st: SensorBoot, now: u64, get: &mut dyn FnMut(&str) -> Option<alloc::vec::Vec<u8>>) -> SensorBoot {
    let f = cntfrq();
    let mut out = alloc::vec::Vec::new();
    let next = match st {
        SensorBoot::Wait => {
            crate::aoc::rails(&mut out);
            match crate::aoc::gsa_state() {
                // Running already (a warm reload, or a sensors process started again): nothing to do.
                Some(2) if crate::aoc::online() => {
                    out.extend_from_slice(b"AoC online already\n");
                    SensorBoot::Done
                }
                Some(0) => match crate::aoc::Loader::begin(&mut out) {
                    Some(l) => SensorBoot::Pieces(l),
                    None => SensorBoot::Done,
                },
                s => {
                    out.extend_from_slice(alloc::format!("GSA AoC state {s:?}: not starting\n").as_bytes());
                    SensorBoot::Done
                }
            }
        }
        SensorBoot::Pieces(mut l) => match l.piece(get, &mut out) {
            Ok(true) => SensorBoot::Pieces(l),
            Ok(false) if l.finish(&mut out) && crate::trusty::aoc_start(&mut out, 0) => SensorBoot::Online(now),
            _ => SensorBoot::Done,
        },
        SensorBoot::Online(t0) if crate::aoc::online() => {
            out.extend_from_slice(alloc::format!("AoC online after A#{} ms\n", now.wrapping_sub(t0) * 1000 / f).as_bytes());
            SensorBoot::Done
        }
        SensorBoot::Online(t0) if now.wrapping_sub(t0) > 4 * f => {
            out.extend_from_slice(b"AoC not online after 4 s\n");
            SensorBoot::Done
        }
        other => other,
    };
    for l in out.split(|&c| c == b'\n').filter(|l| !l.is_empty()) {
        if let Ok(t) = core::str::from_utf8(l) {
            crate::flashlog::line(&alloc::format!("sensors: {t}"));
        }
    }
    next
}

/// Each thermal zone's temperature, whole °C, as the service loop last read it (thermal_poll; acpm::TMU_ZONES order); 0 = never read.
pub static THERMAL: [core::sync::atomic::AtomicU8; 7] = [const { core::sync::atomic::AtomicU8::new(0) }; 7];
/// The X3's rate before the thermal throttle took it down, kHz; 0 = not throttled.
static THROTTLED_FROM: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// The CPU temperature at which the X3 is taken down (°C); it gets its rate back below this less 10. Linux's control temperature for the big cluster is 95 (zuma's `BIG` thermal zone, its passive trip), its "hot" 115. `diag thermal at <C>` moves it, to test the throttle on a cool device; every boot starts at 95.
static THROTTLE_AT: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(95);
/// The X3's rate under the throttle, kHz: half its top rate, the firmware takes it as asked (measured 2026-10-23, the core then counts ~1297 M dependent adds/s).
const THROTTLE_KHZ: u32 = 1_400_000;

/// From the service loop on the kernel's core, once a second: every thermal zone over ACPM (28 us each, measured), for fact 10 and `diag thermal`; and the throttle — the hottest CPU zone (big, mid, little) at THROTTLE_AT takes the X3 (CPUCL2, the kernel's own core and the one that spins) to THROTTLE_KHZ, and 10 below gives it back. The mid and little clusters already run at 1.065 and 1.425 GHz and are left alone.
fn thermal_poll() {
    use core::sync::atomic::Ordering::Relaxed;
    static LAST: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let (f, now) = (cntfrq(), cntpct());
    let last = LAST.load(Relaxed);
    if last != 0 && now.wrapping_sub(last) < f {
        return;
    }
    LAST.store(now, Relaxed);
    for (i, t) in THERMAL.iter().enumerate() {
        if let Ok((c, _, _)) = crate::acpm::tmu_read(i as u32) {
            t.store(c, Relaxed);
        }
    }
    let cpu = THERMAL[..3].iter().map(|t| t.load(Relaxed)).max().unwrap_or(0);
    let from = THROTTLED_FROM.load(Relaxed);
    let at = THROTTLE_AT.load(Relaxed);
    if from == 0 && cpu >= at {
        let was = crate::acpm::dvfs_get(4).unwrap_or(2_910_000);
        if crate::acpm::dvfs_set(4, THROTTLE_KHZ).is_ok() {
            THROTTLED_FROM.store(was.max(1), Relaxed);
        }
    } else if from != 0 && cpu < at.saturating_sub(10) && crate::acpm::dvfs_set(4, from).is_ok() {
        THROTTLED_FROM.store(0, Relaxed);
    }
    bell_facts();
}

/// The vault's key count for fact 7, kept by the service loop (gauge_poll).
static VAULT_KEYS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(u64::MAX);
static VAULT_KEYS_AT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// A power measurement that does not disturb what it measures (2026-10-25: the old `display power` busy-waited between samples on the kernel's core, so every reading carried the X3 spinning flat out and the display stalled behind it). `diag power start <s> [gap 1/16 s] [input]` cuts the charger (and with `input` deselects CHGIN too) and returns at once; the service loop takes a gauge sample on its ordinary passes whenever one is due, from half a second after the cut (the gauge's own settling), and puts the charger back at the deadline whatever the host does. `diag power` reads the result. Fields: start, settle end, end, next sample, gap (ticks), charger mode before, input before, current sum (µA), voltage sum (µV), samples, min and max current (µA).
struct PowerWin {
    settle: u64,
    end: u64,
    next: u64,
    gap: u64,
    charger_prev: Option<u8>,
    input_prev: Option<u8>,
    sum_ua: i64,
    sum_uv: u64,
    n: u32,
    min_ua: i32,
    max_ua: i32,
    secs: u64,
    /// The clock policy's state at each sample (CLOCK_STATE: 0 not applied, 1 rest, 2 active, 3 dark), so a reading says what it measured.
    states: [u32; 4],
}
static mut POWER_WIN: Option<PowerWin> = None;
static mut POWER_RESULT: Option<alloc::string::String> = None;

fn power_start(secs: u64, gap16: u64, input: bool, delay: u64) -> alloc::string::String {
    let (f, now) = (cntfrq(), cntpct());
    if unsafe { (*(&raw const POWER_WIN)).is_some() } {
        return "a window is already running\n".into();
    }
    let secs = secs.clamp(1, 64);
    let mut msg = alloc::string::String::new();
    let charger_prev = match crate::hsi2c::charger::set_mode(0) {
        Ok(p) => Some(p),
        Err(e) => {
            msg += &alloc::format!("charger mode write failed: G#{e:X}; measuring as is\n");
            None
        }
    };
    let input_prev = if input && charger_prev.is_some() { crate::hsi2c::charger::input_select(false).ok() } else { None };
    // `after <n>`: sampling starts n seconds later (up to 120), so the cable can come out first.
    let settle = now + f / 2 + delay.min(120) * f;
    unsafe {
        POWER_RESULT = None;
        POWER_WIN = Some(PowerWin { settle, end: settle + secs * f, next: settle, gap: (gap16.clamp(1, 64) * f / 16).max(1), charger_prev, input_prev, sum_ua: 0, sum_uv: 0, n: 0, min_ua: i32::MAX, max_ua: i32::MIN, secs, states: [0; 4] });
    }
    msg += &alloc::format!("power window: {secs} s from half a second after the charger cut{}; read it with `diag power`\n", if input_prev.is_some() { ", CHGIN deselected" } else { "" });
    msg
}

/// From the service loop: the running power window's sample when one is due, and its end.
fn power_poll() {
    let Some(w) = (unsafe { (*(&raw mut POWER_WIN)).as_mut() }) else { return };
    let now = cntpct();
    if now >= w.next && now < w.end {
        w.next = now.max(w.next) + w.gap;
        if let Ok(g) = crate::hsi2c::gauge::sample() {
            w.sum_ua += g.current_ua as i64;
            w.sum_uv += g.vcell_uv as u64;
            w.n += 1;
            w.min_ua = w.min_ua.min(g.current_ua);
            w.max_ua = w.max_ua.max(g.current_ua);
            // CLOCK_STATE is only ever 0 to 3 (clock_profile's `want`): indexed as is.
            w.states[unsafe { CLOCK_STATE } as usize] += 1;
        }
    }
    if now >= w.end {
        let mut r = alloc::string::String::new();
        if let Some(p) = w.input_prev {
            if let Err(e) = crate::hsi2c::charger::restore_input(p) {
                r += &alloc::format!("CHARGER INPUT RESTORE FAILED: G#{e:X} — write G#69[G#C5] = G#{p:02X} by hand\n");
            }
        }
        if let Some(p) = w.charger_prev {
            if let Err(e) = crate::hsi2c::charger::restore(p) {
                r += &alloc::format!("CHARGER RESTORE FAILED: G#{e:X} — write G#69[G#B9] = G#{p:02X} by hand\n");
            }
        }
        if w.n > 0 {
            let ma = w.sum_ua / w.n as i64 / 1000;
            let mv = w.sum_uv / w.n as u64 / 1000;
            r += &alloc::format!("{} s, {} samples: {ma} mA (min {}, max {}) at {mv} mV = {} mW (negative = discharging); clocks dark {}, rest {}, active {}, unset {} of the samples\n", w.secs, w.n, w.min_ua / 1000, w.max_ua / 1000, ma * mv as i64 / 1000, w.states[3], w.states[1], w.states[2], w.states[0]);
        } else {
            r += "no gauge samples\n";
        }
        // Into the flash log too: a window run with the cable out ends before the host can ask, and if the USB link does not come back the reset that recovers it would take the result with it (2026-10-25).
        crate::flashlog::line(&alloc::format!("power: {}", r.trim_end()));
        unsafe {
            POWER_RESULT = Some(r);
            POWER_WIN = None;
        }
    }
}

/// The clock policy (POWER.md, 2026-10-25): two operating points, chosen from what the kernel knows. Rest — nobody touching, no transfer on the USB link — puts every domain at the lowest point that still carries the rest-state work: the X3 at 2.0 GHz (its 2.9 GHz step alone is 0.66 W, and the display's between-frames redraws fit at 2.0), MIF at 1014 MHz (2028 → 1014 is 0.36 W; lower bought nothing), INT at its 178 MHz floor, the workers' cluster at its 402 MHz floor, CPU0's (parked) at its 324 MHz floor and the DSU (the clusters' shared cache and snoop) at its 324 MHz floor (90 mW; BCI at its floor bought nothing measurable and stays) (zuma-cpu.dtsi: ACPM answers Ok below a floor and the device hard-resets, so floors, never under). Active — a touch taken by the touch driver, a PT transfer moving, or the display sending frames faster than its rest flips (a focused widget), held a second after the last — is the boot's own clocks. Each entry: (ACPM DVFS domain, kHz), applied in order (the X3 first going down, MIF first going up).
const CLOCKS_REST: [(u32, u32); 7] = [(4, 2_000_000), (0, 1_014_000), (1, 178_000), (3, 402_000), (2, 324_000), (5, 324_000), (14, 400_000)];
/// Dark — the screen off, nobody touching, no transfer: nothing is drawn, so the X3 and MIF go to their floors and the display's clock to its 143 MHz floor (measured 70 mW under rest with the screen off).
const CLOCKS_DARK: [(u32, u32); 7] = [(4, 500_000), (0, 421_000), (1, 178_000), (3, 402_000), (2, 324_000), (5, 324_000), (14, 143_000)];
const CLOCKS_ACTIVE: [(u32, u32); 7] = [(0, 2_028_000), (5, 1_197_000), (1, 356_000), (3, 1_065_000), (2, 1_425_000), (14, 400_000), (4, 2_914_000)];
/// The policy's state: 0 not yet applied, 1 rest, 2 active, 3 dark; and whether it runs (`diag dvfs policy off` holds the clocks wherever `dvfs set` puts them, for measuring). Switches counted for DIAG.
static mut CLOCK_STATE: u8 = 0;
static mut CLOCK_POLICY_ON: bool = true;
static mut CLOCK_SWITCHES: u32 = 0;
static mut CLOCK_WAIT_MAX_US: u64 = 0;

/// The process table'"s
/// The process table's generation last looked at, and whether the GPU's clock is up for its service.
static mut PROCS_SEEN: u32 = u32::MAX;
static mut GPU_CLOCKED: bool = false;

fn clock_policy(pt_busy: bool, booted_for: u64) {
    let (f, now) = (cntfrq(), cntpct());
    // After the jump from Android the clocks are whatever Android last asked for, the X3 at 377 MHz on the eleventh jump of 2026-11-03 (the attest mark took 19 s to refine, 3.6 s at the boot's clocks, and the screen looked frozen until the policy's first switch at 16 s): the active profile, the clocks a cold boot runs with, at once. The 16 s wait below is for the cold boot's firmware loads, which a jump does not run.
    if unsafe { CLOCK_POLICY_ON && CLOCK_STATE == 0 } && platform::jumped() {
        clock_profile(2);
        return;
    }
    // Not while the boot is still loading firmware (the AoC's 29 MB copy, Trusty) at the boot's own clocks.
    if unsafe { !CLOCK_POLICY_ON } || booted_for < 16 * f {
        return;
    }
    let input = crate::process::INPUT_AT.load(core::sync::atomic::Ordering::Relaxed);
    // A PT transfer moving: the kernel's own, or one the USB process carries (it says so: ptlink.rs MOVING_AT).
    let moving = crate::ptlink::MOVING_AT.load(core::sync::atomic::Ordering::Relaxed);
    let pt_busy = pt_busy || (moving != 0 && now.wrapping_sub(moving) < f / 16);
    let active = pt_busy || (input != 0 && now.wrapping_sub(input) < f);
    if pt_busy {
        unsafe { PT_BUSY_AT = now };
    }
    let active = active || now.wrapping_sub(unsafe { PT_BUSY_AT }) < f;
    // Frames faster than the rest flips: counted over quarter seconds, six or more (24 a second; the rest flips are 10) marks the display animating — a focused widget gets the active clocks as a finger does (2026-10-25: the focused scope ran 61 frames a second at the rest clocks, 93 at the active ones).
    unsafe {
        if now.wrapping_sub(FRAMES_WINDOW.0) >= f / 4 {
            let n = crate::process::FRAME_STARTS.load(core::sync::atomic::Ordering::Relaxed);
            if n.wrapping_sub(FRAMES_WINDOW.1) >= 6 {
                FRAMES_FAST_AT = now;
            }
            FRAMES_WINDOW = (now, n);
        }
    }
    let active = active || (unsafe { FRAMES_FAST_AT } != 0 && now.wrapping_sub(unsafe { FRAMES_FAST_AT }) < f);
    let dark = !active && crate::process::pref(crate::process::PREF_SCREEN_OFF) == 1;
    let want = if active { 2 } else if dark { 3 } else { 1 };
    let was = unsafe { CLOCK_STATE };
    if want != was {
        // The workers are off while dark (2026-10-25: their cluster drew ~28 mW idle in `wfe` with the screen off; Android's 8): started before leaving the dark profile, stopped after entering it, so no clock request goes to their cluster while it is down. Threads meant for the workers run on the X3 meanwhile (process.rs: a mask with no core up runs anywhere).
        let workers_dark = DARK_WORKERS_OFF.load(core::sync::atomic::Ordering::Relaxed);
        if was == 3 && workers_dark {
            crate::smp::launch_all();
            let t0 = cntpct();
            while crate::smp::UP.load(core::sync::atomic::Ordering::Acquire) != 0xF && cntpct().wrapping_sub(t0) < f / 20 {
                core::hint::spin_loop();
            }
        }
        clock_profile(want);
        if want == 3 && workers_dark {
            crate::smp::stop_all();
        }
        // The GPU's clock with the dark profile, while a gpu process keeps it up.
        if unsafe { GPU_CLOCKED } && (want == 3) != (was == 3) {
            gpu_rate(want != 3);
        }
    }
}
/// The GPU's clock at its top (G3D 890, G3DL2 900 MHz) or its floor (150 / 160 MHz), the L2 first going up and last going down (GPU.md "The GPU's clock"). At the top while a `gpu` process lives and the screen is lit; at the floor otherwise, the dark profile included (2026-11-10: the GPU's rails held ~40 mW with the screen off at its top, idle).
fn gpu_rate(top: bool) {
    let r = if top {
        (crate::acpm::dvfs_set(8, 900_000), crate::acpm::dvfs_set(7, 890_000))
    } else {
        (crate::acpm::dvfs_set(7, 150_000), crate::acpm::dvfs_set(8, 160_000))
    };
    crate::process::push_console(alloc::format!("gpu clock: {} ({:?})\n", if top { "G3D 890, G3DL2 900 MHz" } else { "the floor, 150 / 160 MHz" }, r).as_bytes());
}

/// `diag smp dark on|off`: whether the workers go off with the dark profile (on by default).
pub static DARK_WORKERS_OFF: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

/// One of the two operating points, every domain in order; then the X3's timer control (its event stream, which wakes the idle loop's `wfe`) and the counter into note 6, and the switch count into note 7, so a hang after a switch says what the core was left with (the first policy build hung the X3 in `wfe` within a minute of going to rest, 2026-10-25).
fn clock_profile(want: u8) {
    let table: &[(u32, u32)] = match want {
        2 => &CLOCKS_ACTIVE,
        3 => &CLOCKS_DARK,
        _ => &CLOCKS_REST,
    };
    // Each request answered before the next and before the loop may sleep: the clusters', MIF's and INT's rate requests take no reply (as Linux sends them), so five went out back to back and the loop went straight into `wfe` with the firmware still at work — and on battery (the charger cut) the X3 never woke from that `wfe` (2026-10-25, twice). A rate read takes a reply, and the firmware answers in order: its answer means the request before it is done. The longest wait goes into DIAG.
    for &(d, khz) in table {
        let t0 = cntpct();
        let _ = crate::acpm::dvfs_set(d, khz);
        let _ = crate::acpm::dvfs_get(d);
        let us = cntpct().wrapping_sub(t0) * 1_000_000 / cntfrq();
        unsafe { CLOCK_WAIT_MAX_US = CLOCK_WAIT_MAX_US.max(us) };
    }
    let v: u64;
    unsafe {
        core::arch::asm!("mrs {}, cnthctl_el2", out(reg) v);
        CLOCK_STATE = want;
        CLOCK_SWITCHES += 1;
        platform::note(6, (CLOCK_STATE as u64) << 56 | v);
        platform::note(7, (CLOCK_SWITCHES as u64) << 40 | cntpct() >> 16);
    }
}
static mut PT_BUSY_AT: u64 = 0;
static mut FRAMES_WINDOW: (u64, u64) = (0, 0);
static mut FRAMES_FAST_AT: u64 = 0;
/// Idle sleeps the EL2 timer's backstop ended (the event stream did not): DIAG BACKSTOP_WAKES.
static BACKSTOP_WAKES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The facts a settings page shows as the facts' bell last rang for them (`bell_facts`): the battery's share, whether it charges, the processes alive, the vault's keys, the hottest zone, the throttle. Only the service loop touches it.
static mut FACTS_SHOWN: [u64; 6] = [crate::process::FACT_UNKNOWN; 6];
/// The processes alive as last counted (`bell_facts`, when the table changed).
static ALIVE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The facts' bell (process.rs SYS_BELL, `BELL_FACTS`; owner, 2026-11-10: "Needs a doorbell"): rung when a fact the settings show differs from what it last rang for, so the About page draws again on a change and never on a timer. Called where those facts are published, on the service loop: the gauge's sample (every two seconds, the kernel's own sampling of the hardware, unchanged), the thermal zones' read (once a second), and the process table changing (PROCS_GEN). A sample that changes nothing shown (the cell's voltage, the battery's temperature, a current that stays on its side of zero) rings nothing.
fn bell_facts() {
    use core::sync::atomic::Ordering::Relaxed;
    let g = unsafe { (*(&raw const GAUGE)).0 };
    // Charging as the About page says it: the current's sign, positive charging (fact 4), unknown kept apart from either.
    let charging = if g[3] == crate::process::FACT_UNKNOWN { 2 } else { (g[3] as i64 > 0) as u64 };
    let hottest = THERMAL.iter().map(|t| t.load(Relaxed)).max().unwrap_or(0) as u64;
    let now = [g[0], charging, ALIVE.load(Relaxed), VAULT_KEYS.load(Relaxed), hottest, (THROTTLED_FROM.load(Relaxed) != 0) as u64];
    let shown = unsafe { &mut *(&raw mut FACTS_SHOWN) };
    if now != *shown {
        *shown = now;
        crate::process::with_lock(|| crate::process::bell_count(crate::process::BELL_FACTS));
    }
}

/// From the service loop on the kernel's core: the battery gauge every two seconds, for facts 1-4, and the vault's key count, for fact 7.
fn gauge_poll() {
    let (f, now) = (cntfrq(), cntpct());
    unsafe {
        let g = &mut *(&raw mut GAUGE);
        if g.1 == 0 || now.wrapping_sub(g.1) > 2 * f {
            g.1 = now;
            g.0 = match crate::hsi2c::gauge::sample() {
                Ok(s) => [s.soc as u64, s.temp_cc as i64 as u64, s.vcell_uv as u64, s.current_ua as i64 as u64],
                Err(_) => [crate::process::FACT_UNKNOWN; 4],
            };
            // The vault's key count only every 64 s: it reads the flash, and every 2 s it woke the UFS link out of hibernate (POWER.md).
            if VAULT_KEYS.load(core::sync::atomic::Ordering::Relaxed) == u64::MAX || now.wrapping_sub(VAULT_KEYS_AT.load(core::sync::atomic::Ordering::Relaxed)) > 64 * f {
                VAULT_KEYS_AT.store(now, core::sync::atomic::Ordering::Relaxed);
                if let Some(keys) = crate::process::with_lock(|| (*(&raw mut SPAWN_STORE)).as_mut().and_then(|s| s.as_mut()).and_then(|s| s.keys().ok())) {
                    VAULT_KEYS.store(keys.len() as u64, core::sync::atomic::Ordering::Relaxed);
                }
            }
            bell_facts();
        }
    }
}

/// The Type-C port controller's USB switch (the MAX77759's TCPC, G#25 on hsi2c_11, vendor register G#93 USBSW_CTRL: G#09 connects D+/D- to the SoC's PHY, 0 parks them): after the cable came out and went back in, the link never came back (2026-10-25, four times) — VBUS present and CC attached on the replug, the eUSB2 repeater correctly tuned, the charger in its mode, yet the host never saw the device. The chip opens the switch for its own BC1.2 charger detection on a new VBUS, and Google's driver closes it on every attach ("switches will not be configured by HW", tcpci_max77759.c enable_data_path_locked); ABL closes it at power-on and ferros never had. So: attached, VBUS present and the switch not G#09 — close it. The controller's role, CC, power status and the switch go into the flash log when `log` is set and whenever it acts. Called by the kernel's USB re-initialisation and by the `usb` power switch (ptlink.rs), both on the kernel's core, which the battery gauge sharing the bus also polls from.
pub(crate) fn tcpc_usb_switch(log: bool) {
    use crate::hsi2c::{read_byte, write_byte, HSI2C_11};
    const TCPC: u8 = 0x25;
    let r = |reg: u8| read_byte(HSI2C_11, TCPC, reg).map(|v| v as u32).unwrap_or(0x8000);
    let (role, mut cc, mut pwr, sw) = (r(0x1A), r(0x1D), r(0x1E), r(0x93));
    // The port as a device needs it (TCPCI): sink on both CC lines (ROLE_CONTROL G#0A, Rd/Rd) with VBUS detection on (POWER_STATUS bit 3). ABL leaves it so on a cold boot (role G#0A, power G#0D). A jump from an Android that had the port in its low-power state (2026-11-04: role G#11, a pull-up as a source, CC_STATUS G#20 looking for a connection, power G#40, detection off) left the host seeing no attach at all, and the USB process never configured. So: detection on (COMMAND G#33, EnableVbusDetect) and the sink role, then the switch below once the host shows on CC with VBUS. A read that failed (G#8000) is left alone.
    if role & 0x8000 == 0 && pwr & 0x8000 == 0 && (role != 0x0A || pwr & 0x08 == 0) {
        let w_cmd = write_byte(HSI2C_11, TCPC, 0x23, 0x33).is_ok();
        let w_role = write_byte(HSI2C_11, TCPC, 0x1A, 0x0A).is_ok();
        let role_back = r(0x1A);
        // A quarter second for the port to see the host on CC and VBUS (tCCDebounce is 100-200 ms), then read again, so this USB process's start closes the switch rather than the next one's.
        let (f, t0) = (cntfrq(), cntpct());
        while cntpct().wrapping_sub(t0) < f / 4 {}
        (cc, pwr) = (r(0x1D), r(0x1E));
        crate::flashlog::line(&alloc::format!("usb: tcpc role G#{role:X}: to the sink role (G#0A) with VBUS detection on; now cc G#{cc:X} power G#{pwr:X}"));
        // When it does not take (2026-11-04, a jump: power G#4A — bit 6, the TCPC still initialising, ignores writes — and the role back at G#11 at every start for a minute), the rest of the picture for the next one: whether the writes were taken, the role read straight back, ALERT, and the vendor CC controls (tcpci_max77759_vendor_reg.h: CC_CTRL1-3 at G#8C-G#8E; CC_CTRL2 bits 3-4 the CC low-power mode, Android's auto ultra-low-power G#18).
        if pwr & 0x40 != 0 || r(0x1A) != 0x0A {
            crate::flashlog::line(&alloc::format!("usb: tcpc not taking it: writes {w_cmd}/{w_role}, role read back G#{role_back:X}, alert G#{:X}, cc_ctrl1-3 G#{:X} G#{:X} G#{:X}", r(0x10) | r(0x11) << 8, r(0x8C), r(0x8D), r(0x8E)));
        }
    }
    let attached = cc & 0x8000 == 0 && cc & 0xF != 0;
    let vbus = pwr & 0x8000 == 0 && pwr & 0x4 != 0;
    let act = attached && vbus && sw != 0x09;
    if act {
        let _ = write_byte(HSI2C_11, TCPC, 0x93, 0x09);
    }
    if log || act {
        crate::flashlog::line(&alloc::format!("usb: tcpc role G#{role:X} cc G#{cc:X} power G#{pwr:X} usb switch G#{sw:X}{}", if act { " -> closed (G#09)" } else { "" }));
    }
}

pub fn run(mut b: Boot) -> ! {
    // Report now too: if the USB bring-up wedges, the watchdog's reset still shows the vault result (handoff) and the crumb shows how far USB got.
    report(&b, 0xA0);
    let generation = if b.x0 & 1 == 1 { (b.x0 & !crate::board::RELOAD_AKITA) >> 1 } else { 0 };
    let mut store = b.store.take();
    unsafe {
        SPAWN_STORE = &raw mut store;
        crate::process::CAPSULES = Some(read_capsule);
        crate::process::PREF_STORE = Some(store_pref);
        crate::process::CONTROL_STORE = Some(store_document);
        crate::process::FACTS = Some(facts);
        crate::process::STORE = Some(store_scoped);
        // The last boot's end, if the fault sink or the panic reporter recorded one, into the flash log: a reboot should never be silent.
        {
            let w = crate::platform::previous_end();
            if w.iter().any(|&v| v != 0) {
                let text: alloc::string::String = w[4].to_le_bytes().iter().chain(w[5].to_le_bytes().iter()).map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { '.' }).collect();
                crate::flashlog::line(&alloc::format!("previous boot ended: panic-num {} heap G#{:X} esr G#{:X} faults {} elr G#{:X} far G#{:X} text {:?}", w[0], w[1], w[2], w[3], w[4], w[5], text));
            }
        }
        crate::process::STORE_REMOVE = Some(store_remove);
        crate::process::STORE_KEYS = Some(store_keys);
        crate::process::STORE_ADDR = Some(store_addr);
        crate::process::DOMAIN_BASE = Some(domain_base);
        crate::process::UNLOCK = Some(store_unlock);
        crate::process::EXPORT = Some(store_export);
        crate::platform::BEFORE_RESET = Some(lock_before_reset);
    }
    let mut home_started = !matches!(b.kind, BootKind::Cold);
    let t_boot = cntpct();
    let exc_start = crate::exception_count();
    platform::mark(0x34);

    // Boot screen: after a --live handoff DECON is still running, so show where this generation stands before USB even comes up (show() touches nothing when the DPU is powered down).
    if !crate::checklist::active() {
        let _ = crate::display::show(&status_text(b.kind, generation, b.probe.as_ref()), cntfrq());
    }
    platform::mark(0x35);

    // The USB DMA master on physical addresses (the HSI0 S2MPU bypassed, every USB SYSMMU context off), reported by DIAG. USB itself is the USB process's (payloads/usb, USB.md): the kernel brings up no controller of its own and readies the link only when the process asks (`power usb on`, ptlink.rs).
    let dma = open_usb_dma_path();
    platform::mark(0x36);

    // The worker cores (smp.rs): up once the kernel is settled on its own core, before the first slice is handed out.
    crate::smp::launch_all();
    platform::mark(0x39);
    // The wall clock (rtc.rs): read now, so the first process to ask has it.
    let _ = crate::rtc::sync();
    let freq = cntfrq();
    let mut t_heartbeat = cntpct();
    // The AoC's start, idle until the `sensors` switch asks for it (sensorlink.rs START).
    let mut sensor_boot = SensorBoot::Done;
    let mut st = PtStats::default();
    // A command's reply as it is built (raw: the USB process frames it), and the reply being built when the next command came, put back after (ptlink.rs); whether the command waits to act once its reply has gone out.
    let mut pt_out_data: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    let mut proc_restore: Option<alloc::vec::Vec<u8>> = None;
    let mut proc_act = false;
    // A bridge `store` command the vault process is answering (`VaultJob`), and the command it came in, kept for a put's value (ptlink takes no other until its reply is taken).
    #[cfg(not(ferros_production))]
    let mut vault_job: Option<VaultJob> = None;
    #[cfg(not(ferros_production))]
    let mut vault_cmd: Option<crate::ptlink::Submitted> = None;
    // A drop of the released person's domain sent to the vault process, its answer awaited (`RELEASE_DROPS`).
    let mut release_drop_sent = false;
    // A kist's capsule being read from the vault process for its launch (`KistJob`).
    let mut kist_job: Option<KistJob> = None;
    // The sensors process's check (below): when it last looked.
    let mut sensors_check = 0u64;

    let cap_diag = ferros_pt::command::dev_cap(ferros_pt::command::caps::DIAG);
    let cap_reload = ferros_pt::command::dev_cap(ferros_pt::command::caps::RELOAD);
    let cap_reboot = ferros_pt::command::dev_cap(ferros_pt::command::caps::REBOOT);
    let cap_run = ferros_pt::command::dev_cap(ferros_pt::command::caps::RUN);
    let cap_spawn = ferros_pt::command::dev_cap(ferros_pt::command::caps::SPAWN);
    let cap_seal = ferros_pt::command::dev_cap(ferros_pt::command::caps::SEAL);
    let cap_mem = ferros_pt::command::dev_cap(ferros_pt::command::caps::MEM);
    let cap_display = ferros_pt::command::dev_cap(ferros_pt::command::caps::DISPLAY);
    let cap_store = ferros_pt::command::dev_cap(ferros_pt::command::caps::STORE);
    // RELOAD and RUN share STAGE; at most one of these is set, so a payload can never be jumped to as a kernel or vice versa.
    let mut reload_ok = false;
    let mut reload_size = 0usize;
    let mut run_ok = false;
    let mut run_size = 0usize;
    // A capsule streamed straight into its arc (the spawn Write pieces): the arc, the capsule's length, the bytes received, the code's offset and length inside it.
    let mut spawn_stage: Option<(crate::process::Arc, usize, usize, usize, usize)> = None;

    unsafe extern "C" {
        static __stack_top: u8;
    }
    let heap_end = ((&raw const __stack_top as usize + 0xFFF) & !0xFFF) + crate::HEAP_SIZE;

    // The generic timer's event stream: an event on every 0 -> 1 transition of counter bit 15, every 65536 ticks — 2.7 ms at 24.576 MHz (bit 12, 333 us, until 2026-10-25: bit 15 is 15-25 mW less with the screen off, and the idle `wfe`'s EL2 timer is armed at the next thread deadline, so deadlines no longer wait on the stream). Earlier: an event on every 0 -> 1 transition of counter bit 12, every 8192 ticks — 333 us at 24.576 MHz. A core in `wfe` wakes on it without taking an interrupt, so the idle loop below can sleep instead of spinning and still answer USB well inside anything the host would notice.
    // The period sets the split. Each wake runs one full loop pass, about 13 us of work, so bit 9 (41.7 us) left the core awake a third of the time — 68% asleep, measured — while bit 12 leaves it awake about 4%. Only the first packet of a new command can wait up to one period; a transfer in progress never sleeps.
    // Without it the loop pinned one core at full speed for as long as ferros was up, and with nothing else running that core was most of the device's heat (owner, 2026-10-21: "why's it so hot? 1 core, always on?").
    unsafe {
        let mut v: u64;
        core::arch::asm!("mrs {}, cnthctl_el2", out(reg) v);
        v = (v & !(0xF << 4 | 1 << 3)) | 1 << 2 | 15 << 4;
        core::arch::asm!("msr cnthctl_el2, {}", "isb", in(reg) v);
    }
    // Ticks spent asleep in the idle loop, and since when, for DIAG's IDLE_PCT.
    let mut asleep: u64 = 0;
    let mut backstop_ppi = false;
    let t_awake_from = cntpct();
    // The gate's own press is spent; from here a press is the screen's (below).
    unsafe { core::ptr::write_volatile(&raw mut crate::__power_pressed, 0) };

    // The ira and wairua (keys.rs): take what CPU0 sealed at the hop now, before the first slice; a quarter second on, stop waiting for workers that never came up, so the blob leaves DRAM.
    crate::keys::serve();
    let keys_settle = cntpct() + cntfrq() / 4;
    let mut keys_settled = false;
    // The restart chord arms once power and volume up are not both down: the chord that jumped here from Android (or started this restart) may still be held as this loop starts, and must not restart ferros again.
    let mut chord_armed = false;
    let mut ipc_marked = false;
    // The radios switched off (preference 15) as the loop last applied them: init started only the ones left on.
    let mut radios_applied = crate::process::pref(crate::process::PREF_RADIOS_OFF) & 0x1F;
    // The GPS driver started for the claims held now (process.rs CLAIMS): set as the first claim starts it, cleared as the last one goes and stops it. A driver that ends by itself while claimed is not started again until the claims go to none and back.
    let mut gps_on = false;
    // ARC_FREES when the driver's spawn was last refused for want of an arc, as the installer's below: tried again once an arc has come back.
    let mut gps_refused_at: Option<u64> = None;
    // The installer (payloads/kists) started for the person present.
    let mut kists_started = false;
    // ARC_FREES when the installer's spawn was last refused for want of an arc (the 8a, 2026-11-10: "arc pool exhausted" at attest, beside the wallpaper's start, and no installer for the rest of the boot): tried again once an arc has come back since.
    let mut kists_refused_at: Option<u64> = None;
    // The panel's VCI as ABL or Android left it (SYS_FACTS 81): the display server reads it to know a panel it must power up (state B).
    crate::panel_power::read_state();
    // The display engine fenced to its own regions when the DPU is up already (ABL's splash, or a jump from a lit Android); a DPU that is down is fenced by the `dpu` switch as it powers it (s2mpu::fence_dpu). Before init starts the display server, so it never runs unfenced.
    if crate::dpu_power::all_on() {
        let r = crate::s2mpu::fence_dpu();
        crate::flashlog::line(&alloc::format!("dpu: S2MPUs fenced at boot: G#{r:X} (1 = fenced)"));
    }
    // The CoreSight system's bus master (the debug port's memory access, STM, the trace sinks) cut off from all memory on a cold boot: ferros uses none of it, and ABL leaves its S2MPU open. Not the cores' path: deny-all on it with ferros running left every core, the display and USB running (2026-11-04). Not after a jump: closed there, the jumped ferros froze right after its vault opened (most likely Android's trace still running on the cores, its sink cut off, backing up into them); that waits for tracing switched off on every core first.
    if !platform::jumped() {
        let r = crate::s2mpu::deny_all(crate::s2mpu::CPUCL0_S2MPU);
        crate::flashlog::line(&alloc::format!("cpucl0: the debug system's S2MPU denied all: G#{r:X} (1 = done)"));
        // The modem's link fenced to its shared memory before init starts its driver (s2mpu::fence_modem). After a jump the hv has denied it all, and the modem is not started.
        let r = crate::s2mpu::fence_modem();
        crate::flashlog::line(&alloc::format!("modem: HSI1's S2MPU fenced to the shared-memory windows: G#{r:X} (1 = fenced)"));
        // BLK_MISC's bus masters (PDMA1, SPDMA1, DIT), none of them ferros's: denied all, as the hv denies them at the jump.
        let r = crate::s2mpu::deny_all(crate::s2mpu::MISC_S2MPU);
        crate::flashlog::line(&alloc::format!("misc: BLK_MISC's S2MPU denied all: G#{r:X} (1 = done)"));
        // The GPU held to its own memory (gpu-mem) before any process can program it: its page tables are a process's.
        let r = crate::s2mpu::fence_gpu();
        crate::flashlog::line(&alloc::format!("gpu: G3D's S2MPU fenced to gpu-mem: G#{r:X} (1 = fenced)"));
    }
    // The device's opt-in to Android (ANDROID_ALLOWED), once.
    if let Some(st) = unsafe { (*(&raw mut SPAWN_STORE)).as_mut() }.and_then(|s| s.as_mut()) {
        let yes = matches!(st.get_by_key("android.allowed"), Ok(Some(v)) if v.as_slice() == [1]);
        ANDROID_ALLOWED.store(if yes { 2 } else { 1 }, core::sync::atomic::Ordering::Relaxed);
    }
    platform::mark(0x3A);
    loop {
        platform::pet_watchdogs();
        // The service loop's pass crumbs (2026-11-08: the reset left G#B4, woken from the idle `wfe`, as the X3's last word, and nothing after it said where the pass went): G#D3 the pass's top, G#D4 a command taken from the USB process, G#D5 with its capability and op (bits 8-39 and 40-47), G#D6 the polls, G#D7 the clock policy, G#D8 past the UFS hibernate and the thermal poll; then G#D1 a slice, G#D2 back, G#B3 into the idle `wfe`, G#B4 out of it.
        platform::crumb(0xD3);
        // The power switches a process left on when it ended (one shot dead, no cleanup of its own): turned off here, on the kernel's core.
        crate::process::power_offs_apply();
        // A person unlocked: wallgen, once (`spawn_wallgen`).
        if let Some(seed) = unsafe { (*(&raw mut WALLGEN_SEED)).take() } {
            spawn_wallgen(&mut store, &seed);
        }
        // A status circle tapped (preference 15, owner 2026-11-07): a radio switched off has its driver stopped and, where the kernel holds its power switch (Bluetooth, NFC, GPS), its chip powered down; switched on, its driver starts again from its capsule. Cellular and WiFi have no kernel switch: their drivers stop, the modem staying as it was until a power-down of its own exists.
        let radios_off = crate::process::pref(crate::process::PREF_RADIOS_OFF) & 0x1F;
        if radios_off != radios_applied {
            const DRIVERS: [&[u8]; 5] = [b"modem", b"pcie1", b"bt", b"nfc", b"gps"];
            for (i, name) in DRIVERS.iter().enumerate() {
                let bit = 1u8 << i;
                if radios_off & bit != 0 && radios_applied & bit == 0 {
                    let killed = crate::process::kill(name);
                    let powered_down = match i {
                        2 => crate::bt::power(false),
                        3 => crate::nfc::power(false),
                        4 => crate::gps::power(false),
                        _ => false,
                    };
                    crate::flashlog::line(&alloc::format!("radio {} switched off: driver {}, power {}", core::str::from_utf8(name).unwrap_or("?"), if killed { "stopped" } else { "was not running" }, if powered_down { "down" } else { "left" }));
                } else if radios_off & bit == 0 && radios_applied & bit != 0 && i == 4 {
                    // GPS switched on is allowed, not started: the driver comes with the first claim (below).
                    crate::flashlog::line("radio gps switched on: allowed, started when something claims it");
                } else if radios_off & bit == 0 && radios_applied & bit != 0 {
                    let mut rep = alloc::vec::Vec::new();
                    restart(&mut store, name, &mut rep);
                    crate::flashlog::line(&alloc::format!("radio {} switched on: {}", core::str::from_utf8(name).unwrap_or("?"), core::str::from_utf8(&rep).unwrap_or("").trim_end()));
                }
            }
            radios_applied = radios_off;
        }
        let person = store.as_ref().is_some_and(|s| s.unlocked());
        // The installer (payloads/kists), once each time a person is in: it serves the map's Kists page and fetches back the capsules a restored list names. Its grants are its own (`kists_grants`), init's page being full.
        let frees = crate::process::ARC_FREES.load(core::sync::atomic::Ordering::Acquire);
        if !person {
            kists_started = false;
            kists_refused_at = None;
        } else if !kists_started && kists_refused_at != Some(frees) {
            kists_started = true;
            let running = crate::process::with_lock(|| crate::process::named_alive_locked(&[b"kists"])) != 0;
            if !running {
                let mut rep = alloc::vec::Vec::new();
                restart(&mut store, b"kists", &mut rep);
                crate::flashlog::line(&alloc::format!("kists after attest: {}", core::str::from_utf8(&rep).unwrap_or("").trim_end()));
                // Refused for room, and only that (an installer that started and died is not tried again): the next arc back (a process ending) brings another try.
                if rep.windows(18).any(|w| w == b"arc pool exhausted") {
                    kists_started = false;
                    kists_refused_at = Some(frees);
                }
            }
        }
        // GPS on demand (owner, 2026-11-10: "Wire it so GPS stays green but no visuals for satellites since we're not rx'ing and all that until something like mahere calls it or I open the GPS screen (which should turn it on whilist GPS screen is open) or Photon a friend pings for my location"): the switch on means GPS is allowed, not that the chip receives. The driver runs while at least one process holds a claim on it (process.rs CLAIMS, SYS_CLAIM: the map's GPS page while it is open, a kist granted GPS while it asks), started as the first claim comes and stopped as the last goes; a claim's change wakes this loop (`sev`), never a timer. From 2026-11-08 until then the driver started at every attest and ran for as long as the switch stayed on.
        // Locked until a person attests (radios at attest), and a GPS switched off holds no claims: either drops every claim, and the driver stops below.
        let gps_allowed = person && radios_off & 1 << 4 == 0;
        if !gps_allowed && crate::process::gps_claims() != 0 {
            let n = crate::process::with_lock(crate::process::gps_claims_drop);
            crate::flashlog::line(&alloc::format!("gps: A#{n} claims dropped ({})", if person { "switched off" } else { "nobody present" }));
        }
        if !person {
            gps_refused_at = None;
        }
        let gps_wanted = gps_allowed && crate::process::gps_claims() != 0;
        if gps_wanted && !gps_on && gps_refused_at != Some(frees) {
            gps_on = true;
            let running = crate::process::with_lock(|| crate::process::named_alive_locked(&[b"gps"])) != 0;
            if !running {
                let mut rep = alloc::vec::Vec::new();
                restart(&mut store, b"gps", &mut rep);
                crate::flashlog::line(&alloc::format!("gps claimed (A#{} claims): {}", crate::process::gps_claims(), core::str::from_utf8(&rep).unwrap_or("").trim_end()));
                // Refused for room, as the installer's above: the next arc back brings another try while the claim stands.
                if rep.windows(18).any(|w| w == b"arc pool exhausted") {
                    gps_on = false;
                    gps_refused_at = Some(frees);
                }
            }
        } else if !gps_wanted && gps_on {
            // The last claim gone: the driver killed, one shot dead (its last fix was kept as it came), and the chip powered down, as the switch does.
            gps_on = false;
            let killed = crate::process::kill(b"gps");
            let powered_down = killed && crate::gps::power(false);
            crate::flashlog::line(&alloc::format!("gps unclaimed: driver {}, power {}", if killed { "stopped" } else { "was not running" }, if powered_down { "down" } else { "left" }));
        }
        // Released (a completed departure): restart, so the device comes back at the attest screen with nothing of the identity left.
        // First the person's domains in the vault process are dropped, a call a pass through the kernel's channel (none while a bridge command holds it); a drop that fails, or no vault to send it to, is logged and the restart goes ahead: the renewed device-only secret already makes those entries unopenable.
        #[cfg(not(ferros_production))]
        let channel_free = vault_job.is_none() && kist_job.is_none();
        #[cfg(ferros_production)]
        let channel_free = kist_job.is_none();
        if channel_free && RELEASE_RESET.load(core::sync::atomic::Ordering::Acquire) {
            if release_drop_sent {
                match crate::process::vault_answer() {
                    Some(Ok((n, _))) if n != crate::process::ERR => {
                        crate::flashlog::line(&alloc::format!("identity released: a domain of the person's dropped in the vault process ({n} values)"));
                        release_drop_sent = false;
                    }
                    Some(_) => {
                        crate::flashlog::line("identity released: a drop in the vault process failed; its entries stay on the disk, unopenable");
                        release_drop_sent = false;
                    }
                    None => {}
                }
            }
            if !release_drop_sent {
                match crate::process::with_lock(|| unsafe { (*(&raw mut RELEASE_DROPS)).pop() }) {
                    Some(msg) => {
                        release_drop_sent = crate::process::vault_call(msg);
                        if !release_drop_sent {
                            crate::flashlog::line("identity released: no vault process to drop the person's domain; its entries stay on the disk, unopenable");
                        }
                    }
                    None => {
                        RELEASE_RESET.store(false, core::sync::atomic::Ordering::Release);
                        crate::flashlog::line("identity released: restarting at the attest screen");
                        platform::reboot();
                    }
                }
            }
        }
        // A launch of Android the `android` switch accepted: the gate's Android path, as `diag android` takes it. The person is checked again here; the reset scrubs ferros's memory on its way (platform::before_reset).
        #[cfg(not(shim_handoff))]
        if ANDROID_REQ.swap(false, core::sync::atomic::Ordering::AcqRel) && unsafe { (*(&raw mut SPAWN_STORE)).as_mut() }.and_then(|s| s.as_mut()).is_some_and(|s| s.unlocked()) {
            // Under the kernel lock, as every UFS command (devinfo is written while processes still run).
            let r = crate::process::with_lock(|| crate::slot::set_active(0));
            unsafe {
                let p = 0xFD60_0FF8usize as *mut u64;
                core::ptr::write_volatile(p, 0x414E_4452_4F49_4431);
                core::arch::asm!("dc civac, {a}", "dsb sy", a = in(reg) p as u64);
            }
            crate::flashlog::line(&alloc::format!("android: launching (slot a {r})"));
            if r == crate::slot::MARKED {
                platform::crumb(0xC7);
                platform::reboot();
            }
        }
        // A bridge `store` command the vault process is answering: its next step; its reply goes out below once the last call is answered.
        #[cfg(not(ferros_production))]
        let vault_waits = match vault_job.as_mut() {
            Some(job) => {
                let params = vault_cmd.as_ref().and_then(|c| ferros_pt::command::parse(c.payload.bytes())).map(|c| c.params).unwrap_or(&[]);
                match vault_job_step(job, params, &mut store) {
                    Some(resp) => {
                        queue_pt_response(&mut pt_out_data, &resp);
                        vault_job = None;
                        vault_cmd = None;
                        false
                    }
                    None => true,
                }
            }
            None => false,
        };
        #[cfg(ferros_production)]
        let vault_waits = false;
        // A kist launch init asked for (KISTS.md): started once the kernel's channel is free (no bridge store command, no release under way), then a read a pass until the capsule is in its arc (`kist_step`).
        #[cfg(not(ferros_production))]
        let bridge_holds_channel = vault_job.is_some();
        #[cfg(ferros_production)]
        let bridge_holds_channel = false;
        if kist_job.is_none() && !bridge_holds_channel && !RELEASE_RESET.load(core::sync::atomic::Ordering::Acquire) {
            // A launch first (a person waiting at a circle), then an install (the installer waits on its word).
            let why = crate::process::kist_take().map(KistWhy::Launch).or_else(|| crate::process::install_take().map(KistWhy::Install));
            if let Some(why) = why {
                let capsule = match &why {
                    KistWhy::Launch(l) => l.capsule,
                    KistWhy::Install(i) => i.capsule,
                };
                // The seals a masked kist's launch is checked against, read once for the job under the kernel lock, as every UFS command.
                let sealed = crate::process::with_lock(|| store.as_mut().and_then(|s| s.get_by_key("sealed").ok().flatten())).unwrap_or_default();
                let job = KistJob { why, capsule, sealed, total: 0, head: alloc::vec::Vec::new(), arc: None, got: 0, t0: cntpct(), waiting: false, checked: None, read: false, told: false };
                if kist_call(&job) {
                    kist_tell(&job, crate::process::INSTALL_READING, 0);
                    kist_job = Some(job);
                } else {
                    match &job.why {
                        KistWhy::Launch(l) => {
                            crate::process::push_console(alloc::format!("kist {}: not started: no vault process, or the kernel's channel to it is busy\n", core::str::from_utf8(&l.name).unwrap_or("?")).as_bytes());
                            crate::process::kist_not_started(&l.capsule, why::READ);
                            crate::process::kist_done();
                        }
                        KistWhy::Install(i) => {
                            crate::process::push_console(b"kist install: not started: no vault process, or the kernel's channel to it is busy\n");
                            crate::process::install_tell(i, crate::process::INSTALL_REFUSED, crate::process::INSTALL_WHY_READ);
                        }
                    }
                }
            }
        }
        if let Some(job) = kist_job.as_mut() {
            if let Some(r) = kist_step(job) {
                let ms = (cntpct() - job.t0) * 1000 / cntfrq();
                let line = match (&job.why, r) {
                    (KistWhy::Launch(l), Ok(_)) => alloc::format!("kist {}: started from the vault ({} bytes read in {ms} ms)\n", core::str::from_utf8(&l.name).unwrap_or("?"), job.total),
                    // Kept with its reason for the map, which init told "started" (`kist_not_started`): its circle says why. One that started under the same name meanwhile runs, and nothing is kept.
                    (KistWhy::Launch(l), Err((code, e))) => {
                        if code != crate::process::KIST_ALREADY {
                            crate::process::kist_not_started(&l.capsule, code);
                        }
                        alloc::format!("kist {}: not started: {e}\n", core::str::from_utf8(&l.name).unwrap_or("?"))
                    }
                    // Checked and sealed (SANDBOX.md "Lifecycle"): the seal joins the vault's `sealed` list, which every masked spawn reads; the installer is told once it is kept.
                    (KistWhy::Install(i), Ok(KistDone::Sealed(seal))) => {
                        let kept = crate::process::with_lock(|| {
                            let Some(s) = store.as_mut() else { return false };
                            let mut list = s.get_by_key("sealed").ok().flatten().unwrap_or_default();
                            if !list.chunks_exact(32).any(|e| e == seal) {
                                list.extend_from_slice(&seal);
                            }
                            s.set("sealed", &list).is_ok()
                        });
                        crate::process::install_tell(i, if kept { crate::process::INSTALL_DONE } else { crate::process::INSTALL_REFUSED }, if kept { 0 } else { crate::process::INSTALL_WHY_KEPT });
                        if kept {
                            alloc::format!("kist install: checked and sealed to this device ({} bytes, {ms} ms)\n", job.total)
                        } else {
                            alloc::format!("kist install: checked, but the vault did not keep its seal\n")
                        }
                    }
                    (KistWhy::Install(_), Ok(KistDone::Launched)) => alloc::string::String::new(),
                    (KistWhy::Install(i), Err((code, e))) => {
                        // A refusal the checker or the seal told already; anything else (the vault, the arc pool) is told here, by the job's reason.
                        if !job.told {
                            let told = if matches!(code, why::ROOM | why::OTHER) { crate::process::INSTALL_WHY_ROOM } else { crate::process::INSTALL_WHY_READ };
                            crate::process::install_tell(i, crate::process::INSTALL_REFUSED, told);
                        }
                        alloc::format!("kist install: not installed: {e}\n")
                    }
                };
                crate::flashlog::line(line.trim_end());
                crate::process::push_console(line.as_bytes());
                if matches!(job.why, KistWhy::Launch(_)) {
                    crate::process::kist_done();
                }
                kist_job = None;
            }
        }
        // The USB process's command handled last pass: its raw reply to the process, the kernel's own outbound reply back in place.
        if let Some(saved) = if vault_waits { None } else { proc_restore.take() } {
            platform::RAW_REPLY.store(false, core::sync::atomic::Ordering::Relaxed);
            let reply = core::mem::replace(&mut pt_out_data, saved);
            let act = core::mem::take(&mut proc_act);
            let replied = platform::RAW_REPLIED.load(core::sync::atomic::Ordering::Relaxed);
            crate::process::with_lock(|| crate::ptlink::reply(reply, replied, act));
        }
        crate::keys::serve();
        if !keys_settled && cntpct() > keys_settle {
            crate::keys::forget_missing();
            keys_settled = true;
        }
        // The killswitch needs IRQs unmasked whenever the kernel isn't deliberately masking them (only the RELOAD handover does): re-assert it every pass, so no path back from a process or a fault can leave it disarmed.
        unsafe { core::arch::asm!("msr daifclr, #2") };
        // Power + volume up: ferros restarts, as the chord leaves Android, with no bootloader (owner, 2026-11-10: "Kill everything and reboot but jump style"; restart.rs). Read from the keys' pins on each pass: either key's edge is an interrupt, which wakes this loop.
        #[cfg(not(shim_handoff))]
        {
            let chord = crate::killswitch::power_held() && crate::killswitch::vol_up_held();
            if chord && chord_armed {
                crate::restart::restart(b.stub);
            }
            chord_armed |= !chord;
        }
        // The power key turns the screen off and on (owner, 2026-10-22): a preference every process sees at once (process.rs PREF_SCREEN_OFF); the display server darkens the panel. The wake policy (proximity) comes later.
        if unsafe { core::ptr::read_volatile(&raw const crate::__power_pressed) } != 0 {
            unsafe { core::ptr::write_volatile(&raw mut crate::__power_pressed, 0) };
            let off = crate::process::pref(crate::process::PREF_SCREEN_OFF) ^ 1;
            // The proximity wake (owner's rule): a press lights the screen only if nothing is near the sensor. Fails open: with no proximity reading yet (sensors not up, or the AoC failed), the press wakes as before, so a sensor fault never locks the owner out.
            let refused = off == 0 && crate::process::pref(crate::process::PREF_PROX_WAKE) == 1 && crate::sensorlink::prox_near() == Some(true);
            if refused {
                crate::flashlog::line("wake refused: proximity near");
            } else {
                crate::process::set_pref(crate::process::PREF_SCREEN_OFF, off);
            }
        }
        let now = cntpct();
        // A command from the USB process waiting: no sleep and no time slice before it runs.
        let inbox_full = crate::ptlink::INBOX_FULL.load(core::sync::atomic::Ordering::Relaxed);
        // Home: start init (and with it the shell and the USB process) on the service loop's first pass, the moment the boot path is done. Until 2026-11-05 it waited for a second after the press (the boot ring's budget, 2026-10-22), which made every boot take at least a second whatever the work took (owner: "I CANNOT PUT BOOTS IN UNDER A SECOND IF WHEN I PRESS POWER IT TAKES EXACTLY 1S"). init's first spawn derives the wairua, which waits for KOHI's words from worker 3: that, not a timer, is what home waits on.
        // The wall clock, re-read hourly (rtc.rs), and the battery gauge every two seconds: both on this core, never inside a syscall.
        platform::crumb(0xD6);
        crate::rtc::poll();
        // The zone (zone.rs): its table read, and found again from what the radios said; then its rule.
        crate::zone::step(&mut store);
        crate::rtc::resolve_zone();
        gauge_poll();
        power_poll();
        // The heap's second region, 2 s in (main.rs heap2_join: its line invalidation kept off the boot path).
        #[cfg(not(shim_handoff))]
        if now.wrapping_sub(t_boot) > 2 * cntfrq() {
            crate::heap2_join();
        }
        crate::odpm::poll();
        // The SoC's unused power domains off, once, as the clock policy starts (pd.rs). After a jump at once, on the first pass: the jump is a killswitch, and a domain Android left on may hold a bus master still running Android's work (the GPU, a codec, the camera's chain, the TPU); its power cut stops it dead, through EL3 as every domain switch goes, where its S2MPU could not be touched safely (owner, 2026-11-04: "kill the rails").
        crate::pd::gate_at_boot(now.wrapping_sub(t_boot), if platform::jumped() { 0 } else { 16 * cntfrq() });
        platform::crumb(0xD7);
        clock_policy(false, now.wrapping_sub(t_boot));
        // The UFS link into hibernate once it has been idle 16 ms (ferros_hal ufs `h8_enter_if_idle`), under the kernel lock as every command is; every command brings it back.
        if ferros_hal::ufs::H8_ENABLED.load(core::sync::atomic::Ordering::Relaxed) {
            crate::process::with_lock(|| ferros_hal::ufs::UfsController::new(0x1320_0000).h8_enter_if_idle(cntfrq() / 64));
        }
        thermal_poll();
        platform::crumb(0xD8);
        // Diagnostic: DECON's frames and trigger through the boot (checklist.rs frame_trace, until its 64 lines are full).
        crate::checklist::frame_trace();
        if !home_started {
            // The wairua first, as its own step on the ring (init's first spawn needs it, and it waits for KOHI's words from worker 3), then home; the ring closes once init has run.
            let _ = crate::wairua::draw();
            crate::checklist::step(crate::checklist::Step::Wairua);
            home_started = true;
            crate::board::probe(6);
            let _ = start_home(&mut store);
            crate::checklist::step(crate::checklist::Step::Init);
            crate::checklist::close();
            // The boot's length to the disk as well, for the next power-on's ring.
            // Under the kernel lock, as every store access once processes run: the USB and other processes drive the flash from the other cores (unlocked, this write hung the device black on 2026-11-05).
            if let Some(doc) = crate::checklist::span_document() {
                let _ = crate::process::with_lock(|| store.as_mut().map(|s| s.set(crate::checklist::SPAN_KEY, &doc).is_ok()));
            }
        }

        if now.wrapping_sub(t_heartbeat) > 64 * freq {
            t_heartbeat = now;
            let sl: alloc::vec::Vec<u64> = crate::process::SLICES.iter().skip(4).take(5).map(|s| s.load(core::sync::atomic::Ordering::Relaxed)).collect();
            // The log region (64 MiB) holds months of these.
            crate::flashlog::line(&alloc::format!("heartbeat: up {} s, commands from the USB process {}, slices C4-C8 {:?}", now.wrapping_sub(t_boot) / freq, st.proc_cmds, sl));
        }
        // The GPU's clock follows its service (owner, 2026-11-05: the attest mark within a 120 Hz frame): at its top while a `gpu` process lives, at its floor otherwise; looked at only when the process table changed (process.rs PROCS_GEN), on this core, where every ACPM call is made. The L2 first going up and last going down (GPU.md "The GPU's clock").
        let table = crate::process::PROCS_GEN.load(core::sync::atomic::Ordering::Acquire);
        if table != unsafe { PROCS_SEEN } {
            unsafe { PROCS_SEEN = table };
            // The process count for the facts' bell (the About page's), counted only when the table changed.
            ALIVE.store(crate::process::alive() as u64, core::sync::atomic::Ordering::Relaxed);
            bell_facts();
            let live = crate::process::running(b"gpu");
            if live != unsafe { GPU_CLOCKED } {
                unsafe { GPU_CLOCKED = live };
                gpu_rate(live && unsafe { CLOCK_STATE } != 3);
            }
        }
        // No sensors process running, whatever path ended it (its end is caught in end_process and kill as well): its block is forgotten, so the kernel never reads memory that may be another process's now. init's keeper starts it again.
        if now.wrapping_sub(sensors_check) > 4 * freq {
            sensors_check = now;
            if !crate::process::running(b"sensors") {
                crate::process::with_lock(crate::sensorlink::forget);
            }
        }
        // The checklist's IPC row, once any process has had a call answered (init, which marks the others, makes none).
        if !ipc_marked && crate::process::round_trips() > 0 {
            ipc_marked = true;
            crate::checklist::mark(crate::checklist::Item::Ipc);
        }
        // The AoC's start, when the `sensors` switch has asked for it and the boot's first seconds are past (the home screen first).
        if matches!(sensor_boot, SensorBoot::Done) && now.wrapping_sub(t_boot) > SENSORS_AFTER_S * freq && crate::sensorlink::START.swap(false, core::sync::atomic::Ordering::Relaxed) {
            // After the jump from a running Android (gate answer 6) the AoC, GSA and Trusty are still in Android's session, and this start knows only the cold path: the fourth jump's service loop stopped feeding the watchdog once sensors asked (APC watchdog, 2026-11-01). Left alone until a restart path exists; the sensors process gives up after its wait.
            if crate::platform::jumped() {
                crate::process::push_console(b"sensors: started by the jump from Android; the AoC is Android's, left alone (no restart path yet)\n");
            } else {
                sensor_boot = SensorBoot::Wait;
            }
        }
        if !matches!(sensor_boot, SensorBoot::Done) {
            let st = &raw mut store;
            let mut get = |k: &str| crate::process::with_lock(|| unsafe { (*st).as_mut().and_then(|s| s.get_by_key(k).ok().flatten()) });
            sensor_boot = sensor_step(core::mem::replace(&mut sensor_boot, SensorBoot::Done), now, &mut get);
        }
        // Give one runnable process a time slice (it comes back at the latest when the slice timer fires), unless a command from the USB process waits: that runs first. `ran` is false when no process could run (all asleep or blocked): then the core sleeps below until the next event-stream tick instead of spinning the pass. With the shell up, processes are always alive, so "alive == 0" never let it sleep: the X3 spun at 2.9 GHz the whole time (owner, 2026-10-23: "the phone is a smidge warm").
        platform::crumb(0xD1);
        let ran = if !inbox_full { crate::process::run_slice() } else { false };
        platform::crumb(0xD2);
        if !inbox_full && !ran {
            let t = cntpct();
            platform::crumb(0xB3);
            // A backstop under the event stream: the EL2 timer armed 2 ms out, so a GIC interrupt ends this `wfe` even if the event stream cannot. On battery at the rest clocks the X3 went into this `wfe` and never came out — the last crumb G#B3, its event stream still on (2026-10-25, three times) — as if the core had been taken down while it waited, where only an interrupt brings a core back. Taken at EL2 the tick only stops the timer (.Lirq_tick). The event stream wakes it first every time it can, so the timer is disarmed before it fires; a sleep that ran past 1.5 ms was ended by the backstop, and DIAG counts those.
            if !backstop_ppi {
                crate::killswitch::enable_ppi(26);
                backstop_ppi = true;
            }
            // Armed at the next sleeping thread's deadline when that is sooner (process::NEXT_WAKE), so deadlines land on time however slow the event stream: with the stream every 2.7 ms (bit 15, 15-25 mW under bit 12's 333 us with the screen off, 2026-10-25) a display waiting on its next flip would otherwise wake up to a stream period late.
            let next = crate::process::NEXT_WAKE.load(core::sync::atomic::Ordering::Acquire);
            let backstop = next.saturating_sub(cntpct()).clamp(cntfrq() / 20_000, cntfrq() / 500);
            unsafe { core::arch::asm!("msr cnthp_tval_el2, {t}", "mov {c}, #1", "msr cnthp_ctl_el2, {c}", "isb", "wfe", "msr cnthp_ctl_el2, xzr", "isb", t = in(reg) backstop, c = out(reg) _) };
            platform::crumb(0xB4);
            let slept = cntpct().wrapping_sub(t);
            if slept > cntfrq() / 500 * 3 / 4 {
                BACKSTOP_WAKES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            }
            asleep += slept;
        }
        // Development, while the jump from Android is brought up: a jumped session that the host has not reached in 60 s (no command from the USB process) leaves a note and resets warm, so the next boot reads its whole log from the pstore ring (tools/husky-pstore.sh). The killswitch, the only other way out of such a session, powers off and the log decays with the RAM (the twenty-seventh jump, 2026-11-02: ferros up, its killswitch armed, no USB, the screen still Android's).
        #[cfg(not(ferros_production))]
        if platform::from_android() && st.proc_cmds == 0 && cntpct().wrapping_sub(t_boot) > 60 * cntfrq() {
            platform::pstore(b"ferros: the jump's session: no host in 60 s, resetting warm to keep this log\n");
            // Every thread and where it waits, so the log says what the session was stuck on (the twenty-ninth jump: the display server and the attest screen ran, init, touch and USB printed nothing).
            let mut threads = alloc::vec::Vec::new();
            crate::process::threads_report(&mut threads);
            platform::pstore(&threads);
            let w = |k: usize| crate::smp::IDLE_WAKES[k].load(core::sync::atomic::Ordering::Relaxed);
            platform::pstore(alloc::format!("workers: up G#{:X}, idle wakes [{}, {}, {}, {}]; x3 parked {}\n", crate::smp::up_mask(), w(0), w(1), w(2), w(3), crate::x3::parked()).as_bytes());
            // Per core, the hand-off words the next cold boot overwrites (the fourth jump of 2026-11-03: the X3 and two workers took their entries and were never heard from): the worker's progress (G#FD60_0B00, low byte the step: 1 arrived, 2 GIC armed, 3 EL0 on, 4-6 loop passes, 8 first slice), its arrival in Rust (0B80), and the hv's two trace words (0C00, 0C80), all in counter ticks.
            for c in 0..9usize {
                let r = |base: usize| unsafe { core::ptr::read_volatile((base + c * 8) as *const u64) };
                platform::pstore(alloc::format!("core {}: step {} at G#{:X}, arrived G#{:X}, hv G#{:X} G#{:X}\n", c, r(0xFD60_0B00) & 0xFF, r(0xFD60_0B00) & !0xFF, r(0xFD60_0B80), r(0xFD60_0C00), r(0xFD60_0C80)).as_bytes());
            }
            // The hand-over blocks the X3 (G#819F_F100) and the workers (G#819F_F200 + k * G#80) read with their MMU off, beside this core's own registers: the jump freezes Android's CPUs but not its DMA masters, so a block that differs from what was written says a device wrote into ferros's memory (the theory for the 2026-11-03 jumps whose cores died before their first Rust line).
            let (mair, tcr, ttbr, sctlr, hcr): (u64, u64, u64, u64, u64);
            unsafe { core::arch::asm!("mrs {a}, mair_el2", "mrs {b}, tcr_el2", "mrs {c}, ttbr0_el2", "mrs {d}, sctlr_el2", "mrs {e}, hcr_el2", a = out(reg) mair, b = out(reg) tcr, c = out(reg) ttbr, d = out(reg) sctlr, e = out(reg) hcr) };
            platform::pstore(alloc::format!("live: mair G#{:X} tcr G#{:X} ttbr G#{:X} sctlr G#{:X} hcr G#{:X}\n", mair, tcr, ttbr, sctlr, hcr).as_bytes());
            for (name, base) in [("x3", 0x819F_F100usize), ("w0", 0x819F_F200), ("w1", 0x819F_F280), ("w2", 0x819F_F300), ("w3", 0x819F_F380)] {
                let r = |o: usize| unsafe { core::ptr::read_volatile((base + o) as *const u64) };
                platform::pstore(alloc::format!("{}: mair G#{:X} tcr G#{:X} ttbr G#{:X} sctlr G#{:X} hcr G#{:X} vbar G#{:X} cptr G#{:X} stack G#{:X} cnthctl G#{:X}\n", name, r(0), r(8), r(16), r(24), r(32), r(40), r(48), r(56), r(88)).as_bytes());
            }
            platform::pstore(&crate::process::take_console());
            platform::reboot();
        }
        // A command the USB process handed over (ptlink.rs, USB.md): the one dispatch below. Its handlers end with `continue`, so what follows — the raw reply handed back — runs at the top of the next pass (`proc_restore`).
        let Some(proc_cmd) = crate::process::with_lock(crate::ptlink::take_inbox) else { continue };
        platform::crumb(0xD4);
        // Every command comes from the process now (the kernel's own USB went 2026-11-01); the name stays for the handlers that answer first and act later.
        let from_proc = true;
        // The process's second submission of a command that answered first (reload, reboot, `diag android`): now act.
        let pt_act = proc_cmd.act;
        let payload: &[u8] = proc_cmd.payload.bytes();
        st.proc_cmds += 1;
        proc_restore = Some(core::mem::take(&mut pt_out_data));
        platform::RAW_REPLY.store(true, core::sync::atomic::Ordering::Relaxed);
        platform::RAW_REPLIED.store(false, core::sync::atomic::Ordering::Relaxed);
        if let Some(cmd) = ferros_pt::command::parse(payload) {
            st.last_cmd = u32::from_le_bytes([cmd.cap[0], cmd.cap[1], cmd.cap[2], cmd.cap[3]]) as u64 | (cmd.op as u64) << 56;
            platform::crumb(0xD5 | (st.last_cmd & 0xFFFF_FFFF) << 8 | (cmd.op as u64) << 40);
            // Production builds (FERROS_PRODUCTION=1) close the development doors over USB, for the locked bootloader (BOOT.md): a cable must not be able to reload the kernel, run code, read or write memory or the vault, read the screen, spawn, or drive a diagnostic, since ABL checks only what it boots. Open: a reboot, and DIAG with no parameters (status, no secrets). The portal will grant the rest to the owner's identity (STATUS.md 7); until then they are closed outright.
            let open = !cfg!(ferros_production) || (cmd.cap == cap_reboot && cmd.op == ferros_pt::Op::Exec) || (cmd.cap == cap_diag && cmd.op == ferros_pt::Op::Read && cmd.params.is_empty());
            if !open {
                queue_pt_response(&mut pt_out_data, b"closed: a production build answers only a reboot and a plain DIAG over USB\n");
            } else if cmd.cap == cap_diag && cmd.op == ferros_pt::Op::Read {
                let mut resp: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
                // Development tools: not in a production build at all (FERROS_PRODUCTION=1, build.rs), which answers only the plain DIAG below and a reboot.
                #[cfg(not(ferros_production))]
                {
                // The camera's power domain is gated after boot (pd.rs; the TPU's too, its tools gone 2026-11-01). Its diags refuse while they are: bringing a domain back without Linux's saved CMU registers is not yet trusted (a TPU on/off by hand preceded a reset in the next power window, 2026-10-25). `diag pd on <name>` does it by hand.
                if let Some(name) = if cmd.params.starts_with(b"cam") { Some("ispfe") } else { None } {
                    if !crate::pd::is_on(name) {
                        resp.extend_from_slice(alloc::format!("the {name} power domain is gated after boot (pd.rs); `diag pd on {name}` first (experimental: see pd.rs)\n").as_bytes());
                        queue_pt_response(&mut pt_out_data, &resp);
                        continue;
                    }
                }
                if cmd.params.starts_with(b"lun ") {
                    // Hex dump of UFS blocks: `diag lun <lun> <first lba> <count>`, non-zero 16-byte rows only (LU3: devinfo LBA 6, pinfo 8, blenv 10; LU0 partitions at the LBAs `diag ufsdesc` prints) — for finding where the bootloader keeps the active slot besides devinfo (2026-10-17). devinfo carries the device's identity (serials, IMEIs, MACs): don't paste it anywhere.
                    let mut it = cmd.params[4..].split(|&c| c == b' ').filter_map(|t| core::str::from_utf8(t).ok()?.parse::<u32>().ok());
                    let (lun, first, count) = (it.next().unwrap_or(3).min(7) as u8, it.next().unwrap_or(6), it.next().unwrap_or(1).min(8));
                    // Under the kernel lock, as every UFS command: processes on the other cores drive the flash too (the vault process's blocks, the partitions' readers).
                    crate::process::with_lock(|| {
                        let ufs = ferros_hal::ufs::UfsController::new(0x1320_0000);
                        let hex = b"0123456789ABCDEF";
                        for lba in first..first + count {
                            let mut ok = false;
                            for _ in 0..4 {
                                if ufs.read_block_lun(lun, lba) == 0 && ufs.last_response_status() == 0 {
                                    ok = true;
                                    break;
                                }
                            }
                            resp.extend_from_slice(alloc::format!("LBA {lba}{}\n", if ok { "" } else { " unreadable" }).as_bytes());
                            if !ok {
                                continue;
                            }
                            let b = ufs.data_buffer();
                            for (i, row) in b.chunks(16).enumerate() {
                                if row.iter().any(|&x| x != 0) {
                                    resp.extend_from_slice(alloc::format!("  {:04X}:", i * 16).as_bytes());
                                    for &x in row {
                                        resp.push(b' ');
                                        resp.push(hex[(x >> 4) as usize]);
                                        resp.push(hex[(x & 15) as usize]);
                                    }
                                    resp.push(b'\n');
                                }
                            }
                        }
                    });
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params.starts_with(b"bootlun ") {
                    // `diag bootlun <1|2>`: write the UFS device's bBootLunEn attribute (1 = Boot LU A, 2 = Boot LU B) and read it back. The ROM boots the bootloader chain from that LU, and the bootloader's "boot slot" follows the LU's letter before devinfo is read (its log: "ufshc: bBootLunEn 0x2" then "boot slot: b"; devinfo's active flag gets re-synced to it). Only 1 or 2 are accepted: 0 disables boot.
                    // Under the kernel lock, as every UFS command: processes on the other cores drive the flash too (the vault process's blocks, the partitions' readers).
                    crate::process::with_lock(|| {
                        let ufs = ferros_hal::ufs::UfsController::new(0x1320_0000);
                        let before = ufs.read_attribute(0x00, 0);
                        let want = cmd.params[8..].first().map(|&c| c.wrapping_sub(b'0') as u32);
                        let line = match want {
                            Some(v @ 1..=2) => {
                                let ok = ufs.write_attribute(0x00, 0, v);
                                alloc::format!("bBootLunEn before {before:?}, write {v}: {}, after {:?}\n", if ok { "accepted" } else { "REFUSED" }, ufs.read_attribute(0x00, 0))
                            }
                            _ => alloc::format!("bBootLunEn {before:?}; only 1 or 2 may be written\n"),
                        };
                        resp.extend_from_slice(line.as_bytes());
                    });
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                // The handoff build has no `slot`: marking an A/B slot is a cold-boot job, and a ferros chainloaded out of a running Android must not rewrite devinfo under it.
                #[cfg(not(shim_handoff))]
                if cmd.params == b"android" || cmd.params == b"android here" {
                    // From the USB process: reply first, and reset when it asks again with its reply sent (ptlink.rs).
                    if from_proc && !pt_act {
                        queue_pt_response(&mut pt_out_data, b"android: resetting once this reply is out\n");
                        proc_act = true;
                        continue;
                    }
                    // The gate's Android path from the host, for bisecting it: slot a active in devinfo, then the plain PSCI reset. The bootloader boots the active slot. `android here` leaves the slot as it is, so the gate of the slot that booted answers, as volume down there does (how Android from slot b is tested, 2026-10-28).
                    let r = if cmd.params == b"android here" { crate::slot::MARKED } else { crate::process::with_lock(|| crate::slot::set_active(0)) };
                    // Tell slot a's boot gate (ferros_hv/src/gate.rs) to answer "Android" this once instead of auto-choosing ferros; cleaned to DRAM, since the gate reads it with the caches off.
                    unsafe {
                        let p = 0xFD60_0FF8usize as *mut u64;
                        core::ptr::write_volatile(p, 0x414E_4452_4F49_4431);
                        core::arch::asm!("dc civac, {a}", "dsb sy", a = in(reg) p as u64);
                    }
                    resp.extend_from_slice(alloc::format!("set_active(a) result {r}; {}\n", if r == crate::slot::MARKED { "resetting" } else { "not resetting" }).as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    if r == crate::slot::MARKED {
                        // The reply went out through the USB process before it asked to act.
                        platform::crumb(0xC7);
                        platform::reboot();
                    }
                    continue;
                }
                if cmd.params == b"recovery" {
                    // `diag recovery`: Android's recovery, once. The gate refuses a recovery boot unless this word is there (ferros_hv gate.rs RECOVERY_ONCE: no reflash without the handle, owner 2026-11-01); ferros leaves it, cleaned to DRAM, then resets into the bootloader's recovery mode. A development door for now; the attest-gated setting comes with the UI.
                    if from_proc && !pt_act {
                        queue_pt_response(&mut pt_out_data, b"recovery: resetting into recovery once this reply is out\n");
                        proc_act = true;
                        continue;
                    }
                    unsafe {
                        let p = 0xFD60_0FF0usize as *mut u64;
                        core::ptr::write_volatile(p, 0x5245_434F_5645_5231);
                        core::arch::asm!("dc civac, {a}", "dsb sy", a = in(reg) p as u64);
                    }
                    platform::crumb(0xC7);
                    let r = platform::reboot_to_mode(platform::REBOOT_MODE_RECOVERY);
                    resp.extend_from_slice(alloc::format!("recovery: the EL3 monitor refused the reboot mode ({r})\n").as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"devinfo" {
                    // The bootloader's slot record as it stands (slot.rs): both words, retries byte and flags byte (b0 unbootable, b1 successful, b2 active, b3 fastboot-ok).
                    // Under the kernel lock, as every UFS command: processes on the other cores drive the flash too (the vault process's blocks, the partitions' readers).
                    crate::process::with_lock(|| {
                        let ufs = ferros_hal::ufs::UfsController::new(0x1320_0000);
                        let mut ok = false;
                        for _ in 0..4 {
                            if ufs.read_block_lun(3, 6) == 0 && ufs.last_response_status() == 0 {
                                ok = true;
                                break;
                            }
                        }
                        if ok {
                            let b = ufs.data_buffer();
                            let w = |s: usize| u32::from_le_bytes([b[0x30 + 4 * s], b[0x31 + 4 * s], b[0x32 + 4 * s], b[0x33 + 4 * s]]);
                            resp.extend_from_slice(alloc::format!("DEVINFO magic {}{}{}{} major {}: slot a word G#{:08X} (retries {}, flags G#{:02X}), slot b word G#{:08X} (retries {}, flags G#{:02X})\n", b[0] as char, b[1] as char, b[2] as char, b[3] as char, u16::from_le_bytes([b[4], b[5]]), w(0), w(0) & 0xFF, (w(0) >> 8) & 0xFF, w(1), w(1) & 0xFF, (w(1) >> 8) & 0xFF).as_bytes());
                        } else {
                            resp.extend_from_slice(b"DEVINFO unreadable\n");
                        }
                    });
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"ufsdesc" {
                    // The UFS device's provisioning, read from the device itself: the Device descriptor (LU count, feature bits), every Unit descriptor (enabled, boot LUN id, WRITE PROTECT TYPE: 0 none / 1 power-on / 2 permanent, block size, block count), and each enabled LU's GPT partition names — so the boot-chain write-protect question (BOOT.md, 2026-10-17) is answered by the flash, not by guessing.
                    // Under the kernel lock, as every UFS command: processes on the other cores drive the flash too (the vault process's blocks, the partitions' readers).
                    crate::process::with_lock(|| {
                        let ufs = ferros_hal::ufs::UfsController::new(0x1320_0000);
                        let (d, n, ocs) = ufs.read_descriptor(0x00, 0);
                        resp.extend_from_slice(alloc::format!("DEVICE_DESC len {n} ocs {ocs}: numLU {} bootEnable {} descrAccessEn {} features G#{:02X}{:02X}\n", d.get(0x08).copied().unwrap_or(0), d.get(0x05).copied().unwrap_or(0), d.get(0x06).copied().unwrap_or(0), d.get(0x1F).copied().unwrap_or(0), d.get(0x1E).copied().unwrap_or(0)).as_bytes());
                        // The provisioning lock and the write-protect flags (JEDEC UFS: attribute bConfigDescrLock G#1B is one-time — once 1 the LU layout is permanent; flags fPermanentWPEn G#02 and fPowerOnWPEn G#03; attribute bBootLunEn G#00).
                        let fmt = |v: Option<u32>| match v { Some(x) => alloc::format!("{x}"), None => alloc::string::String::from("?") };
                        let fmtb = |v: Option<bool>| match v { Some(x) => alloc::format!("{}", x as u8), None => alloc::string::String::from("?") };
                        resp.extend_from_slice(alloc::format!("bConfigDescrLock {} bBootLunEn {} bCurrentPowerMode {} fPermanentWPEn {} fPowerOnWPEn {} fDeviceInit {}\n", fmt(ufs.read_attribute(0x1B, 0)), fmt(ufs.read_attribute(0x00, 0)), fmt(ufs.read_attribute(0x02, 0)), fmtb(ufs.read_flag(0x02)), fmtb(ufs.read_flag(0x03)), fmtb(ufs.read_flag(0x01))).as_bytes());
                        let (c, cn, cocs) = ufs.read_descriptor(0x01, 0);
                        resp.extend_from_slice(alloc::format!("CONFIG_DESC len {cn} ocs {cocs}: bootEnable {} descrAccessEn {} initPowerMode {} highPriorityLUN {}\n", c.get(0x03).copied().unwrap_or(0), c.get(0x04).copied().unwrap_or(0), c.get(0x05).copied().unwrap_or(0), c.get(0x06).copied().unwrap_or(0)).as_bytes());
                        for lu in 0..8u8 {
                            let (u, n, ocs) = ufs.read_descriptor(0x02, lu);
                            if n < 0x13 || ocs != 0 {
                                resp.extend_from_slice(alloc::format!("LU{lu}: no descriptor (len {n}, ocs {ocs})\n").as_bytes());
                                continue;
                            }
                            let blocks = u64::from_be_bytes([u[0x0B], u[0x0C], u[0x0D], u[0x0E], u[0x0F], u[0x10], u[0x11], u[0x12]]);
                            let bs = 1u64 << u[0x0A];
                            let wp = match u[0x05] { 0 => "none", 1 => "POWER-ON", 2 => "PERMANENT", _ => "?" };
                            resp.extend_from_slice(alloc::format!("LU{lu}: enable {} bootLunId {} writeProtect {} memType {} blockSize {bs} blocks {blocks} = {} MiB\n", u[0x03], u[0x04], wp, u[0x08], blocks * bs >> 20).as_bytes());
                            if u[0x03] == 0 {
                                continue;
                            }
                            // GPT: header at LBA 1, the entry array where it says (usually LBA 2), 128-byte entries, UTF-16LE names at +56.
                            if ufs.read_block_lun(lu, 1) != 0 || ufs.last_response_status() != 0 {
                                resp.extend_from_slice(b"    (LBA 1 unreadable)\n");
                                continue;
                            }
                            let h = *ufs.data_buffer();
                            if &h[0..8] != b"EFI PART" {
                                resp.extend_from_slice(b"    (no GPT)\n");
                                continue;
                            }
                            let entry_lba = u64::from_le_bytes(h[0x48..0x50].try_into().unwrap());
                            let count = u32::from_le_bytes(h[0x50..0x54].try_into().unwrap()).min(256) as usize;
                            let esize = u32::from_le_bytes(h[0x54..0x58].try_into().unwrap()) as usize;
                            let per_block = if esize > 0 { 4096 / esize } else { 0 };
                            let mut names = alloc::string::String::new();
                            let mut i = 0usize;
                            while i < count && per_block > 0 {
                                if ufs.read_block_lun(lu, (entry_lba as usize + i / per_block) as u32) != 0 {
                                    break;
                                }
                                let blk = *ufs.data_buffer();
                                for e in 0..per_block {
                                    if i >= count {
                                        break;
                                    }
                                    let ent = &blk[e * esize..(e + 1) * esize];
                                    if ent[..16].iter().any(|&b| b != 0) {
                                        let name: alloc::string::String = ent[56..128].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).filter_map(|c| char::from_u32(c as u32)).collect();
                                        let first = u64::from_le_bytes(ent[32..40].try_into().unwrap());
                                        let last = u64::from_le_bytes(ent[40..48].try_into().unwrap());
                                        let attr = u64::from_le_bytes(ent[48..56].try_into().unwrap());
                                        names.push_str(&alloc::format!(" {name}@{first}({}K)", (last + 1 - first) * bs >> 10));
                                        if attr != 0 {
                                            // GPT attribute bits (Android's A/B HALs keep slot state in bits 48+ on some SoCs); printed only when set, to see whether husky's bootloader uses them.
                                            names.push_str(&alloc::format!("[attr G#{attr:X}]"));
                                        }
                                    }
                                    i += 1;
                                }
                            }
                            resp.extend_from_slice(alloc::format!("    GPT:{names}\n").as_bytes());
                        }
                    });
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"rep" {
                    // The eUSB2 repeater's whole 8-bit register map over I2C (HSI2C11, address from the DT), one "REP_xx=" line per register; G#8000_xxxx = the read failed (TRANS_STATUS low bits). For diffing cold vs handoff state.
                    let i2c = ferros_hal::hsi2c::Hsi2c::new(ferros_hal::hsi2c::PIXEL8_HSI2C11_BASE);
                    i2c.init();
                    let hex = b"0123456789ABCDEF";
                    for reg in 0..=255u8 {
                        let v = match i2c.read_reg(ferros_hal::hsi2c::PIXEL8_EUSB_REPEATER_ADDR, reg) {
                            Ok(v) => v as u32,
                            Err(t) => 0x8000_0000 | (t & 0xFFFF),
                        };
                        let label = [b'R', b'E', b'P', b'_', hex[(reg >> 4) as usize], hex[(reg & 0xF) as usize], b'='];
                        append_hex(&mut resp, &label, v);
                    }
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                #[cfg(not(shim_handoff))]
                if cmd.params == b"ready" && b.kind == BootKind::Cold {
                    // Redraw the "Ready?" screen (dev check; the checklist comes back on the next boot) and count what landed.
                    let (text, tri) = crate::ready::show();
                    append_hex(&mut resp, b"READY_TEXT_PX=", text);
                    append_hex(&mut resp, b"READY_TRIANGLE_PX=", tri);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"heap peak reset" {
                    // `diag heap peak reset`: a new heap high-water mark from now (HEAP_PEAK in DIAG).
                    ferros_heap::reset_peak();
                    resp.extend_from_slice(b"heap peak reset\n");
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params.starts_with(b"key ") {
                    // `diag key power` (docs/INPUT-INJECT.md, `ferros-bridge input key`): a press of the power key put where its interrupt puts one (`__power_pressed`, main.rs `.Lirq_power`), so the screen toggle below and the proximity wake rule take it as they take a finger's. Refused while a volume key is physically down: the press would make a chord with it (power + volume up is the hv's jump under Android, both volume keys the kill), and no chord is ever injected. The volume keys themselves have nothing to inject into: the killswitch and the gate read their pins' levels, and nothing queues their presses.
                    if cmd.params == b"key power" {
                        if crate::killswitch::volume_key_down() {
                            resp.extend_from_slice(b"key power refused: a volume key is held, and a press now would be a chord\n");
                        } else {
                            unsafe { core::ptr::write_volatile(&raw mut crate::__power_pressed, 1) };
                            resp.extend_from_slice(b"key power pressed\n");
                        }
                    } else {
                        resp.extend_from_slice(b"key: only `key power`; the volume keys are read as levels (the killswitch, the gate) and nothing queues their presses\n");
                    }
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"console" {
                    // What processes have sent their consoles since the last read (background slices included), and how many are alive.
                    append_hex(&mut resp, b"ALIVE=", crate::process::alive() as u32);
                    resp.extend_from_slice(&crate::process::take_console());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"cpu" {
                    // The CPU's own feature registers (this core): what the instruction set offers, read rather than assumed — the 1:1 target for codegen, the checker and toka. 64-bit values as HI/LO pairs.
                    macro_rules! id {
                        ($name:literal, $reg:literal) => {{
                            let v: u64;
                            unsafe { core::arch::asm!(concat!("mrs {}, ", $reg), out(reg) v) };
                            append_hex(&mut resp, concat!($name, "_HI=").as_bytes(), (v >> 32) as u32);
                            append_hex(&mut resp, concat!($name, "_LO=").as_bytes(), v as u32);
                        }};
                    }
                    id!("MIDR", "midr_el1");
                    id!("MPIDR", "mpidr_el1");
                    id!("ISAR0", "id_aa64isar0_el1");
                    id!("ISAR1", "id_aa64isar1_el1");
                    id!("ISAR2", "s3_0_c0_c6_2");
                    id!("PFR0", "id_aa64pfr0_el1");
                    id!("PFR1", "id_aa64pfr1_el1");
                    id!("ZFR0", "s3_0_c0_c4_4");
                    id!("SMFR0", "s3_0_c0_c4_5");
                    id!("MMFR0", "id_aa64mmfr0_el1");
                    id!("MMFR1", "id_aa64mmfr1_el1");
                    id!("MMFR2", "id_aa64mmfr2_el1");
                    id!("DFR0", "id_aa64dfr0_el1");
                    id!("CTR", "ctr_el0");
                    id!("DCZID", "dczid_el0");
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"bench" {
                    // Memory speed, for the cache map's before/after: 8 copies of 512 KiB (4 MiB moved) and BLAKE3 over 512 KiB, timed in microseconds, plus SCTLR_EL2 (M b0, C b2, I b12).
                    let n = 512 * 1024;
                    let src = alloc::vec![0x5Au8; n];
                    let mut dst = alloc::vec![0u8; n];
                    let us = |t: u64| (t * 1_000_000 / freq) as u32;
                    let t0 = cntpct();
                    for _ in 0..8 {
                        dst.copy_from_slice(&src);
                        core::hint::black_box(&mut dst);
                    }
                    let t1 = cntpct();
                    let h = blake3::hash(core::hint::black_box(&src));
                    let t2 = cntpct();
                    let sctlr: u64;
                    unsafe { core::arch::asm!("mrs {}, sctlr_el2", out(reg) sctlr) };
                    append_hex(&mut resp, b"BENCH_COPY_4MIB_US=", us(t1 - t0));
                    append_hex(&mut resp, b"BENCH_BLAKE3_512KIB_US=", us(t2 - t1));
                    append_hex(&mut resp, b"BENCH_HASH0=", h.as_bytes()[0] as u32);
                    append_hex(&mut resp, b"SCTLR_EL2=", sctlr as u32);
                    resp.extend_from_slice(b"END\n");
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"devinfo" {
                    // The bootloader's A/B slot record: devinfo is UFS LUN 3 (sdd1), starting at 512-byte sector 48 = 4 KiB LBA 6. Only the 128-byte header goes out ("DEVINFO_xx=" one 32-bit word per line): the rest of the partition is factory identity data (serials, MACs, IMEIs) that has no business on the wire. Needs the disk up (any boot that opened the vault).
                    if store.is_none() {
                        resp.extend_from_slice(b"no disk on this boot\n");
                    } else {
                        // Under the kernel lock, as every UFS command: processes on the other cores drive the flash too (the vault process's blocks, the partitions' readers).
                        crate::process::with_lock(|| {
                            let ufs = ferros_hal::ufs::UfsController::new(0x1320_0000);
                            // A LUN's first command after power-on can report UNIT ATTENTION; it succeeds on retry.
                            let mut ocs = 0xFF;
                            for _ in 0..4 {
                                ocs = ufs.read_block_lun(3, 6);
                                if ocs == 0 && ufs.last_response_status() == 0 {
                                    break;
                                }
                            }
                            append_hex(&mut resp, b"DEVINFO_OCS=", ocs as u32 | (ufs.last_response_status() as u32) << 8);
                            let d = ufs.data_buffer();
                            let hx = b"0123456789ABCDEF";
                            for i in 0..32 {
                                let w = u32::from_le_bytes([d[i * 4], d[i * 4 + 1], d[i * 4 + 2], d[i * 4 + 3]]);
                                let key = [b'D', b'E', b'V', b'I', b'N', b'F', b'O', b'_', hx[i * 4 >> 4], hx[i * 4 & 15], b'='];
                                append_hex(&mut resp, &key, w);
                            }
                            // Fingerprint of everything past the header (rest of LBA 6 + all of LBA 7): tells whether a slot change touched anything else (a checksum, a mirror) without sending the identity data itself.
                            let mut h = blake3::Hasher::new();
                            h.update(&d[0x80..]);
                            let mut ocs7 = ufs.read_block_lun(3, 7);
                            if ocs7 != 0 || ufs.last_response_status() != 0 {
                                ocs7 = ufs.read_block_lun(3, 7);
                            }
                            h.update(ufs.data_buffer());
                            let f = h.finalize();
                            let fb = f.as_bytes();
                            append_hex(&mut resp, b"DEVINFO_REST_OCS=", ocs7 as u32);
                            append_hex(&mut resp, b"DEVINFO_REST_B3=", u32::from_be_bytes([fb[0], fb[1], fb[2], fb[3]]));
                        });
                    }
                    resp.extend_from_slice(b"END\n");
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"boot" {
                    // `diag boot`: reset-to-entry, then each boot step's time since ferros was entered (checklist.rs).
                    resp.extend_from_slice(crate::checklist::boot_times().as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"cam uw scan" || cmd.params == b"cam uw scan init" {
                    crate::camera::uw_scan(&mut resp, cmd.params == b"cam uw scan init");
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"cam front up" {
                    // `diag cam front up`: power the front camera in its own sequence and read its identifier (camera.rs).
                    crate::camera::front_up(&mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"cam uw up" {
                    // `diag cam uw up`: power the ultrawide in its own sequence and read its identifier (camera.rs).
                    crate::camera::uw_up(&mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params.starts_with(b"dvfs") {
                    // `diag dvfs`: which core the kernel runs on, its clock measured (2^20 dependent adds), and every DVFS domain's rate as the ACPM firmware holds it. `diag dvfs set <domain index> <kHz>`: ask the firmware for a rate (acpm::dvfs_set), then measure again.
                    let measure = || {
                        let t0 = cntpct();
                        let mut acc = 0u64;
                        for _ in 0..(1u64 << 20) {
                            unsafe { core::arch::asm!("add {0}, {0}, #1", inout(reg) acc) };
                        }
                        let us = ((cntpct() - t0) * 1_000_000 / cntfrq()).max(1);
                        (1u64 << 20) / us + (acc & 0)
                    };
                    let words: alloc::vec::Vec<&[u8]> = cmd.params.split(|&b| b == b' ').filter(|w| !w.is_empty()).collect();
                    // `diag dvfs policy on|off`: the rest/active clock policy (clock_policy) runs, or holds off so `dvfs set` sticks.
                    // `diag dvfs profile rest|active`: one of the policy's operating points by hand.
                    if words.len() == 3 && words[1] == b"profile" {
                        clock_profile(match words[2] { b"active" => 2, b"dark" => 3, _ => 1 });
                    }
                    if words.len() == 3 && words[1] == b"policy" {
                        unsafe {
                            CLOCK_POLICY_ON = words[2] == b"on";
                            if CLOCK_POLICY_ON {
                                CLOCK_STATE = 0;
                            }
                        }
                    }
                    resp.extend_from_slice(alloc::format!("clock policy {}, state {} (1 rest, 2 active, 3 dark), {} switches, longest request {} us; display frame starts {}, workers up G#{:X}\n", if unsafe { CLOCK_POLICY_ON } { "on" } else { "off" }, unsafe { CLOCK_STATE }, unsafe { CLOCK_SWITCHES }, unsafe { CLOCK_WAIT_MAX_US }, crate::process::FRAME_STARTS.load(core::sync::atomic::Ordering::Relaxed), crate::smp::UP.load(core::sync::atomic::Ordering::Relaxed)).as_bytes());
                    if words.len() == 4 && words[1] == b"set" {
                        let num = |w: &[u8]| core::str::from_utf8(w).ok().and_then(|t| t.parse::<u32>().ok());
                        match (num(words[2]), num(words[3])) {
                            (Some(d), Some(khz)) if (d as usize) < crate::acpm::DVFS_DOMAINS.len() => {
                                let r = crate::acpm::dvfs_set(d, khz);
                                // The firmware takes a moment to move the PLL; measure after it has.
                                let t = cntpct() + cntfrq() / 100;
                                while cntpct() < t {}
                                resp.extend_from_slice(alloc::format!("set {} to A#{khz} kHz: {r:?}\n", crate::acpm::DVFS_DOMAINS[d as usize]).as_bytes());
                            }
                            _ => resp.extend_from_slice(b"cpu set <domain index> <kHz>\n"),
                        }
                    }
                    let mpidr: u64;
                    unsafe { core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr) };
                    resp.extend_from_slice(alloc::format!("kernel core MPIDR G#{mpidr:X}, clock ~A#{} MHz\n", measure()).as_bytes());
                    for (i, name) in crate::acpm::DVFS_DOMAINS.iter().enumerate() {
                        match crate::acpm::dvfs_get(i as u32) {
                            Ok(k) => resp.extend_from_slice(alloc::format!("{i:>2} {name:<7} A#{k} kHz\n").as_bytes()),
                            Err(e) => resp.extend_from_slice(alloc::format!("{i:>2} {name:<7} error G#{e:X}\n").as_bytes()),
                        }
                    }
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if let Some(k) = cmd.params.strip_prefix(b"core ") {
                    // `diag core <k>`: SGI 2 to core k (MPIDR Aff1 = k, Aff0 = 0), whose IRQ handler dumps its position; a moment later the dump is printed.
                    let k = core::str::from_utf8(k).ok().and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(99);
                    if k < 16 {
                        unsafe {
                            core::ptr::write_volatile(&raw mut crate::__core_dump[k as usize], [0; 16]);
                            core::arch::asm!("msr S3_0_C12_C11_5, {v}", "isb", v = in(reg) (k << 16) | (2u64 << 24) | 1);
                        }
                        let t = cntpct();
                        while cntpct().wrapping_sub(t) < cntfrq() / 100 {}
                        let d = unsafe { core::ptr::read_volatile(&raw const crate::__core_dump[k as usize]) };
                        resp.extend_from_slice(alloc::format!("core A#{k}: ELR G#{:X} LR G#{:X} FP G#{:X} SPSR G#{:X} SP G#{:X} SP_EL0 G#{:X} x0 G#{:X} x1 G#{:X} x2 G#{:X} x3 G#{:X}\n", d[0], d[9], d[8], d[1], d[2], d[3], d[4], d[5], d[6], d[7]).as_bytes());
                        if d[0] == 0 {
                            resp.extend_from_slice(b"(no dump: the core did not take the SGI within 10 ms)\n");
                        }
                    }
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"rtc" {
                    // `diag rtc`: the PMIC's clock read now, the unix time it gives, the base the kernel keeps, and the zone.
                    match crate::rtc::read() {
                        Ok((y, mo, d, h, mi, s)) => resp.extend_from_slice(alloc::format!("rtc: {y}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02} UTC\n").as_bytes()),
                        Err(e) => resp.extend_from_slice(alloc::format!("rtc: read failed, ACPM G#{e:X}\n").as_bytes()),
                    }
                    let _ = crate::rtc::sync();
                    let base = crate::rtc::BASE.load(core::sync::atomic::Ordering::Relaxed);
                    resp.extend_from_slice(alloc::format!("unix now A#{}; zone A#{} s east of UTC\n", base.wrapping_add(cntpct()) / cntfrq(), crate::rtc::TZ.load(core::sync::atomic::Ordering::Relaxed)).as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"zone" || cmd.params.starts_with(b"zone ") {
                    // `diag zone`: the zone in effect, both choices, what was found and what the radios said (zone.rs). `diag "zone net <mcc> [<offset s east> [<dst 0|1>]]"` and `diag "zone at <lat arc-s> <lon arc-s>"` hand in a clue as the modem and the GPS driver do, for a bench without either.
                    let words: alloc::vec::Vec<&str> = core::str::from_utf8(&cmd.params[4..]).unwrap_or("").split_whitespace().collect();
                    let num = |k: usize| words.get(k).and_then(|w| w.parse::<i64>().ok());
                    // WHY the bounds: the numbers are typed at the bridge; an MCC is three digits, an offset within a day, a place on the earth, as SYS_ZONE holds the radios to.
                    match (words.first().copied(), num(1), num(2)) {
                        (Some("net"), Some(mcc), off) if (0..1000).contains(&mcc) => {
                            let dst = num(3).map(|d| d == 1);
                            crate::process::with_lock(|| crate::zone::network(mcc as u16, off.filter(|o| o.unsigned_abs() < 86_400).map(|o| (o as i32, dst))));
                            resp.extend_from_slice(b"zone: the network's word handed in; found at the next pass\n");
                        }
                        (Some("at"), Some(lat), Some(lon)) if lat.unsigned_abs() <= 90 * 3600 && lon.unsigned_abs() <= 180 * 3600 => {
                            crate::process::with_lock(|| crate::zone::position((lat << 23) / 3600, (lon << 23) / 3600));
                            resp.extend_from_slice(b"zone: a position handed in; found at the next pass\n");
                        }
                        (None, _, _) => {}
                        _ => resp.extend_from_slice(b"zone: net <mcc> [<offset s east> [<dst 0|1>]], or at <lat arc-s> <lon arc-s>\n"),
                    }
                    resp.extend_from_slice(crate::zone::describe().as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if let Some(c) = cmd.params.strip_prefix(b"thermal at ").and_then(|t| core::str::from_utf8(t).ok()).and_then(|t| t.trim().parse::<u8>().ok()) {
                    THROTTLE_AT.store(c.max(20), core::sync::atomic::Ordering::Relaxed);
                    resp.extend_from_slice(alloc::format!("throttle at A#{} C, release below A#{} C (takes effect within a second)\n", c.max(20), c.max(20) - 10).as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"bootconfig" {
                    // `diag bootconfig`: ABL's bootconfig for this boot, as kept at entry (modem.rs `keep_bootconfig`): normal and recovery boots compared, for the gate's recovery check.
                    match unsafe { (*(&raw const crate::modem::BOOTCONFIG)).as_ref() } {
                        Some(t) => resp.extend_from_slice(t),
                        None => resp.extend_from_slice(b"no bootconfig kept (not a real boot, or none found)\n"),
                    }
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"facts" {
                    // `diag facts`: the sensor words of SYS_FACTS as a process receives them, and whether the sensors process has published its block (sensorlink.rs).
                    let v = crate::process::with_lock(facts);
                    let fl = |w: u64| f32::from_bits(w as u32);
                    resp.extend_from_slice(alloc::format!("source: {}\ngravity ({:.3}, {:.3}, {:.3}) m/s², samples A#{}\nproximity distance {:.1}, samples A#{}\nspectral samples A#{}\nscope: next sample A#{}\n", if crate::process::with_lock(crate::sensorlink::live) { "the sensors process" } else { "none: no sensors process has published" }, fl(v[12]), fl(v[12] >> 32), fl(v[13]), v[13] >> 32, fl(v[27]), v[29] >> 32, v[26], if crate::process::with_lock(crate::sensorlink::live) { crate::process::with_lock(|| crate::sensorlink::word(crate::sensorlink::SCOPE_N).unwrap_or(0)) } else { 0 }).as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if let Some(a) = cmd.params.strip_prefix(b"i2c ").and_then(|t| core::str::from_utf8(t).ok()) {
                    // `diag i2c <bus G#base> <target G#> <read n> [write bytes G#..]`: a raw transfer on an HS-I2C controller that is already up (hsi2c::ready): the bytes written, then n read. For identifying chips; it never brings a bus up.
                    let t: alloc::vec::Vec<&str> = a.split_whitespace().collect();
                    let hx = |s: &str| u64::from_str_radix(s.trim_start_matches("G#"), 16).ok();
                    match (t.first().and_then(|s| hx(s)), t.get(1).and_then(|s| hx(s)), t.get(2).and_then(|s| s.parse::<usize>().ok())) {
                        (Some(base), Some(target), Some(n)) => {
                            let w: alloc::vec::Vec<u8> = t[3..].iter().filter_map(|s| hx(s)).map(|v| v as u8).collect();
                            let mut r = alloc::vec![0u8; n.min(64)];
                            let res = if n == 0 { crate::hsi2c::write_bytes(base as usize, target as u8, &w) } else if w.is_empty() { crate::hsi2c::read_one(base as usize, target as u8, &mut r) } else { crate::hsi2c::write_read(base as usize, target as u8, &w, &mut r) };
                            resp.extend_from_slice(alloc::format!("i2c G#{base:X} target G#{target:X}: {res:?} read {:02X?}\n", r).as_bytes());
                        }
                        _ => resp.extend_from_slice(b"i2c <bus base> <target> <read n> [write bytes]\n"),
                    }
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"aoc" {
                    // `diag aoc`: the AoC block from the AP (aoc.rs `report`). Its bring-up tools (on, off, peek, boot, log, mmu, fault, services, svc, usf, pins, gpio; gsa, ffa, trusty) went on 2026-11-01: the `sensors` switch starts it.
                    crate::aoc::report(&mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"thermal" || cmd.params == b"thermal init" {
                    // `diag thermal`: every TMU zone as the ACPM firmware reads it (acpm::tmu_read) — the hottest probe, the zone's status, each probe; `diag thermal init` sends TMU_IPC_INIT first (Linux does once at probe).
                    if cmd.params == b"thermal init" {
                        resp.extend_from_slice(alloc::format!("init: {:?}\n", crate::acpm::tmu_init().map(|c| alloc::format!("caps G#{c:X}")).map_err(|e| alloc::format!("G#{e:X}"))).as_bytes());
                    }
                    let t0 = cntpct();
                    let _ = crate::acpm::tmu_read(0);
                    resp.extend_from_slice(alloc::format!("one zone read: A#{} us; throttle at A#{} C; X3 throttled from: A#{} kHz (0 = not)\n", cntpct().wrapping_sub(t0) * 1_000_000 / cntfrq(), THROTTLE_AT.load(core::sync::atomic::Ordering::Relaxed), THROTTLED_FROM.load(core::sync::atomic::Ordering::Relaxed)).as_bytes());
                    for (i, name) in crate::acpm::TMU_ZONES.iter().enumerate() {
                        match crate::acpm::tmu_read(i as u32) {
                            Ok((t, st, p)) => resp.extend_from_slice(alloc::format!("{name:>6}: A#{t} C  stat G#{st:X}  probes {:?}\n", p).as_bytes()),
                            Err(e) => resp.extend_from_slice(alloc::format!("{name:>6}: G#{e:X}\n").as_bytes()),
                        }
                    }
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"threads" {
                    crate::process::threads_report(&mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                // `diag "ira sources"`: each ira source's agreement, health and short hash (ira.rs sources_report), to prove a board's sources stable across boots before a vault is sealed under them. Read-only.
                if cmd.params == b"ira sources" {
                    resp.extend_from_slice(crate::ira::sources_report().as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                // Slot marking lives only in the cold build: a ferros chainloaded under Android must not rewrite devinfo (slot.rs is cold-only).
                #[cfg(not(shim_handoff))]
                if cmd.params == b"slot" || cmd.params.starts_with(b"slot word ") {
                    // `diag slot`: both A/B slot words from devinfo (retries | flags << 8; flags b0 unbootable, b1 successful, b2 active, b3 de-activated). `diag slot word <a|b> <retries hex> <flags hex>` sets one (slot.rs set_word: the A/B fallback test, a spare's flags).
                    if let Some(rest) = cmd.params.strip_prefix(b"slot word ") {
                        let t = core::str::from_utf8(rest).unwrap_or("");
                        let mut it = t.split_whitespace();
                        let slot = match it.next() { Some("a") => Some(0usize), Some("b") => Some(1), _ => None };
                        let r = it.next().and_then(|v| u8::from_str_radix(v, 16).ok());
                        let f = it.next().and_then(|v| u8::from_str_radix(v, 16).ok());
                        match (slot, r, f) {
                            (Some(s), Some(r), Some(f)) => resp.extend_from_slice(alloc::format!("set_word: G#{:X}\n", crate::process::with_lock(|| crate::slot::set_word(s, r, f))).as_bytes()),
                            _ => resp.extend_from_slice(b"slot word <a|b> <retries hex> <flags hex>\n"),
                        }
                    }
                    match crate::process::with_lock(crate::slot::words) {
                        Some([a, b]) => resp.extend_from_slice(alloc::format!("slot a G#{a:04X}  slot b G#{b:04X}\n").as_bytes()),
                        None => resp.extend_from_slice(b"devinfo not readable\n"),
                    }
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if let Some(name) = cmd.params.strip_prefix(b"part avb ") {
                    // `diag part avb <partition>`: its AVB vbmeta (behind the footer, or at the start of a vbmeta partition) as one line of hex, read-only (partfs.rs): the installed build's own descriptors, for the owner's re-signed top-level vbmeta.
                    match crate::partfs::avb_vbmeta(core::str::from_utf8(name).unwrap_or("").trim()) {
                        Ok(v) => {
                            resp.extend_from_slice(b"AVB ");
                            for b in v {
                                resp.extend_from_slice(alloc::format!("{b:02x}").as_bytes());
                            }
                            resp.push(b'\n');
                        }
                        Err(e) => resp.extend_from_slice(alloc::format!("part avb: {e}\n").as_bytes()),
                    }
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"keys" || cmd.params == b"keys sleep" || cmd.params == b"keys time" || cmd.params == b"keys flip" {
                    // `diag keys [sleep|time|flip]`: where the ira and wairua are held, the self-test and check, a power-down rehearsal, the cost of a use, a flipped bit caught and repaired (keys.rs).
                    crate::keys::diag(cmd.params.strip_prefix(b"keys").unwrap_or(b"").trim_ascii(), &mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"keyreg" || cmd.params.starts_with(b"keyreg ") {
                    // `diag keyreg [dbg|check|ao]`: which unused CPU registers could hold the ira and wairua, on the X3 and worker 0 (keyreg.rs).
                    crate::keyreg::diag(cmd.params.strip_prefix(b"keyreg").unwrap_or(b"").trim_ascii(), &mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"pets" {
                    // `diag pets`: the process watchdog's measurements (process.rs pets_report): each process's longest stretch between pets.
                    crate::process::pets_report(&mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"sched" {
                    // `diag sched`: the kernel's scheduler trace (process.rs) — who called whom, who waited, for how long.
                    crate::process::sched_report(&mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"prof" || cmd.params.starts_with(b"prof ") {
                    // `diag "prof [start [hz] | stop | pull | free]"`: the whole-system sampling profiler (prof.rs); tools/flame drives it and reads the rings.
                    crate::prof::diag(core::str::from_utf8(&cmd.params[4..]).unwrap_or(""), &mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"x3" {
                    // `diag x3`: start the big core on a timing probe and report (x3.rs).
                    let f = cntfrq();
                    let rate = |t: u64| if t > 0 { (1u64 << 24) * f / t / 1_000_000 } else { 0 };
                    let (d0, i0) = crate::x3::local();
                    resp.extend_from_slice(alloc::format!("CPU0: dependent adds ~A#{} M/s (= its clock), four chains ~A#{} M/s\n", rate(d0), rate(i0)).as_bytes());
                    let (ret, d, i, mpidr) = crate::x3::probe();
                    resp.extend_from_slice(alloc::format!("CPU_ON G#{:X}; X3 (MPIDR G#{mpidr:X}): dependent adds ~A#{} M/s (= its clock), four chains ~A#{} M/s\n", ret as u64, rate(d), rate(i)).as_bytes());
                    let (entry, mmu, chain, info) = crate::x3::trace();
                    let (on, off) = unsafe { crate::x3::TIMELINE };
                    resp.extend_from_slice(alloc::format!("X3 marks: entry G#{entry:X}, MMU on G#{mmu:X}, chain done G#{chain:X}; AFFINITY_INFO now {info} (0 on, 1 off, 2 on pending); read on at {} us, off at {} us after CPU_ON\n", on as i64, off as i64).as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                // `diag pref <name> <value>`: one device setting (settings.rs FIELDS, or `tz` in seconds east of UTC) set as a change on the device is, and the settings document written again. Unknown names and values out of range are refused, nothing written.
                if let Some(rest) = cmd.params.strip_prefix(b"pref ") {
                    let text = core::str::from_utf8(rest).unwrap_or("");
                    let mut it = text.split_whitespace();
                    let (name, val) = (it.next().unwrap_or(""), it.next().and_then(|v| v.parse::<i64>().ok()));
                    // `diag "pref zone <IANA id>"`: this device's zone by hand (its rule from the vault's `zones` document at the next pass); `pref zone auto`, automatic (the zone found here, the id by hand standing in); `pref zone none`, by hand with no zone (the fixed offset `tz`). `pref zone_follow 0` and `1`: this device's own choice, or the identity's (zone.rs).
                    let word = text.split_whitespace().nth(1).unwrap_or("");
                    let ok = match (name, val) {
                        ("zone", _) if it.clone().count() == 0 => {
                            let was = crate::zone::state().device;
                            let c = match word {
                                "auto" => Some(crate::zone::Choice { auto: true, id: was.id }),
                                "none" => Some(crate::zone::Choice { auto: false, id: crate::zone::Id::NONE }),
                                w => crate::zone::Id::new(w.as_bytes()).filter(|z| !z.is_none()).map(|id| crate::zone::Choice { auto: false, id }),
                            };
                            c.is_some_and(|c| crate::process::with_lock(|| crate::zone::set_device(c, || true)))
                        }
                        ("zone_follow", Some(v @ (0 | 1))) => crate::process::with_lock(|| crate::zone::set_follow(v == 1, || true)),
                        // `diag "pref language <ISO 639-3>"` (eng, spa, mri, …; `none` for none): this device's own language (lang.rs). `pref language_follow 0` and `1`: this device's own, or the identity's. Three lowercase letters or nothing: anything else is refused.
                        ("language", _) => {
                            let w = word.as_bytes();
                            let code = match w {
                                b"none" => Some(0),
                                [a, b, c] if w.iter().all(u8::is_ascii_lowercase) => Some((*a - b'a') as u16 * 676 + (*b - b'a') as u16 * 26 + (*c - b'a') as u16),
                                _ => None,
                            };
                            code.is_some_and(|c| crate::process::with_lock(|| crate::lang::set_device(c, || true)))
                        }
                        ("language_follow", Some(v @ (0 | 1))) => crate::process::with_lock(|| crate::lang::set_own(v == 0, || true)),
                        ("tz", Some(z)) if z.abs() < 86_400 => {
                            crate::rtc::set_tz(z);
                            true
                        }
                        (n, Some(v)) => match crate::settings::FIELDS.iter().find(|f| f.0 == n) {
                            Some(&(_, index)) if (0..=255).contains(&v) && crate::process::pref_ok(index, v as u8) => {
                                crate::process::set_pref(index, v as u8);
                                true
                            }
                            _ => false,
                        },
                        _ => false,
                    };
                    let kept = ok && crate::settings::document(None).is_some_and(|d| crate::process::with_lock(|| store.as_mut().is_some_and(|s| s.set(crate::settings::KEY, &d).is_ok())));
                    resp.extend_from_slice(alloc::format!("pref {name}: {}\n", if !ok { "refused" } else if kept { "set and kept" } else { "set, NOT kept" }).as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                // `diag kill <name>…`: each named process ended, its arc and surface freed (development: a bridge-spawned app has no other way out).
                if let Some(names) = cmd.params.strip_prefix(b"kill ") {
                    for name in names.split(|&b| b == b' ').filter(|n| !n.is_empty()) {
                        // A driver killed by hand is started again by init's keeper (usb, pcie1); `diag "kill init pcie1"` stops the keeper with it.
                        let killed = crate::process::kill(name);
                        resp.extend_from_slice(name);
                        resp.extend_from_slice(if killed { b" killed\n" } else { b" was not running\n" });
                    }
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                // `diag arcs`: every process's arc (name, base, size, block).
                if cmd.params == b"arcs" {
                    resp.extend_from_slice(crate::process::arcs_report().as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                // `diag frames`: the frame pool's memory map on this board (frames.rs, BOARDS.md "Memory map").
                if cmd.params == b"frames" {
                    resp.extend_from_slice(crate::frames::map_report().as_bytes());
                    resp.extend_from_slice(crate::process::with_lock(crate::frames::pool_report).as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if let Some(names) = cmd.params.strip_prefix(b"restart ") {
                    restart(&mut store, names, &mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"pmic rails" {
                    // `diag pmic rails`: the two rails the camera's clocks run on, read through ACPM: S5M_VDD_INT (main PMIC BUCK5M, control G#1F, outputs G#20/G#C7) and S1S_VDD_CAM (sub PMIC BUCK1S, control G#0E, outputs G#0F/G#CA). Volts = 200 mV + 6.25 mV x code (s2mpg14/15-register.h). Under Linux, streaming: INT 568.75 mV, CAM 600 mV (idle 537.5 mV).
                    let mv = |r: Result<u8, u32>| match r { Ok(c) => alloc::format!("G#{c:02X} ({} uV)", 200_000 + c as u32 * 6250), Err(e) => alloc::format!("error G#{e:X}") };
                    let hx = |r: Result<u8, u32>| match r { Ok(c) => alloc::format!("G#{c:02X}"), Err(e) => alloc::format!("error G#{e:X}") };
                    let (m, sb, t) = (crate::acpm::MAIN, crate::acpm::SUB, crate::acpm::T_PMIC);
                    resp.extend_from_slice(alloc::format!("VDD_INT (BUCK5M): ctrl {} out1 {} out2 {}\nVDD_CAM (BUCK1S): ctrl {} out1 {} out2 {}\n",
                        hx(crate::acpm::read(m, t, 0x1F)), mv(crate::acpm::read(m, t, 0x20)), mv(crate::acpm::read(m, t, 0xC7)),
                        hx(crate::acpm::read(sb, t, 0x0E)), mv(crate::acpm::read(sb, t, 0x0F)), mv(crate::acpm::read(sb, t, 0xCA))).as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"cam bus" {
                    // `diag cam bus`: mux and bring up both camera HS-I2C controllers (no device addressed).
                    crate::camera::buses_up(&mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params.starts_with(b"cam pdma") {
                    // `diag cam pdma`: hand the front end's register program to the PDMA engine (camera.rs), the only path to the region the CPU cannot write.
                    let mut b2 = crate::process::with_lock(|| store.as_mut().and_then(|s| s.get_by_key("cam.frame").ok().flatten()));
                    if b2.is_none() && cfg!(cam_frame_blob) {
                        b2 = Some(include_bytes!(concat!(env!("OUT_DIR"), "/cam_frame.bin")).to_vec());
                    }
                    let init = match b2.as_deref() {
                        Some(b) if b.len() > 8 && &b[..4] == b"CFRM" => {
                            let n = u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as usize;
                            if 8 + n <= b.len() { Some(b[8..8 + n].to_vec()) } else { None }
                        }
                        _ => crate::process::with_lock(|| store.as_mut().and_then(|s| s.get_by_key("cam.boitata.init").ok().flatten())),
                    };
                    let prog = match b2.as_deref() {
                        Some(b) if b.len() > 8 && &b[..4] == b"CFRM" => {
                            let n = u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as usize;
                            if 8 + n <= b.len() { Some(b[8 + n..].to_vec()) } else { None }
                        }
                        _ => crate::process::with_lock(|| store.as_mut().and_then(|s| s.get_by_key("cam.ispfe.prog").ok().flatten())),
                    };
                    let words: alloc::vec::Vec<&[u8]> = cmd.params.split(|&c| c == b' ').collect();
                    // `cam pdma uw ...`: the ultrawide's own init and CPU setup (FERROS_CAM_UW), `cam pdma front ...` the front camera's (FERROS_CAM_FRONT), instead of the main camera's bundle.
                    let (init, prog) = if words.contains(&&b"uw"[..]) { crate::camera::Cam::uw_bundle() } else if words.contains(&&b"front"[..]) { crate::camera::Cam::front_bundle() } else { (init, prog) };
                    crate::camera::pdma_probe(&mut resp, init, prog, if words.contains(&&b"slit"[..]) { 64 } else { 0 }, words.contains(&&b"fast"[..]), crate::camera::Shot::parse(&cmd.params));
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"cam frame" {
                    // `diag cam frame`: `cam stream` plus the front end's DMA program from the vault (cam.ispfe.prog, tools/camera-prog.py); reports which output buffers were written.
                    // Both tables can come as one value, `cam.frame` = 'CFRM' + init length (u32 LE) + init + program, because only one vault write per boot succeeds at the moment (2026-10-20).
                    let mut bundle = crate::process::with_lock(|| store.as_mut().and_then(|s| s.get_by_key("cam.frame").ok().flatten()));
                    if bundle.is_none() && cfg!(cam_frame_blob) {
                        bundle = Some(include_bytes!(concat!(env!("OUT_DIR"), "/cam_frame.bin")).to_vec());
                    }
                    let (init, prog) = match bundle.as_deref() {
                        Some(b) if b.len() > 8 && &b[..4] == b"CFRM" => {
                            let n = u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as usize;
                            if 8 + n <= b.len() { (Some(b[8..8 + n].to_vec()), Some(b[8 + n..].to_vec())) } else { (None, None) }
                        }
                        _ => (crate::process::with_lock(|| store.as_mut().and_then(|s| s.get_by_key("cam.boitata.init").ok().flatten())), crate::process::with_lock(|| store.as_mut().and_then(|s| s.get_by_key("cam.ispfe.prog").ok().flatten()))),
                    };
                    crate::camera::frame_probe(&mut resp, init, prog);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"cam stream" || cmd.params == b"cam raw" || cmd.params == b"cam lochs" {
                    // `diag cam stream`: replay the sensor init, configure CSI link 1, stream 300 ms, report what the link saw and dump its words.
                    // The init comes from the same places `cam frame` takes it from: the vault's own key, the bundled `cam.frame` value, or the image's built-in bundle. The vault lost these keys in the move to manifestus, and the dump is the whole point of the command, so it should not depend on which of the three is present.
                    let mut init = crate::process::with_lock(|| store.as_mut().and_then(|s| s.get_by_key("cam.boitata.init").ok().flatten()));
                    if init.is_none() {
                        let mut b = crate::process::with_lock(|| store.as_mut().and_then(|s| s.get_by_key("cam.frame").ok().flatten()));
                        if b.is_none() && cfg!(cam_frame_blob) {
                            b = Some(include_bytes!(concat!(env!("OUT_DIR"), "/cam_frame.bin")).to_vec());
                        }
                        if let Some(b) = b.as_deref() {
                            if b.len() > 8 && &b[..4] == b"CFRM" {
                                let n = u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as usize;
                                if 8 + n <= b.len() {
                                    init = Some(b[8..8 + n].to_vec());
                                }
                            }
                        }
                    }
                    if cmd.params == b"cam raw" { crate::camera::raw_probe(&mut resp, init) } else if cmd.params == b"cam lochs" { crate::camera::lochs_probe(&mut resp, init) } else { crate::camera::stream_probe(&mut resp, init) }
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"cam slit" || cmd.params == b"cam slitsweep" {
                    // `diag cam slit`: set our own window and frame rate through the sensor's CCS registers and measure the rate change (camera.rs).
                    let mut t = crate::process::with_lock(|| store.as_mut().and_then(|s| s.get_by_key("cam.boitata.init").ok().flatten()));
                    if t.is_none() {
                        let mut b = crate::process::with_lock(|| store.as_mut().and_then(|s| s.get_by_key("cam.frame").ok().flatten()));
                        if b.is_none() && cfg!(cam_frame_blob) {
                            b = Some(include_bytes!(concat!(env!("OUT_DIR"), "/cam_frame.bin")).to_vec());
                        }
                        if let Some(b) = b.as_deref() {
                            if b.len() > 8 && &b[..4] == b"CFRM" {
                                let n = u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as usize;
                                if 8 + n <= b.len() {
                                    t = Some(b[8..8 + n].to_vec());
                                }
                            }
                        }
                    }
                    if cmd.params == b"cam slitsweep" { crate::camera::slit_sweep(&mut resp, t) } else { crate::camera::slit(&mut resp, t, 64) }
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"cam preexist" {
                    // `diag cam preexist`: front-end state Linux had before the capture window, which nothing in the replay sets up (camera.rs).
                    crate::camera::preexisting(&mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"cam probe" {
                    // `diag cam probe`: whether an impossible capability read is the sensor's answer or this driver's (camera.rs).
                    crate::camera::read_probe(&mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"cam preview" {
                    // `diag cam preview`: the camera app's 1344 x 1008 NV12 preview, its luma painted 1:1 (camera.rs).
                    crate::camera::preview(&mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"cam iova" {
                    // `diag cam iova`: every buffer the camera IOMMU maps, by walking its page table in DRAM (camera.rs).
                    crate::camera::iova_scan(&mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"cam inherit" {
                    // `diag cam inherit`: after a live handoff with the camera running, read the frame Linux's pipeline wrote and paint it (camera.rs).
                    crate::camera::inherit_probe(&mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"cam scan all" {
                    // `diag cam scan all`: power each camera module in turn and read its model ID (camera.rs).
                    crate::camera::scan_all(&mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"cam buses" {
                    // `diag cam buses`: all four camera I2C controllers up, nothing powered and no device addressed (camera.rs).
                    crate::camera::buses_all(&mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"cam clamp" {
                    // `diag cam clamp`: prove the camera-PMIC overvoltage guard refuses what it must, with no I2C at all (camera.rs).
                    crate::camera::clamp_test(&mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"cam caps" {
                    // `diag cam caps`: the sensor's MIPI CCS capability block — what rates, windows and modes the silicon says it allows (camera.rs).
                    crate::camera::caps_dump(&mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"cam events" {
                    // `diag cam events`: every ISP interrupt source that is reporting, with its bits named (camera.rs, camera_events.rs).
                    crate::camera::events_dump(&mut resp, false);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"cam link" {
                    // `diag cam link`: every CSI link 1 word the camera stack touches, for diffing a cold ferros against a streaming Android through the handoff (camera.rs).
                    crate::camera::link_dump(&mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"cam fe" {
                    // `diag cam fe`: power the ISP front end, restore its clocks, read CSI link 1's version.
                    crate::camera::fe_up(&mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"cam up" {
                    // `diag cam up`: power the main camera module (rails programmed and verified first), then read the sensor's model ID.
                    crate::camera::power_up(&mut resp);
                    crate::camera::sensor_id(&mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"cam off" {
                    // `diag cam off`: power the main camera module down.
                    crate::camera::power_down(&mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"cam scan" {
                    // `diag cam scan`: which addresses acknowledge on the camera-PMIC bus.
                    crate::camera::scan(&mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params.starts_with(b"acpm ") {
                    // `diag acpm r <speedy> <type> <reg>` / `diag acpm u <speedy> <type> <reg> <val> <mask>` (hex): one PMIC transaction over the ACPM channel, for experiments without a rebuild.
                    let words: alloc::vec::Vec<&[u8]> = cmd.params[5..].split(|&c| c == b' ').filter(|w| !w.is_empty()).collect();
                    let hx = |w: &[u8]| -> Option<u32> { let w = w.strip_prefix(b"G#").or_else(|| w.strip_prefix(b"0x")).unwrap_or(w); u32::from_str_radix(core::str::from_utf8(w).ok()?, 16).ok() };
                    let nums: alloc::vec::Vec<u32> = words.iter().skip(1).filter_map(|w| hx(w)).collect();
                    let out = match (words.first().copied(), nums.as_slice()) {
                        (Some(b"r"), [sp, ty, rg]) => alloc::format!("read {sp:X}/{ty:X}/{rg:X}: {:?}\n", crate::acpm::read(*sp, *ty, *rg).map(|v| alloc::format!("G#{v:02X}")).map_err(|e| alloc::format!("G#{e:X}"))),
                        (Some(b"u"), [sp, ty, rg, v, m]) => {
                            let r = crate::acpm::update(*sp, *ty, *rg, *v as u8, *m as u8);
                            alloc::format!("update {sp:X}/{ty:X}/{rg:X} val G#{v:X} mask G#{m:X}: {:?}; now {:?}\n", r.map_err(|e| alloc::format!("G#{e:X}")), crate::acpm::read(*sp, *ty, *rg).map(|v| alloc::format!("G#{v:02X}")).map_err(|e| alloc::format!("G#{e:X}")))
                        }
                        _ => alloc::string::String::from("usage: acpm r <speedy> <type> <reg> | acpm u <speedy> <type> <reg> <val> <mask> (hex)\n"),
                    };
                    resp.extend_from_slice(out.as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"cam pmicon" {
                    // `diag cam pmicon`: raise the camera PMIC's supply enables (sub-PMIC GPIO1, GPIO3) and read its ID.
                    crate::camera::pmic_power_on(&mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"cam pmic" {
                    // `diag cam pmic`: the camera PMIC's ID and the main camera's regulators, read only.
                    crate::camera::pmic_dump(&mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"kv selftest" {
                    // `diag kv selftest`: one large put through the manifestus vault, with its block I/O counted and every appended block re-read (kvault.rs).
                    match crate::process::with_lock(|| store.as_mut().map(|s| s.selftest())) {
                        Some(t) => resp.extend_from_slice(t.as_bytes()),
                        None => resp.extend_from_slice(b"no vault open\n"),
                    }
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if let Some(n) = cmd.params.strip_prefix(b"evnt ").and_then(|t| core::str::from_utf8(t).ok()).and_then(|t| t.trim().parse::<u64>().ok()) {
                    // `diag evnt <bit>`: the kernel core's event stream on counter bit n (8-15: the selector is four bits; 12, every 333 us, is the boot's), for measuring what its wakeups cost. The idle wfe's 2 ms EL2-timer backstop holds whatever the setting.
                    let n = n.clamp(8, 15);
                    unsafe {
                        let mut v: u64;
                        core::arch::asm!("mrs {}, cnthctl_el2", out(reg) v);
                        v = (v & !(0xF << 4)) | (n & 0xF) << 4;
                        core::arch::asm!("msr cnthctl_el2, {}", "isb", in(reg) v);
                    }
                    resp.extend_from_slice(alloc::format!("event stream on counter bit {n}: every {} us\n", (2u64 << n) * 1_000_000 / cntfrq()).as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                #[cfg(not(ferros_production))]
                if cmd.params == b"wdt rate" {
                    // `diag wdt rate`: the watchdog's real tick, so the window (platform::WDT_COUNT) is known in seconds, not assumed from the oscillator's rate.
                    let (ticks, us) = crate::platform::wdt_rate();
                    // WHY the zero test: the counter's rate is what is being measured, and a watchdog whose clock is stopped counts 0 ticks; dividing by it would panic the kernel inside a diagnostic.
                    let window_ms = if ticks == 0 { 0 } else { crate::platform::WDT_COUNT as u64 * us / ticks as u64 / 1000 };
                    resp.extend_from_slice(alloc::format!("watchdog: {ticks} ticks in {us} us, so the window of {} ticks is {window_ms} ms\n", crate::platform::WDT_COUNT).as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                #[cfg(not(ferros_production))]
                if cmd.params == b"cvac canvas" {
                    // What it costs to hand the display's canvas to the GPU (GPU.md, the present stage): the GPU does not snoop the CPU's caches, so whatever the display drew must be cleaned to the point of coherency first, and EL0 cannot (SCTLR_EL2.UCI clear). Times `dc cvac` over the whole 32 MiB canvas twice: once with whatever lines the display left dirty, once with none. A clean writes back, it changes no data.
                    let (lo, hi) = crate::cachemap::DISPLAY_CANVAS;
                    let mut us = [0u64; 2];
                    for pass in &mut us {
                        let t0 = cntpct();
                        let mut a = lo;
                        while a < hi {
                            unsafe { core::arch::asm!("dc cvac, {a}", a = in(reg) a as u64) };
                            a += 64;
                        }
                        unsafe { core::arch::asm!("dsb sy") };
                        *pass = cntpct().wrapping_sub(t0) * 1_000_000 / cntfrq();
                    }
                    resp.extend_from_slice(alloc::format!("canvas {} MiB cleaned: {} us as the display left it, {} us again (clean lines)\n", (hi - lo) >> 20, us[0], us[1]).as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                #[cfg(not(ferros_production))]
                if cmd.params == b"s2mpu deny cpucl0" {
                    // The experiment before the CPUCL0 S2MPU joins the jump's deny-all (2026-11-04): if the cores' traffic went through it, this freezes the device on the spot; if not, everything carries on and the debug subsystem is cut off from memory until the next reset.
                    let r = crate::s2mpu::deny_all(crate::s2mpu::CPUCL0_S2MPU);
                    resp.extend_from_slice(alloc::format!("cpucl0 S2MPU deny-all: G#{r:X} (1 = done); protection now G#{:X}\n", unsafe { core::ptr::read_volatile((crate::s2mpu::CPUCL0_S2MPU + 0x58) as *const u32) }).as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"dpu" || cmd.params.starts_with(b"dpu ") {
                    // `diag dpu [on|off]`: the DPU's power domains (dpu_power.rs, DISPLAY-DESIGN.md §8).
                    crate::dpu_power::diag(core::str::from_utf8(&cmd.params[3..]).unwrap_or(""), &mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"pd" || cmd.params.starts_with(b"pd ") {
                    // `diag pd [off <name>|all]`: the unused SoC power domains (pd.rs).
                    crate::pd::diag(core::str::from_utf8(&cmd.params[2..]).unwrap_or(""), &mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"odpm" || cmd.params == b"odpm start" {
                    // `diag odpm start`, then `diag odpm`: every supply rail's power from the PMICs' meters (odpm.rs).
                    if cmd.params == b"odpm start" {
                        resp.extend_from_slice(crate::odpm::start().as_bytes());
                    } else {
                        crate::odpm::report(&mut resp);
                    }
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params.starts_with(b"ufs h8") {
                    // `diag ufs h8 [on|off]`: the UFS link's hibernate on idle (ferros_hal ufs), and its counts.
                    if cmd.params == b"ufs h8 on" || cmd.params == b"ufs h8 off" {
                        ferros_hal::ufs::H8_ENABLED.store(cmd.params == b"ufs h8 on", core::sync::atomic::Ordering::Relaxed);
                    }
                    let c = |i: usize| ferros_hal::ufs::H8_COUNTS[i].load(core::sync::atomic::Ordering::Relaxed);
                    resp.extend_from_slice(alloc::format!("ufs hibernate on idle {}: {} entries, {} exits, {} failures; lost commands {}\n", if ferros_hal::ufs::H8_ENABLED.load(core::sync::atomic::Ordering::Relaxed) { "on" } else { "off" }, c(0), c(1), c(2), ferros_hal::ufs::UFS_LOST.load(core::sync::atomic::Ordering::Relaxed)).as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params.starts_with(b"ufs runs") {
                    // `diag "ufs runs [on|off|reset]"`: SYS_BLOCK's multi-block commands (a run a command, ferros_hal `read_run_lun`) and what the vault process's block calls cost: calls, blocks, time inside them, MB/s, reads and writes. `off` sends every run a block a command again, to measure both on one boot; `reset` zeroes the counts.
                    use core::sync::atomic::Ordering::Relaxed;
                    match &cmd.params[..] {
                        b"ufs runs on" | b"ufs runs off" => ferros_hal::ufs::RUNS_ON.store(cmd.params == b"ufs runs on", Relaxed),
                        b"ufs runs reset" => crate::process::BLOCK_STATS.iter().for_each(|s| s.store(0, Relaxed)),
                        _ => {}
                    }
                    let s = |i: usize| crate::process::BLOCK_STATS[i].load(Relaxed);
                    let f = cntfrq();
                    let line = |what: &str, k: usize| {
                        let (calls, blocks, ticks) = (s(k), s(k + 1), s(k + 2));
                        let us = ticks * 1_000_000 / f;
                        alloc::format!("  {what}: A#{calls} calls, A#{blocks} blocks, A#{} ms inside, A#{} us a call, A#{} MB/s\n", us / 1000, us / calls.max(1), (blocks << 12) / us.max(1))
                    };
                    resp.extend_from_slice(alloc::format!("ufs runs {}, PRDT stride A#{} (0 not probed yet, 1 refused: a block a command)\n", if ferros_hal::ufs::RUNS_ON.load(Relaxed) { "on" } else { "off" }, ferros_hal::ufs::PRDT_STRIDE.load(Relaxed)).as_bytes());
                    resp.extend_from_slice(line("reads", 0).as_bytes());
                    resp.extend_from_slice(line("writes", 3).as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"smp dark on" || cmd.params == b"smp dark off" {
                    DARK_WORKERS_OFF.store(cmd.params == b"smp dark on", core::sync::atomic::Ordering::Relaxed);
                    resp.extend_from_slice(alloc::format!("workers off while dark: {}\n", cmd.params == b"smp dark on").as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"smp off" || cmd.params == b"smp on" {
                    // `diag smp off|on`: the four workers switched off (PSCI CPU_OFF, as before a reload) or started again — for measuring what idle cores cost; threads pinned to the workers wait meanwhile.
                    if cmd.params == b"smp off" {
                        crate::smp::stop_all();
                    } else {
                        crate::smp::launch_all();
                    }
                    let (up, _) = crate::smp::report();
                    resp.extend_from_slice(alloc::format!("workers up G#{up:X}\n").as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"screen off" || cmd.params == b"screen on" {
                    // `diag screen off|on`: what the power key does (preference 2), from the host, for measuring.
                    crate::process::set_pref(crate::process::PREF_SCREEN_OFF, (cmd.params == b"screen off") as u8);
                    resp.extend_from_slice(b"screen preference set\n");
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"power" || cmd.params.starts_with(b"power start") {
                    // `diag power start <s> [gap in 1/16 s] [input]`, then `diag power`: the system's draw over a window, sampled by the service loop's own passes (power_start / power_poll), the charger cut for it.
                    if let Some(a) = cmd.params.strip_prefix(b"power start").and_then(|t| core::str::from_utf8(t).ok()) {
                        let v: alloc::vec::Vec<u64> = a.split_whitespace().take_while(|w| *w != "after").filter_map(|t| t.parse().ok()).collect();
                        let delay = a.split_whitespace().skip_while(|w| *w != "after").nth(1).and_then(|t| t.parse().ok()).unwrap_or(0);
                        resp.extend_from_slice(power_start(v.first().copied().unwrap_or(16), v.get(1).copied().unwrap_or(2), a.contains("input"), delay).as_bytes());
                    } else {
                        match unsafe { (&*(&raw const POWER_WIN), &*(&raw const POWER_RESULT)) } {
                            (Some(w), _) => resp.extend_from_slice(alloc::format!("running: {} samples so far\n", w.n).as_bytes()),
                            (None, Some(r)) => resp.extend_from_slice(r.as_bytes()),
                            (None, None) => resp.extend_from_slice(b"no window yet: diag power start <s>\n"),
                        }
                    }
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"identity" || cmd.params == b"identity release" {
                    // `diag identity`: the identity class's state (AUTH.md); `diag "identity release"`: every identity entry deleted, the binding and any kept seed gone, the device-only secret renewed — what a completed departure does, for a developer starting the device over.
                    // Under the kernel lock, as every UFS command (release writes the vault; the state reads it).
                    let s = if cmd.params == b"identity release" { crate::process::with_lock(|| store_unlock(4, None)).map(|_| "released") } else { Ok("") };
                    let st = crate::process::with_lock(|| store_unlock(0, None)).unwrap_or(0);
                    resp.extend_from_slice(alloc::format!("identity: {} unlocked {}, bound {}\n", s.unwrap_or("release failed"), st & 1 == 1, st & 2 == 2).as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"log" || cmd.params.starts_with(b"log ") {
                    // `diag log [KiB]`: the last 16 KiB of the flash text log (flashlog.rs), or the last KiB asked for (up to 256: a reply's room).
                    let kib = core::str::from_utf8(&cmd.params[3..]).ok().and_then(|t| t.trim().parse::<usize>().ok()).unwrap_or(16).clamp(1, 256);
                    resp.extend_from_slice(&crate::flashlog::tail(kib * 1024));
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if let Some(a) = cmd.params.strip_prefix(b"part f2fs ") {
                    // `diag "part f2fs <part> <path>"`: an f2fs partition's checkpoint and roll-forward chain (ferros_f2fs: what fsync wrote after the checkpoint, Android's newest after a jump), and a file's inode as read, read-only.
                    let a = alloc::string::String::from_utf8_lossy(a).into_owned();
                    let mut w = a.split_whitespace();
                    let s = match (w.next(), w.next()) {
                        (Some(part), Some(path)) => match crate::partfs::mount_f2fs(part) {
                            Ok(mut fs) => {
                                let head = alloc::format!("{part}: checkpoint G#{:X}, roll-forward chain A#{} nodes, A#{} taken\n", fs.checkpoint_ver, fs.chain_len, fs.rolled_forward());
                                match fs.open(path.as_bytes()) {
                                    Ok(ino) => alloc::format!("{head}{path}: inode A#{}, size A#{}{}\n", ino.number, ino.size, if fs.rolled(ino.number) { ", from roll-forward" } else { "" }),
                                    Err(e) => alloc::format!("{head}{path}: {e:?}\n"),
                                }
                            }
                            Err(e) => alloc::format!("{e}\n"),
                        },
                        _ => alloc::string::String::from("part f2fs <part> <path>\n"),
                    };
                    resp.extend_from_slice(s.as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if let Some(name) = cmd.params.strip_prefix(b"part find ") {
                    // `diag "part find <name>"`: a GPT partition's LU and LBA range, read-only (partfs.rs find).
                    let name = alloc::string::String::from_utf8_lossy(name).into_owned();
                    let s = match crate::partfs::find(&name) {
                        Some(p) => alloc::format!("{name}: LU A#{} LBA A#{}..=A#{}\n", p.lun, p.first, p.last),
                        None => alloc::format!("{name}: not found\n"),
                    };
                    resp.extend_from_slice(s.as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params.starts_with(b"part vermagic ") {
                    // `diag "part vermagic <logical>"`: which kernel the modules in a logical partition (vendor_dlkm_a, say) were built for — read-only, from super (partfs.rs vermagic).
                    let name = alloc::string::String::from_utf8_lossy(&cmd.params[14..]).into_owned();
                    crate::partfs::vermagic(&mut resp, &name);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"modem hwcfg" {
                    // `diag modem hwcfg`: the handover block's inputs (modem.rs hwcfg): ABL's board fields, hardware_config.json, persist's cpsha size; the IMEIs never shown.
                    crate::modem::hwcfg(&mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"cam state" {
                    // `diag cam state`: the main camera's power-up dependencies, read only (camera.rs), so a cold boot is measured before anything is written.
                    crate::camera::state(&mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                }
                resp.extend_from_slice(if crate::board::akita() { b"ferros on Pixel 8a (Tensor G3), ".as_slice() } else { b"ferros on Pixel 8 Pro (Tensor G3), ".as_slice() });
                resp.extend_from_slice(match b.kind {
                    BootKind::Handoff => b"chainloaded from Linux\n".as_slice(),
                    BootKind::Cold if crate::ready::GATE_TELEMETRY.load(core::sync::atomic::Ordering::Relaxed) != 0 => b"through the hv's gate\n",
                    BootKind::Cold => b"cold boot from ABL\n",
                });
                let r = |a: usize| unsafe { read_volatile(a as *const u32) };
                append_hex(&mut resp, b"GEN=", generation as u32);
                // Percent of time since the service loop started that the core spent asleep in `wfe`, rather than spinning.
                // CPU0's state as PSCI sees it (AFFINITY_INFO: 0 on, 1 off, 2 on pending): whether the park's CPU_OFF took (x3.rs).
                {
                    let r: i64;
                    unsafe { core::arch::asm!("smc #0", inout("x0") 0x8400_0004u64 => r, in("x1") 0u64, in("x2") 0u64) };
                    append_hex(&mut resp, b"CPU0_AFFINITY=", r as u32);
                }
                append_hex(&mut resp, b"BACKSTOP_WAKES=", BACKSTOP_WAKES.load(core::sync::atomic::Ordering::Relaxed) as u32);
                append_hex(&mut resp, b"IDLE_PCT=", (asleep * 100 / cntpct().wrapping_sub(t_awake_from).max(1)) as u32);
                append_hex(&mut resp, b"UPTIME_MS=", (cntpct().wrapping_sub(t_boot) / (freq / 1000)) as u32);
                // The watchdog window's measurement (platform::pet_watchdogs): the longest gap between two pets from any core, when it ended (uptime), and the pets so far.
                append_hex(&mut resp, b"WDT_GAP_MAX_US=", (crate::platform::PET_GAP_MAX.load(core::sync::atomic::Ordering::Relaxed) * 1_000_000 / freq) as u32);
                append_hex(&mut resp, b"WDT_GAP_AT_MS=", ((crate::platform::PET_GAP_AT.load(core::sync::atomic::Ordering::Relaxed) - crate::checklist::ENTRY.load(core::sync::atomic::Ordering::Relaxed)) / (freq / 1000)) as u32);
                append_hex(&mut resp, b"WDT_PETS=", crate::platform::PETS.load(core::sync::atomic::Ordering::Relaxed) as u32);
                append_hex(&mut resp, b"WDT_HANDOVER_US=", crate::platform::HANDOVER_GAP_US.load(core::sync::atomic::Ordering::Relaxed) as u32);
                append_hex(&mut resp, b"WDT_HANDOVER_KIND=", crate::platform::HANDOVER_KIND.load(core::sync::atomic::Ordering::Relaxed) as u32);
                // The kernel lock's longest hold since boot (process.rs `lock`): what keeps the other cores waiting, and how near anything comes to the watchdog's second. Decoded with the stall records in `stall_lines` below.
                append_hex(&mut resp, b"LOCK_HOLD_MAX_US=", (crate::process::LOCK_HOLD_MAX.load(core::sync::atomic::Ordering::Relaxed) * 1_000_000 / freq) as u32);
                resp.extend_from_slice(stall_lines(freq).as_bytes());
                append_hex(&mut resp, b"EXC=", crate::exception_count().wrapping_sub(exc_start) as u32);
                append_hex(&mut resp, b"UFS_LOST=", ferros_hal::ufs::UFS_LOST.load(core::sync::atomic::Ordering::Relaxed));
                // The commands the USB process has handed over (ptlink.rs).
                append_hex(&mut resp, b"PT_PROC_CMDS=", st.proc_cmds);
                // The hottest thermal zone, °C (thermal_poll), and the X3's pre-throttle rate in MHz while the thermal throttle holds it (0 = not throttled).
                append_hex(&mut resp, b"SOC_C=", THERMAL.iter().map(|t| t.load(core::sync::atomic::Ordering::Relaxed)).max().unwrap_or(0) as u32);
                append_hex(&mut resp, b"THROTTLED_MHZ=", THROTTLED_FROM.load(core::sync::atomic::Ordering::Relaxed) / 1000);
                for k in 4..9 {
                    append_hex(&mut resp, alloc::format!("IRQ_STAGE_C{k}=").as_bytes(), unsafe { core::ptr::read_volatile(&raw const crate::__irq_stage[k]) } as u32);
                }
                append_hex(&mut resp, b"SPSR_MASKED=", crate::process::SPSR_MASKED.load(core::sync::atomic::Ordering::Relaxed) as u32);
                append_hex(&mut resp, b"TICK_REARMS=", crate::process::TICK_REARMS.load(core::sync::atomic::Ordering::Relaxed) as u32);
                // The "Ready?" gate is a cold-boot concern (ready.rs is cold-only); the handoff build has no gate telemetry.
                #[cfg(not(shim_handoff))]
                {
                    let g = crate::ready::GATE_TELEMETRY.load(core::sync::atomic::Ordering::Relaxed);
                    append_hex(&mut resp, b"GATE_ANSWER=", (g & 0xFF) as u32);
                    append_hex(&mut resp, b"GATE_MS=", (g >> 8 & 0xFF_FFFF) as u32);
                    append_hex(&mut resp, b"GATE_POWER_HELD_MS=", (g >> 32) as u32);
                }
                // The post-mortem notes (platform::note): this boot's and the previous boot's — the lock's holder (core + 1) and each core's last crumb with the core in the top byte.
                for k in 0..7 {
                    let (v, pv) = (platform::note_read(k), platform::note_read(8 + k));
                    append_hex(&mut resp, alloc::format!("NOTE_{k}_HI=").as_bytes(), (v >> 32) as u32);
                    append_hex(&mut resp, alloc::format!("NOTE_{k}=").as_bytes(), v as u32);
                    append_hex(&mut resp, alloc::format!("PREV_NOTE_{k}_HI=").as_bytes(), (pv >> 32) as u32);
                    append_hex(&mut resp, alloc::format!("PREV_NOTE_{k}=").as_bytes(), pv as u32);
                }
                {
                    let (up, on) = crate::smp::report();
                    append_hex(&mut resp, b"SMP_UP=", up);
                    for (k, v) in on.iter().enumerate() {
                        append_hex(&mut resp, alloc::format!("SMP_ON_{k}=").as_bytes(), *v as u32);
                    }
                    for (k, s) in crate::process::SLICES.iter().enumerate().filter(|(k, _)| (4..=8).contains(k)) {
                        append_hex(&mut resp, alloc::format!("SLICES_C{k}=").as_bytes(), s.load(core::sync::atomic::Ordering::Relaxed) as u32);
                    }
                    append_hex(&mut resp, b"LOCK_SPINS=", crate::process::LOCK_SPINS.load(core::sync::atomic::Ordering::Relaxed) as u32);
                    for (k, w) in crate::smp::IDLE_WAKES.iter().enumerate() {
                        append_hex(&mut resp, alloc::format!("IDLE_WAKES_C{}=", k + 4).as_bytes(), w.load(core::sync::atomic::Ordering::Relaxed) as u32);
                    }
                    // Why idle cores take the lock: the queue count, the interrupt flag, and how far off the next deadline is (signed ms; negative = already passed, which would make every core look on every event-stream tick).
                    append_hex(&mut resp, b"IDLE_RUNNABLE_N=", crate::process::RUNNABLE_N.load(core::sync::atomic::Ordering::Relaxed) as u32);
                    append_hex(&mut resp, b"IDLE_IRQ_ANY=", unsafe { core::ptr::read_volatile(&raw const crate::process::__irq_any) } as u32);
                    let nw = crate::process::NEXT_WAKE.load(core::sync::atomic::Ordering::Relaxed);
                    let now = cntpct();
                    append_hex(&mut resp, b"IDLE_NEXT_WAKE_MS=", if nw == u64::MAX { 0x7FFF_FFFF } else { ((nw as i64 - now as i64) * 1000 / freq as i64) as u32 });
                }
                {
                    let (bad, at, val) = platform::canary_check();
                    append_hex(&mut resp, b"CANARY_BAD=", bad);
                    append_hex(&mut resp, b"CANARY_BASE=", platform::canary_base() as u32);
                    append_hex(&mut resp, b"CANARY_AT=", at as u32);
                    append_hex(&mut resp, b"CANARY_VAL_HI=", (val >> 32) as u32);
                    append_hex(&mut resp, b"CANARY_VAL_LO=", val as u32);
                }
                {
                    let (size, used) = platform::stack_used_max();
                    append_hex(&mut resp, b"STACK_SIZE=", size);
                    append_hex(&mut resp, b"STACK_USED_MAX=", used);
                }
                append_hex(&mut resp, b"SNPSID=", r(DWC3 + 0xC120));
                append_hex(&mut resp, b"GSTS=", r(DWC3 + 0xC118));
                append_hex(&mut resp, b"DSTS=", r(DWC3 + 0xC70C));
                {
                    use core::sync::atomic::Ordering::Relaxed;
                    let k = &crate::killswitch::REPORT;
                    append_hex(&mut resp, b"KILL_GICR_FRAME=", k[0].load(Relaxed));
                    append_hex(&mut resp, b"KILL_GICD_CTLR=", k[1].load(Relaxed));
                    append_hex(&mut resp, b"KILL_HCR_BEFORE=", k[2].load(Relaxed));
                    append_hex(&mut resp, b"KILL_KEYS_IDLE=", k[3].load(Relaxed));
                    let kohi = crate::wairua::peek();
                    append_hex(&mut resp, b"KOHI_DONE=", kohi.is_some() as u32);
                    for (name, h) in crate::kohi::SOURCES.iter().zip(kohi.map(|k| k.1).unwrap_or_default().iter()) {
                        // present b0, rct_fail b1, apt_fail b2, level_fail b8..15, level-0 ones/2 b16..31
                        let mut key = alloc::vec::Vec::from(&b"KOHI_"[..]);
                        key.extend_from_slice(name.to_ascii_uppercase().as_bytes());
                        key.push(b'=');
                        append_hex(&mut resp, &key, h.packed());
                        key.pop();
                        key.extend_from_slice(b"_US=");
                        append_hex(&mut resp, &key, h.us);
                    }
                    append_hex(&mut resp, b"KOHI_OK=", kohi.is_some_and(|k| k.0) as u32);
                    append_hex(&mut resp, b"KOHI_MS=", crate::wairua::kohi_ms());
                    append_hex(&mut resp, b"VHE=", crate::cachemap::vhe() as u32);
                    #[cfg(all(feature = "pixel8", not(shim_handoff)))]
                    crate::s2mpu::report(&mut resp);
                    append_hex(&mut resp, b"KILL_IRQS=", unsafe { read_volatile(&raw const crate::__irq_count) } as u32);
                    let daif: u64;
                    unsafe { core::arch::asm!("mrs {}, daif", out(reg) daif) };
                    append_hex(&mut resp, b"DAIF=", daif as u32);
                }
                unsafe extern "C" {
                    static _start: u8;
                }
                append_hex(&mut resp, b"BASE=", b.link_base as u32);
                append_hex(&mut resp, b"RUNNING_AT=", &raw const _start as usize as u32);
                append_hex(&mut resp, b"STAGE=", b.stage as u32);
                append_hex(&mut resp, b"BOOT_EL=", crate::boot_el() as u32);
                append_hex(&mut resp, b"BOOT_SCTLR=", crate::boot_sctlr() as u32);
                append_hex(&mut resp, b"HEAP_END=", heap_end as u32);
                // The kernel heap now: free bytes, free blocks, the largest (what a big vault value needs in one piece).
                if let Some((free, blocks, largest)) = crate::heap_stats() {
                    append_hex(&mut resp, b"HEAP_FREE=", free as u32);
                    append_hex(&mut resp, b"HEAP_FREE_BLOCKS=", blocks as u32);
                    append_hex(&mut resp, b"HEAP_LARGEST_FREE=", largest as u32);
                }
                append_hex(&mut resp, b"HEAP_IN_USE=", ferros_heap::IN_USE.load(core::sync::atomic::Ordering::Relaxed) as u32);
                append_hex(&mut resp, b"HEAP_PEAK=", ferros_heap::PEAK.load(core::sync::atomic::Ordering::Relaxed) as u32);
                if let Some(probe) = &b.probe {
                    append_hex(&mut resp, b"UFS_LINK=", probe.link_up as u32);
                    append_hex(&mut resp, b"VAULT_OPENED=", probe.opened as u32);
                    append_hex(&mut resp, b"VAULT_VERIFIED=", probe.verified as u32);
                    append_hex(&mut resp, b"VAULT_SEALED=", probe.sealed as u32);
                    append_hex(&mut resp, b"VAULT_STAGE=", probe.stage as u32);
                    for (i, w) in probe.root16.chunks(4).enumerate() {
                        let label: &[u8] = [b"VAULT_ROOT0=".as_slice(), b"VAULT_ROOT1=", b"VAULT_ROOT2=", b"VAULT_ROOT3="][i];
                        append_hex(&mut resp, label, u32::from_be_bytes([w[0], w[1], w[2], w[3]]));
                    }
                    append_hex(&mut resp, b"WAIRUA_OK=", crate::wairua::peek().is_some_and(|k| k.0) as u32);
                    append_hex(&mut resp, b"IRA_FAIL=", probe.ira_fail);
                }
                if let Some(prev) = &b.prev {
                    // Why the previous session ended (crumb byte G#Bx = exit reason, G#EExx = panic; a nonzero fault ELR = the fault sink fired).
                    append_hex(&mut resp, b"PREV_EXIT=", prev[3] as u32);
                    append_hex(&mut resp, b"PREV_EXC=", prev[5] as u32);
                    // The post-mortem notes (platform::note): the lock's holder, then cores 4-8's last crumb (core in the top byte).
                    // (notes: below, from platform::NOTES, which survives every kind of boot)
                    append_hex(&mut resp, b"PREV_FAULT_ELR=", prev[20] as u32);
                    append_hex(&mut resp, b"PREV_FAULT_FAR=", prev[21] as u32);
                    // The PT record (leave(): words 24..27): the last command (cap bytes, op), and at a stall exit the open transfer and its age.
                    append_hex(&mut resp, b"PREV_PT_CMD_CAP=", prev[24] as u32);
                    append_hex(&mut resp, b"PREV_PT_CMD_OP=", (prev[24] >> 56) as u32);
                    append_hex(&mut resp, b"PREV_PT_OPEN_SID=", prev[25] as u32);
                    append_hex(&mut resp, b"PREV_PT_OPEN_RECV=", (prev[25] >> 32) as u32);
                    append_hex(&mut resp, b"PREV_PT_OPEN_TOTAL=", prev[26] as u32);
                    append_hex(&mut resp, b"PREV_PT_AGE_MS=", (prev[26] >> 32) as u32);
                    append_hex(&mut resp, b"PREV_ALIVE=", prev[27] as u32);
                }
                for (label, v) in &b.extras {
                    append_hex(&mut resp, label, *v);
                }
                // The ferros partition as the GPT names it (ufs_device::locate): first LBA and blocks, 0 when not found.
                let (pf, pb) = crate::ufs_device::part().unwrap_or((0, 0));
                append_hex(&mut resp, b"FERROS_PART_FIRST=", pf);
                append_hex(&mut resp, b"FERROS_PART_BLOCKS=", pb);
                // Reboot mode as the bootloader will see it, read through the EL3 monitor (proves the fastboot path without rebooting; 0 = normal).
                append_hex(&mut resp, b"PMU_REBOOT_CMD=", platform::priv_reg(platform::PMU_REBOOT_CMD, false, 0) as u32);
                append_hex(&mut resp, b"DSS_REASON=", r(platform::DSS_HEADER + platform::DSS_OFFSET_EMERGENCY_REASON));
                append_hex(&mut resp, b"DSS_SCRATCH=", r(platform::DSS_HEADER + platform::DSS_OFFSET_SCRATCH));
                append_hex(&mut resp, b"S2MPU_HSI0_CTRL_LINUX=", dma.s2mpu_ctrl);
                append_hex(&mut resp, b"SYSMMU_NUM_VM=", dma.num_vm as u32);
                for vm in 0..dma.num_vm {
                    append_hex(&mut resp, b"SYSMMU_VM_CTRL_LINUX=", dma.vm_ctrl_before[vm]);
                    append_hex(&mut resp, b"SYSMMU_VM_CTRL_NOW=", dma.vm_ctrl_after[vm]);
                }
                resp.extend_from_slice(b"END\n");
                queue_pt_response(&mut pt_out_data, &resp);
            } else if cmd.cap == cap_store {
                // A development door: compiled out of a production build (its `open` check refuses it there anyway).
                #[cfg(not(ferros_production))]
                {
                // A name the vault process holds (and the listing, while it serves): its job, the reply when its last call is answered (`vault_job_step`, at the top of a later pass).
                match vault_job_start(&mut store, cmd.op, cmd.params) {
                    Some(Ok(job)) => vault_job = Some(job),
                    Some(Err(resp)) => queue_pt_response(&mut pt_out_data, &resp),
                    // Under the kernel lock: syscalls on the worker cores reach the vault holding it (the map's places, a setting), so this core takes it too — one owner at a time.
                    None => {
                        let resp = crate::process::with_lock(|| store_command(store.as_mut(), cmd.op, cmd.params));
                        queue_pt_response(&mut pt_out_data, &resp);
                    }
                }
                }
            } else if cmd.cap == cap_display && cmd.op == ferros_pt::Op::Write {
                // A development door: compiled out of a production build (its `open` check refuses it there anyway).
                #[cfg(not(ferros_production))]
                {
                // A leading G#01 byte = hold (bridge `display --hold`). The DSI tools — G#02 a packet script, G#03 the DSIM dump, G#04 a DCS read, G#06 the refresh rate, G#08 the raster, G#09 the idle floor, G#0A the modeset — left with dpu.rs and dsim.rs on 2026-11-01 (KERNEL-OUT.md decision 6): the display server drives the panel. Anything else is text for the painter.
                match cmd.params.split_first() {
                    // G#0B [samples] [interval, 1/16 s units] [1 = battery only] = battery gauge samples over hsi2c_11 (hsi2c::gauge; bridge `display power [n] [sixteenths] [battery]`): voltage, current, the average of the samples. Battery only: the charger's buck is switched off for the run (the system then draws from the battery and the current is the system's), half a second for the gauge to see it, and the charger put back whatever happens.
                    Some((11, args)) => {
                        let mut rep = alloc::vec::Vec::new();
                        let n = args.first().copied().unwrap_or(1).max(1) as u32;
                        let gap = args.get(1).copied().unwrap_or(8) as u64 * freq / 16;
                        let battery = args.get(2).copied().unwrap_or(0) >= 1;
                        // 2 = also open the CHGIN input (CHG_CNFG_12 CHGINSEL) for the window, the stronger isolation.
                        let open_input = args.get(2).copied().unwrap_or(0) == 2;
                        let wait = |ticks: u64| {
                            let t0: u64;
                            unsafe { core::arch::asm!("mrs {}, cntpct_el0", out(reg) t0) };
                            loop {
                                let t: u64;
                                unsafe { core::arch::asm!("mrs {}, cntpct_el0", out(reg) t) };
                                if t.wrapping_sub(t0) >= ticks {
                                    break;
                                }
                            }
                        };
                        let mut input_prev: Option<u8> = None;
                        let charger = if battery {
                            match crate::hsi2c::charger::set_mode(0) {
                                Ok(prev) => {
                                    rep.extend_from_slice(alloc::format!("charger mode G#{:X} -> buck off\n", prev & 0xF).as_bytes());
                                    if open_input {
                                        match crate::hsi2c::charger::input_select(false) {
                                            Ok(p) => {
                                                input_prev = Some(p);
                                                rep.extend_from_slice(alloc::format!("charger input CHG_CNFG_12 G#{p:02X} -> CHGIN deselected\n").as_bytes());
                                            }
                                            Err(e) => rep.extend_from_slice(alloc::format!("charger input write failed: G#{e:X}\n").as_bytes()),
                                        }
                                    }
                                    wait(freq / 2);
                                    Some(prev)
                                }
                                Err(e) => {
                                    rep.extend_from_slice(alloc::format!("charger mode write failed: G#{e:X}; sampling as is\n").as_bytes());
                                    None
                                }
                            }
                        } else {
                            None
                        };
                        let mut sum_ua: i64 = 0;
                        let mut sum_uv: u64 = 0;
                        let mut got = 0u32;
                        for i in 0..n {
                            if i > 0 {
                                wait(gap);
                            }
                            match crate::hsi2c::gauge::sample() {
                                Ok(g) => {
                                    sum_ua += g.current_ua as i64;
                                    sum_uv += g.vcell_uv as u64;
                                    got += 1;
                                    rep.extend_from_slice(alloc::format!("soc {}% temp {}.{:02} C vcell {} mV current {} mA (avg {} mA, avg vcell {} mV)\n", g.soc, g.temp_cc / 100, (g.temp_cc % 100).unsigned_abs(), g.vcell_uv / 1000, g.current_ua / 1000, g.avg_current_ua / 1000, g.avg_vcell_uv / 1000).as_bytes());
                                }
                                Err(e) => rep.extend_from_slice(alloc::format!("gauge read failed: G#{e:X} (1 controller not up, 2 busy, 4 no device, 8 bus error, 10 timeout, 20 empty)\n").as_bytes()),
                            }
                        }
                        if let Some(p) = input_prev {
                            match crate::hsi2c::charger::restore_input(p) {
                                Ok(()) => rep.extend_from_slice(alloc::format!("charger input G#{p:02X} restored\n").as_bytes()),
                                Err(e) => rep.extend_from_slice(alloc::format!("CHARGER INPUT RESTORE FAILED: G#{e:X} — write G#69[G#C5] = G#{p:02X} by hand\n").as_bytes()),
                            }
                        }
                        if let Some(prev) = charger {
                            match crate::hsi2c::charger::restore(prev) {
                                Ok(()) => rep.extend_from_slice(alloc::format!("charger mode G#{:X} restored\n", prev & 0xF).as_bytes()),
                                Err(e) => rep.extend_from_slice(alloc::format!("CHARGER RESTORE FAILED: G#{e:X} — write G#69[G#B9] = G#{prev:02X} by hand\n").as_bytes()),
                            }
                        }
                        if got > 0 {
                            let ma = sum_ua / got as i64 / 1000;
                            let mv = sum_uv / got as u64 / 1000;
                            rep.extend_from_slice(alloc::format!("MEAN over {got}: {ma} mA at {mv} mV = {} mW (negative = discharging)\n", ma * mv as i64 / 1000).as_bytes());
                        }
                        queue_pt_response(&mut pt_out_data, &rep);
                    }
                    // G#0D [target] [reg] = an 8-bit register read on hsi2c_11; G#0E [target] [reg] [val] = an 8-bit write, read back after (bridge `display i2c <target> <reg> [val]`). The PMIC-side chips: gauge G#36, charger G#69, PMIC G#66.
                    Some((13, args)) if args.len() >= 2 => {
                        let mut rep = alloc::vec::Vec::new();
                        match crate::hsi2c::read_byte(crate::hsi2c::HSI2C_11, args[0], args[1]) {
                            Ok(v) => rep.extend_from_slice(alloc::format!("G#{:02X}[G#{:02X}] = G#{v:02X}\n", args[0], args[1]).as_bytes()),
                            Err(e) => rep.extend_from_slice(alloc::format!("i2c read failed: G#{e:X}\n").as_bytes()),
                        }
                        queue_pt_response(&mut pt_out_data, &rep);
                    }
                    Some((14, args)) if args.len() >= 3 => {
                        let mut rep = alloc::vec::Vec::new();
                        match crate::hsi2c::write_byte(crate::hsi2c::HSI2C_11, args[0], args[1], args[2]).and_then(|_| crate::hsi2c::read_byte(crate::hsi2c::HSI2C_11, args[0], args[1])) {
                            Ok(v) => rep.extend_from_slice(alloc::format!("G#{:02X}[G#{:02X}] <- G#{:02X}, reads back G#{v:02X}\n", args[0], args[1], args[2]).as_bytes()),
                            Err(e) => rep.extend_from_slice(alloc::format!("i2c write failed: G#{e:X}\n").as_bytes()),
                        }
                        queue_pt_response(&mut pt_out_data, &rep);
                    }
                    // G#0F [1 = ferros's own framebuffer (cachemap DISPLAY_FB), 0 = ABL's surface] = where the painter draws and the layer reads (bridge `display fb own|abl`), then a frame to prove it.
                    Some((15, args)) if !args.is_empty() => {
                        let mut rep = alloc::vec::Vec::new();
                        if !crate::display::use_own_fb(args[0] == 1) {
                            rep.extend_from_slice(b"no framebuffer region on this boot path\n");
                        } else {
                            let r = crate::display::show(if args[0] == 1 { b"ferros: own framebuffer" } else { b"ferros: ABL's surface" }, freq);
                            rep.extend_from_slice(&r);
                        }
                        queue_pt_response(&mut pt_out_data, &rep);
                    }
                    // G#10 [dither word] = the DQE's display dither register (DQE_DISP_DITHER: EN b0, MODE b1 = shift, FRAME_CON b2 = temporal, FRAME_OFFSET [4:3], TABLE_SEL R/G/B b5..b7) with the DQE shadow update requested; the next present applies it (bridge `display dither <hex>`). For finding the setting that doesn't speckle black at 10 bpc.
                    Some((16, args)) if !args.is_empty() => {
                        let mut rep = alloc::vec::Vec::new();
                        let dqe = 0x194D_0000usize;
                        unsafe {
                            write_volatile((dqe + 0x2800) as *mut u32, args[0] as u32);
                            let shd = 0x1947_0050usize;
                            write_volatile(shd as *mut u32, read_volatile(shd as *const u32) | 1 << 24);
                        }
                        crate::display::append_hex(&mut rep, b"DQE_DISP_DITHER=", unsafe { read_volatile((dqe + 0x2800) as *const u32) });
                        crate::display::append_hex(&mut rep, b"DQE_CGC_DITHER=", unsafe { read_volatile((dqe + 0x2400) as *const u32) });
                        crate::display::append_hex(&mut rep, b"DECON_GLOBAL_CON=", unsafe { read_volatile(0x1947_0020usize as *const u32) });
                        crate::display::append_hex(&mut rep, b"DATA_PATH_CON_0=", unsafe { read_volatile(0x1947_0200usize as *const u32) });
                        queue_pt_response(&mut pt_out_data, &rep);
                    }
                    // G#11 [diameter hi] [diameter lo] [r] [g] [b] [surround r] [g] [b] = a measurement disc centred on the panel over a surround; diameter 0 = full field (bridge `display patch <diameter px> <rrggbb> [surround rrggbb]`, tools/display-profile.sh).
                    Some((17, args)) if args.len() >= 8 => {
                        let diam = (args[0] as usize) << 8 | args[1] as usize;
                        let c = |i: usize| (args[i] as u32) << 16 | (args[i + 1] as u32) << 8 | args[i + 2] as u32;
                        let rep = crate::display::patch(diam, c(2), c(5), freq);
                        queue_pt_response(&mut pt_out_data, &rep);
                    }
                    // G#12 [diameter hi lo] [r g b, 10-bit, hi lo each] [surround r g b, the same] = the measurement disc in 10 bits a channel, the layer in 2:10:10:10 (bridge `display patch10`).
                    Some((18, args)) if args.len() >= 14 => {
                        let diam = (args[0] as usize) << 8 | args[1] as usize;
                        let v = |i: usize| (args[i] as u32) << 8 | args[i + 1] as u32;
                        let rep = crate::display::patch10(diam, [v(2), v(4), v(6)], [v(8), v(10), v(12)], freq);
                        queue_pt_response(&mut pt_out_data, &rep);
                    }
                    // G#13 [r g b, 10-bit, hi lo each] = DECON's background colour with the base window off; G#13 alone = the window back on (bridge `display bg <r> <g> <b>` / `display bg off`).
                    Some((19, args)) => {
                        let v = |i: usize| (args[i] as u32) << 8 | args[i + 1] as u32;
                        let c = if args.len() >= 6 { Some([v(0), v(2), v(4)]) } else { None };
                        let rep = crate::display::bg10(c, freq);
                        queue_pt_response(&mut pt_out_data, &rep);
                    }
                    // G#14 [diameter hi lo] [word, 4 bytes BE] [surround word, 4 bytes BE] = the 10-bit layer's disc with raw words (bridge `display patchraw`).
                    Some((20, args)) if args.len() >= 10 => {
                        let diam = (args[0] as usize) << 8 | args[1] as usize;
                        let w = |i: usize| u32::from_be_bytes([args[i], args[i + 1], args[i + 2], args[i + 3]]);
                        let rep = crate::display::patch_raw(diam, w(2), w(6), freq);
                        queue_pt_response(&mut pt_out_data, &rep);
                    }
                    // G#15 = the binary ruler: every row its own number in binary across the width (bridge `display ruler`).
                    Some((21, _)) => {
                        let rep = crate::display::ruler(freq);
                        queue_pt_response(&mut pt_out_data, &rep);
                    }
                    // G#0C [r] [g] [b] = a solid colour over the whole panel (bridge `display fill rrggbb`).
                    Some((12, args)) if args.len() >= 3 => {
                        let rep = crate::display::fill((args[0] as u32) << 16 | (args[1] as u32) << 8 | args[2] as u32, freq);
                        queue_pt_response(&mut pt_out_data, &rep);
                    }
                    // G#07 = set DECON0 to Android's state (10 bpc, dither, SRAM, QoS, interrupts), then paint a frame to apply it (bridge `display tune`).
                    Some((7, _)) => {
                        let mut rep = alloc::vec::Vec::new();
                        if !crate::display::dpu_on() {
                            rep.extend_from_slice(b"DPU powered down\n");
                        } else {
                            crate::display::configure_like_android(&mut rep);
                            let r = crate::display::show(b"ferros: DECON as Android", freq);
                            rep.extend_from_slice(&r);
                        }
                        queue_pt_response(&mut pt_out_data, &rep);
                    }
                    // G#05 = the panel's TE rate as DECON counts it (bridge `display terate`).
                    Some((5, _)) => {
                        let mut rep = alloc::vec::Vec::new();
                        if !crate::display::dpu_on() {
                            rep.extend_from_slice(b"DPU powered down\n");
                        } else {
                            let ((pin_hz, irq_hz), width_us) = crate::display::te_profile(freq);
                            rep.extend_from_slice(alloc::format!("TE: {pin_hz} Hz on the pin ({width_us} us high), {irq_hz} Hz by DECON interrupt\n").as_bytes());
                        }
                        queue_pt_response(&mut pt_out_data, &rep);
                    }
                    _ => {
                        // A leading 1 is `display --hold`, which held off the idle exit; there is none now (the kernel has no USB of its own to time out), so it only marks the text.
                        let text = match cmd.params.split_first() {
                            Some((1, rest)) => rest,
                            _ => cmd.params,
                        };
                        let rep = crate::display::show(text, freq);
                        queue_pt_response(&mut pt_out_data, &rep);
                    }
                }
                }
            } else if cfg!(not(ferros_production)) && cmd.cap == cap_mem && cmd.op == ferros_pt::Op::Write && cmd.params.len() >= 12 {
                // A development door: compiled out of a production build (its `open` check refuses it there anyway).
                #[cfg(not(ferros_production))]
                {
                // MEM (read and poke any physical address, the screen grabs among them) is a development affordance: a production build answers it as an unknown command (owner, 2026-10-24). A stopgap: in production it becomes a portal verb the owner's identity holds in full (STATUS.md next, PORTAL.md §5).
                // 32-bit poke: params [addr:8 BE][val:4 BE]. Address bit 63 set = through the EL3 monitor (SMC_CMD_PRIV_REG) for registers the S2MPU blocks from EL2. Response: [smc_ret:8 LE][readback:4 LE] (smc_ret 0 for a direct write).
                let raw = u64::from_be_bytes(cmd.params[0..8].try_into().unwrap());
                let val = u32::from_be_bytes(cmd.params[8..12].try_into().unwrap());
                let addr = raw & !(1 << 63);
                let (ret, back) = if raw >> 63 == 1 {
                    let ret = platform::priv_reg(addr, true, val);
                    (ret, platform::priv_reg(addr, false, 0) as u32)
                } else {
                    unsafe {
                        write_volatile(addr as usize as *mut u32, val);
                        core::arch::asm!("dsb sy");
                        (0, read_volatile(addr as usize as *const u32))
                    }
                };
                let mut resp = [0u8; 12];
                resp[..8].copy_from_slice(&ret.to_le_bytes());
                resp[8..].copy_from_slice(&back.to_le_bytes());
                queue_pt_response(&mut pt_out_data, &resp);
                }
            } else if cfg!(not(ferros_production)) && cmd.cap == cap_mem && cmd.op == ferros_pt::Op::Read && cmd.params.len() >= 12 {
                // A development door: compiled out of a production build (its `open` check refuses it there anyway).
                #[cfg(not(ferros_production))]
                {
                // Physical memory / MMIO read: params [addr:8 BE][len:4 BE], up to 64 KiB per call. Aligned requests use 32-bit loads so MMIO windows read correctly; anything else is read bytewise. Address bit 63 set = 32-bit reads through the EL3 monitor. MMU is off, so every address is physical (a bad address takes the fault sink: report + reset to Android).
                let raw = u64::from_be_bytes(cmd.params[0..8].try_into().unwrap());
                let addr = (raw & !(1 << 63)) as usize;
                // DRAM (G#8000_0000 up), 8-byte aligned: up to 4 MiB in one call with aligned 64-bit loads, valid on every memory type the cache map gives DRAM (a screencap of the 32 MiB canvas in 8 calls, not 512: tools/screencap). Registers below DRAM: 32-bit loads, 64 KiB, as before.
                let want = u32::from_be_bytes(cmd.params[8..12].try_into().unwrap()) as usize;
                let dram = raw >> 63 == 0 && addr >= 0x8000_0000 && addr % 8 == 0 && want % 8 == 0;
                let len = want.min(if dram { 4 << 20 } else { 64 << 10 });
                let mut out = alloc::vec![0u8; len];
                if dram {
                    for (i, c) in out.chunks_mut(8).enumerate() {
                        c.copy_from_slice(&unsafe { read_volatile((addr + i * 8) as *const u64) }.to_le_bytes());
                    }
                } else if raw >> 63 == 1 {
                    for (i, c) in out.chunks_mut(4).enumerate() {
                        let v = platform::priv_reg((addr + i * 4) as u64, false, 0) as u32;
                        c.copy_from_slice(&v.to_le_bytes()[..c.len()]);
                    }
                } else if addr % 4 == 0 && len % 4 == 0 {
                    for (i, c) in out.chunks_mut(4).enumerate() {
                        c.copy_from_slice(&unsafe { read_volatile((addr + i * 4) as *const u32) }.to_le_bytes());
                    }
                } else {
                    for (i, b) in out.iter_mut().enumerate() {
                        *b = unsafe { read_volatile((addr + i) as *const u8) };
                    }
                }
                queue_pt_response(&mut pt_out_data, &out);
                }
            } else if (cmd.cap == cap_reload || cmd.cap == cap_run) && cmd.op == ferros_pt::Op::Write {
                // A development door: compiled out of a production build (its `open` check refuses it there anyway).
                #[cfg(not(ferros_production))]
                {
                // Stage into STAGE, write-verify against the source. RELOAD additionally requires the ARM64 MZ header and must fit below the heap of the image it will replace.
                let params = cmd.params;
                let is_kernel = params.len() > 0x1000 && params[..4] == [0x4D, 0x5A, 0x00, 0x91];
                let fits = !params.is_empty() && params.len() <= STAGE_MAX && heap_end <= b.stage;
                let ok_shape = if cmd.cap == cap_reload { is_kernel && fits } else { fits };
                let mut ok = false;
                if ok_shape {
                    let src_hash = blake3::hash(params);
                    unsafe { core::ptr::copy_nonoverlapping(params.as_ptr(), b.stage as *mut u8, params.len()) };
                    let staged = unsafe { core::slice::from_raw_parts(b.stage as *const u8, params.len()) };
                    ok = blake3::hash(staged) == src_hash;
                }
                let size = if ok { params.len() } else { 0 };
                if cmd.cap == cap_reload {
                    reload_ok = ok;
                    reload_size = size;
                    run_ok = false;
                } else {
                    run_ok = ok;
                    run_size = size;
                    reload_ok = false;
                }
                queue_pt_response(&mut pt_out_data, &(size as u32).to_le_bytes());
                }
            } else if cmd.cap == cap_reload && cmd.op == ferros_pt::Op::Exec {
                // A development door: compiled out of a production build (its `open` check refuses it there anyway).
                #[cfg(not(ferros_production))]
                {
                if from_proc && !pt_act {
                    // From the USB process: no reply (as over the kernel's own USB), and the handover when it asks again with the COMPLETE sent (ptlink.rs).
                    proc_act = reload_ok && reload_size > 0;
                } else if reload_ok && reload_size > 0 {
                    // The USB process sent its COMPLETE before asking to act: give the host a moment, detach, then hand off to the copy stub.
                    for _ in 0..(1 << 24) {
                        core::hint::spin_loop();
                    }
                    platform::crumb(0xA6);
                    // The workers off first: the copy below rewrites the kernel under them (smp.rs).
                    ferros_hal::ufs::H8_ENABLED.store(false, core::sync::atomic::Ordering::Relaxed);
                    crate::process::with_lock(|| ferros_hal::ufs::UfsController::new(0x1320_0000).h8_wake_now());
                    // A profiler run stopped, its timer off on this core (the workers' go with them): the next kernel meets no tick.
                    crate::prof::shutdown();
                    crate::smp::stop_all();
                    unsafe {
                        // The copy overwrites the vector table: no interrupt may land mid-copy. The new image re-arms the killswitch first thing.
                        core::arch::asm!("msr daifset, #2");
                        // Copy the stub (cacheable) first, then hand over with every dirty line written back and the MMU off: the stub and the next image run MMU-off, as from ABL.
                        let dctl = read_volatile((DWC3 + 0xC704) as *const u32);
                        write_volatile((DWC3 + 0xC704) as *mut u32, dctl & !(1 << 31));
                        let stub = &raw const ferros_reload_stub as usize;
                        let stub_len = &raw const ferros_reload_stub_end as usize - stub;
                        core::ptr::copy_nonoverlapping(stub as *const u8, b.stub as *mut u8, stub_len);
                        // The last pet before the copy stub, stamped for the next kernel's arm_safety (WDT_HANDOVER_US): the copy and its boot run unpetted.
                        platform::handover_stamp(b'R');
                        crate::cachemap::off_for_handover();
                        let len = (reload_size + 7) & !7;
                        // Bit 63 carries the board (board::RELOAD_AKITA): a reloaded kernel has no device tree to read it from, and husky's frame map names DRAM akita does not have (frames.rs).
                        let next_x0 = ((generation + 1) << 1) | 1 | if crate::board::akita() { crate::board::RELOAD_AKITA } else { 0 };
                        core::arch::asm!(
                            "dsb sy",
                            "ic iallu",
                            "dsb sy",
                            "isb",
                            "br {stub}",
                            stub = in(reg) b.stub,
                            in("x0") b.stage,
                            in("x1") b.link_base,
                            in("x2") len,
                            in("x3") next_x0,
                            options(noreturn)
                        );
                    }
                }
                }
            } else if cmd.cap == cap_spawn && cmd.op == ferros_pt::Op::Write {
                // A development door: compiled out of a production build (its `open` check refuses it there anyway).
                #[cfg(not(ferros_production))]
                {
                // A capsule too large for the staging slot, streamed straight into its arc (STD-TARGET.md): [total u64][need u64][code offset u64][code length u64][piece offset u64][piece]. The first piece allocates the arc (zeroed, from the big pool past 16 MiB); each piece lands at the code offset plus its own; the reply is the bytes received so far. The Exec that follows builds the process in that arc.
                let pr = cmd.params;
                let w = |i: usize| u64::from_le_bytes(pr[i * 8..i * 8 + 8].try_into().unwrap()) as usize;
                let mut received = 0usize;
                if pr.len() >= 40 {
                    let (total, need, co, cl, off) = (w(0), w(1), w(2), w(3), w(4));
                    let piece = &pr[40..];
                    if off == 0 {
                        // The arc pools under the kernel lock, as every arc taken or given back: a worker ending a process frees its arc into the same lists meanwhile (this path took and gave back unlocked until 2026-11-08).
                        if let Some((a, ..)) = spawn_stage.take() {
                            crate::process::with_lock(|| crate::process::arc_free(a));
                        }
                        if let Some(arc) = crate::process::with_lock(|| crate::process::arc_alloc(need, false)) {
                            spawn_stage = Some((arc, total, 0, co, cl));
                        }
                    }
                    if let Some((arc, t, r, _, _)) = spawn_stage.as_mut() {
                        if *t == total && off == *r && off + piece.len() <= total && crate::process::CODE + total + crate::process::STACK <= arc.size {
                            unsafe { core::ptr::copy_nonoverlapping(piece.as_ptr(), (arc.base + crate::process::CODE + off) as *mut u8, piece.len()) };
                            *r += piece.len();
                            received = *r;
                        }
                    }
                }
                queue_pt_response(&mut pt_out_data, &(received as u32).to_le_bytes());
                }
            } else if cmd.cap == cap_spawn && cmd.op == ferros_pt::Op::Exec {
                // A development door: compiled out of a production build (its `open` check refuses it there anyway).
                #[cfg(not(ferros_production))]
                {
                // Run the staged payload as a process (EL0, own arc, a console capability plus the requested endpoint grants); reply [exit][console]. Params: [name len][name][grant count][(kind 1 serve / 2 use / 3 hardware / 4 entropy / 5 dma, len, name)*][input].
                let staged = spawn_stage.take();
                if (run_ok && run_size > 0) || staged.is_some() {
                    let code = unsafe { core::slice::from_raw_parts(b.stage as *const u8, run_size) };
                    let pr = cmd.params;
                    let mut at = 0usize;
                    let mut take = |n: usize| { let s = &pr[at.min(pr.len())..(at + n).min(pr.len())]; at += n; s };
                    let nl = take(1).first().copied().unwrap_or(0) as usize;
                    let name = take(nl);
                    let ng = take(1).first().copied().unwrap_or(0) as usize;
                    let mut grants = alloc::vec::Vec::new();
                    for _ in 0..ng {
                        let kind = take(1).first().copied().unwrap_or(0);
                        let l = take(1).first().copied().unwrap_or(0) as usize;
                        let g = take(l);
                        grants.push(match kind {
                            1 => crate::process::Grant::Serve(g),
                            3 => crate::process::Grant::Hardware(g),
                            4 => crate::process::Grant::Entropy,
                            5 => crate::process::Grant::Dma(g),
                            6 => crate::process::Grant::Store(g),
                            7 => crate::process::Grant::Power(g),
                            8 => crate::process::Grant::Irq(g),
                            9 => crate::process::Grant::Ira,
                            10 => crate::process::Grant::Part(g),
                            11 => crate::process::Grant::Unlock,
                            12 => crate::process::Grant::Pt,
                            13 => crate::process::Grant::Sensors,
                            14 => crate::process::Grant::Blocks(g),
                            15 => crate::process::Grant::Vault,
                            16 => crate::process::Grant::Domain,
                            17 => crate::process::Grant::Kists,
                            _ => crate::process::Grant::Use(g),
                        });
                    }
                    let input = &pr[at.min(pr.len())..];
                    // Signed capsules only: the gate (oracle.rs) must pass, or spawn never completes.
                    let sealed = crate::process::with_lock(|| store.as_mut().and_then(|s| s.get_by_key("sealed").ok().flatten())).unwrap_or_default();
                    let nm = if name.is_empty() { b"proc" } else { name };
                    let spawned = match staged {
                        Some((arc, total, received, co, cl)) if received == total => crate::process::spawn_capsule_staged(nm, arc, total, co, cl, input, &grants),
                        Some((arc, ..)) => {
                            crate::process::with_lock(|| crate::process::arc_free(arc));
                            Err("spawn: the streamed capsule is incomplete")
                        }
                        None => crate::process::spawn_capsule(nm, code, input, &grants, &sealed),
                    };
                    match spawned {
                        Ok(o) => {
                            mark_ran(&o);
                            let mut resp: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(8 + o.console.len());
                            resp.extend_from_slice(&o.exit.to_le_bytes());
                            resp.extend_from_slice(&o.console);
                            queue_pt_response(&mut pt_out_data, &resp);
                        }
                        Err(e) => {
                            let mut resp = alloc::vec::Vec::from(&b"ERR:"[..]);
                            resp.extend_from_slice(e.as_bytes());
                            queue_pt_response(&mut pt_out_data, &resp);
                        }
                    }
                } else {
                    queue_pt_response(&mut pt_out_data, b"ERR:NOPAYLOAD");
                }
                }
            } else if cmd.cap == cap_seal && cmd.op == ferros_pt::Op::Exec {
                // A development door: compiled out of a production build (its `open` check refuses it there anyway).
                #[cfg(not(ferros_production))]
                {
                // Install the staged masked capsule: gate + checker once, then its seal joins the vault's `sealed` list (idempotent). Reply [0][seal] or ERR.
                if run_ok && run_size > 0 {
                    let capsule = unsafe { core::slice::from_raw_parts(b.stage as *const u8, run_size) };
                    let resp = match (crate::process::install_capsule(capsule), store.as_mut()) {
                        (Ok(seal), Some(store)) => {
                            let mut list = crate::process::with_lock(|| store.get_by_key("sealed").ok().flatten()).unwrap_or_default();
                            if !list.chunks_exact(32).any(|e| e == seal) {
                                list.extend_from_slice(&seal);
                            }
                            match crate::process::with_lock(|| store.set("sealed", &list)) {
                                Ok(_) => {
                                    let mut r = alloc::vec![0u8];
                                    r.extend_from_slice(&seal);
                                    r
                                }
                                Err(_) => alloc::vec::Vec::from(&b"ERR:install: vault write failed"[..]),
                            }
                        }
                        (Ok(_), None) => alloc::vec::Vec::from(&b"ERR:install: no vault"[..]),
                        (Err(e), _) => {
                            let mut r = alloc::vec::Vec::from(&b"ERR:"[..]);
                            r.extend_from_slice(e.as_bytes());
                            r
                        }
                    };
                    queue_pt_response(&mut pt_out_data, &resp);
                } else {
                    queue_pt_response(&mut pt_out_data, b"ERR:NOPAYLOAD");
                }
                }
            } else if cmd.cap == cap_run && cmd.op == ferros_pt::Op::Exec {
                // A development door: compiled out of a production build (its `open` check refuses it there anyway).
                #[cfg(not(ferros_production))]
                {
                // CALL the staged payload. ABI: extern "C" fn(in_ptr, in_len, out_ptr, out_cap) -> u64, entry at offset 0, PC-relative code only. A payload fault lands in the fault sink (report + reset to Android); a payload that never returns stops the watchdog pets.
                if run_ok && run_size > 0 {
                    crate::cachemap::sync_code(b.stage, run_size);
                    let mut out = alloc::vec![0u8; 4096];
                    let entry: extern "C" fn(*const u8, usize, *mut u8, usize) -> u64 = unsafe { core::mem::transmute(b.stage) };
                    let ret = entry(cmd.params.as_ptr(), cmd.params.len(), out.as_mut_ptr(), out.len());
                    let n = (ret as usize).min(out.len());
                    let mut resp: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(8 + n);
                    resp.extend_from_slice(&ret.to_le_bytes());
                    resp.extend_from_slice(&out[..n]);
                    queue_pt_response(&mut pt_out_data, &resp);
                } else {
                    queue_pt_response(&mut pt_out_data, b"ERR:NOPAYLOAD");
                }
                }
            } else if cmd.cap == cap_reboot && cmd.op == ferros_pt::Op::Exec {
                // Mode byte 1 = fastboot: stamp the PMU reboot mode through the EL3 monitor and reset straight into the bootloader. If the monitor refuses, fall back to a normal exit (report + Android).
                if from_proc && !pt_act {
                    // From the USB process: no reply (as over the kernel's own USB), and the reset when it asks again with the COMPLETE sent (ptlink.rs).
                    proc_act = true;
                    continue;
                }
                let fastboot = cmd.params.first().copied() == Some(1);
                if fastboot {
                    report(&b, EXIT_FASTBOOT);
                    unsafe {
                        let dctl = read_volatile((DWC3 + 0xC704) as *const u32);
                        write_volatile((DWC3 + 0xC704) as *mut u32, dctl & !(1 << 31));
                    }
                    // Only returns on failure: platform::WATCHDOG_DID_NOT_FIRE, or the EL3 monitor's refusal (nonzero, negative = error). Kept in report slot r[23] so the exit report says which.
                    FASTBOOT_FAIL.store(platform::reboot_to_bootloader(), core::sync::atomic::Ordering::Relaxed);
                }
                leave(&b, EXIT_REBOOT, &st);
            }
        }
        // A put the vault process takes over later passes reads its value from the command: it stays with the job.
        #[cfg(not(ferros_production))]
        if matches!(vault_job, Some(VaultJob::Put { .. })) && vault_cmd.is_none() {
            vault_cmd = Some(proc_cmd);
        }
    }
}
