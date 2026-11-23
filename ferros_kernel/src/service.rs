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
/// The GPU came to its reset and was fenced to gpu-mem at this boot (pd.rs `gpu_fresh`, after a jump the crowbar), so the GPU service may start.
static GPU_FRESH: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Largest RELOAD/RUN image the staging slot takes.
const STAGE_MAX: usize = 4 << 20;
/// The largest value the vault takes (3 MiB). Until 2026-11-12 sealing a value and reading it back each needed about twice its size in one block of the kernel heap; since then a value over `ferros_pieces::PIECE` is sealed and stored a piece at a time and a restart streams it into a staging arc (`spawn_from_pieces`), but the readers that take a capsule whole (init's spawn through `read_capsule`, `store get`) still hold it in one block, and the gate's parse a second, so the bound stays. The largest capsule today is the display server's, 2.8 MB. Larger things (std images) go through the streamed spawn, not the vault.
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
/// Under the vault's lock alone (2026-11-22, kernlock): the kernel lock was held across the whole transfer, 126 ms for a 1.6 MB `store get` on the Pro (`diag lock`, service.rs `store_command` under `with_lock` and `vault`), and a camera stream died behind it. Nothing here is the kernel lock's: the vault's blocks and its in-memory identity are the vault lock's to read (kvault `id` and `bound` are written under both locks, read under either, and a set, get or remove writes neither), `note_old_put` and `zone::table_stale` are atomics. The one piece of kernel state, a settings document applied to the preferences, is applied by the caller once the vault's lock is let go (`store_host`), as the kernel lock ranks above it.
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
            // A value larger than STORE_VALUE_MAX is refused (status 5): 4 and 5 MiB values ran the 22 MiB kernel heap out and panicked the kernel (2026-10-31), when a value was sealed and read whole (`STORE_VALUE_MAX` says what still holds it whole).
            if len > STORE_VALUE_MAX {
                crate::flashlog::line(&alloc::format!("store set {key:?} {len} bytes refused: over {STORE_VALUE_MAX}"));
                return alloc::vec![5];
            }
            crate::flashlog::line(&alloc::format!("store set {key:?} {len} bytes"));
            crate::process::note_old_put(&key);
            match store.set(&key, &params[2 + klen..]) {
                Ok(h) => {
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
            // The value read straight into the reply after its status byte: a value in pieces goes in a piece at a time, never whole in the heap twice.
            let mut r = alloc::vec![0u8];
            match store.get_into(&key, &mut r) {
                Ok(true) => r,
                Ok(false) => alloc::vec![1],
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

/// A host store command on the kernel's own vault (`store_command`) with the kernel lock let go across the vault's transfer, then a settings document it set applied to the preferences at once, as a change on the device does (SYS_SET_PREF): every live process sees it on its next frame. The apply takes the kernel lock itself (`settings::apply` with `held` false).
/// Between the store and the apply a process's own preference change may land (its document written at its trap's end, `keep_settings`); the apply then puts the stored document's values over it in memory, as a later change would. A development door's race, said here.
fn store_host(store: &mut Option<ShimStore>, op: ferros_pt::Op, params: &[u8]) -> alloc::vec::Vec<u8> {
    let resp = vault(|| store_command(store.as_mut(), op, params));
    // A status 0 on a write means `store_command` found its key inside the parameters (status 3 otherwise), so the key's length and the slices below are in range.
    if op == ferros_pt::Op::Write && resp.first() == Some(&0) {
        let klen = u16::from_be_bytes([params[0], params[1]]) as usize;
        if &params[2..2 + klen] == crate::settings::KEY.as_bytes() {
            let _ = crate::settings::apply(&params[2 + klen..], false);
        }
    }
    resp
}

/// A bridge `store` command the vault process answers (VAULT-OUT.md: the names it holds), through the kernel's own channel (process.rs `vault_call`), a transfer a call. Its PT reply waits until the last call is answered: the USB process polls for a reply that is not ready yet (ptlink.rs).
/// Also a kist offered to stow (PT `ferros.kist.offer`, `offer_command`): its parts go in as puts into `capsules.` by address, in a production build too, so only `Put` is ever made there.
#[cfg_attr(ferros_production, allow(dead_code))]
enum VaultJob {
    /// A value read whole: its full name, what has come so far.
    Get { full: alloc::string::String, got: alloc::vec::Vec<u8> },
    /// A value written in pieces from the command's parameters (the command kept beside the job: `vault_cmd` in `run`): its full name (its scope's name, when `addr` names it by a 32-byte address), where the value starts in them, the next offset into it; and for a value larger than one command carries (a stowed kist's capsule), where this command's part starts in the value and whether more commands follow.
    Put { full: alloc::string::String, addr: Option<[u8; 32]>, from: usize, at: usize, base: u64, more: bool },
    Remove { full: alloc::string::String },
    /// Every name: the kernel's own vault's first (already in `out`, after its status byte), then each scope's from the vault process in turn, `scope` the one being read, `at` into its listing, `part` its listing so far.
    List { out: alloc::vec::Vec<u8>, scope: usize, at: u64, part: alloc::vec::Vec<u8> },
    /// A single-copy extent's blocks (docs/RAW-VIDEO.md: a clip pulled to the host), a UFS command's 1 MiB a call: its full name, the next block, the blocks still to read, what has come; or with `info` its record alone.
    Extent { full: alloc::string::String, block: u64, left: u64, got: alloc::vec::Vec<u8>, one: Option<(u8, u32)>, kist: Option<([u8; 32], [u8; 32])> },
    /// One call on the hold as the person (the bridge's `items`, docs/STORE.md): the call as ferros_items `wire` makes it, sent whole; the answer [0][the value u64 LE][the bytes], the bridge reading the value itself.
    Items { call: alloc::vec::Vec<u8> },
}

/// A bridge `store extent` read: params [G#FF][block u64 LE][blocks u32 LE][name], at most `EXTENT_PULL` blocks; [G#FE][name] its record. A name in a kist's own domain (a clip Lumis recorded): [G#FD][developer 32][option 32][block u64 LE][blocks u32 LE][name] and [G#FC][developer 32][option 32][name].
const EXTENT_PULL: u64 = (STORE_VALUE_MAX >> 12) as u64;

/// One transfer of the kernel's channel (process.rs STORE_MAX, the vault process's one-transfer bound).
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
            // [key length u16 BE][key][value]. Bit 15 of the length: a part of a larger value, [offset u64 LE][more u8] after the key. Bit 14: the key is a scope's prefix and a 32-byte address (a stowed kist's capsule, by its blake3).
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
            match vault(|| store.as_mut().map(|s| s.keys())) {
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
        ferros_pt::Op::Read if params.first() == Some(&0xFF) && params.len() > 13 => {
            let block = u64::from_le_bytes(params[1..9].try_into().ok()?);
            let n = u32::from_le_bytes(params[9..13].try_into().ok()?) as u64;
            // WHY: the count is the host's; the reply is built whole in the kernel's heap, as a get's is (STORE_VALUE_MAX).
            if n == 0 || n > EXTENT_PULL {
                return Some(Err(alloc::vec![5]));
            }
            VaultJob::Extent { full: name(&params[13..])?, block, left: n, got: alloc::vec::Vec::new(), one: None, kist: None }
        }
        ferros_pt::Op::Read if params.first() == Some(&0xFE) => VaultJob::Extent { full: name(&params[1..])?, block: 0, left: 0, got: alloc::vec::Vec::new(), one: Some((crate::process::EOP_INFO, crate::process::EXTENT_INFO as u32)), kist: None },
        ferros_pt::Op::Read if matches!(params.first(), Some(&0xFD) | Some(&0xFC)) && params.len() > 65 => {
            let (developer, option): ([u8; 32], [u8; 32]) = (params[1..33].try_into().ok()?, params[33..65].try_into().ok()?);
            let info = params[0] == 0xFC;
            let (block, n, at) = if info { (0, 0, 65) } else { (u64::from_le_bytes(params.get(65..73)?.try_into().ok()?), u32::from_le_bytes(params.get(73..77)?.try_into().ok()?) as u64, 77) };
            let rel = params.get(at..)?;
            // WHY: the count and the name are the host's: the reply is built whole in the kernel's heap (STORE_VALUE_MAX), and a name is a value's (printable ASCII up to 96 bytes), as a process's own call takes it.
            if (!info && (n == 0 || n > EXTENT_PULL)) || rel.is_empty() || rel.len() > 96 || !rel.iter().all(|&b| (0x21..0x7F).contains(&b)) {
                return Some(Err(alloc::vec![5]));
            }
            VaultJob::Extent { full: alloc::string::String::from_utf8(rel.to_vec()).ok()?, block, left: n, got: alloc::vec::Vec::new(), one: info.then_some((crate::process::EOP_INFO, crate::process::EXTENT_INFO as u32)), kist: Some((developer, option)) }
        }
        // A call on the hold as the person (G#F9, the bridge's `items`): [G#F9][the call]. WHY: the length is the host's: at least the call's head, at most one transfer and its head.
        ferros_pt::Op::Read if params.first() == Some(&0xF9) => {
            if params.len() < 46 || params.len() > 1 + crate::process::ITEMS_CALL_MAX {
                return Some(Err(alloc::vec![5]));
            }
            VaultJob::Items { call: params[1..].to_vec() }
        }
        // The clips area's records (G#FB) or a drop of one of the domain's extents by its first block (G#FA), for the bridge's `store extent-list` / `extent-drop`: [code][first block u64][1: a kist's domain follows (developer 32, option 32), 0: a scope's][name], the name a scope's full name (or any of the kist's: it is not looked up). Never from a process (`extent_forward` refuses both ops).
        // G#F8, the bridge's `store extent-drop-orphan`: a sealed extent dropped by its first block whatever domain owns it, for one whose owning build no longer exists (development builds only, as this whole path is; the name is not looked up).
        ferros_pt::Op::Read if matches!(params.first(), Some(&0xFB) | Some(&0xFA) | Some(&0xF8)) && params.len() > 10 => {
            let block = u64::from_le_bytes(params[1..9].try_into().ok()?);
            let (kist, at) = if params[9] == 1 { (Some((params.get(10..42)?.try_into().ok()?, params.get(42..74)?.try_into().ok()?)), 74) } else { (None, 10) };
            let rel = params.get(at..)?;
            // WHY: the name is the host's: a value's (printable ASCII up to 96 bytes), as a process's own call takes it.
            if rel.is_empty() || rel.len() > 96 || !rel.iter().all(|&b| (0x21..0x7F).contains(&b)) {
                return Some(Err(alloc::vec![5]));
            }
            let one = match params[0] {
                0xFB => (crate::process::EOP_LIST, 1 << 16),
                0xFA => (crate::process::EOP_DROP_AT, 0),
                _ => (crate::process::EOP_DROP_ORPHAN, 0),
            };
            VaultJob::Extent { full: alloc::string::String::from_utf8(rel.to_vec()).ok()?, block, left: 0, got: alloc::vec::Vec::new(), one: Some(one), kist }
        }
        ferros_pt::Op::Read => VaultJob::Get { full: name(params)?, got: alloc::vec::Vec::new() },
        ferros_pt::Op::Exec => VaultJob::Remove { full: name(params)? },
    };
    Some(vault_job_call(&job, params).map(|_| job))
}

/// A bridge get or remove of a name the kernel's vault does not hold, under a scope this kernel does not know (process.rs `foreign_name`): the vault process asked under the name's own prefix, as a kernel that knows the scope would ask it. Its job, its first call made; None when there is nothing to ask (no vault process, a put, a name of no scope's shape, the call not made).
/// A kernel without a scope another kernel added (or one that dropped it) then still sees what that kernel moved: on 2026-11-15 the flashed kernel said `cam.frame` absent after the camera kernel had moved it, and it was stored again by hand.
#[cfg(not(ferros_production))]
fn foreign_job(op: ferros_pt::Op, params: &[u8]) -> Option<VaultJob> {
    if !crate::process::vault_serving() {
        return None;
    }
    let full = core::str::from_utf8(params).ok().filter(|k| crate::process::foreign_name(k).is_some()).map(alloc::string::String::from)?;
    crate::flashlog::line(&alloc::format!("store {full:?}: not in the kernel's vault and under no scope this kernel knows; asking the vault process"));
    let job = match op {
        ferros_pt::Op::Read => VaultJob::Get { full, got: alloc::vec::Vec::new() },
        ferros_pt::Op::Exec => VaultJob::Remove { full },
        ferros_pt::Op::Write => return None,
    };
    vault_job_call(&job, params).ok().map(|()| job)
}

/// A part of a kist offered to stow (`ferros.kist.offer` Write: [capsule 32][offset u64 LE][more u8][bytes], checked by the caller): a put into the device's `capsules.` under the capsule's address, as the bridge's part form of `store` puts one, its first call made. Err(the PT reply) when the vault process is not serving or the call could not be made.
fn offer_part(params: &[u8]) -> Result<VaultJob, alloc::vec::Vec<u8>> {
    if !crate::process::vault_serving() {
        crate::flashlog::line("kist offer refused (A#4): the vault process is not serving");
        let mut r = alloc::vec![4u8];
        r.extend_from_slice(b"the vault process is not serving");
        return Err(r);
    }
    let job = VaultJob::Put { full: alloc::string::String::from("capsules"), addr: Some(params[..32].try_into().unwrap()), from: 41, at: 0, base: u64::from_le_bytes(params[32..40].try_into().unwrap()), more: params[40] != 0 };
    vault_job_call(&job, params).map(|()| job)
}

/// A PT answer of a status and its words ([status][words]), as the store and offer commands give them.
fn held_status(status: u8, words: &str) -> alloc::vec::Vec<u8> {
    let mut r = alloc::vec![status];
    r.extend_from_slice(words.as_bytes());
    r
}

/// The job's next call to the vault process; Err(the PT reply) when it cannot be made.
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
        VaultJob::Items { call } => crate::process::vault_stamp_items(call),
        VaultJob::List { scope, at, .. } => crate::process::vault_stamp_list(*scope, *at, VAULT_PIECE as u32).map(|(m, _)| m),
        VaultJob::Extent { full, block, left, one, kist, .. } => {
            // [extent op][block u64][length u32], as process.rs `extent_forward` stamps a process's call.
            let (op, len) = match one {
                Some(o) => *o,
                None => (crate::process::EOP_READ, ((*left).min((crate::process::EXTENT_MAX >> 12) as u64) << 12) as u32),
            };
            rest.push(op);
            rest.extend_from_slice(&block.to_le_bytes());
            rest.extend_from_slice(&len.to_le_bytes());
            match kist {
                Some((dev, opt)) => crate::process::vault_stamp_kist(crate::process::VOP_EXTENT, dev, opt, full.as_bytes(), &rest),
                None => vault_stamp(crate::process::VOP_EXTENT, full, &rest),
            }
        }
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
fn vault_job_step(job: &mut VaultJob, params: &[u8], store: &mut Option<ShimStore>) -> Option<alloc::vec::Vec<u8>> {
    use crate::process::{ERR, NOTHING};
    let (value, bytes) = match crate::process::vault_answer()? {
        Ok(a) => a,
        Err(()) => return Some(alloc::vec![4]),
    };
    match job {
        VaultJob::Get { full, got } => match value {
            NOTHING => return Some(store_host(store, ferros_pt::Op::Read, full.as_bytes())),
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
        VaultJob::Extent { block, left, got, one, .. } => {
            if value == ERR || value == NOTHING {
                return Some(alloc::vec![if value == NOTHING { 1 } else { 2 }]);
            }
            got.extend_from_slice(&bytes);
            let n = (bytes.len() >> 12) as u64;
            // A read that brings no whole block would ask for the same blocks for ever: the vault's error.
            if one.is_none() && n == 0 {
                return Some(alloc::vec![2]);
            }
            *block += n;
            *left = left.saturating_sub(n); // WHY/PROOF: the vault answers at most the length asked, but its count is its own; a longer answer would wrap the count of blocks left
            if one.is_some() || *left == 0 {
                let mut r = alloc::vec::Vec::with_capacity(1 + got.len());
                r.push(0);
                r.extend_from_slice(got);
                return Some(r);
            }
        }
        VaultJob::Items { .. } => {
            let mut r = alloc::vec::Vec::with_capacity(9 + bytes.len());
            r.push(0);
            r.extend_from_slice(&value.to_le_bytes());
            r.extend_from_slice(&bytes);
            return Some(r);
        }
        VaultJob::Remove { full } => {
            return Some(match value {
                0 => alloc::vec![0],
                NOTHING => store_host(store, ferros_pt::Op::Exec, full.as_bytes()),
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

/// A kist's capsule streamed from the vault process through the kernel's channel a transfer a call: the first transfers into `head` until the code's header is in (`kist_need` sizes the arc from it, masked or native as its code is), then straight into the arc at the code offset, as the bridge's streamed spawn lands its pieces. For a launch init asked for (process.rs `kist_request`), `spawn_kist` then checks it (a masked kist against its seal in `sealed`) and builds the process. For a stow the stower asked for (`stow_request`), the checker and the seal follow, a stage a pass, each told to the stower (`stow_tell`).
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
    /// A stow read whole and checked, waiting for its seal on the next pass: the gate's word on it, its modes and its code's span.
    checked: Option<(crate::oracle::Located, crate::oracle::Modes, usize, usize)>,
    /// A stow read whole, waiting for the checker on the next pass.
    read: bool,
    /// A stow refused and already told why.
    told: bool,
    /// A launch's two hashes taken as the bytes land in the arc (`kist_hash`): None until the arc is there, or for a capsule whose first bytes name no signature slot (hashed whole at the check, as before).
    twin: Option<ferros_kist::twin::Twin>,
    /// The counter ticks spent hashing as the bytes came.
    hash_ticks: u64,
}

enum KistWhy {
    Launch(crate::process::KistLaunch),
    Stow(crate::process::KistStow),
}

/// What a kist job came to: a launch built, a stow's seal for the `sealed` list, or a stow's native code taken unchecked on a development build (`KistStow::native_ok`).
enum KistDone {
    Launched,
    Sealed([u8; 32]),
    Native,
}

use crate::process::KistFault;
use ferros_kist::launch::why;

/// The kist job's next read (from `got`); false when the channel would not take it.
fn kist_call(job: &KistJob) -> bool {
    let mut rest = [0u8; 12];
    rest[..8].copy_from_slice(&(job.got as u64).to_le_bytes());
    // A megabyte a call where the vault process answers that much (`kernel_get_max`), else a STORE_MAX transfer.
    rest[8..].copy_from_slice(&(crate::process::kernel_get_max() as u32).to_le_bytes());
    match crate::process::vault_stamp_addr(crate::process::VOP_GET, "capsules", &job.capsule, &rest) {
        Some(msg) => crate::process::vault_call(msg),
        None => false,
    }
}

/// The kist's arc, sized from its first bytes (`kist_need`), masked when its code is (the guard above it), with those bytes copied in. Ok(false): the pool has no room yet while a process ends (a relaunch's own predecessor still leaving a core, its arc coming back at its next trap), so the job waits for that, not for a time; Err when there is no room and nothing is coming back.
fn kist_arc(job: &mut KistJob) -> Result<bool, KistFault> {
    let (need, ceiling) = crate::process::kist_need(&job.head, job.total).map_err(|e| (why::READ, e))?;
    let masked = crate::process::kist_masked(&job.head, job.total);
    // Its frames made clean first with the kernel lock let go (frames.rs `prepare`), so the take under it zeroes none.
    crate::frames::prepare(crate::process::arc_frames(need, masked));
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

/// A stow told a stage, if it is one.
fn kist_tell(job: &KistJob, stage: u64, value: u64) {
    if let KistWhy::Stow(i) = &job.why {
        crate::process::stow_tell(i, stage, value);
    }
}

/// A launch's bytes in its arc hashed up to what has come (`ferros_kist::twin`: the address and the signature's hash in one pass), the hasher made at the first bytes the arc holds from the slot its header names. The read's own pass is the only one over the bytes; `spawn_kist` checks the two hashes without hashing again.
fn kist_hash(job: &mut KistJob) {
    let (KistWhy::Launch(_), Some(arc)) = (&job.why, job.arc) else { return };
    let t = cntpct();
    if job.twin.is_none() {
        // Only from a capsule's first byte: the first call finds nothing hashed, and a job whose header names no slot hashes at the check instead.
        let first = unsafe { core::slice::from_raw_parts((arc.base + crate::process::CODE) as *const u8, job.got.min(ferros_kist::fetch::HEAD)) };
        match ferros_kist::fetch::head(first) {
            Ok(h) => job.twin = Some(ferros_kist::twin::Twin::new(h.slot)),
            Err(_) => return,
        }
    }
    let Some(tw) = job.twin.as_mut() else { return };
    let from = tw.len();
    let came = unsafe { core::slice::from_raw_parts((arc.base + crate::process::CODE + from) as *const u8, job.got - from) };
    tw.update(came);
    job.hash_ticks += cntpct() - t;
}

/// The kist job after its last bytes or its arc: a launch's process built once the capsule is whole; a stow told it is read and its checker due on the next pass; else the next read.
fn kist_next(job: &mut KistJob) -> Option<Result<KistDone, KistFault>> {
    if job.got >= job.total {
        // The last bytes hashed while the job still holds the arc.
        kist_hash(job);
        let Some(arc) = job.arc.take() else { return Some(Err((why::READ, "the capsule ended before its code"))) };
        return match &job.why {
            KistWhy::Launch(l) => {
                // The hashes whole only when every byte went thru the hasher; else `spawn_kist` hashes the arc itself.
                let t = cntpct();
                let pre = job.twin.take().filter(|tw| tw.len() == job.total).and_then(|tw| {
                    let first = unsafe { core::slice::from_raw_parts((arc.base + crate::process::CODE) as *const u8, job.total.min(ferros_kist::fetch::HEAD)) };
                    let slot = ferros_kist::fetch::head(first).ok()?.slot;
                    let (address, file) = tw.finish();
                    Some(crate::process::KistHashes { address, file, slot })
                });
                job.hash_ticks += cntpct() - t;
                Some(crate::process::spawn_kist(l, arc, job.total, &job.sealed, pre.as_ref()).map(|()| KistDone::Launched))
            }
            KistWhy::Stow(_) => {
                job.arc = Some(arc);
                job.read = true;
                kist_tell(job, crate::process::STOW_CHECKING, 0);
                // The checker on the next pass, so the stower shows the stage before the work: the loop goes round at once (no wait for an event).
                unsafe { core::arch::asm!("sev") };
                None
            }
        };
    }
    // The next read asked first, so the vault process works on it while this core hashes what came.
    let asked = kist_call(job);
    kist_hash(job);
    if !asked {
        return Some(Err((why::READ, "the kernel's channel to the vault process would not take the next read")));
    }
    None
}

/// A stow's stage after its reading, one a pass: the checker (SANDBOX.md: once, at its stow), then the seal. Done: its seal, for the service loop to keep in `sealed`.
fn stow_step(job: &mut KistJob) -> Option<Result<KistDone, KistFault>> {
    let arc = job.arc?;
    let code = match job.checked.take() {
        None => match crate::process::stow_check(&arc, job.total, &job.capsule) {
            Ok(c) => {
                job.checked = Some(c);
                kist_tell(job, crate::process::STOW_SEALING, 0);
                unsafe { core::arch::asm!("sev") };
                return None;
            }
            Err(c) => c,
        },
        Some((g, m, _, _)) => match crate::process::stow_seal(&g, &m, &job.capsule) {
            Ok(seal) => {
                job.arc = None;
                crate::process::with_lock(|| crate::process::arc_free(arc));
                return Some(Ok(KistDone::Sealed(seal)));
            }
            Err(c) => c,
        },
    };
    kist_tell(job, crate::process::STOW_REFUSED, code);
    job.told = true;
    Some(Err((why::CHECKS, "refused (the reason is on the console)")))
}

/// One step of the kist job: waiting for its arc, a stow's next stage once it is read, or the answer to its last read if it has come, then its next read. Some(the outcome) once it is done; the arc is the process's or back in the pool by then.
fn kist_step(job: &mut KistJob) -> Option<Result<KistDone, KistFault>> {
    use crate::process::{ERR, NOTHING};
    let r = (|| {
        if job.read {
            return stow_step(job);
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
            NOTHING => return Some(Err((why::NOT_HERE, "no such capsule in the vault (stow it first)"))),
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
                    // A stow's native code is known from its first bytes: refused there (never stowed from the network, SANDBOX.md), or for a stow on a development build taken unchecked (`native_ok`), with nothing more read and no arc asked for. A production build refuses it either way.
                    if let KistWhy::Stow(i) = &job.why {
                        if !crate::process::kist_masked(&job.head, job.total) {
                            if i.native_ok && !cfg!(ferros_production) {
                                return Some(Ok(KistDone::Native));
                            }
                            crate::process::push_console(b"kist stow: native code: a development build is not stowed from the network, and a production build stows none\n");
                            kist_tell(job, crate::process::STOW_REFUSED, crate::process::STOW_WHY_NATIVE);
                            job.told = true;
                            return Some(Err((why::CHECKS, "native code")));
                        }
                    }
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
        // The stower's progress bar: told at each mebibyte read, not each transfer.
        if before >> 20 != job.got >> 20 {
            kist_tell(job, crate::process::STOW_READING, job.got as u64);
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
///
/// Who may touch it (2026-11-18, process.rs "the storage locks"): every read or write of the vault holds `VAULT`, from the service loop alone (`vault`) or from a syscall under the kernel lock; kvault.rs's blocks panic otherwise.
/// The unlocked identity's state in memory (kvault's `id` and the bound mark's cache) is written only holding both the kernel lock and `VAULT` (an unlock, a lock, a bind, a release, the person key, the mark's first read), so it may be read holding either: the domain keys, a kist's alias and init's identity poll read it under the kernel lock alone, and never wait for a long vault read; the service loop reads whether a person is present from kvault's `PERSON`, kept beside it.
static mut SPAWN_STORE: *mut Option<ShimStore> = core::ptr::null_mut();

/// `f` holding the vault's lock (process.rs `VAULT`) and not the kernel lock: the service loop's way into its vault since 2026-11-18, so the other cores' syscalls go on while it reads a capsule or the AoC's firmware (24 to 148 ms under the kernel lock before, `diag lock`).
/// Never for a closure that touches the kernel's tables (the process table, an arc, the preferences): that takes `with_lock` first, then this inside it.
#[track_caller]
fn vault<R>(f: impl FnOnce() -> R) -> R {
    crate::process::VAULT.with(f)
}

/// Android from within ferros, behind an attest (owner, 2026-11-01 and 2026-11-04: "launch Android from within ferros so it's gated behind an attest", from the deck). The device's opt-in, the vault's device-class `android.allowed` (one byte, 1), as the service loop read it at start: 2 allowed, 1 not, 0 not read yet (SYS_FACTS 82, so the map shows the Android circle only on a device that allows it). Written by the opt-in (the disclaimer, next) and for now from the host.
pub static ANDROID_ALLOWED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The disclaimer text's number (ferros_lang `Msg::AndroidDisclaimer`; 0 the text of 2026-11-01 as shown from 2026-11-21): kept with the person's acceptance, so a changed text can be asked again.
const DISCLAIMER_VERSION: u8 = 0;

/// The person's acceptance of the Android disclaimer as kept in their identity class (kvault `ANDROID_DISCLAIMER`): section `android_disclaimer`, `version` (`DISCLAIMER_VERSION`) and `accepted` (Eagle time).
fn disclaimer_document() -> Option<alloc::vec::Vec<u8>> {
    use vsf::schema::{SectionSchema, TypeConstraint};
    let now = crate::settings::eagle_now();
    let section = SectionSchema::new("android_disclaimer")
        .field("version", TypeConstraint::AnyUnsigned)
        .field("accepted", TypeConstraint::AnyEagleTime)
        .build()
        .set("version", vsf::VsfType::u(DISCLAIMER_VERSION as usize, false))
        .and_then(|b| b.set("accepted", vsf::VsfType::e(vsf::EtType::e6(now))))
        .and_then(|b| b.encode())
        .ok()?;
    vsf::VsfBuilder::new().creation_time_oscillations(now).provenance_only().add_unboxed("android_disclaimer", section).build().ok()
}

/// A launch the `android` switch accepted, for the service loop's next pass.
static ANDROID_REQ: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// The person's standing choice is still to be applied this boot (owner, 2026-11-21: "Start into: ferros / Android", settings.rs `START_ANDROID`): set by main.rs only for a boot nobody answered (a production boot, or a development gate's untouched second; not a press of power, the chord's FERROS_ONCE, a refused recovery boot, the jump or a reload), taken by the service loop at the first attest: with the setting at Android on a device that allows it, the launch (`ANDROID_REQ`).
pub static START_INTO: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// Android's recovery request in misc (ferros_hal::bcb `ASK_*`, 0 none; docs/BOOT.md "Recovery requests from Android"; owner, 2026-11-22: "boot into ferros and then have the user attest, then grant the one time boot wipe"): read at boot (`recovery_ask_read`) and left in misc untouched until the person answers it on the Android page (SYS_UNLOCK op 13, requests 6 to 8). A launch of Android waits for that answer (`ANDROID_REQ`), so no request is cleared on the way past it.
pub static RECOVERY_ASK: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
/// The person's answer, for the service loop: 1 yes (RECOVERY_ONCE and a reset into recovery, the arguments still in misc, so recovery does what Android asked without the person choosing it again), 2 no (the command and arguments cleared, nothing else in misc). Set only by op 13 with a person present.
pub static RECOVERY_ANSWER: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
/// A launch of Android held for that answer has been said in the flash log (once a hold).
static ANDROID_HELD_SAID: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// A request's words for the flash log and the diag.
fn recovery_ask_words(ask: u8) -> &'static str {
    match ask {
        ferros_hal::bcb::ASK_NONE => "none",
        ferros_hal::bcb::ASK_WIPE => "a factory reset (erase Android's data)",
        ferros_hal::bcb::ASK_RESCUE => "a rescue (Android could not start; recovery offers to erase its data)",
        ferros_hal::bcb::ASK_REMOTE => "a remote wipe (refused: no one chose it at the device)",
        _ => "something that can change what boots (an update, a sideload, fastbootd, the recovery menu: needs a second fleet device, not built)",
    }
}

/// Whether the person may allow `ask` with their attest alone (owner, 2026-11-02: a wipe or a crash rescue touches Android's data only; anything that changes what boots needs a second fleet device too; a remote wipe is refused).
pub(crate) fn recovery_allowable(ask: u8) -> bool {
    ask == ferros_hal::bcb::ASK_WIPE || ask == ferros_hal::bcb::ASK_RESCUE
}

/// The held request changed (`RECOVERY_ASK`): kept, and the facts' bell rung (process.rs `BELL_FACTS`), which wakes the map, so the Android page and the Deck button's question follow at once, not at the next touch. On the service loop.
fn recovery_ask_set(ask: u8) {
    if RECOVERY_ASK.swap(ask, core::sync::atomic::Ordering::AcqRel) != ask {
        crate::process::with_lock(|| crate::process::bell_count(crate::process::BELL_FACTS));
    }
}

/// Android's recovery request as this boot finds it in misc (main.rs, once the disk answers and the flash log is up): kept in `RECOVERY_ASK` and said with its arguments; never cleared here.
pub fn recovery_ask_read() {
    let line = match crate::slot::misc_request() {
        Ok((ask, args)) => {
            RECOVERY_ASK.store(ask, core::sync::atomic::Ordering::Release);
            if ask == ferros_hal::bcb::ASK_NONE {
                return;
            }
            // Android's text, one line: each byte outside printable ASCII (the newlines between its arguments among them) as '|'.
            let args: alloc::string::String = args.iter().map(|&c| if (0x20..0x7F).contains(&c) { c as char } else { '|' }).collect();
            alloc::format!("recovery: Android asks for {}, its arguments \"{args}\"; kept in misc until the person answers after the attest", recovery_ask_words(ask))
        }
        Err(e) => alloc::format!("recovery: misc not read (G#{e:X}): no question this boot, and a launch of Android reads it again"),
    };
    crate::flashlog::line(&line);
}

/// RECOVERY_ONCE for the hv's gate (ferros_hv gate.rs: the gate refuses a recovery boot without it; no reflash without the handle, owner 2026-11-01), cleaned to DRAM, then the reset into the bootloader's recovery mode. Returns only when the EL3 monitor refused the reboot mode (its answer).
fn recovery_once_reset() -> u64 {
    unsafe {
        let p = 0xFD60_0FF0usize as *mut u64;
        core::ptr::write_volatile(p, 0x5245_434F_5645_5231);
        core::arch::asm!("dc civac, {a}", "dsb sy", a = in(reg) p as u64);
    }
    platform::crumb(0xC7);
    platform::reboot_to_mode(platform::REBOOT_MODE_RECOVERY)
}

/// The person said yes to Android's request (the service loop, a person present): misc read again, and only while it still asks what the person was shown and that is theirs to allow, RECOVERY_ONCE and the reset into recovery. misc is not touched: recovery reads the arguments Android left and does what it asked.
fn recovery_yes() {
    let ask = RECOVERY_ASK.load(core::sync::atomic::Ordering::Acquire);
    if !recovery_allowable(ask) {
        crate::flashlog::line(&alloc::format!("recovery: yes refused: {} is not the person's alone to allow", recovery_ask_words(ask)));
        return;
    }
    // From a jumped session every reset ferros orders is a power cycle, which loses the gate's word in DRAM (husky_cold_slot_b, 2026-11-15): the gate would refuse the recovery boot and the person would see the question again.
    if platform::jumped() {
        crate::flashlog::line("recovery: yes not taken in a jumped session (the reset loses the gate's word): restart ferros, then answer again");
        return;
    }
    match crate::slot::misc_request() {
        Ok((now, _)) if now == ask => {}
        Ok((now, _)) => {
            recovery_ask_set(now);
            crate::flashlog::line(&alloc::format!("recovery: yes not taken: misc now asks for {}, not what the person was shown", recovery_ask_words(now)));
            return;
        }
        Err(e) => {
            crate::flashlog::line(&alloc::format!("recovery: yes not taken: misc not read (G#{e:X})"));
            return;
        }
    }
    crate::flashlog::line(&alloc::format!("recovery: the person allowed {} once: RECOVERY_ONCE, resetting into recovery", recovery_ask_words(ask)));
    let r = recovery_once_reset();
    crate::flashlog::line(&alloc::format!("recovery: the EL3 monitor refused the reboot mode ({r})"));
}

/// The person said no (the service loop, a person present): the command and arguments cleared in misc, nothing else there (slot.rs `misc_clear_recovery`), and ferros stays. A launch of Android held for the answer goes on after it (`ANDROID_REQ`).
fn recovery_no() {
    let m = crate::slot::misc_clear_recovery();
    if m == crate::slot::MARKED || m == crate::slot::ALREADY {
        recovery_ask_set(ferros_hal::bcb::ASK_NONE);
        crate::flashlog::line(if m == crate::slot::MARKED { "recovery: the person said no: Android's request cleared in misc" } else { "recovery: the person said no: misc held no request" });
    } else {
        crate::flashlog::line(&alloc::format!("recovery: the person said no, but misc was not cleared (G#{m:X}): the question stays"));
    }
}
/// A completed departure released the identity (SYS_UNLOCK op 4): the service loop restarts the device, which comes back at the attest screen (AUTH.md's "no way back to the attest screen" gap, closed the plain way).
static RELEASE_RESET: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// The drops of the released person's domains in the vault process (process.rs `vault_stamp_drops`), each a stamp carrying that domain's keys, waiting for the service loop to send them before the restart. Under the kernel lock (filled in a syscall, taken by the loop).
static mut RELEASE_DROPS: alloc::vec::Vec<alloc::vec::Vec<u8>> = alloc::vec::Vec::new();
/// The release was a removal (SYS_UNLOCK op 15; AUTH.md "Remove this device"; owner, 2026-11-22: "then the killswitch"): once the drops are sent the service loop powers the device off (PSCI SYSTEM_OFF) instead of restarting it, so the next power-on is a cold boot at the attest screen.
static RELEASE_OFF: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// More drops for the release under way (process.rs op 15: each kist's own domain of the person's, stamped while their keys still existed). Under the kernel lock, as `RELEASE_DROPS` is filled.
pub(crate) fn release_drops_add(mut drops: alloc::vec::Vec<alloc::vec::Vec<u8>>) {
    crate::flashlog::line(&alloc::format!("identity removed: A#{} of the kists' own domains of the person's to drop in the vault process too", drops.len()));
    unsafe { (*(&raw mut RELEASE_DROPS)).append(&mut drops) };
}

/// The kernel's half of a removal's dry run (SYS_UNLOCK op 15 with its dry bit, `diag "identity remove"`): what the release would take from the kernel's vault and registers, as lines of text with every number in A#. Read only. None with no vault.
/// Under the kernel lock, as every caller of `store_unlock` holds it.
pub(crate) fn remove_preview_text() -> Option<alloc::string::String> {
    let p = vault(|| unsafe { (*(&raw mut SPAWN_STORE)).as_mut() }.and_then(|s| s.as_mut()).map(|s| s.remove_preview()))?.ok()?;
    let yn = |b: bool| if b { "yes" } else { "no" };
    Some(alloc::format!(
        "person present: {}; bound to an identity: {}\nkernel's vault, the identity class: A#{} entries deleted\ntags kept to attest with: A#{} deleted\nthe seed kept for attest at boot: {}\nthe device-only secret: {}\nthe person key in the kernel: {}\nthe person's keys in registers: dropped (the lock)\n",
        yn(p.unlocked),
        yn(p.bound),
        p.entries,
        p.tags,
        if p.kept_seed { "there, deleted" } else { "none" },
        if p.secret { "there, deleted (a new one at the next attest, so nothing missed opens again)" } else { "none yet" },
        if p.person_key { "held, wiped" } else { "not held" },
    ))
}

/// The `android` switch (process.rs POWERS): accepted only on a device that allows Android and while a person is present (the vault's identity class unlocked: the kernel's own state, set by an attest through SYS_UNLOCK, never a process's word). Nothing in the base OS holds it (owner, 2026-11-04: nothing is built in but system settings): it is for an Android launcher a person stows, and this check is the lock whoever holds it. The service loop then does the gate's Android path: ANDROID_ONCE, slot a, the reset with its scrub (`diag android`'s sequence).
pub fn android_request(on: bool) -> bool {
    let present = unsafe { (*(&raw mut SPAWN_STORE)).as_mut() }.and_then(|s| s.as_mut()).is_some_and(|s| s.unlocked());
    let ok = on && present && ANDROID_ALLOWED.load(core::sync::atomic::Ordering::Relaxed) == 2;
    crate::flashlog::line(if ok { "android: launch accepted (attested, allowed)" } else if !present { "android: launch refused: no one has attested" } else { "android: launch refused: this device does not allow Android" });
    if ok {
        ANDROID_REQ.store(true, core::sync::atomic::Ordering::Release);
    }
    ok
}

/// The settings document (SETTINGS.md: the preferences, the radio knobs, the zone, the language) into the device vault, holding the vault's lock and not the kernel lock: `doc` and its generation `at` from process.rs `settings_snapshot`, taken under the kernel lock where the change was made.
/// A document older than one already kept is not written (answered kept: the newer one carries its change), so writes made out of order on different cores never put an older state over a newer one.
/// Registered as process.rs `SETTINGS_WRITE` (the trap's end of SYS_SET_PREF, SYS_CONTROL, SYS_ZONE and SYS_LANG), and the service loop's own writes go thru it (`keep_settings_now`).
fn settings_write(doc: &[u8], at: u64) -> bool {
    vault(|| {
        let kept = unsafe { &mut *(&raw mut SETTINGS_KEPT) };
        if at <= *kept {
            return true;
        }
        let ok = unsafe { (*(&raw mut SPAWN_STORE)).as_mut().and_then(|s| s.as_mut()).map(|s| s.set(crate::settings::KEY, doc).is_ok()).unwrap_or(false) };
        if ok {
            *kept = at;
        }
        ok
    })
}
/// The newest settings document's generation on the flash (`settings_write`). Under the vault's lock.
static mut SETTINGS_KEPT: u64 = 0;

/// The service loop's write of the settings document as it stands: the snapshot under the kernel lock, the write under the vault's alone. Whether it was kept.
pub(crate) fn keep_settings_now() -> bool {
    crate::process::with_lock(crate::process::settings_snapshot).is_some_and(|(doc, at)| settings_write(&doc, at))
}

/// `SYS_STORE`'s vault: the kernel has already checked the capability and prefixed the key with its scope.
/// Under the kernel lock (the syscall's) and the vault's, as each of the syscalls' ways into the vault below (`SPAWN_STORE`).
fn store_scoped(key: &str, value: Option<&[u8]>) -> Result<Option<alloc::vec::Vec<u8>>, ()> {
    vault(|| store_scoped_held(key, value))
}
fn store_scoped_held(key: &str, value: Option<&[u8]>) -> Result<Option<alloc::vec::Vec<u8>>, ()> {
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

/// The base of the device's vault domains (process.rs `domain_keys`, VAULT-OUT.md), from the ira, derived once.
/// The person's base is held sealed and never handed out: their domain keys come from `identity_domain_keys`, and this answers None for them.
pub(crate) fn domain_base(identity: bool) -> Option<[u8; 32]> {
    if identity {
        return None;
    }
    let base = unsafe { &mut *(&raw mut DEVICE_DOMAIN) };
    if base.is_none() {
        *base = crate::ira::derive(DEVICE_DOMAIN_CONTEXT);
    }
    *base
}
const DEVICE_DOMAIN_CONTEXT: &str = "ferros 2026-11-07 device vault domain";
static mut DEVICE_DOMAIN: Option<[u8; 32]> = None;

/// The person's domain keys under `prefix` (process.rs `domain_keys`), derived from their held domain key in registers (kvault `identity_domain_keys`); None while nobody is present.
/// Under the kernel lock alone (a syscall's): the identity's keys in memory, which are written only holding both locks (`SPAWN_STORE`), so this never waits for a vault read on another core.
pub(crate) fn identity_domain_keys(prefix: &[u8], addr_context: &str, seal_context: &str) -> Option<[u8; 64]> {
    unsafe { (*(&raw mut SPAWN_STORE)).as_mut() }.and_then(|s| s.as_mut())?.identity_domain_keys(prefix, addr_context, seal_context)
}

/// Before any reset the kernel orders: the identity's key dropped (AUTH.md: it locks when power goes or the device switches OS, and a warm reset keeps DRAM).
/// Without the locks: a reset path (platform.rs `before_reset`), on a core that may hold either, which must never wait; nothing runs after it but the reset.
fn lock_before_reset() {
    if let Some(st) = unsafe { (*(&raw mut SPAWN_STORE)).as_mut() }.and_then(|s| s.as_mut()) {
        st.lock();
    }
}

/// The identity class's lock (AUTH.md): op 0 asks (bit 0 unlocked, bit 1 bound, bit 2 the person key held), 1 unlocks from a seed (the number of entries the identity has on this device, or NOTHING: the device is bound to another identity), 2 locks, 3 binds the device to the unlocked identity, 4 releases it (every identity entry deleted).
/// Op 0 reads the identity's state in memory under the kernel lock alone (`SPAWN_STORE`), the vault read only for the bound mark's first read; every other op holds the vault's lock too.
/// Every op but the question rings the identity's state bell after it (process.rs `identity_ring`, SYS_UNLOCK op 11), done or failed part way (an unlock refused has locked); the bell rings only when the state differs from the one it rang last.
fn store_unlock(op: u8, seed: Option<&[u8; 32]>) -> Result<u64, ()> {
    if op == 0 {
        let st = unsafe { (*(&raw mut SPAWN_STORE)).as_mut() }.and_then(|s| s.as_mut()).ok_or(())?;
        let bound = match st.bound_known() {
            Some(b) => b,
            None => vault(|| st.bound().unwrap_or(false)),
        };
        return Ok(st.unlocked() as u64 | (bound as u64) << 1 | (st.person_held() as u64) << 2);
    }
    let r = vault(|| store_unlock_held(op, seed));
    identity_ring();
    r
}

/// The identity's state as op 0 answers it, into the identity's state bell (process.rs `identity_ring`).
/// Under the kernel lock, as every caller of `store_unlock` and `store_tag` holds it (a syscall, or `with_lock` from a diag).
fn identity_ring() {
    if let Ok(s) = store_unlock(0, None) {
        crate::process::identity_ring(s);
    }
}

fn store_unlock_held(op: u8, seed: Option<&[u8; 32]>) -> Result<u64, ()> {
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
        // The person key from the identity service (FACETS.md D1): held beside the identity's keys, wiped at lock. The log says that it came, never a byte of it.
        (10, Some(k)) => match st.set_person(&mut k.clone()) {
            Ok(true) => {
                crate::flashlog::line("person key held: kists' aliases can be derived");
                Ok(0)
            }
            Ok(false) => Ok(0),
            Err(_) => Err(()),
        },
        // The retype gate (ahau_core::retype, process.rs op 12): 1 the seed typed again is the person present's and is now kept for unlock at boot (`retype_keep`), 0 it is not and nothing was written, 2 it is but could not be kept; Err while nobody is present. The log says which, never a byte of either seed.
        (12, Some(s)) => match st.retype_keep(s) {
            Ok(m) => {
                crate::flashlog::line(if m { "attest at boot: the handle typed again matches the person present; the seed kept for the next boot" } else { "attest at boot: the handle typed again does not match; nothing changed" });
                Ok(m as u64)
            }
            Err(crate::kvault::KvError::Locked) => Err(()),
            Err(e) => {
                crate::flashlog::line(&alloc::format!("attest at boot: the handle typed again matches, but the seed could not be kept: {e:?}"));
                Ok(2)
            }
        },
        (3, _) => {
            st.bind().map_err(|_| ())?;
            crate::flashlog::line("identity bound: this device now opens only for it");
            Ok(0)
        }
        // 18: whether `seed` (a handle typed again just now) is the person present's (`retype_matches`, compared in registers): 1 it is, 0 it is not; Err while nobody is present. Nothing written. The first step of a removal (process.rs op 15).
        (18, Some(s)) => st.retype_matches(s).map(|m| m as u64).map_err(|_| ()),
        // 4 the release (a completed departure, `diag "identity release"`); 19 a removal (process.rs op 15, after op 18 matched): the same release, then the device powers off rather than restarting (`RELEASE_OFF`).
        (4 | 19, _) => {
            // The person's domains in the vault process, stamped while their keys still exist (release locks them); the service loop sends the drops before its restart.
            let mut drops = crate::process::vault_stamp_drops();
            let n = match st.release() {
                Ok(n) => n,
                Err(_) => {
                    for d in drops.iter_mut() {
                        crate::keys::wipe_to_dram(d);
                    }
                    return Err(());
                }
            };
            crate::flashlog::line(&alloc::format!("identity released: {n} entries deleted from the kernel's vault, the device-only secret renewed; {} of the person's domains to drop in the vault process", drops.len()));
            // The person's zone goes with them (zone.rs): this device's own choice rules again.
            let _ = crate::zone::set_identity(None, || crate::process::settings_snapshot().is_some_and(|(d, g)| settings_write(&d, g)));
            unsafe { (*(&raw mut RELEASE_DROPS)).append(&mut drops) };
            if op == 19 {
                RELEASE_OFF.store(true, core::sync::atomic::Ordering::Release);
                crate::flashlog::line("identity removed (the person typed their handle on this device): the device powers off once the drops are sent");
            }
            RELEASE_RESET.store(true, core::sync::atomic::Ordering::Release);
            Ok(0)
        }
        // Android on this device (process.rs SYS_UNLOCK ops 13 and 14; docs/BOOT.md "Android only by the identity's choice"). Every one but the state needs a person present.
        // 13: start Android now (the Settings page's action), on a device that allows it: the service loop's launch (`ANDROID_REQ`). 1 accepted, 0 refused.
        (13, _) => {
            if !st.unlocked() {
                return Err(());
            }
            let ok = ANDROID_ALLOWED.load(core::sync::atomic::Ordering::Relaxed) == 2;
            crate::flashlog::line(if ok { "android: start now, the person's action on the Settings page" } else { "android: start now refused: this device does not allow Android" });
            if ok {
                ANDROID_REQ.store(true, core::sync::atomic::Ordering::Release);
            }
            Ok(ok as u64)
        }
        // 14: the disclaimer's typed confirmation (ahau's keyboard): the handle typed again is the person present's, so their acceptance is kept in their identity class (once per person) and this device allows Android. 1 done, 0 they do not match, 2 they match but it could not be kept.
        (14, Some(s)) => match st.retype_matches(s) {
            Ok(true) => {
                let accepted = disclaimer_document().map(|d| st.set(crate::kvault::ANDROID_DISCLAIMER, &d).is_ok()).unwrap_or(false);
                let allowed = st.set("android.allowed", &[1]).is_ok();
                crate::flashlog::line(&alloc::format!("android: the disclaimer accepted by the person present (handle typed again): acceptance kept {accepted}, allowed on this device {allowed}"));
                if allowed {
                    ANDROID_ALLOWED.store(2, core::sync::atomic::Ordering::Relaxed);
                }
                Ok(if accepted && allowed { 1 } else { 2 })
            }
            Ok(false) => {
                crate::flashlog::line("android: the disclaimer's handle typed again does not match; nothing changed");
                Ok(0)
            }
            Err(_) => Err(()),
        },
        // 15: Android off on this device (the setting goes back to ferros in process.rs). The person's acceptance stays theirs. 1 done.
        (15, _) => {
            if !st.unlocked() {
                return Err(());
            }
            let ok = st.set("android.allowed", &[0]).is_ok();
            crate::flashlog::line(&alloc::format!("android: off on this device (the person's choice), kept {ok}"));
            if ok {
                ANDROID_ALLOWED.store(1, core::sync::atomic::Ordering::Relaxed);
            }
            Ok(ok as u64)
        }
        // 16: Android on again for a person who accepted the disclaimer before (on this device: their identity class). 1 done, 0 no acceptance kept.
        (16, _) => {
            if !st.unlocked() {
                return Err(());
            }
            let before = matches!(st.get_by_key(crate::kvault::ANDROID_DISCLAIMER), Ok(Some(_)));
            let ok = before && st.set("android.allowed", &[1]).is_ok();
            crate::flashlog::line(if ok { "android: on again on this device (the person accepted the disclaimer before)" } else { "android: on again refused: no acceptance of the disclaimer kept for the person present" });
            if ok {
                ANDROID_ALLOWED.store(2, core::sync::atomic::Ordering::Relaxed);
            }
            Ok(ok as u64)
        }
        // 17: the state, bit 0 this device allows Android, bit 1 the person present accepted the disclaimer.
        (17, _) => {
            let accepted = st.unlocked() && matches!(st.get_by_key(crate::kvault::ANDROID_DISCLAIMER), Ok(Some(_)));
            Ok((ANDROID_ALLOWED.load(core::sync::atomic::Ordering::Relaxed) == 2) as u64 | (accepted as u64) << 1)
        }
        _ => Ok(st.unlocked() as u64 | (st.bound().unwrap_or(false) as u64) << 1 | (st.person_held() as u64) << 2),
    }
}

/// A kist's alias seed (FACETS.md "The alias"; process.rs SYS_IRA's alias bit) for its gate identity as the kernel stamped it at launch: None while nobody is present or before the identity service has handed in the person key.
pub(crate) fn alias(developer: &[u8; 32], option: &[u8; 32]) -> Option<[u8; 32]> {
    unsafe { (*(&raw mut SPAWN_STORE)).as_mut() }.and_then(|s| s.as_mut())?.alias(developer, option)
}

/// Attest with a tag (AUTH.md; SYS_UNLOCK ops 6-9, process.rs `tag_syscall`). The flash log names a tag only by the start of its id (a keyed hash of its UID, kvault `TAG_ID_CONTEXT`), never by its UID.
/// An unlock by a tag rings the identity's bell after it, as `store_unlock` does (keeping, forgetting and listing tags change no state it carries).
fn store_tag(op: crate::process::TagOp) -> Result<u64, ()> {
    let unlock = matches!(op, crate::process::TagOp::Unlock(..));
    let r = vault(|| store_tag_held(op));
    if unlock {
        identity_ring();
    }
    r
}
fn store_tag_held(op: crate::process::TagOp) -> Result<u64, ()> {
    use crate::process::TagOp;
    let st = unsafe { (*(&raw mut SPAWN_STORE)).as_mut() }.and_then(|s| s.as_mut()).ok_or(())?;
    match op {
        TagOp::Unlock(uid, out) => match st.tag_unlock(uid) {
            Ok(seed) => {
                *out = seed;
                crate::flashlog::line("identity unlocked by a kept tag");
                Ok(st.identity_entries() as u64)
            }
            Err(crate::kvault::KvError::Belongs) => {
                crate::flashlog::line("tag refused: this device is bound to another identity");
                Ok(crate::process::NOTHING)
            }
            Err(e) => {
                crate::flashlog::line(&alloc::format!("tag refused ({e:?})"));
                Err(())
            }
        },
        TagOp::Keep(uid) => match st.tag_keep(uid) {
            Ok(true) => {
                crate::flashlog::line("tag kept for the identity present");
                Ok(0)
            }
            Ok(false) => Ok(1),
            Err(e) => {
                crate::flashlog::line(&alloc::format!("tag not kept ({e:?})"));
                Err(())
            }
        },
        TagOp::Forget(which) => {
            let n = st.tag_forget(which).map_err(|_| ())?;
            crate::flashlog::line(&alloc::format!("{n} kept tags forgotten"));
            Ok(n as u64)
        }
        TagOp::List(out) => {
            let tags = st.tag_list().map_err(|_| ())?;
            for (id, added) in &tags {
                out.extend_from_slice(id);
                out.extend_from_slice(&added.to_le_bytes());
            }
            Ok(tags.len() as u64)
        }
    }
}

/// The identity's own entries on this device as one VSF document, for the identity service to seal into its fleet before a departure (SYS_UNLOCK op 5; the Fleet page's "save first"): section `identity_export`, one `entry` field per vault name, its values the name (`d`) and the stored bytes (`v'r'`). None while locked.
fn store_export() -> Option<alloc::vec::Vec<u8>> {
    let entries = vault(|| unsafe { (*(&raw mut SPAWN_STORE)).as_mut() }.and_then(|s| s.as_mut())?.identity_export().ok())?;
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
    vault(|| {
        let st = unsafe { (*(&raw mut SPAWN_STORE)).as_mut() }.and_then(|s| s.as_mut()).ok_or(())?;
        crate::flashlog::line(&alloc::format!("store remove {key:?} (process)"));
        st.remove_key(key).map_err(|_| ())
    })
}

fn store_keys() -> Result<alloc::vec::Vec<alloc::string::String>, ()> {
    vault(|| unsafe { (*(&raw mut SPAWN_STORE)).as_mut() }.and_then(|s| s.as_mut()).ok_or(())?.keys().map_err(|_| ()))
}

fn store_addr(prefix: &str, addr: &[u8; 32], op: crate::process::AddrOp) -> Result<Option<alloc::vec::Vec<u8>>, ()> {
    vault(|| store_addr_held(prefix, addr, op))
}
fn store_addr_held(prefix: &str, addr: &[u8; 32], op: crate::process::AddrOp) -> Result<Option<alloc::vec::Vec<u8>>, ()> {
    use crate::process::AddrOp;
    let st = unsafe { (*(&raw mut SPAWN_STORE)).as_mut() }.and_then(|s| s.as_mut()).ok_or(())?;
    match op {
        AddrOp::Get => st.get_addr(prefix, addr).map_err(|_| ()),
        AddrOp::Set(v) => st.set_addr(prefix, addr, v).map(|_| None).map_err(|_| ()),
        AddrOp::Remove => st.remove_addr(prefix, addr).map(|was| was.then_some(alloc::vec::Vec::new())).map_err(|_| ()),
        AddrOp::List => st.addrs(prefix).map(Some).map_err(|_| ()),
    }
}

/// Holding the vault's lock; since 2026-11-18 SYS_SPAWN calls it with the kernel lock let go (process.rs `spawn_queued`).
fn read_capsule(key: &[u8]) -> Option<alloc::vec::Vec<u8>> {
    let k = core::str::from_utf8(key).ok()?;
    match vault(|| unsafe { (*(&raw mut SPAWN_STORE)).as_mut()?.as_mut() }.map(|s| s.get_by_key(k)))? {
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
    // The long holds (process.rs LONG_HOLD), oldest first: t<ms> the take, how long, where.
    let n = crate::process::LONG_HOLD_N.load(Relaxed) as usize;
    // WHY saturating: fewer holds than the ring keeps start at the first; past it, the ring's oldest.
    let first = n.saturating_sub(crate::process::LONG_HOLDS);
    for j in first..n {
        let h = &crate::process::LONG_HOLD[j % crate::process::LONG_HOLDS];
        s.push_str(&alloc::format!("lock held long: t{} for A#{} us at {}\n", h[0].load(Relaxed) / (freq / 1000), h[1].load(Relaxed) * 1_000_000 / freq, site_text(h[2].load(Relaxed))));
    }
    let prev_lock = platform::note_read(8);
    if prev_lock & 0xFF != 0 {
        s.push_str(&alloc::format!("lock at the previous boot's end: held by core A#{}, taken at {}\n", (prev_lock & 0xFF) - 1, site_text(prev_lock >> 8)));
    }
    // Each core's last crumb as the previous boot ended (platform::CORE_CRUMB_LAST): G#90 + n entering syscall n, G#70 + n leaving it, and how long before that boot's last crumb anywhere.
    let last: [u64; 9] = core::array::from_fn(|c| unsafe { core::ptr::read_volatile((platform::CORE_CRUMB_LAST + 0x80 + c * 8) as *const u64) });
    let newest = last.iter().map(|w| w >> 32).max().unwrap_or(0);
    if newest != 0 {
        s.push_str("last crumbs at the previous boot's end:");
        for (c, w) in last.iter().enumerate() {
            s.push_str(&alloc::format!(" core A#{c} G#{:X} A#{} us before;", w & 0xFFFF_FFFF, (newest - (w >> 32)) * 1024 * 1_000_000 / freq));
        }
        s.push('\n');
    }
    for (prev, what) in [(false, "this boot"), (true, "the previous boot")] {
        let w = platform::stall_read(prev);
        if w[0] & 0xFF == 0 {
            continue;
        }
        let holder = (w[0] >> 40) & 0xFF;
        let lock = if holder == 0 { alloc::string::String::from("the lock free") } else { alloc::format!("the lock held by core A#{} from {}", holder - 1, site_text(w[4] >> 8)) };
        let at = if w[1] == 0 {
            let ufs = if w[2] & 0xFF == 0 { alloc::string::String::from("the UFS lock free") } else { alloc::format!("the UFS lock held by core A#{} from {}", (w[2] & 0xFF) - 1, site_text(w[2] >> 8)) };
            let hold = if w[5] >> 63 == 0 { alloc::string::String::from("the hold not a syscall's") } else { alloc::format!("the hold syscall A#{} (x1 G#{:X}) by thread A#{}", w[5] & 0xFF, (w[5] >> 8) & 0xFF, (w[5] >> 16) & 0xFFFF) };
            alloc::format!("it did not answer (interrupts masked); {ufs}; the stalled core holds the storage locks G#{:X}; {hold}", w[3])
        } else { alloc::format!("ELR G#{:X} LR G#{:X} SPSR G#{:X}", w[1], w[2], w[3]) };
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
        // 9: the display serves the pucks' endpoint here (docs/PUCKS.md, 2026-11-17): the slot held the display's old heap region, which the display has not read since 2026-11-11, and init's grants page is full.
        Dma(b"display-fb"), Serve(b"puck"),
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
        // 68: was the PMU a second time (the modem takes slot 40's, the same grant, since 2026-11-05); now the layer arena (GPU.md "Layers"), whole: init passes it on as a 32 MiB block of it, the child's own (process.rs SYS_SPAWN; GPU-KISTS.md plan step 1, 2026-11-12), the attest screen's. The grants page holds 108 slots (process.rs PREFS at G#FC0), so slots are reused, never appended past 108.
        Dma(b"arena"),
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
        // 108 use `kists`, the stower's endpoint (payloads/kists, which the kernel starts with its own list, `kists_grants`): the map's Kists page stows, updates and unstows through it. The last slot the page holds (0 the console, 1 to 108).
        Use(b"kists"),
        // The `android` switch (android_request) is in no one's list: the base OS has no Android launcher (owner, 2026-11-04: nothing built in but system settings). It goes to a launcher a person stows, once stowing exists.
    ]
}

/// The person's wallpaper seed from their last unlock (kvault `unlock`), waiting for the service loop to start wallgen with it.
static mut WALLGEN_SEED: Option<[u8; 32]> = None;

pub fn wallgen_after_unlock(seed: [u8; 32]) {
    unsafe { WALLGEN_SEED = Some(seed) };
}

/// wallgen (payloads/wallgen) with the person's wallpaper seed as its input, from its capsule in the kernel's vault: it makes their wallpaper once on this device, on the GPU, and keeps it in their space, or ends at once if it already has. 1 the vault scope `map.`, 2 use display, 3 use gpu, 4 a 32 MiB block of the layer arena, its own (GPU-KISTS.md plan step 0: the picture drawn only by the GPU service; plan step 1: the block wallgen's alone). Started from the service loop, after an unlock (an unlock is a syscall, under the kernel's lock: no spawn there).
fn spawn_wallgen(store: &mut Option<ShimStore>, seed: &[u8; 32]) {
    // Under the vault's lock, each block under the UFS lock, as every UFS command must be: the vault process drives the same controller from the workers (SYS_BLOCK), and a read outside the lock collided with its moves at an attest and wedged the link (2026-11-07).
    let Some(capsule) = vault(|| store.as_mut().and_then(|s| s.get_by_key("wallgen").ok().flatten())) else {
        crate::process::push_console(b"home: no wallgen capsule in the vault; no wallpaper is made (the display draws none itself)\n");
        return;
    };
    let sealed = seals_for_launch(store);
    use crate::process::Grant::*;
    match crate::process::spawn_capsule(b"wallgen", &capsule, seed, &[Store(b"map"), Use(b"display"), Use(b"gpu"), Dma(b"layers-32")], &sealed) {
        Ok(o) => {
            mark_ran(&o);
            crate::process::console_flashlog(&o.console);
        }
        Err(e) => crate::process::push_console(alloc::format!("home: wallgen refused: {e}\n").as_bytes()),
    }
}

/// The vault process's capsule the kernel image carries (FERROS_VAULT_CAPSULE at build; VAULT-OUT.md decision 0), empty when the build carried none. The boot starts the vault from it first; a restart from the stored one first (the development loop pushes one).
#[cfg(ferros_vault_capsule)]
static VAULT_CARRIED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/vault.capsule.vsf"));
#[cfg(not(ferros_vault_capsule))]
static VAULT_CARRIED: &[u8] = &[];

/// The vault process's capabilities (payloads/vault: 1 serve `vault`, 2 the `vault` block window, 3 entropy, 4 the vault right, 5 the `clips` block window: its single-copy extents, docs/RAW-VIDEO.md). Not init's to hand out: the kernel starts it before init (VAULT-OUT.md, decision 0), and init's grants page is full.
fn vault_grants() -> alloc::vec::Vec<crate::process::Grant<'static>> {
    use crate::process::Grant::*;
    alloc::vec![Serve(b"vault"), Blocks(b"vault"), Entropy, Vault, Blocks(b"clips")]
}

/// The GPS driver's grants in its slot order (payloads/gps): 1 serve gps, 2 the SPI port, 3 gpio-far-alive, 4 gpio-gpp0, 5 gpio-hsi2, 6 the vault scope `gps.`, 7 the power switch, 8 HOST_REQ's interrupt. 9 use udp (since 2026-11-18: the orbits for the predicted sky, CelesTrak over plain HTTP, at most daily). 10 use rmnet (2026-11-19, the 8a: the KEPLER core booted thru the modem driver's gnss_boot ops; the Pro's driver never calls it). Its own list, as the vault's: init's page is full at 108, and the kernel starts this driver itself, only once a person has attested (radios at attest, GPS locked until then).
fn gps_grants() -> alloc::vec::Vec<crate::process::Grant<'static>> {
    use crate::process::Grant::*;
    alloc::vec![Serve(b"gps"), Hardware(b"usi3-spi"), Hardware(b"gpio-far-alive"), Hardware(b"gpio-gpp0"), Hardware(b"gpio-hsi2"), Store(b"gps"), Power(b"gps"), Irq(b"gps-host-req"), Use(b"udp"), Use(b"rmnet")]
}

/// The GPU service's grants in its slot order (payloads/gpu): 1 its registers, 2 its memory, 3 the vault scope `gpu.` (its firmware), 4 its job interrupt, 5 the layer arena whole (it reads a client's kernel from the client's own block), 6 serve gpu, 7 the apps' surfaces (since 2026-11-11: a client's kernel writes its own surface, Mahere's map; the GPU's fence admits them, s2mpu::fence_gpu), 8 the GPU MMU's interrupt (2026-11-12: a client's job that reaches outside its own memory faults in its own address space, and the service ends that job at once), 9 the `gpu-clock` switch (2026-11-20: the GPU's clock up while a job is queued or running, down when the queues empty, `gpu_clock`). Its own list since 2026-11-08, as the vault's and the GPS driver's: init's page gave its five slots to kists, and the kernel starts the GPU service at home, before init (so before the attest screen, its first client).
fn gpu_grants() -> alloc::vec::Vec<crate::process::Grant<'static>> {
    use crate::process::Grant::*;
    alloc::vec![Dma(b"gpu-regs"), Dma(b"gpu-mem"), Store(b"gpu"), Irq(b"gpu-job"), Dma(b"arena"), Serve(b"gpu"), Dma(b"surfaces"), Irq(b"gpu-mmu"), Power(b"gpu-clock")]
}

/// The stower's grants in its slot order (payloads/kists; docs/KISTS.md "The stower"): 1 use udp (the network, plain HTTP), 2 the vault scope `capsules.` (it stores what it fetched, by content address), 3 the vault scope `kists.` (the person's list), 4 serve `kists` (the map's Kists page), 5 use fgtw (the identity service's doorbell: a restore follows the person's space), 6 the Stow right (the kernel checks and seals what it stored), 7 use icons (the icon decoder: a kist's icon decoded once at stow, 2026-11-20). Its own list, as the GPS driver's: init's page is full, and the kernel starts it once a person has attested (the list is theirs).
fn kists_grants() -> alloc::vec::Vec<crate::process::Grant<'static>> {
    use crate::process::Grant::*;
    alloc::vec![Use(b"udp"), Store(b"capsules"), Store(b"kists"), Serve(b"kists"), Use(b"fgtw"), Stow, Use(b"icons")]
}

/// The icon decoder's grants (std/icondec, docs/KISTS.md "Icons"): 1 serve icons, nothing else: no vault, no network, no device. rav1d is a large decoder fed a stranger's bytes, so it holds nothing to misuse; the stower hands it an icon's AV1 and stores what it hands back. Started on demand (process.rs `ON_DEMAND`) from its capsule, `icons`, as the torch is.
fn icons_grants() -> alloc::vec::Vec<crate::process::Grant<'static>> {
    use crate::process::Grant::*;
    alloc::vec![Serve(b"icons")]
}

/// The torch's grants in its slot order (payloads/torch, docs/TORCH.md): 1 serve torch, 2 hsi2c_14 (the LM3644's bus), 3 the `torch` switch. Its own list, as the GPS driver's: init's page is full, and the kernel starts it when a call on `torch` finds no server (process.rs `ON_DEMAND`).
fn torch_grants() -> alloc::vec::Vec<crate::process::Grant<'static>> {
    use crate::process::Grant::*;
    alloc::vec![Serve(b"torch"), Hardware(b"hsi2c14"), Power(b"torch")]
}

/// The audio service's grants in its slot order (payloads/audio, docs/AUDIO.md): 1 serve audio, 2 the AoC's IPC block (`aoc-ipc`: its playback ring and its control queue), 3 the AoC's mailboxes (`aoc-mbox`), 4 the mailbox block f1's interrupt (`aoc-mbox-f1`, the playback period's clock), 5 the amplifiers' SPI port (`usi1-spi`), 6 gpio-gpn0 (the right amplifier's select), 7 the `amp` switch (both resets, the rail, the port's clock), 8 entropy (the streams' handles), and since 2026-11-22 9 `aoc-play-lo`, 10 `aoc-capture` and 11 `aoc-play-hi` (the playback and capture rings, which lie past `aoc-ipc`; the playback ring takes two windows: cachemap.rs AOC_PLAY_LO). Its own list, as the torch's: init's page is full, and the kernel starts it when a call on `audio` finds no server (process.rs `ON_DEMAND`). It and the `amp` development tool (payloads/amp) both drive the amplifiers: never both at once.
fn audio_grants() -> alloc::vec::Vec<crate::process::Grant<'static>> {
    use crate::process::Grant::*;
    alloc::vec![Serve(b"audio"), Dma(b"aoc-ipc"), Hardware(b"aoc-mbox"), Irq(b"aoc-mbox-f1"), Hardware(b"usi1-spi"), Hardware(b"gpio-gpn0"), Power(b"amp"), Entropy, Dma(b"aoc-play-lo"), Dma(b"aoc-capture"), Dma(b"aoc-play-hi")]
}

/// The camera driver's grants in its slot order (payloads/camera, docs/CAMERA.md "The driver"): 1 serve camera, 2 hsi2c_1 (the main sensor's bus and its lens actuator), 3 the ISP front end (its core, statistics blocks and PDMA engine), 4 the CSI links and the combo D-PHY, 5 SYSREG_ISPFE, 6 the vault scope `cam.` (the sensors' inits, the CPU's setup writes, the PDMA jobs), 7 the `camera` switch, 8 the camera's region (frames on demand, SYS_DMA x1 = 3), 9 the front end's line 0 (its frame clock), and since 2026-11-15 the other cameras' sensor buses after them, so the first nine keep their places: 10 hsi2c_3 (the ultrawide's), 11 hsi2c_2 (the front's), 12 hsi2c_4 (the tele's). Its own list, as the torch's: init's page is full, and the kernel starts it when a call on `camera` finds no server (process.rs `ON_DEMAND`); it ends itself once its holder closes.
fn camera_grants() -> alloc::vec::Vec<crate::process::Grant<'static>> {
    use crate::process::Grant::*;
    alloc::vec![Serve(b"camera"), Hardware(b"hsi2c1"), Hardware(b"ispfe"), Hardware(b"csis"), Hardware(b"ispfe-sysreg"), Store(b"cam"), Power(b"camera"), Dma(b"camera"), Irq(b"ispfe0"), Hardware(b"hsi2c3"), Hardware(b"hsi2c2"), Hardware(b"hsi2c4")]
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
    if name == b"icons" {
        return Some(icons_grants());
    }
    if name == b"torch" {
        return Some(torch_grants());
    }
    if name == b"audio" {
        return Some(audio_grants());
    }
    if name == b"camera" {
        return Some(camera_grants());
    }
    // init itself, started again at its death (process.rs `KEPT`) with the grants `start_home` gives it: it starts only what of the shell is missing (payloads/init `spawn_unique`).
    if name == b"init" {
        return Some(home_grants());
    }
    let slots: &[usize] = match name {
        // The same slots init hands out (payloads/init): a restarted server must see exactly what a booted one does (the display's vsync thread had no interrupt grant after a restart and never started, 2026-10-23).
        b"display" => &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 21, 22, 25, 26, 27, 97, 98, 99, 100, 101, 102, 103, 20],
        b"touch" => &[14, 15, 16, 23, 91],
        // 1 use display, 2 use touch, 3 settings, 4 the vault scope `map.`, 5 use fgtw (the Fleet page), 6 the vault scope `kists.` (the person's kists and answers), 7 use launch, 8 use kists (the stower: the Kists page), 9 use nfc (the NFC page's add), 10 use wlan (the WiFi page's picker) (payloads/map; as init gives them: with eight a restarted map had no NFC add, its torch was slot 9 not 11, and the WiFi page said the driver gives no list, 2026-11-18 on the 8a).
        b"map" => &[17, 18, 19, 20, 47, 107, 106, 108, 80, 44],
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
        // 1 use display, 2 use touch, 3 a surface, 4 the vault scope `ahau.`, 5 the vault scope `fgtw.`, 6 the ira key, 7 settings (the layout switch), 8 use fgtw (the identity service's verdict and the mint), 9 the identity unlock right, 10 a second surface, the keyboard's back buffer, 11 use gpu, 12 a 32 MiB block of the layer arena, its own, 13 use wlan (the network page) (payloads/ahau; as init gives them: without 13 a restarted attest screen's network page said "The WiFi is starting." forever, 2026-11-10).
        b"ahau" => &[17, 18, 22, 51, 48, 49, 19, 47, 85, 22, 24, 68, 44],
        // 1 the controller's registers, 2 its DMA memory, 3 its interrupt, 4 the kernel's PT commands, 5 the `usb` power switch (payloads/usb).
        b"usb" => &[86, 87, 88, 89, 90],
        // 1 the AoC's IPC block, 2 its mailboxes, 3 the vault scope `aoc.`, 4 the Sensors right, 5 the `sensors` switch (payloads/sensors).
        b"sensors" => &[92, 93, 94, 95, 96],
        _ => return None,
    };
    let all = home_grants();
    // The attest screen's two surfaces as init's spawn hands them out (SYS_SPAWN): each a free slot with frames of its own behind it. Passed as the whole region, a restarted attest screen drew into surface 0 whoever held it, and its back buffer was its own front (2026-11-01: every board redrew in place, black between). Not for the display server, which takes the region's base (surface 0 at boot).
    let own = name == b"ahau";
    let mut out: alloc::vec::Vec<crate::process::Grant<'static>> = slots.iter().filter_map(|&s| all.get(s - 1).copied()).map(|g| match g {
        crate::process::Grant::Dma(b"surfaces") if own => crate::process::Grant::Dma(b"surface"),
        // The arena as init's spawn passes it (SYS_SPAWN): a 32 MiB block, the attest screen's own (GPU-KISTS.md plan step 1).
        crate::process::Grant::Dma(b"arena") => crate::process::Grant::Dma(b"layers-32"),
        g => g,
    }).collect();
    // The identity service serves the facets as the slot after its last, as SYS_SPAWN hands it (docs/FACETS.md).
    if name == b"fgtw" {
        out.push(crate::process::Grant::Serve(b"facets"));
    }
    Some(out)
}

/// A capsule the vault keeps in pieces (kvault.rs `set`, over `ferros_pieces::PIECE`), spawned without its bytes ever whole in the kernel heap: under the kernel lock, its length read, a staging arc taken from the frame pool and the pieces read straight into it one at a time (the heap holds one piece), then the gate where it lies and the process built from its code (`spawn_capsule_in_place`), and the staging arc given back. None when the capsule is not kept in pieces (stored before 2026-11-12, or under `PIECE`): the caller reads it whole as before.
/// Why (2026-11-12, twice on the Pro, PREV_EXIT G#EE91): read whole, the display's 2.8 MB capsule was one 3 MB block of the heap and the gate's parse and clone two more, while a store put of it had needed the sealed copy and its staging, two more; with Mahere running, the 22 MiB heap had the bytes and no such holes.
/// The seals found only in their image form at a launch (process.rs `seal_ok`), moved to the address form (SANDBOX.md "Seal"; owner, 2026-11-17): the new seal written beside the old one, the list read back, and only from a list read back holding the new seal is the old one dropped. A crash between the writes leaves the old seal listed until the new one is, so the image runs either way; a move whose old seal is gone (a release, a move before it) writes nothing.
fn move_seals(store: &mut Option<ShimStore>) {
    for (old, new) in crate::process::seal_moves() {
        // Some(true) moved, Some(false) nothing left to move, None a write or its read back failed.
        let moved = vault(|| {
            let s = store.as_mut()?;
            let list = seals_read(s).ok()?;
            let Some(both) = ferros_kist::seal::with_new(&list, &old, &new) else { return Some(false) };
            seals_write(s, &both).ok()?;
            let back = seals_read(s).ok()?;
            let after = ferros_kist::seal::without_old(&back, &old, &new)?;
            seals_write(s, &after).ok().map(|_| true)
        });
        match moved {
            Some(true) => crate::process::push_console(b"seal: an image's seal moved to the address form\n"),
            Some(false) => {}
            None => crate::process::push_console(b"seal: an image's seal not moved (its old seal kept; its next launch tries again)\n"),
        }
    }
}

/// The seals of the installed masked images as the vault keeps them (`kvault::SEALED`, a `seals` document since 2026-11-17, ferros_kist::seal `list_document`): Ok(the seals, 32 bytes each, concatenated, as the checks take them; none when there is no list), Err(why) when there is a list that is not its document.
/// Such a list is never taken for an empty one where a seal is added: the write would drop every other seal.
fn seals_read(s: &mut ShimStore) -> Result<alloc::vec::Vec<u8>, &'static str> {
    match s.get_by_key(crate::kvault::SEALED) {
        Ok(None) => Ok(alloc::vec::Vec::new()),
        Ok(Some(doc)) => ferros_kist::seal::list_of(&doc).ok_or("the seals list is not its document"),
        Err(_) => Err("the seals list could not be read"),
    }
}

/// The seals written as their document.
fn seals_write(s: &mut ShimStore, list: &[u8]) -> Result<(), &'static str> {
    let doc = ferros_kist::seal::list_document(list, crate::settings::eagle_now()).ok_or("the seals list did not build")?;
    s.set(crate::kvault::SEALED, &doc).map(|_| ()).map_err(|_| "the vault did not keep the seals list")
}

/// The seals for a launch, read under the vault's lock: none when there is no vault, or a list that does not read (said on the console, the flash log saying why since the open: kvault's move to VSF), so a masked image is then refused as not installed and nothing is written over the list.
fn seals_for_launch(store: &mut Option<ShimStore>) -> alloc::vec::Vec<u8> {
    match vault(|| store.as_mut().map(seals_read)) {
        Some(Ok(list)) => list,
        Some(Err(why)) => {
            crate::process::push_console(alloc::format!("seal: {why}; no masked image runs until it reads (the flash log says why)\n").as_bytes());
            alloc::vec::Vec::new()
        }
        None => alloc::vec::Vec::new(),
    }
}

/// A seal added to the list (an install, a stow's check), unless it is there already: refused, the list untouched, when the list does not read.
fn seal_add(s: &mut ShimStore, seal: &[u8; 32]) -> Result<(), &'static str> {
    let mut list = seals_read(s)?;
    if list.chunks_exact(32).any(|e| e == seal) {
        return Ok(());
    }
    list.extend_from_slice(seal);
    seals_write(s, &list)
}

fn spawn_from_pieces(store: &mut Option<ShimStore>, name: &[u8], input: &[u8], grants: &[crate::process::Grant], sealed: &[u8], once: bool) -> Option<Result<crate::process::Outcome, &'static str>> {
    let key = core::str::from_utf8(name).ok()?;
    // Three holds, none of the kernel lock across a read (2026-11-18; one hold of the kernel lock from the length to the last piece before, 24 ms for pcie1's 633 KB and 148 ms at the most, `diag lock`): the length under the vault's lock, the staging arc under the kernel lock, the pieces into it under the vault's lock again.
    let total = vault(|| store.as_mut()?.pieces_len(key).ok().flatten())?;
    let need = crate::process::CODE + total;
    crate::frames::prepare(crate::process::arc_frames(need, false));
    let Some(arc) = crate::process::with_lock(|| crate::process::arc_alloc_room(need, need, false)) else {
        return Some(Err("the arc pool has no room to stage the capsule"));
    };
    let at = arc.base + crate::process::CODE;
    // WHY the bound: the length was read in a hold of the vault's lock before this one, and a store put of the same name between the two may have made the value longer than the arc was sized for.
    // PROOF it is needed: without it a longer value's pieces would be copied past [CODE, CODE + total) of the staging arc, over frames another arc holds.
    let mut over = false;
    let r = vault(|| {
        store.as_mut().map(|s| {
            s.read_pieces(key, |off, bytes| {
                if off + bytes.len() > total {
                    over = true;
                    return;
                }
                // SAFETY: [at + off, at + off + len) lies inside [CODE, CODE + total) of the staging arc (the bound above), which is the kernel's alone until it is freed.
                unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), (at + off) as *mut u8, bytes.len()) }
            })
        })
    });
    match r {
        Some(Ok(Some(n))) if n == total && !over => {}
        r => {
            crate::flashlog::line(&alloc::format!("restart {key}: reading its pieces failed: {r:?}{}", if over { " (the value grew past the length read first)" } else { "" }));
            crate::process::with_lock(|| crate::process::arc_free(arc));
            return Some(Err("the capsule's pieces did not read"));
        }
    }
    crate::flashlog::line(&alloc::format!("restart {key}: capsule {total} bytes from its pieces; spawning"));
    // SAFETY: the staging arc's [CODE, CODE + total) holds the capsule just read; nothing else holds the arc until it is freed below.
    let capsule = unsafe { core::slice::from_raw_parts_mut((arc.base + crate::process::CODE) as *mut u8, total) };
    let r = crate::process::spawn_capsule_in_place(name, capsule, input, grants, sealed, once);
    crate::process::with_lock(|| crate::process::arc_free(arc));
    Some(r)
}

/// `diag restart <name>…`: each named shell process killed and started again from its capsule in the vault, in the order given (display before map: the map hands a fresh display server its scene at start). The development loop for the shell — build, `store put`, restart — without a reboot.
fn restart(store: &mut Option<ShimStore>, names: &[u8], rep: &mut alloc::vec::Vec<u8>) {
    restart_with(store, names, b"", rep, false)
}

/// A kept process started again at its death (docs/KERNEL.md "Restarts"): `restart` with nothing killed and the spawn made only while no process of that name is alive, checked under the kernel lock as it is made, so it never starts a second beside one init's keeper or a restart by hand made meanwhile (answered "<name>: running already").
fn restart_once(store: &mut Option<ShimStore>, name: &[u8], rep: &mut alloc::vec::Vec<u8>) {
    restart_with(store, name, b"", rep, true)
}

/// Cellular (`i` 0) or WiFi (1) switched off: its driver, already stopped, started once with its power-down input, `off` for the modem (payloads/modem: the PCIe PHY down and isolated, then the CP's power pins as Android's gpio_power_off_cp drops them, its PMIC with CP_PWR) and G#F0 for pcie1 (Bluetooth off first, PERST low, the PHY down and isolated, WL_REG_ON low); each ends once its chip is down. True when that process started. After a jump from Android too (2026-11-15): neither power-down touches the controller's ELBI or DBI, which stalled the bus when Android left its link suspended (the PHY powered down, its isolation released); both crowbar Android's chip into the cold state (the Pro, b878: WiFi's `off`, then the WiFi driver joined and leased; the modem's `off` took Android's live CP down).
fn radio_down(store: &mut Option<ShimStore>, i: usize) -> bool {
    let (name, input): (&[u8], &[u8]) = if i == 0 { (b"modem", b"off") } else { (b"pcie1", &[0xF0]) };
    let mut rep = alloc::vec::Vec::new();
    restart_with(store, name, input, &mut rep, false);
    crate::flashlog::line(&alloc::format!("radio {} power-down: {}", core::str::from_utf8(name).unwrap_or("?"), core::str::from_utf8(&rep).unwrap_or("").trim_end()));
    rep.ends_with(b" started\n") || rep.ends_with(b" restarted\n")
}

/// `restart` with an input for each process started (its spawn input, as `bridge spawn` gives one): the radio switches start the modem and WiFi drivers once with their power-down input (`off`, G#F0) to take their chips down.
fn restart_with(store: &mut Option<ShimStore>, names: &[u8], input: &[u8], rep: &mut alloc::vec::Vec<u8>, once: bool) {
    // Vault reads under the vault's lock (`vault`: the vault's one owner at a time, the kernel lock free meanwhile); the spawns below take the kernel lock themselves.
    let sealed = seals_for_launch(store);
    for name in names.split(|&b| b == b' ').filter(|n| !n.is_empty()) {
        let Some(grants) = shell_grants(name) else {
            rep.extend_from_slice(b"not a shell process: ");
            rep.extend_from_slice(name);
            rep.push(b'\n');
            continue;
        };
        // What reads the person's space starts only when init's `wait_person` would start it (owner, 2026-11-04: "Attest should ONLY load attest", and "if it's not attached to an identity, not usable"): fgtw once a person has attested, the map once the device is also bound to that identity. A restart does not get around either. A vault that cannot say whether it is bound counts as not bound.
        let person = crate::kvault::PERSON.load(core::sync::atomic::Ordering::Acquire);
        // Under both locks: `bound` reads the vault (a UFS command) the first time and keeps the mark, state the kernel lock's readers read without the vault's (`SPAWN_STORE`).
        let attached = person && crate::process::with_lock(|| vault(|| store.as_mut().is_some_and(|s| s.bound().unwrap_or(false))));
        let refused: Option<&[u8]> = match name {
            b"fgtw" if !person => Some(b"not started, nobody has attested: "),
            b"gps" if !person => Some(b"not started, GPS is locked until a person attests: "),
            b"kists" if !person => Some(b"not started, nobody has attested (the stower keeps a person's list): "),
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
        // `once` (a kept process's restart at its death, `restart_once`): nothing killed, and the spawn refused if one of that name is alive by the time it is made.
        let killed = !once && crate::process::kill(name);
        // What it left switched on goes off before its successor starts, so the new process finds it off rather than switched off under it.
        crate::process::power_offs_apply();
        crate::flashlog::line(&alloc::format!("restart {nm}: {} ; reading the capsule", if killed { "killed" } else { "was not running" }));
        platform::crumb(0x52);
        // A capsule kept in pieces never enters the heap whole (`spawn_from_pieces`).
        if let Some(r) = spawn_from_pieces(store, name, input, &grants, &sealed, once) {
            platform::crumb(0x53);
            match r {
                Ok(o) => {
                    crate::process::console_flashlog(&o.console);
                    rep.extend_from_slice(alloc::format!("{nm} {}\n", if killed { "restarted" } else { "started" }).as_bytes());
                    crate::flashlog::line(&alloc::format!("restart {nm}: started from its pieces"));
                }
                Err(e) => {
                    rep.extend_from_slice(alloc::format!("{nm}: {e}\n").as_bytes());
                    crate::flashlog::line(&alloc::format!("restart {nm}: {e}"));
                }
            }
            platform::crumb(0x54);
            continue;
        }
        // The vault restarts from the stored capsule (the development loop pushes one), else from the one the kernel image carries.
        let stored = vault(|| store.as_mut().and_then(|s| s.get_by_key(core::str::from_utf8(name).unwrap_or("")).ok().flatten()));
        let Some(capsule) = stored.or_else(|| (name == b"vault" && !VAULT_CARRIED.is_empty()).then(|| VAULT_CARRIED.to_vec())) else {
            rep.extend_from_slice(b"no capsule in the vault for ");
            rep.extend_from_slice(name);
            rep.push(b'\n');
            continue;
        };
        crate::flashlog::line(&alloc::format!("restart {nm}: capsule {} bytes; spawning", capsule.len()));
        platform::crumb(0x53);
        match crate::process::spawn_capsule_as(name, &capsule, input, &grants, &sealed, once) {
            Ok(o) => {
                crate::process::console_flashlog(&o.console);
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

/// The ring has completed: start the vault's `init`, which starts the shell (the touch controller is the touch process's to bring up, thru its `touch` switch: KERNEL-OUT.md move 1). Its console goes to `diag console`.
/// Returns false when the shell is off this boot (`home.off`).
fn start_home(store: &mut Option<ShimStore>) -> bool {
    crate::checklist::HOME.store(cntpct(), core::sync::atomic::Ordering::Relaxed);
    // The bootloader's lock state once a boot, first on the console (bootlock.rs; owner, 2026-11-13: shown, never a nag).
    crate::process::push_console(crate::bootlock::console_line().as_bytes());
    // FERROS_RESCUE (2026-11-03): the vault's usb, pcie1 and bt capsules replaced by the builds this kernel carries, whatever is stored (a rescue kernel is flashed only on purpose, and a broken build of either kind leaves the bridge dead). A masked build of a driver that reaches its registers by raw pointers dies at once, and with usb dead the bridge cannot put the capsule back: the chicken-and-egg this switch exists for. Flash it once, let it mend the vault, flash the ordinary kernel again.
    #[cfg(ferros_rescue)]
    if let Some(s) = store.as_mut() {
        for (name, bytes) in [("usb", include_bytes!("../../target/rescue-usb.capsule.vsf") as &[u8]), ("pcie1", include_bytes!("../../target/rescue-pcie1.capsule.vsf") as &[u8]), ("bt", include_bytes!("../../target/rescue-bt.capsule.vsf") as &[u8])] {
            let r = vault(|| s.set(name, bytes));
            crate::process::push_console(alloc::format!("rescue: the vault's {name} replaced by the kernel's known-good capsule ({})\n", if r.is_ok() { "ok" } else { "failed" }).as_bytes());
        }
    }
    // Any board without `init` in its vault (a fresh start, `vault fresh`, or akita's first boots): the USB process alone, from the native capsule this kernel carries (target/akita-usb.capsule.vsf, the usb payload whatever the board), so the bridge reaches the device and can store the shell.
    // With `init` stored, the path below.
    // husky too since 2026-11-18 (the owner's fresh start of both devices): before, a husky vault without init came up with no USB at all.
    let bare = vault(|| store.as_mut().and_then(|s| s.get_by_key("init").ok().flatten())).is_none();
    if bare {
        #[cfg(ferros_usb_capsule)]
        // Without a vault there is no ira and so no oracle secret: the capsule is checked by its signature and runs without an ihi (spawn_capsule would refuse it: "no ira: no system secret, no ihi", the first akita boots, 2026-11-06).
        match shell_grants(b"usb").map(|g| crate::process::spawn_capsule_no_ihi(b"usb", USB_CARRIED, b"", &g)) {
            Some(Ok(o)) => {
                mark_ran(&o);
                crate::process::push_console(b"home: no init in the vault: the usb process from the kernel's capsule\n");
                crate::checklist::dot(0);
                crate::board::probe(7);
                crate::board::probe_starve(9);
                crate::process::console_flashlog(&o.console);
            }
            Some(Err(e)) => {
                crate::process::push_console(b"home: no init in the vault, and usb refused: ");
                crate::board::probe(71);
                crate::process::push_console(e.as_bytes());
                crate::process::push_console(b"\n");
            }
            None => {
                crate::process::push_console(b"home: no init in the vault, and no usb grants\n");
                crate::board::probe(72);
            }
        }
        #[cfg(not(ferros_usb_capsule))]
        crate::process::push_console(b"home: no init in the vault, and this kernel carries no usb capsule (target/akita-usb.capsule.vsf)\n");
        // The vault process too when the kernel image carries one (FERROS_VAULT_CAPSULE), never one from this vault: with no init the vault may be one this kernel cannot read, and the carried capsule is checked by its signature alone (native, no seal needed).
        // So the bridge's `store get` reaches what the vault process keeps (the device's firmware, replays, camera inputs) before a fresh start, and `store put` after it lands there again (2026-11-18, the Pro: its kernel vault part-moved to VSF, the vault process's store untouched).
        if !VAULT_CARRIED.is_empty() {
            match crate::process::spawn_capsule(b"vault", VAULT_CARRIED, b"", &vault_grants(), &[]) {
                Ok(o) => {
                    mark_ran(&o);
                    crate::process::console_flashlog(&o.console);
                    crate::process::push_console(b"home: no init in the vault: the vault process from the kernel image\n");
                }
                Err(e) => crate::process::push_console(alloc::format!("home: no init in the vault, and the kernel image's vault refused: {e}\n").as_bytes()),
            }
        }
        return true;
    }
    // `home.off` in the vault (any value): no shell this boot, so the panel is the bridge's alone — a measurement session (tools/display-profile.sh) must not have the map redraw because an instrument rests on the glass. Removed with `store rm home.off`.
    // Under the vault's lock, as every vault read once it is shared (nothing else runs yet, but the rule has no exceptions).
    if vault(|| store.as_mut().and_then(|s| s.get_by_key("home.off").ok().flatten())).is_some() {
        crate::process::push_console(b"home: off (vault home.off): no shell this boot\n");
        // Without init nothing else starts usb (the kernel's own USB went 2026-11-01), and a measurement session needs the bridge. Not on the glass: an instrument may be reading it.
        usb_rescue("off (vault home.off), no init this boot", false);
        return false;
    }
    let Some(capsule) = vault(|| store.as_mut().and_then(|s| s.get_by_key("init").ok().flatten())) else {
        crate::process::push_console(b"home: no init capsule in the vault\n");
        usb_rescue("no init capsule in the vault", true);
        return true;
    };
    let sealed = seals_for_launch(store);
    // The vault process first (VAULT-OUT.md, decision 0), so it is serving when the processes init starts ask for what they keep; until it is, the kernel's vault answers them. From the capsule the kernel image carries (FERROS_VAULT_CAPSULE at build), else the `vault` entry of the kernel's own vault (kept by the kernel: never forwarded, never moved). Neither: every store call is the kernel's vault's, as before.
    let mut vault_up = false;
    if !VAULT_CARRIED.is_empty() {
        match crate::process::spawn_capsule(b"vault", VAULT_CARRIED, b"", &vault_grants(), &sealed) {
            Ok(o) => {
                mark_ran(&o);
                crate::process::console_flashlog(&o.console);
                crate::process::push_console(b"home: vault from the kernel image\n");
                vault_up = true;
            }
            Err(e) => crate::process::push_console(alloc::format!("home: the kernel image's vault refused: {e}; the stored one next\n").as_bytes()),
        }
    }
    if !vault_up {
        if let Some(vc) = vault(|| store.as_mut().and_then(|s| s.get_by_key("vault").ok().flatten())) {
            match crate::process::spawn_capsule(b"vault", &vc, b"", &vault_grants(), &sealed) {
                Ok(o) => {
                    mark_ran(&o);
                    crate::process::console_flashlog(&o.console);
                }
                Err(e) => crate::process::push_console(alloc::format!("home: vault refused: {e}\n").as_bytes()),
            }
        }
    }
    // The GPU service, from its capsule in the vault, with its own grants (`gpu_grants`): before init, so it serves before the attest screen asks it. Only once the GPU came to its reset and was fenced (`GPU_FRESH`, every boot, after a jump too); otherwise none, and the attest screen draws on the CPU without it.
    if GPU_FRESH.load(core::sync::atomic::Ordering::Relaxed) {
        match vault(|| store.as_mut().and_then(|s| s.get_by_key("gpu").ok().flatten())) {
            Some(gc) => match crate::process::spawn_capsule(b"gpu", &gc, b"", &gpu_grants(), &sealed) {
                Ok(o) => {
                    mark_ran(&o);
                    crate::process::console_flashlog(&o.console);
                }
                Err(e) => crate::process::push_console(alloc::format!("home: gpu refused: {e}\n").as_bytes()),
            },
            None => crate::process::push_console(b"home: no gpu capsule in the vault\n"),
        }
    }
    // The pipe stays at ABL's 8 bpc here: the display server takes it to the 10 bpc blend at its start (payloads/display modeset.rs; KERNEL-OUT.md decision 6, 2026-11-01).
    // The touch controller is the touch process's to bring up (its `touch` power switch, then the ready event and scan mode: payloads/touch; KERNEL-OUT.md move 1).
    #[cfg(not(ferros_init_refuse))]
    let grants = home_grants();
    // FERROS_INIT_REFUSE=1 (a development build, never flashed): init's spawn refused the way it was on 2026-11-21, by more grants than its page holds, to prove `usb_rescue` on the device (docs/KERNEL.md "When init does not start").
    // 2^7 more always overflow: the page holds 108 slots in all.
    #[cfg(ferros_init_refuse)]
    let grants = {
        let mut g = home_grants();
        g.extend((0..1 << 7).map(|_| crate::process::Grant::Entropy));
        g
    };
    match crate::process::spawn_capsule(b"init", &capsule, b"", &grants, &sealed) {
        Ok(o) => {
            mark_ran(&o);
            // From here the vault, usb and display have `glass::BOOT_DEADLINE_SHIFT` to come up (`glass::boot_watch`).
            crate::glass::INIT_AT.store(cntpct(), core::sync::atomic::Ordering::Relaxed);
            // The processes init started are its spawn syscalls, each handing out capabilities.
            if crate::process::alive() > 0 {
                crate::checklist::mark(crate::checklist::Item::Syscall);
                crate::checklist::mark(crate::checklist::Item::Caps);
            }
            crate::process::console_flashlog(&o.console)
        }
        Err(e) => {
            crate::process::push_console(b"home: init refused: ");
            crate::process::push_console(e.as_bytes());
            crate::process::push_console(b"\n");
            usb_rescue(&alloc::format!("init refused ({e})"), true);
        }
    }
    crate::checklist::at("home: init ran");
    true
}

/// The usb process's native capsule the kernel image carries (target/akita-usb.capsule.vsf beside the tree at build, `ferros_usb_capsule`), empty when the build carried none. One copy in the image for both its starts: a vault with no init (`start_home`) and init not running (`usb_rescue`).
#[cfg(ferros_usb_capsule)]
static USB_CARRIED: &[u8] = include_bytes!("../../target/akita-usb.capsule.vsf");
#[cfg(not(ferros_usb_capsule))]
static USB_CARRIED: &[u8] = &[];

/// init is not running, so nothing will start usb, and the kernel has had no USB of its own since 2026-11-01: the usb process from the capsule the kernel image carries, as a vault with no init gets it, so the bridge still reaches the device and `diag console` and the flash log say why.
/// Called when init's spawn is refused at home, when the vault has no init, under `home.off`, and at the boot's deadline when init is not running and no usb is up (it died, or crash-looped, before it started usb: `glass::boot_watch`).
/// Why (2026-11-21): a kernel change made init's spawn refused ("more grants than the grants page holds"), and the Pro came up with no init, no USB, no display and no shell; nothing could reach it from the bridge until a hand held its power button.
/// Native, checked by its signature, without an ihi (the usb process reads none, and a refused init may be a missing ira); nothing started when a usb process runs already.
/// `glass`: the line drawn on the panel by the kernel too (not under `home.off`, whose panel an instrument may be reading).
fn usb_rescue(why: &str, glass: bool) {
    let say = |s: &str| {
        crate::process::push_console(alloc::format!("{s}\n").as_bytes());
        crate::flashlog::line(s);
        if glass {
            crate::process::with_lock(|| crate::glass::notice(crate::glass::Note::words(s), true));
        }
    };
    if crate::process::running(b"usb") {
        say(&alloc::format!("usb rescue: {why}; a usb process runs already"));
        return;
    }
    if USB_CARRIED.is_empty() {
        say(&alloc::format!("usb rescue: {why}, and this kernel carries no usb capsule (target/akita-usb.capsule.vsf at build): no USB until a reset"));
        return;
    }
    match shell_grants(b"usb").map(|g| crate::process::spawn_capsule_no_ihi(b"usb", USB_CARRIED, b"", &g)) {
        Some(Ok(o)) => {
            mark_ran(&o);
            crate::process::console_flashlog(&o.console);
            say(&alloc::format!("usb rescue: {why}: the usb process started from the kernel's capsule, so the bridge can reach this device"));
        }
        Some(Err(e)) => say(&alloc::format!("usb rescue: {why}, and the kernel's usb capsule refused: {e}: no USB until a reset")),
        None => say(&alloc::format!("usb rescue: {why}, and no usb grants: no USB until a reset")),
    }
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
    // 10 and 86: the hottest thermal zone's temperature and which zone it is (thermal.rs: 0 BIG, 1 MID, 2 LITTLE, 3 G3D, 4 ISP, 5 TPU, 6 AUR); 11 whether ferros's thermal policy holds a domain under the clock policy's rate.
    if let Some((zone, c)) = crate::thermal::hottest() {
        v[10] = c as u64;
        v[86] = zone as u64;
    }
    v[11] = crate::thermal::throttled() as u64;
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
    // 87: the bootloader's lock state as ABL told this boot (bootlock.rs): bits 0-7 the device state (1 locked, 2 unlocked), bits 8-15 the verified boot state (1 green, 2 yellow, 3 orange, 4 red), unknown without a bootconfig (a reload). Not the person's: no capability.
    v[87] = crate::bootlock::STATE.load(Relaxed);
    // 33-80: those drivers' own measures (SYS_STATUS), eight tagged words each, in the same order, then the usb driver's.
    for i in 0..6 {
        for w in 0..8 {
            v[33 + 8 * i + w] = crate::process::STATUS[i][w].load(Relaxed);
        }
    }
    // The sensors process's block when it publishes one (sensorlink.rs, KERNEL-OUT.md move 4); with none, the sensor words stay unknown (the kernel has no USF of its own since 2026-11-01).
    if crate::sensorlink::live() {
        use crate::sensorlink::{word, GRAVITY_XY, GRAVITY_Z, MAG_XY, MAG_Z, PAIR, PROX, SPECTRAL};
        let w = |i: usize| word(i).unwrap_or(0);
        // 88-89 the magnetic field, once a sample has come (its count in z's high half; the compass puck, 2026-11-17).
        if w(MAG_Z) >> 32 != 0 {
            v[88] = w(MAG_XY);
            v[89] = w(MAG_Z);
        }
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

/// The AoC starts no sooner than this long after the service loop does: the deck first.
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
    /// At the settle (the window's true start): the counter, the gauge's integrals (hsi2c gauge `charge`: RepSOC, RepCap, QH), and the wake counts (`wakes_now`), for the drain and the wakes over the whole window.
    start: Option<(u64, Option<(u16, u16, u16)>, [u64; 4])>,
}

/// The kernel core's idle sleeps and the ticks spent in them (the service loop's idle, plain or deep), for DIAG X3_SLEEPS and the power window's wake count.
pub static X3_SLEEPS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// Wakes of the kernel core's idle `wfe` that went back to sleep with no pass (the doze, `DOZE_LOG2`), for DIAG X3_DOZES.
pub static X3_DOZES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The doze's bound, as a right shift of the counter's frequency (`diag clock doze <n>`; 9: 2^-9 s, under the event stream's 2.7 ms; 0: no doze, a pass at every wake as before 2026-11-19).
pub static DOZE_LOG2: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(9);

/// Whether the kernel's core has work of its own now, read without the kernel lock: a queued thread it may run (process.rs `RUNNABLE_KERNEL`), a sleeper's deadline due or a device line pending (`run_slice` wakes their waiters), or no worker up (every thread is this core's).
fn for_this_core() -> bool {
    use core::sync::atomic::Ordering::Acquire;
    crate::process::RUNNABLE_KERNEL.load(Acquire) != 0 || crate::process::NEXT_WAKE.load(Acquire) <= cntpct() || unsafe { core::ptr::read_volatile(&raw const crate::process::__irq_any) } != 0 || crate::smp::UP.load(Acquire) == 0
}
pub static X3_ASLEEP: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// What a power window counts across itself: the kernel core's idle sleeps, the interrupts taken, the ticks asleep, the watchdog pets (a pass of the service loop each, more or less).
fn wakes_now() -> [u64; 4] {
    use core::sync::atomic::Ordering::Relaxed;
    [X3_SLEEPS.load(Relaxed), unsafe { core::ptr::read_volatile(&raw const crate::__irq_count) }, X3_ASLEEP.load(Relaxed), crate::platform::PETS.load(Relaxed)]
}
static mut POWER_WIN: Option<PowerWin> = None;
static mut POWER_RESULT: Option<alloc::string::String> = None;

fn power_start(secs: u64, gap16: u64, input: bool, delay: u64) -> alloc::string::String {
    let (f, now) = (cntfrq(), cntpct());
    if unsafe { (*(&raw const POWER_WIN)).is_some() } {
        return "a window is already running\n".into();
    }
    // Up to 4096 s: an hour on battery is what a drain per hour wants (the gauge's own integrals, read at the two ends, carry it; `gap` sets only how often the current is sampled beside them).
    let secs = secs.clamp(1, 4096);
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
        POWER_WIN = Some(PowerWin { settle, end: settle + secs * f, next: settle, gap: (gap16.clamp(1, 64) * f / 16).max(1), charger_prev, input_prev, sum_ua: 0, sum_uv: 0, n: 0, min_ua: i32::MAX, max_ua: i32::MIN, secs, states: [0; 4], start: None });
    }
    msg += &alloc::format!("power window: {secs} s from half a second after the charger cut{}; read it with `diag power`\n", if input_prev.is_some() { ", CHGIN deselected" } else { "" });
    msg
}

/// When the running power window next wants the loop (its start, its next sample, its end), for the dark idle's deadline. None without a window.
fn power_due() -> Option<u64> {
    let w = unsafe { (*(&raw const POWER_WIN)).as_ref() }?;
    let next = if w.start.is_none() { w.settle } else { w.next };
    Some(next.min(w.end))
}

/// From the service loop: the running power window's sample when one is due, and its end.
fn power_poll() {
    let Some(w) = (unsafe { (*(&raw mut POWER_WIN)).as_mut() }) else { return };
    let now = cntpct();
    if w.start.is_none() && now >= w.settle {
        w.start = Some((now, crate::hsi2c::gauge::charge().ok(), wakes_now()));
    }
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
        // Over the whole window, from the settle: the gauge's integrals (RepSOC in 1/256 %, RepCap and QH in mAh at the 5 mΩ sense) and the wakes, each per hour or per second.
        if let Some((t0, c0, k0)) = w.start {
            let secs_x16 = (now.wrapping_sub(t0) * 16 / cntfrq()).max(1);
            if let (Some((soc0, cap0, qh0)), Ok((soc1, cap1, qh1))) = (c0, crate::hsi2c::gauge::charge()) {
                let dsoc = soc0 as i64 - soc1 as i64;
                let dqh = qh0.wrapping_sub(qh1) as i16 as i64;
                r += &alloc::format!("gauge over A#{} s: RepSOC G#{soc0:04X} -> G#{soc1:04X} ({dsoc} of 1/256 %, A#{} thousandths of a percent an hour), RepCap A#{cap0} -> A#{cap1} mAh, QH G#{qh0:04X} -> G#{qh1:04X} (A#{dqh} mAh, A#{} mA average)\n", secs_x16 / 16, dsoc * 1000 * 16 * 3600 / 256 / secs_x16 as i64, dqh * 16 * 3600 / secs_x16 as i64);
            }
            let k1 = wakes_now();
            let per_s = |k: usize| (k1[k].wrapping_sub(k0[k])) * 16 / secs_x16;
            r += &alloc::format!("wakes: the kernel core's idle sleeps A#{} a second, interrupts A#{} a second, watchdog pets A#{} a second, asleep A#{} % of the time\n", per_s(0), per_s(1), per_s(3), k1[2].wrapping_sub(k0[2]) * 100 / now.wrapping_sub(t0).max(1));
        }
        // Into the flash log too: a window run with the cable out ends before the host can ask, and if the USB link does not come back the reset that recovers it would take the result with it (2026-10-25).
        crate::flashlog::line(&alloc::format!("power: {}", r.trim_end()));
        unsafe {
            POWER_RESULT = Some(r);
            POWER_WIN = None;
        }
    }
}

/// The clock policy (POWER.md, 2026-10-25; the adaptive clock since 2026-11-13). Two parts.
/// The X3 and the A715s, the two clusters that run work, each follow their own load (`governor`, owner 2026-11-13: "Adaptive clock").
/// The shared domains follow one of three tiers chosen from what the kernel knows: dark (the screen off, no transfer), active (the governor asking the X3 over `X3_REST_KHZ` or the A715s over `A715_REST_KHZ`, sustained GPU work, frames shown at 24 a second or more, or a PT transfer), rest otherwise.
/// Rest puts every shared domain at the lowest point that still carries the rest-state work: MIF at 1014 MHz (2028 → 1014 is 0.36 W; lower bought nothing), INT at its 178 MHz floor, CPU0's cluster (parked) at its 324 MHz floor and the DSU (the clusters' shared cache and snoop) at its 324 MHz floor (90 mW; BCI at its floor bought nothing measurable and stays) (zuma-cpu.dtsi: ACPM answers Ok below a floor and the device hard-resets, so floors, never under).
/// Active is the boot's own clocks.
/// Each entry: (ACPM DVFS domain, kHz), applied in order (the X3 first going down, MIF first going up); the X3's and the A715s' entries are the governor's starting points (the boot's top, the floors in the dark) and otherwise its own.
const CLOCKS_REST: [(u32, u32); 7] = [(4, 2_000_000), (0, 1_014_000), (1, 178_000), (3, 402_000), (2, 324_000), (5, 324_000), (14, 400_000)];
/// Dark: the screen off, no transfer. Nothing is drawn, so the X3 and MIF go to their floors and the display's clock to its 143 MHz floor (measured 70 mW under rest with the screen off).
const CLOCKS_DARK: [(u32, u32); 7] = [(4, 500_000), (0, 421_000), (1, 178_000), (3, 402_000), (2, 324_000), (5, 324_000), (14, 143_000)];
const CLOCKS_ACTIVE: [(u32, u32); 7] = [(0, 2_028_000), (5, 1_197_000), (1, 356_000), (3, 1_065_000), (2, 1_425_000), (14, 400_000), (4, 2_914_000)];
/// The policy's state (the shared domains' tier): 0 not yet applied, 1 rest, 2 active, 3 dark; and whether it runs (`diag dvfs policy off` holds the clocks wherever `dvfs set` puts them, for measuring). Switches counted for DIAG.
static mut CLOCK_STATE: u8 = 0;
/// The clock policy's state now (`CLOCK_STATE`), for the store calls' measure (process.rs `STORE_BY_CLOCK`): a byte written by the service loop alone, read as it stands.
pub fn clock_state() -> u8 {
    unsafe { core::ptr::read_volatile(&raw const CLOCK_STATE) }
}
static mut CLOCK_POLICY_ON: bool = true;
static mut CLOCK_SWITCHES: u32 = 0;
static mut CLOCK_WAIT_MAX_US: u64 = 0;

/// The adaptive clock: the X3 and the A715s each at the rate their own load needs, measured from the time their cores spend asleep (smp.rs `IDLE_TICKS`, the service loop's `X3_IDLE_TICKS`), on the service loop's own passes (no tick of its own).
/// Every window (`GOV_WIN`, a 64th of a second) each cluster's busy share is its busiest core's: time awake over the window, in 256ths.
/// Up at once: a touch or an injected batch (process.rs `INPUT_AT`) or a PT transfer puts both clusters at their top; a saturated window (busy 7/8 or more: the load wants more than the clock gives, and how much more is unknown) moves that cluster at least a step of the ramp up (`GOV_RAMP`).
/// Down smoothly: the load's level moves an eighth of the way a window (`GOV_DECAY`) toward the rate that would leave the averaged busy share at 4/5 (the running rate × the average × 5/4), and up to it at once when that is higher.
/// The average takes a quarter of each window's share (`GOV_AVG`), so a 10 Hz redraw is seen as its share of the time and not as a string of busy and idle windows.
/// The rate asked is the lowest operating point at or above the level, under the thermal cap (thermal.rs `capped`), sent only when it changes.
struct Gov {
    domain: u32,
    /// The operating points, kHz, lowest first (zuma-b0-ipop.dts `pixel_em`, cpu8 and cpu4): the X3's whole table; the A715s' up to 1065 MHz, their active clock until the owner picks (THERMAL.md decision 3; 2367 trips the clock divider).
    opps: &'static [u32],
    /// The cores: bit c for worker core c (MPIDR Aff1, smp.rs `UP`), or 0 for the kernel's own core.
    cores: u32,
    /// What the load asks, kHz: up at once, down an eighth of the way a window.
    level: u32,
    /// The operating point asked (before the thermal cap): its index in `opps`.
    at: usize,
    /// The busy share averaged over windows, and the last window's, in 256ths.
    avg: u32,
    last: u32,
    /// Windows in a row with the busy share at 7/8 or more.
    sat: u32,
    /// Each core's idle ticks (the A510s': busy ticks) at the window's start, by Aff1 (the kernel's core in slot 0 for the X3's).
    idle: [u64; 8],
    /// Counter ticks spent at each operating point (since boot or `diag clock reset`).
    res: [u64; 16],
    /// Rate requests sent.
    asks: u64,
    /// The busy lift (`GOV_BUSY_KHZ`): windows in a row the load asked over the calm ceiling, and in a row it asked at or under it; whether the busy lift holds this cluster now.
    over: u64,
    under: u64,
    busy: bool,
    /// The least busy core's share over the last window, and its average as `avg` is kept, in 256ths: the whole cluster busy, which the busy lift begins on.
    least: u32,
    whole: u32,
}
const X3_OPPS: [u32; 15] = [500_000, 880_000, 1_164_000, 1_298_000, 1_557_000, 1_745_000, 1_885_000, 2_049_000, 2_147_000, 2_294_000, 2_363_000, 2_556_000, 2_687_000, 2_850_000, 2_914_000];
const A715_OPPS: [u32; 15] = [402_000, 578_000, 697_000, 712_000, 910_000, 1_065_000, 1_221_000, 1_328_000, 1_418_000, 1_572_000, 1_836_000, 1_945_000, 2_130_000, 2_245_000, 2_367_000];
/// The A715s' ceiling while nobody interacts (`interactive`): 1065 MHz, their active clock before the owner's lift (THERMAL.md decision 3, 2026-11-17: "lift the core cap, yes" during interaction).
const A715_CALM_KHZ: u32 = 1_065_000;
/// The A510s' operating points, kHz (zuma-b0-ipop.dts `pixel_em` cpu0; zuma-cpu.dtsi cpufreq_domain0 324 to 1704 MHz, and under 324 the device resets: POWER.md "Floors are real").
const A510_OPPS: [u32; 10] = [324_000, 610_000, 820_000, 955_000, 1_098_000, 1_197_000, 1_328_000, 1_425_000, 1_548_000, 1_704_000];
/// The A510s' ceiling while nobody interacts: 1425 MHz, the boot's and the active tier's rate for them, the last point before the energy model's cliff (57 to 155 at 1548 MHz in `pixel_em`, THERMAL.md). While lifted their whole table, as the A715s' (`GOV_LIFT_KHZ` above the A715s' calm, or no lift at all).
const A510_CALM_KHZ: u32 = 1_425_000;
/// The clusters' ceiling while someone interacts (`diag clock lift <kHz>`; the A715s' whole table by default, 0 for no lift: the calm ceiling always). The X3 has no calm ceiling: its whole table either way.
static GOV_LIFT_KHZ: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(2_367_000);
/// How long the lift holds after the last input (a touch interrupt or an injected batch), as a right shift of the counter's frequency (`diag clock hold <n>`, 1: half a second). A real deadline: a finger lifted ends the input but not the frames it set going (a fling, a settle), whose first frames must not be drawn at the calm clock, and Photon's frames during a drag came 50 to 70 ms apart (2026-11-17), so the animation's own sign (`process::ANIM_AT`, frames 2^-4 s apart or closer) can take over only once the lifted clock has sped them up.
static GOV_HOLD: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1);
/// Windows the governor ran lifted, lifts begun, and windows lifted by a camera stream alone (`diag clock`).
static mut GOV_LIFTED: [u64; 3] = [0; 3];
static mut GOV_WAS_LIFTED: bool = false;

/// How long a running camera stream holds the lift after the front end's last answered interrupt, as a right shift of the counter's frequency (`diag clock cam <n>`; 2: a quarter second; 0: the camera holds nothing).
/// 2026-11-17, the Pro: Lumis draws each new camera image in 82 to 85 ms at the calm clock and 37 to 39 ms lifted, so its shown frames never came within the animation's gap and the preview stayed at the calm clock.
/// A quarter second is about four frames' gap at the init's 15 a second, so the lift ends within about four frames of the stream stopping.
static CAM_HOLD: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(2);
/// The camera front end's line (`ispfe0`) as an `IRQS` index plus one, found at the first look (0 not yet); its acks counted at the last look; and when they last moved (counter ticks, 0 never).
static mut CAM_LINE: usize = 0;
static mut CAM_ACKS_SEEN: u32 = 0;
static mut CAM_AT: u64 = 0;

/// Whether a camera stream runs: the camera driver answered its front end's interrupt (process.rs `IRQ_ACKS`, the line `ispfe0`) within the hold (`CAM_HOLD`).
/// The driver acks the line only after an interrupt it answered, never after a wait that timed out, so a stalled or stopped stream stops the count and the hold runs out.
/// Read on the service loop's passes like the rest of `interactive`: no timer of its own.
fn camera_streaming(now: u64, f: u64) -> bool {
    use core::sync::atomic::Ordering::Relaxed;
    let hold = CAM_HOLD.load(Relaxed);
    unsafe {
        if CAM_LINE == 0 {
            CAM_LINE = crate::process::IRQS.iter().position(|l| l.name == "ispfe0").unwrap() + 1;
        }
        let acks = crate::process::IRQ_ACKS[CAM_LINE - 1].load(Relaxed);
        if acks != CAM_ACKS_SEEN {
            CAM_ACKS_SEEN = acks;
            CAM_AT = now;
        }
        hold != 0 && CAM_AT != 0 && now.wrapping_sub(CAM_AT) < f >> hold
    }
}

/// What holds the clock policy's lift now, one bit each: input within the hold (bit 0, `GOV_HOLD`), an animation's frames within the display's gap of one another the last 2^-`ANIM_GAP_LOG2` s (bit 1, `process::ANIM_AT`, its own hold: the gap it was measured by, so the lift ends one frame interval after the frames stop), a running camera stream (bit 2, `camera_streaming`).
/// Read on the service loop's passes: the touch interrupt and every syscall's kick wake the kernel's core, so a lift begins at the pass after the input; the frames and the camera's answers are seen at the pass after them (the governor's window at the latest, 2^-6 s).
fn lift_causes(now: u64, f: u64) -> u8 {
    use core::sync::atomic::Ordering::Relaxed;
    let input = crate::process::INPUT_AT.load(Relaxed);
    let anim = crate::process::ANIM_AT.load(Relaxed);
    let gap = f >> crate::process::ANIM_GAP_LOG2.load(Relaxed);
    (input != 0 && now.wrapping_sub(input) < f >> GOV_HOLD.load(Relaxed)) as u8 | ((anim != 0 && now.wrapping_sub(anim) < gap) as u8) << 1 | (camera_streaming(now, f) as u8) << 2
}

/// Whether someone is using the device now (the clock policy's lift): anything in `lift_causes`.
fn interactive(now: u64, f: u64) -> bool {
    lift_causes(now, f) != 0
}

/// The A715s' ceiling while their own load holds the busy lift and nobody interacts (`diag clock busy <kHz>`; 0: no busy lift, the calm ceiling as before 2026-11-19).
/// The busy lift: a load that keeps every core of the cluster at the governor's saturated mark (7/8 averaged) at the calm ceiling for 2^`GOV_BUSY_HOLD` windows asks more than the calm ceiling gives, so the ceiling rises to this; it falls back once the load has asked at or under the calm ceiling for as long.
/// The test is on what the load asks (the running rate × the averaged share × 5/4, the governor's own `want`), which stays the same as the clock moves for a paced load and grows with it for one bound by the cores, so a lift begun is not ended by the clock it bought.
/// Why 1572 MHz (THERMAL.md "The busy lift", the Pro, 2026-11-19): a 12.5 MP Lumis save encoded in 8.58 s at 1065, 5.76 s at 1572 and 5.63 s at 2367, the zones ending at 66, 75 and 90 C, every cluster's divider stepping only at 2367.
/// A 50 MP save took 31.3 s at 1572 and 48.0 s at 2367 (the dividers stepping), and `pixel_em` gives an A715 449 mW at 1572 against 208 at 1065 and 1514 at 2367: with the device's other 2 W or so the save's energy is least at 1572.
static GOV_BUSY_KHZ: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1_572_000);
/// The busy lift's hold, as windows in a row (a power of two, `diag clock busyhold <n>`; 4: 16 windows, a quarter second at the 2^-6 s window), both to begin and to end.
static GOV_BUSY_HOLD: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(4);
/// Windows the A715s ran busy-lifted with no other lift, and busy lifts begun (`diag clock`).
static mut GOV_BUSY_COUNTS: [u64; 2] = [0; 2];

/// The A715s' ceiling while a RAW clip is recorded (`diag clock clips <kHz>`; 0: none): THERMAL.md "The busy lift"'s 1572 MHz, the least energy for a sustained encode.
static CLIPS_KHZ: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1_572_000);
/// Whether a recording holds the A715s at `CLIPS_KHZ` rather than under it (`diag clock clipshold <0|1>`): for CPU coders; off, the GPU path's default.
static CLIPS_HOLD: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Whether a RAW clip is being recorded: a clip's extent is given to its writer (from its open to its seal or its writer's end, process.rs `EXTENTS_OPEN`), or, for a writer that writes thru the vault process, the `clips` window was written within the last second.
fn recording() -> bool {
    use core::sync::atomic::Ordering::Relaxed;
    let at = crate::process::CLIPS_AT.load(Relaxed);
    let open = crate::process::EXTENTS_OPEN.load(Relaxed) || (at != 0 && cntpct().wrapping_sub(at) < cntfrq());
    open && CLIPS_KHZ.load(Relaxed) != 0
}

/// A cluster's highest operating point allowed now: the X3's whole table; the A715s' up to `GOV_LIFT_KHZ` while lifted, else up to `GOV_BUSY_KHZ` while their own load holds the busy lift, else up to `A715_CALM_KHZ`; the A510s' whole table while lifted (and the lift on), else up to `A510_CALM_KHZ`.
fn gov_top(g: &Gov, lifted: bool) -> usize {
    use core::sync::atomic::Ordering::Relaxed;
    let busy = GOV_BUSY_KHZ.load(Relaxed);
    let cap = match (g.domain, lifted) {
        (4, _) => u32::MAX,
        (2, true) if GOV_LIFT_KHZ.load(Relaxed) > A715_CALM_KHZ => u32::MAX,
        (2, _) => A510_CALM_KHZ,
        (_, true) => GOV_LIFT_KHZ.load(Relaxed).max(A715_CALM_KHZ),
        (_, false) if g.busy && busy > A715_CALM_KHZ => busy,
        (_, false) => A715_CALM_KHZ,
    };
    // A RAW clip being recorded (the `clips` window written within the last second, process.rs `CLIPS_AT`) holds the A715s at `CLIPS_KHZ` at most, whatever lifts them (docs/RAW-VIDEO.md: at the camera's 2367 MHz lift a minute at 30/s reached 85 C in 37 s; at 1572 it ended at 75 C).
    // It is also the recording's own lift, so every clip codes at the clocks its minutes were measured at whatever else lifts or not: the A715s up to `CLIPS_KHZ` and the A510s their whole table, from the clip's open to its seal only (2026-11-19: the first clip after a RAM reload, with no camera lift yet, coded its A510 bands at their calm 324 MHz, 54 ms a frame, and dropped 47 of 64).
    let cap = match (g.domain, recording()) {
        (3, true) => CLIPS_KHZ.load(Relaxed),
        (2, true) => u32::MAX,
        _ => cap,
    };
    // The highest point at or under the ceiling (the table starts at the floor, which every ceiling is above).
    g.opps.iter().rposition(|&o| o <= cap).unwrap_or(0)
}
/// The shared tier is active while the governor asks more than these (the rest profile's X3 before the adaptive clock, and the A715s' middle).
const X3_REST_KHZ: u32 = 2_049_000;
const A715_REST_KHZ: u32 = 712_000;
/// The A510s' middle, likewise: over it they ask the active tier too (their cluster shares the DSU and MIF with the others).
const A510_REST_KHZ: u32 = 955_000;
static mut GOV: [Gov; 3] = [
    Gov { domain: 4, opps: &X3_OPPS, cores: 0, level: 2_914_000, at: 14, avg: 0, last: 0, sat: 0, idle: [0; 8], res: [0; 16], asks: 0, over: 0, under: 0, busy: false, least: 0, whole: 0 },
    Gov { domain: 3, opps: &A715_OPPS, cores: 0xF0, level: 1_065_000, at: 5, avg: 0, last: 0, sat: 0, idle: [0; 8], res: [0; 16], asks: 0, over: 0, under: 0, busy: false, least: 0, whole: 0 },
    Gov { domain: 2, opps: &A510_OPPS, cores: 0xF, level: 324_000, at: 0, avg: 0, last: 0, sat: 0, idle: [0; 8], res: [0; 16], asks: 0, over: 0, under: 0, busy: false, least: 0, whole: 0 },
];
/// Whether the governor runs (`diag clock gov on|off`; off, the X3 and the A715s follow the tier tables as before 2026-11-13).
static GOV_ON: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);
/// The window, as a right shift of the counter's frequency (6: a 64th of a second, 15.6 ms, two frames at 120 Hz); the decay and the average, as shifts of the distance moved a window (3: an eighth; 2: a quarter); the GPU's sustained work, the averaged GPU calls a window in 256ths at which the shared tier goes active (128: a call every other window); the ramp past a saturated window's rate, as a right shift (2: a quarter). Each `diag clock <name> <value>`.
static GOV_WIN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(6);
static GOV_DECAY: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(3);
static GOV_AVG: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(2);
static GOV_GPU: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(128);
static GOV_RAMP: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(2);
/// The operating points under the top that input boosts a cluster to (`diag clock boost <n>`; 0, the top: a finger gets the whole core at once).
static GOV_BOOST: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// The window's start, the input last seen (`INPUT_AT`), the GPU calls at the window's start and their average a window (256ths), the windows looked at, and the shared tier's ticks in each state (`res` for the tiers).
static mut GOV_AT: u64 = 0;
static mut GOV_INPUT: u64 = 0;
static mut GOV_GPU_SEEN: u64 = 0;
static mut GOV_GPU_AVG: u64 = 0;
/// Frames shown: the display's new pictures (process.rs `PRESENTS`, never DECON's frame-start interrupt, which the 8a's panel takes on every TE), averaged a window as the GPU's calls are, in 256ths; at `GOV_SHOWN` (96: 24 frames a second, the old policy's "animating") the shared tier goes active, for a focused widget or an app drawing with little CPU work (2026-10-25: the focused scope ran 61 frames a second at the rest clocks, 93 at the active ones). Since 2026-11-17 the threshold is 192: the average takes a quarter of each 2^-6 s window, so at 96 a single window with two frames in it (128) crossed it, and on b923 at rest that alone made half the tier's switches (64 of 128 in 64 s); at 192 none of 32 were the frames'.
static mut GOV_SHOWN_SEEN: u64 = 0;
static mut GOV_SHOWN_AVG: u64 = 0;
static GOV_SHOWN: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(192);

/// What asks the shared domains for the active tier now, one bit each (`TIER_CAUSES`): the governor asking the X3 over `X3_REST_KHZ` (bit 0) or the A715s over `A715_REST_KHZ` (bit 1), the GPU's sustained work (bit 2), the frames shown (bit 3), the A510s over `A510_REST_KHZ` (bit 6). `clock_policy` adds input with the governor off (bit 4) and a PT transfer's hold (bit 5). None: rest, once the dwell has passed (`TIER_DWELL`).
fn tier_active(govs: &[Gov; 3]) -> u8 {
    use core::sync::atomic::Ordering::Relaxed;
    (govs[0].opps[govs[0].at] > X3_REST_KHZ) as u8 | ((govs[1].opps[govs[1].at] > A715_REST_KHZ) as u8) << 1 | ((unsafe { GOV_GPU_AVG } >= GOV_GPU.load(Relaxed)) as u8) << 2 | ((unsafe { GOV_SHOWN_AVG } >= GOV_SHOWN.load(Relaxed)) as u8) << 3 | ((govs[2].opps[govs[2].at] > A510_REST_KHZ) as u8) << 6
}
pub const TIER_CAUSES: usize = 7;
const TIER_CAUSE_NAMES: [&str; TIER_CAUSES] = ["X3", "A715", "GPU", "frames", "input", "transfer", "A510"];
/// The rest tier is taken only once nothing has asked for the active one for 2^-`TIER_DWELL` s (`diag clock dwell <n>`; 0, the default, no dwell: the tier follows its causes window by window).
/// A real deadline, checked on the service loop's passes: a switch is seven ACPM requests, each waited for by a rate read (`clock_profile`), and on b922 (2026-11-17) the tier switched 2973 times in 16 minutes, about three a second, its causes crossing their thresholds from one 2^-6 s window to the next. Measured on b923 at rest (Photon in front, 64 s each, the frames threshold at 96): no dwell 128 switches with the active tier 14 % of the time, 2^-2 s 66 switches at 51 %, 2^-1 s 45 at 71 %: the causes come in bursts about a second apart, so a dwell halves the switches only by holding MIF and the DSU up most of the time (about 0.36 W). Kept as a knob.
static TIER_DWELL: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// When something last asked for the active tier.
static mut TIER_BUSY_AT: u64 = 0;
/// The measure of the tiers' switching (`diag clock`): each switch to active by the causes that asked (a switch counts once under each cause it had), each tier's runs by length (bucket b: under 2^b ms, the last everything longer), and the ticks spent switching.
static mut TIER_WHY: [u64; TIER_CAUSES] = [0; TIER_CAUSES];
static mut TIER_RUNS: [[u64; 12]; 4] = [[0; 12]; 4];
static mut TIER_SINCE: u64 = 0;
static mut TIER_SWITCH_TICKS: u64 = 0;
/// A tier switch's requests sent one after another with one rate read after the last (`clock_profile`; `diag clock pipeline 0|1`, on by default since 2026-11-17), or each waited for before the next as before. On b923 a switch took about 4.5 ms of the service loop, its five to seven requests each waited for in turn.
static TIER_PIPELINE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);
/// A tier's requests sent, and those skipped for a domain already at the rate (`acpm::ASKED`).
static mut TIER_ASKS: u64 = 0;
static mut TIER_SKIPPED: u64 = 0;
static mut GOV_WINDOWS: u64 = 0;
static mut TIER_RES: [u64; 4] = [0; 4];
/// Counter ticks the kernel's core has spent asleep in the service loop's `wfe`, added as each sleep ends.
static X3_IDLE_TICKS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The governor's operating point for a cluster set from a profile's entry: the lowest at or above it, the level at it.
fn gov_sync(domain: u32, khz: u32) {
    let g = unsafe { &mut *(&raw mut GOV) };
    for g in g.iter_mut().filter(|g| g.domain == domain) {
        g.at = g.opps.iter().position(|&o| o >= khz).unwrap_or(g.opps.len() - 1);
        g.level = khz;
        g.sat = 0;
    }
}

/// One cluster's busy share over the window just ended, in 256ths: its busiest core's time awake. A core that is not up (the workers stopped in the dark) counts for nothing, its idle mark kept current.
fn gov_share(g: &mut Gov, el: u64) -> Option<u32> {
    use core::sync::atomic::Ordering::Relaxed;
    let up = crate::smp::UP.load(Relaxed);
    let mut share = None;
    let mut least = 256u32;
    // The kernel's core alone (its idle ticks in slot 0), or each of the cluster's cores by Aff1.
    for c in 0..8 {
        if g.cores == 0 && c != 0 {
            break;
        }
        if g.cores != 0 && g.cores >> c & 1 == 0 {
            continue;
        }
        // The A510s by the time they ran threads (smp.rs `busy`, a run still going counted as it goes: they sleep until an event, and a long sleep not yet ended would read as busy); the others by the time they slept.
        let little = g.domain == 2;
        let mark = if g.cores == 0 { X3_IDLE_TICKS.load(Relaxed) } else if little { crate::smp::busy(c, cntpct()) } else { crate::smp::IDLE_TICKS[c].load(Relaxed) };
        // WHY saturating, and the max, for the A510s: `smp::busy` can read a pass that ended between its two loads as not yet counted, under the mark the last window took with that pass still going; the count must not go back (it would wrap to a whole window busy), so the mark holds and the time lands in the next window.
        let d = if little { mark.saturating_sub(g.idle[c]) } else { mark.wrapping_sub(g.idle[c]) };
        g.idle[c] = if little { g.idle[c].max(mark) } else { mark };
        if g.cores != 0 && up >> c & 1 == 0 {
            continue;
        }
        // WHY the mins: a sleep (or a run) is added when it ends, so one begun in the window before lands whole in this one and can be longer than the window; the busy time would then wrap to a huge number, or the share pass 256.
        let busy = if little { d.min(el) } else { el - d.min(el) };
        let s = (busy * 256 / el) as u32;
        share = Some(share.map_or(s, |m: u32| m.max(s)));
        least = least.min(s);
    }
    g.least = if share.is_some() { least } else { 0 };
    share
}

/// The governor's window: each cluster's share measured and its rate moved (`Gov`). `boost` puts the X3 and the A715s at their top (never the A510s, which follow their pinned threads' load alone). Returns whether the shared tier should be active for what the clusters ask or the GPU does.
fn governor(now: u64, f: u64, boost: bool) -> u8 {
    use core::sync::atomic::Ordering::Relaxed;
    let el = now.wrapping_sub(unsafe { GOV_AT });
    let govs = unsafe { &mut *(&raw mut GOV) };
    // The first look only takes the marks: the time before it is the boot's, at the boot's clocks.
    if unsafe { GOV_AT } == 0 {
        unsafe {
            GOV_AT = now;
            GOV_GPU_SEEN = crate::process::GPU_CALLS.load(Relaxed);
            GOV_SHOWN_SEEN = crate::process::PRESENTS.load(Relaxed);
        }
        for g in govs.iter_mut() {
            let _ = gov_share(g, 1);
        }
        return 0;
    }
    // The owner's lift (THERMAL.md decision 3): while someone interacts, the A715s' whole table; otherwise their calm ceiling, and a lift that ends brings them under it at once, inside a window too.
    let causes = lift_causes(now, f);
    let lifted = causes != 0;
    unsafe {
        if lifted && !GOV_WAS_LIFTED {
            GOV_LIFTED[1] += 1;
        }
        GOV_WAS_LIFTED = lifted;
    }
    if el < f >> GOV_WIN.load(Relaxed) {
        // Inside a window a boost moves the clusters at once and leaves the window's measure to its end.
        let up = crate::smp::UP.load(Relaxed);
        for g in govs.iter_mut().filter(|g| g.cores == 0 || g.cores & up != 0) {
            let top = gov_top(g, lifted);
            // WHY the min: a ceiling of the policy, not a guard on the table. Input jumps a cluster no higher than its calm ceiling (the X3: its top; the A715s: 1065 MHz, where input took them before the lift); above it the A715s climb by the load (a saturated window ramps a quarter, 1065 to 2367 MHz in three saturated windows, under 2^-4 s), not in one step: THERMAL.md measured one jump of the A715s from 1065 to 2367 MHz trip the battery-current divider and drop the X3 to 364 MHz.
            let b = boost_at(g).min(gov_top(g, false));
            // The A510s are never boosted: they run only pinned threads, and their clock follows those threads' load alone.
            if boost && g.domain != 2 && g.at < b {
                g.level = g.opps[b];
                gov_ask(g, b, f);
            } else if g.at > top {
                g.level = g.opps[top];
                gov_ask(g, top, f);
            }
        }
        return tier_active(govs);
    }
    if lifted {
        unsafe { GOV_LIFTED[0] += 1 };
        unsafe { GOV_LIFTED[2] += (causes == 4) as u64 };
    }
    unsafe {
        GOV_AT = now;
        GOV_WINDOWS += 1;
        let calls = crate::process::GPU_CALLS.load(Relaxed);
        let n = calls.wrapping_sub(GOV_GPU_SEEN);
        GOV_GPU_SEEN = calls;
        let s = GOV_AVG.load(Relaxed);
        GOV_GPU_AVG = GOV_GPU_AVG - (GOV_GPU_AVG >> s) + ((n << 8) >> s);
        let shown = crate::process::PRESENTS.load(Relaxed);
        let n = shown.wrapping_sub(GOV_SHOWN_SEEN);
        GOV_SHOWN_SEEN = shown;
        GOV_SHOWN_AVG = GOV_SHOWN_AVG - (GOV_SHOWN_AVG >> s) + ((n << 8) >> s);
    }
    let (avg_s, decay_s, ramp_s) = (GOV_AVG.load(Relaxed), GOV_DECAY.load(Relaxed), GOV_RAMP.load(Relaxed));
    for g in govs.iter_mut() {
        let Some(share) = gov_share(g, el) else { continue };
        g.res[g.at] += el;
        g.last = share;
        g.avg = g.avg - (g.avg >> avg_s) + (share >> avg_s);
        g.whole = g.whole - (g.whole >> avg_s) + (g.least >> avg_s);
        g.sat = if share >= 224 { g.sat + 1 } else { 0 };
        let rate = g.opps[g.at];
        // The rate that would leave the averaged share at 4/5: the running rate × the average × 5/4, in 256ths of the share.
        let want = (rate as u64 * g.avg as u64 * 5 >> 10) as u32;
        // The busy lift (`GOV_BUSY_KHZ`, the A715s only), window by window: 2^`GOV_BUSY_HOLD` windows in a row with every core of the cluster at the governor's saturated mark (7/8, the least busy core's averaged share `whole`) begin it; as many in a row with the load asking no more than the calm ceiling end it.
        // The whole cluster saturated to begin, not its busiest core over the 4/5 target (2026-11-19, the Pro at rest): a driver polling on one A715 (gps, after a reload) held the busiest core at 190 to 205 of 256, and a thread left running (kists.1) one core at 256, and either lifted the cluster with nobody using the device; a save's four encoding threads hold all four at 256.
        if g.domain == 3 {
            let hold = 1u64 << GOV_BUSY_HOLD.load(Relaxed);
            let allowed = GOV_BUSY_KHZ.load(Relaxed) > A715_CALM_KHZ && !crate::thermal::throttled();
            let begin = allowed && g.whole >= 224;
            let end = !allowed || want <= A715_CALM_KHZ;
            g.over = if begin { g.over + 1 } else { 0 };
            g.under = if end { g.under + 1 } else { 0 };
            let was = g.busy;
            g.busy = if g.busy { g.under < hold } else { g.over >= hold };
            unsafe {
                GOV_BUSY_COUNTS[1] += (g.busy && !was) as u64;
                GOV_BUSY_COUNTS[0] += (g.busy && !lifted) as u64;
            }
        }
        // A saturated window: the load wants more than this rate and how much more is unknown, so the level goes at least a step of the ramp past the rate (`GOV_RAMP` 2: a quarter, 500 MHz to the X3's top in eight windows; 0: double, in three).
        let ramp = rate + (rate >> ramp_s);
        let want = if g.sat != 0 && ramp > want { ramp } else { want };
        g.level = if want >= g.level { want } else { g.level - ((g.level - want) >> decay_s) };
        // WHY the mins: the policy's ceilings, as above.
        let top = gov_top(g, lifted);
        let b = boost_at(g).min(gov_top(g, false));
        if boost && g.domain != 2 && g.level < g.opps[b] {
            g.level = g.opps[b];
        }
        // Over the ceiling (a lift ended, or a load that grew past it): at the ceiling, and the decay goes on from there.
        if g.level > g.opps[top] {
            g.level = g.opps[top];
        }
        // With `diag clock clipshold 1`, a RAW clip recording holds the A715s at `CLIPS_KHZ`, not under it: CPU coders work in bursts (a frame's bands, then the wait for the next), which the windows read as half a load, and the governor asked 1065 MHz while a frame's coding wanted the whole budget (2026-11-19, 60/s: four A715 coders 24 ms a frame at a fixed 1572, more under the governor's 1065).
        // Off by default since the recorder codes on the GPU (ferros_clip gpu): its A715 work is a frame's copy and packing, a few ms of each frame, and a held 1572 MHz only spends power.
        if g.domain == 3 && recording() && CLIPS_HOLD.load(core::sync::atomic::Ordering::Relaxed) != 0 {
            g.level = g.opps[top];
        }
        let at = g.opps.iter().position(|&o| o >= g.level).unwrap_or(g.opps.len() - 1);
        gov_ask(g, at, f);
    }
    tier_active(govs)
}

/// Whether the governor sets `domain` rather than the tier tables: the X3 and the A715s always; the A510s only while the kernel runs on the X3, as then they run only pinned threads (smp.rs). With the kernel on CPU0 (akita, a FERROS_X3=0 build) the A510s' cluster is the kernel's own and keeps the tiers' rates, as before.
fn gov_owns(domain: u32) -> bool {
    domain != 2 || crate::process::kernel_mask() == crate::process::MASK_BIG
}

/// The operating point input boosts a cluster to: `GOV_BOOST` points under its top (0: the top).
fn boost_at(g: &Gov) -> usize {
    // WHY the saturating subtraction: the knob is the bridge's number, and one larger than the table would index below its first point; it then means the floor.
    (g.opps.len() - 1).saturating_sub(GOV_BOOST.load(core::sync::atomic::Ordering::Relaxed) as usize)
}

/// A cluster to operating point `at`, if it is not there: the request under the thermal cap, then a rate read, which the firmware answers only once the request is done (`clock_profile`, why).
fn gov_ask(g: &mut Gov, at: usize, f: u64) {
    if at == g.at {
        return;
    }
    g.at = at;
    g.asks += 1;
    let t0 = cntpct();
    let _ = crate::acpm::dvfs_set(g.domain, crate::thermal::capped(g.domain, g.opps[at]));
    let _ = crate::acpm::dvfs_get(g.domain);
    let us = cntpct().wrapping_sub(t0) * 1_000_000 / f;
    unsafe { CLOCK_WAIT_MAX_US = CLOCK_WAIT_MAX_US.max(us) };
}

/// The rate the clock policy wants for an ACPM DVFS domain now, before any thermal cap (thermal.rs): the governor's for the X3 and the A715s, the tier's entry for the others, or the GPU's top or floor while a `gpu` process lives. None before the policy's first switch, while `diag dvfs policy off` holds the clocks by hand, or for a domain the policy does not set.
pub fn profile_khz(domain: u32) -> Option<u32> {
    let state = clock_state();
    if unsafe { !CLOCK_POLICY_ON } {
        return None;
    }
    if domain == 7 {
        return unsafe { GPU_CLOCKED }.then_some(if state == 3 { 150_000 } else { 890_000 });
    }
    if state != 0 && GOV_ON.load(core::sync::atomic::Ordering::Relaxed) {
        if let Some(g) = unsafe { (*(&raw const GOV)).iter().find(|g| g.domain == domain && gov_owns(domain)) } {
            return Some(g.opps[g.at]);
        }
    }
    let table: &[(u32, u32)] = match state {
        1 => &CLOCKS_REST,
        2 => &CLOCKS_ACTIVE,
        3 => &CLOCKS_DARK,
        _ => return None,
    };
    table.iter().find(|e| e.0 == domain).map(|e| e.1)
}

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
    // A PT transfer moving: the kernel's own, or one the USB process carries (it says so: ptlink.rs MOVING_AT); held a second after the last, as before the adaptive clock.
    let moving = crate::ptlink::MOVING_AT.load(core::sync::atomic::Ordering::Relaxed);
    let pt_busy = pt_busy || (moving != 0 && now.wrapping_sub(moving) < f / 16);
    if pt_busy {
        unsafe { PT_BUSY_AT = now };
    }
    let pt_hold = unsafe { PT_BUSY_AT } != 0 && now.wrapping_sub(unsafe { PT_BUSY_AT }) < f;
    // Input since the last look: both clusters to their top at once.
    let input = crate::process::INPUT_AT.load(core::sync::atomic::Ordering::Relaxed);
    let touched = input != unsafe { GOV_INPUT };
    unsafe { GOV_INPUT = input };
    // The camera's switch on: the memory side stays where it is put for the whole open, below (`CAM_MEM`).
    let cam = CAM_MEM.load(core::sync::atomic::Ordering::Relaxed) && crate::camera_power::held();
    // Never dark while a camera streams or a clip records (owner, 2026-11-21: "if the screen is off, that should not affect video recording"): the dark tier stops the worker cores, floors the X3 and MIF and drops the GPU to its floor.
    // The screen off still stops the UI's drawing (the display tells the app in front it is behind).
    let capturing = crate::camera_power::held() || crate::process::EXTENTS_OPEN.load(core::sync::atomic::Ordering::Relaxed);
    let dark = !pt_hold && !cam && !capturing && crate::process::pref(crate::process::PREF_SCREEN_OFF) == 1;
    let was = unsafe { CLOCK_STATE };
    let gov = GOV_ON.load(core::sync::atomic::Ordering::Relaxed);
    // The governor runs in every tier once the policy has applied one; in the dark the workers are down and only the X3 moves.
    // Governor off (or before the first switch): the tiers alone, active for a second after a touch as before 2026-11-13.
    let causes = if gov && was != 0 { governor(now, f, (touched && !dark) || pt_busy) } else { ((input != 0 && now.wrapping_sub(input) < f) as u8) << 4 } | (pt_hold as u8) << 5;
    if causes != 0 {
        unsafe { TIER_BUSY_AT = now };
    }
    // Rest only once nothing has asked for active for the dwell (`TIER_DWELL`); up to active at once.
    let dwell = TIER_DWELL.load(core::sync::atomic::Ordering::Relaxed);
    let held = was == 2 && dwell != 0 && now.wrapping_sub(unsafe { TIER_BUSY_AT }) < f >> dwell;
    let want = if dark { 3 } else if causes != 0 || held || cam { 2 } else { 1 };
    if want != was && was != 0 {
        unsafe {
            if want == 2 {
                for (k, w) in (*(&raw mut TIER_WHY)).iter_mut().enumerate() {
                    *w += (causes >> k & 1) as u64;
                }
            }
            // The run that ends, by its length: bucket b holds runs under 2^b ms; the last, 2^11 ms and longer.
            // WHY the min: a run has no upper end (a night at rest), and there are twelve buckets.
            let ms = now.wrapping_sub(TIER_SINCE) * 1000 / f;
            let b = (64 - ms.leading_zeros() as usize).min(11);
            TIER_RUNS[was as usize][b] += 1;
            TIER_SINCE = now;
        }
    }
    if was != 0 && unsafe { TIER_AT } != 0 {
        unsafe { TIER_RES[was as usize] += now.wrapping_sub(TIER_AT) };
    }
    unsafe { TIER_AT = now };
    if want != was {
        // The workers are off while dark (2026-10-25: their cluster drew ~28 mW idle in `wfe` with the screen off; Android's 8): started before leaving the dark profile, stopped after entering it, so no clock request goes to their cluster while it is down. Threads meant for the workers run on the X3 meanwhile (process.rs: a mask with no core up runs anywhere).
        let workers_dark = DARK_WORKERS_OFF.load(core::sync::atomic::Ordering::Relaxed);
        if was == 3 && workers_dark {
            crate::smp::launch_all();
            let t0 = cntpct();
            while crate::smp::UP.load(core::sync::atomic::Ordering::Acquire) & crate::smp::launched() != crate::smp::launched() && cntpct().wrapping_sub(t0) < f / 20 {
                core::hint::spin_loop();
            }
        }
        let t_switch = cntpct();
        clock_profile(want);
        unsafe { TIER_SWITCH_TICKS += cntpct().wrapping_sub(t_switch) };
        if want == 3 && workers_dark {
            crate::smp::stop_all();
        }
        // The GPU's clock with the dark profile, while a gpu process keeps it up.
        if unsafe { GPU_CLOCKED } && (want == 3) != (was == 3) {
            gpu_rate(want != 3);
        }
    }
    // The memory side of the lift (THERMAL.md decision 3): while someone interacts with the screen lit, MIF and the DSU at the tops of their tables, else the tier's own; not while ferros's thermal policy holds any domain down.
    // While the camera's switch is on the lift holds from the switch's on to its off, thru heat and quiet alike (`CAM_MEM`): a change of the memory's clock under a stream is the change the front end's writes cannot ride out.
    let lift = cam || (want != 3 && GOV_LIFT_KHZ.load(core::sync::atomic::Ordering::Relaxed) > A715_CALM_KHZ && interactive(now, f) && !crate::thermal::throttled());
    if lift != unsafe { SHARED_LIFTED } {
        shared_lift(lift);
    }
}

/// MIF and the DSU at the lift's rates while someone interacts (`clock_policy`), or back at the tier's.
/// Measured 2026-11-17 on the Pro (membench, 2^24-byte buffers): with the A715s at 2367 MHz, four threads' copy 35.9 GB/s at the active tier's MIF 2028 and DSU 1197 MHz, 41.3 at MIF 3744 and DSU 1800; one thread's read 20.8 against 24.4; with the A715s at 1065, four threads' copy 32.5 against 39.5 at MIF 3744 alone. Both are their tables' tops (zuma.dtsi devfreq_mif and devfreq_dsu `freq_info`), which Android's memory-latency and DSU-latency governors reach under load.
const SHARED_LIFT: [(u32, u32); 2] = [(0, 3_744_000), (5, 1_800_000)];
/// Whether the camera's switch holds the memory side (`diag clock cammem on|off`, off by default): while the switch is on, the tier stays active (never rest or dark, so INT stays at its 356 MHz) and MIF and the DSU at the lift's rates, from before the sensor streams until after it stops.
/// A suspect of 2026-11-17 (a MIF change mid-stream stopping the front end), not the cause found: the deaths measured on 2026-11-18 were the descriptor's release behind the kernel lock (camera_power.rs `ferros_cam_fast`), at 36 C with 2194 lifts before them; kept off unless a run says otherwise.
pub static CAM_MEM: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static mut SHARED_LIFTED: bool = false;
/// Times the memory side was lifted (`diag clock`).
static mut SHARED_LIFTS: u64 = 0;
fn shared_lift(on: bool) {
    // Going up MIF first, coming down the DSU first, as the tiers order them; each request waited for by a rate read (`clock_profile`, why).
    let order: [usize; 2] = if on { [0, 1] } else { [1, 0] };
    for i in order {
        let (d, lifted) = SHARED_LIFT[i];
        let Some(khz) = (if on { Some(lifted) } else { profile_khz(d) }) else { continue };
        let _ = crate::acpm::dvfs_set(d, khz);
        let _ = crate::acpm::dvfs_get(d);
    }
    unsafe {
        SHARED_LIFTED = on;
        SHARED_LIFTS += on as u64;
    }
}
static mut TIER_AT: u64 = 0;
/// The GPU's clock at its top (G3D 890, G3DL2 900 MHz) or its floor (150 / 160 MHz), the L2 first going up and last going down (GPU.md "The GPU's clock"). At the top while a `gpu` process lives and the screen is lit; at the floor otherwise, the dark profile included (2026-11-10: the GPU's rails held ~40 mW with the screen off at its top, idle).
fn gpu_rate(top: bool) {
    // G3D under the thermal policy's cap (thermal.rs `capped`); the L2 stays at its rate, at or above the GPU's.
    let r = if top {
        (crate::acpm::dvfs_set(8, 900_000), crate::acpm::dvfs_set(7, crate::thermal::capped(7, 890_000)))
    } else {
        (crate::acpm::dvfs_set(7, crate::thermal::capped(7, 150_000)), crate::acpm::dvfs_set(8, 160_000))
    };
    crate::process::push_console(alloc::format!("gpu clock: {} ({:?})\n", if top { "G3D 890, G3DL2 900 MHz" } else { "the floor, 150 / 160 MHz" }, r).as_bytes());
}

/// Whether the GPU's clock follows the GPU service's `gpu-clock` switch (`gpu_clock`) rather than its being alive: set at the switch's first use, for the boot.
pub static GPU_BY_SWITCH: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// The switch's turns up and down since boot (`diag clock`), and the longest of either in counter ticks (its two ACPM requests and their answers).
pub static GPU_CLOCK_UPS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static GPU_CLOCK_DOWNS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static GPU_CLOCK_LONGEST: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The turns' ticks summed, for their mean.
pub static GPU_CLOCK_TICKS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The `gpu-clock` switch (2026-11-20, docs/GPU-KISTS.md "Power and clock by events"; the owner, 2026-11-11: "the GPU powers up when work is queued and gates when its queue empties, as events, never a timer"): on when the GPU service has a job queued or running, off when its queues are empty, each turn made by the service at that event.
/// The GPU at its top (G3D 890, G3DL2 900 MHz) while on and the screen lit, at its floor (150 / 160 MHz) while off or dark; the dark profile's own turn follows it (`clock_policy`). On the kernel's core, as every switch, with the lock let go (`SWITCH_UNLOCKED`): its ACPM requests are the clock policy's, made on this same core, never beside them.
/// Quiet: a turn a frame would be a line a frame; `diag clock` says the counts.
pub fn gpu_clock(on: bool) -> bool {
    GPU_BY_SWITCH.store(true, core::sync::atomic::Ordering::Relaxed);
    if on == unsafe { GPU_CLOCKED } {
        return true;
    }
    unsafe { GPU_CLOCKED = on };
    if unsafe { CLOCK_STATE } == 3 {
        return true;
    }
    let t0 = cntpct();
    if on {
        let _ = crate::acpm::dvfs_set(8, 900_000);
        let _ = crate::acpm::dvfs_set(7, crate::thermal::capped(7, 890_000));
        let _ = crate::acpm::dvfs_get(7);
        GPU_CLOCK_UPS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    } else {
        let _ = crate::acpm::dvfs_set(7, crate::thermal::capped(7, 150_000));
        let _ = crate::acpm::dvfs_set(8, 160_000);
        let _ = crate::acpm::dvfs_get(8);
        GPU_CLOCK_DOWNS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
    let took = cntpct().wrapping_sub(t0);
    GPU_CLOCK_LONGEST.fetch_max(took, core::sync::atomic::Ordering::Relaxed);
    GPU_CLOCK_TICKS.fetch_add(took, core::sync::atomic::Ordering::Relaxed);
    true
}

/// `diag smp dark on|off`: whether the workers go off with the dark profile (on by default).
pub static DARK_WORKERS_OFF: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

/// One of the three tiers, every domain in order; then the X3's timer control (its event stream, which wakes the idle loop's `wfe`) and the counter into note 6, and the switch count into note 7, so a hang after a switch says what the core was left with (the first policy build hung the X3 in `wfe` within a minute of going to rest, 2026-10-25).
/// With the governor on, the X3's and the A715s' entries are applied only on the policy's first switch and going dark (their floors, the workers' before they stop), the governor's starting points; otherwise the governor's own.
fn clock_profile(want: u8) {
    let table: &[(u32, u32)] = match want {
        2 => &CLOCKS_ACTIVE,
        3 => &CLOCKS_DARK,
        _ => &CLOCKS_REST,
    };
    let own = GOV_ON.load(core::sync::atomic::Ordering::Relaxed) && unsafe { CLOCK_STATE } != 0 && want != 3;
    // Each request answered before the next and before the loop may sleep: the clusters', MIF's and INT's rate requests take no reply (as Linux sends them), so five went out back to back and the loop went straight into `wfe` with the firmware still at work — and on battery (the charger cut) the X3 never woke from that `wfe` (2026-10-25, twice). A rate read takes a reply, and the firmware answers in order: its answer means the request before it is done. The longest wait goes into DIAG.
    // Each rate under the thermal policy's cap for its domain (thermal.rs `capped`), so a switch never undoes a cap.
    // The memory side lifted (`shared_lift`) and the screen staying lit: MIF and the DSU are left where the lift put them, so a tier switch while someone interacts asks nothing of them (2026-11-17, b922: the tier switched 2973 times in 16 minutes and each switch set them to the tier's and the next pass back up, 3660 lifts in all, two ACPM requests each). Going dark the table applies and the lift ends.
    let keep_shared = unsafe { SHARED_LIFTED } && want != 3;
    // A domain already at the rate asked is not asked again (DISP at 400 MHz in both rest and active; `acpm::ASKED` records every request); and with `TIER_PIPELINE` the requests go out one after another with one rate read after the last, which the firmware answers only once every request before it is done (it answers in order, as above), instead of a read after each. The firmware's queue takes two requests ahead of its answers (acpm.rs `call` waits for room).
    let pipeline = TIER_PIPELINE.load(core::sync::atomic::Ordering::Relaxed);
    let mut last = None;
    let t0 = cntpct();
    for &(d, khz) in table {
        if d == 4 || d == 3 || (d == 2 && gov_owns(2)) {
            if own {
                continue;
            }
            gov_sync(d, khz);
        }
        if keep_shared && SHARED_LIFT.iter().any(|&(sd, _)| sd == d) {
            continue;
        }
        let rate = crate::thermal::capped(d, khz);
        if crate::acpm::ASKED[d as usize].load(core::sync::atomic::Ordering::Relaxed) == rate {
            unsafe { TIER_SKIPPED += 1 };
            continue;
        }
        let t1 = cntpct();
        let _ = crate::acpm::dvfs_set(d, rate);
        unsafe { TIER_ASKS += 1 };
        if pipeline {
            last = Some(d);
        } else {
            let _ = crate::acpm::dvfs_get(d);
            let us = cntpct().wrapping_sub(t1) * 1_000_000 / cntfrq();
            unsafe { CLOCK_WAIT_MAX_US = CLOCK_WAIT_MAX_US.max(us) };
        }
    }
    if let Some(d) = last {
        let _ = crate::acpm::dvfs_get(d);
        let us = cntpct().wrapping_sub(t0) * 1_000_000 / cntfrq();
        unsafe { CLOCK_WAIT_MAX_US = CLOCK_WAIT_MAX_US.max(us) };
    }
    let v: u64;
    unsafe {
        // Unless kept (above), the tier's table set MIF and the DSU: the memory side's lift is off until `clock_policy` asks it again.
        SHARED_LIFTED = keep_shared;
        core::arch::asm!("mrs {}, cnthctl_el2", out(reg) v);
        CLOCK_STATE = want;
        CLOCK_SWITCHES += 1;
        platform::note(6, (CLOCK_STATE as u64) << 56 | v);
        platform::note(7, (CLOCK_SWITCHES as u64) << 40 | cntpct() >> 16);
    }
}

/// `diag clock`: the adaptive clock's state (each cluster's rate, level, busy shares and time at each operating point), the tiers' time, and the touch-to-frame measure (process.rs `INPUT_TO_FRAME`).
/// `diag clock reset` zeroes the times and the measure; `diag clock gov on|off`; `diag clock win|decay|avg|ramp|boost|gpu|frames <n>` sets a knob (`GOV_WIN`); `diag clock lift <kHz>` the A715s' ceiling while interacting (0: no lift), `hold <n>`, `gap <n>` and `cam <n>` its holds as shifts (`lift_causes`; cam 0: a camera stream holds nothing); `diag clock dwell <n>` the shared tier's dwell before rest (`TIER_DWELL`, 0 none), `diag clock pipeline 0|1` its requests pipelined or each waited for; `diag clock cammem on|off` whether the camera's switch holds the memory side (`CAM_MEM`), `cambts on|off` whether its on sets the front end's bus shapers, `camfast on|off` whether armed actions run (irqarm.rs `ON`: the camera arms its descriptor release on its line; off for an A/B).
fn clock_diag(params: &[u8], out: &mut alloc::vec::Vec<u8>) {
    use core::sync::atomic::Ordering::Relaxed;
    let words: alloc::vec::Vec<&[u8]> = params.split(|&b| b == b' ').filter(|w| !w.is_empty()).collect();
    let num = |w: &[u8]| core::str::from_utf8(w).ok().and_then(|t| t.parse::<u32>().ok());
    let f = cntfrq();
    let govs = unsafe { &mut *(&raw mut GOV) };
    match (words.get(1).copied(), words.get(2).and_then(|w| num(w))) {
        (Some(b"reset"), _) => {
            for g in govs.iter_mut() {
                g.res = [0; 16];
                g.asks = 0;
            }
            unsafe { TIER_RES = [0; 4] };
            unsafe { GOV_LIFTED = [0; 3] };
            unsafe { GOV_BUSY_COUNTS = [0; 2] };
            unsafe { SHARED_LIFTS = 0 };
            unsafe { TIER_WHY = [0; TIER_CAUSES] };
            unsafe { TIER_RUNS = [[0; 12]; 4] };
            unsafe { TIER_SWITCH_TICKS = 0 };
            unsafe { (TIER_ASKS, TIER_SKIPPED) = (0, 0) };
            for b in &crate::process::INPUT_TO_FRAME {
                b.store(0, Relaxed);
            }
            crate::process::INPUT_TO_FRAME_SUM.store(0, Relaxed);
            crate::process::INPUT_TO_FRAME_MAX.store(0, Relaxed);
        }
        (Some(b"gov"), _) => {
            let on = words.get(2) == Some(&&b"on"[..]);
            GOV_ON.store(on, Relaxed);
            // Back to the tier's own table for the clusters, or the governor from there.
            if unsafe { CLOCK_STATE } != 0 {
                unsafe { CLOCK_STATE = 0 };
            }
        }
        (Some(b"win"), Some(n)) => GOV_WIN.store(n, Relaxed),
        (Some(b"decay"), Some(n)) => GOV_DECAY.store(n, Relaxed),
        (Some(b"avg"), Some(n)) => GOV_AVG.store(n, Relaxed),
        (Some(b"gpu"), Some(n)) => GOV_GPU.store(n as u64, Relaxed),
        (Some(b"frames"), Some(n)) => GOV_SHOWN.store(n as u64, Relaxed),
        (Some(b"ramp"), Some(n)) => GOV_RAMP.store(n, Relaxed),
        (Some(b"boost"), Some(n)) => GOV_BOOST.store(n, Relaxed),
        (Some(b"lift"), Some(n)) => GOV_LIFT_KHZ.store(n, Relaxed),
        (Some(b"busy"), Some(n)) => GOV_BUSY_KHZ.store(n, Relaxed),
        (Some(b"clips"), Some(n)) => CLIPS_KHZ.store(n, Relaxed),
        (Some(b"clipshold"), Some(n)) => CLIPS_HOLD.store(n, Relaxed),
        (Some(b"busyhold"), Some(n)) => GOV_BUSY_HOLD.store(n, Relaxed),
        (Some(b"doze"), Some(n)) => DOZE_LOG2.store(n, Relaxed),
        (Some(b"hold"), Some(n)) => GOV_HOLD.store(n, Relaxed),
        (Some(b"cam"), Some(n)) => CAM_HOLD.store(n, Relaxed),
        (Some(b"cammem"), _) => CAM_MEM.store(words.get(2) == Some(&&b"on"[..]), Relaxed),
        (Some(b"cambts"), _) => crate::camera_power::BTS.store(words.get(2) == Some(&&b"on"[..]), Relaxed),
        (Some(b"camfast"), _) => crate::irqarm::ON.store(words.get(2) == Some(&&b"on"[..]), Relaxed),
        (Some(b"gap"), Some(n)) => crate::process::ANIM_GAP_LOG2.store(n, Relaxed),
        (Some(b"dwell"), Some(n)) => TIER_DWELL.store(n, Relaxed),
        (Some(b"pipeline"), Some(n)) => TIER_PIPELINE.store(n != 0, Relaxed),
        _ => {}
    }
    let causes = lift_causes(cntpct(), f);
    let why = ["input", "frames", "camera"].iter().enumerate().filter(|(k, _)| causes >> k & 1 != 0).map(|(_, n)| *n).collect::<alloc::vec::Vec<_>>().join("+");
    out.extend_from_slice(alloc::format!("lift (THERMAL.md decision 3): the A715s up to A#{} MHz while interacting (A#{} MHz calm), held f>>A#{} after input, while frames come within f>>A#{} of each other, and f>>A#{} after the camera front end's last answered interrupt (0: never; A#{} answers seen); {} now; A#{} windows lifted (A#{} by the camera alone), A#{} lifts begun; memory side (MIF A#3744, DSU A#1800 MHz) {}, A#{} lifts; the camera's switch {} it; armed actions (the camera releases its descriptor by one; diag irqs) {}\n",GOV_LIFT_KHZ.load(Relaxed).max(A715_CALM_KHZ) / 1000, A715_CALM_KHZ / 1000, GOV_HOLD.load(Relaxed), crate::process::ANIM_GAP_LOG2.load(Relaxed), CAM_HOLD.load(Relaxed), unsafe { CAM_ACKS_SEEN }, if causes != 0 { alloc::format!("lifted by {why}") } else { "calm".into() }, unsafe { GOV_LIFTED[0] }, unsafe { GOV_LIFTED[2] }, unsafe { GOV_LIFTED[1] }, if unsafe { SHARED_LIFTED } { "lifted" } else { "at the tier" }, unsafe { SHARED_LIFTS }, if !CAM_MEM.load(Relaxed) { "does not hold (cammem off)" } else if crate::camera_power::held() { "holds" } else { "is off; when on it holds" }, if crate::irqarm::ON.load(Relaxed) { "on" } else { "off" }).as_bytes());
    let a715 = &govs[1];
    out.extend_from_slice(alloc::format!("recording (docs/RAW-VIDEO.md): while a RAW clip is recorded the A715s run up to A#{} MHz (0: as the policy has them){} and the A510s up to their top; {} now\n", CLIPS_KHZ.load(Relaxed) / 1000, if CLIPS_HOLD.load(Relaxed) != 0 { ", held there (clipshold 1)" } else { "" }, if recording() { "recording" } else { "not recording" }).as_bytes());
    // WHY the max: before the first turn the count is 0, and the mean of no turns is shown as 0.
    out.extend_from_slice(alloc::format!("GPU clock (docs/GPU-KISTS.md \"Power and clock by events\"): {} by {}; the switch up A#{} times, down A#{}, a turn A#{} us on average, its longest A#{} us\n", if unsafe { GPU_CLOCKED } { "up" } else { "at its floor" }, if GPU_BY_SWITCH.load(Relaxed) { "the service's queue (gpu-clock)" } else { "the service's being alive" }, GPU_CLOCK_UPS.load(Relaxed), GPU_CLOCK_DOWNS.load(Relaxed), GPU_CLOCK_TICKS.load(Relaxed) * 1_000_000 / f / (GPU_CLOCK_UPS.load(Relaxed) + GPU_CLOCK_DOWNS.load(Relaxed)).max(1), GPU_CLOCK_LONGEST.load(Relaxed) * 1_000_000 / f).as_bytes());
    out.extend_from_slice(alloc::format!("busy lift (THERMAL.md \"The busy lift\"): the A715s up to A#{} MHz (0: none) while every core of theirs stays at 7/8 busy or more under A#{} MHz for 2^A#{} windows in a row, until their load asks no more than that as long; {} now (least busy core A#{}/256 averaged; A#{} windows over, A#{} under); A#{} windows busy-lifted with no other lift, A#{} busy lifts begun\n", GOV_BUSY_KHZ.load(Relaxed) / 1000, A715_CALM_KHZ / 1000, GOV_BUSY_HOLD.load(Relaxed), if a715.busy { "on" } else { "off" }, a715.whole, a715.over, a715.under, unsafe { GOV_BUSY_COUNTS[0] }, unsafe { GOV_BUSY_COUNTS[1] }).as_bytes());
    out.extend_from_slice(alloc::format!("the kernel core's doze: 2^-A#{} s (0: none), A#{} wakes back to sleep with no pass, A#{} sleeps since boot\n", DOZE_LOG2.load(Relaxed), X3_DOZES.load(Relaxed), X3_SLEEPS.load(Relaxed)).as_bytes());
    let ms = |t: u64| t * 1000 / f;
    out.extend_from_slice(alloc::format!("adaptive clock {}: window f>>A#{} ({} us), decay >>A#{}, average >>A#{}, GPU tier at A#{}/256 (now A#{}), frames tier at A#{}/256 (now A#{}); tier {} (1 rest, 2 active, 3 dark), A#{} switches, A#{} windows\n", if GOV_ON.load(Relaxed) { "on" } else { "off" }, GOV_WIN.load(Relaxed), (f >> GOV_WIN.load(Relaxed)) * 1_000_000 / f, GOV_DECAY.load(Relaxed), GOV_AVG.load(Relaxed), GOV_GPU.load(Relaxed), unsafe { GOV_GPU_AVG }, GOV_SHOWN.load(Relaxed), unsafe { GOV_SHOWN_AVG }, clock_state(), unsafe { CLOCK_SWITCHES }, unsafe { GOV_WINDOWS }).as_bytes());
    let tr = unsafe { TIER_RES };
    out.extend_from_slice(alloc::format!("tiers, ms: rest A#{}, active A#{}, dark A#{}\n", ms(tr[1]), ms(tr[2]), ms(tr[3])).as_bytes());
    let why = unsafe { TIER_WHY };
    out.extend_from_slice(alloc::format!("tier dwell f>>A#{} (0 none); switches to active by cause:", TIER_DWELL.load(Relaxed)).as_bytes());
    for (k, n) in why.iter().enumerate() {
        out.extend_from_slice(alloc::format!(" {} A#{}", TIER_CAUSE_NAMES[k], n).as_bytes());
    }
    out.extend_from_slice(alloc::format!("; switching took A#{} us in all; A#{} requests sent, A#{} skipped (already at the rate), {}\n", unsafe { TIER_SWITCH_TICKS } * 1_000_000 / f, unsafe { TIER_ASKS }, unsafe { TIER_SKIPPED }, if TIER_PIPELINE.load(Relaxed) { "pipelined" } else { "each waited for" }).as_bytes());
    let runs = unsafe { TIER_RUNS };
    for (t, name) in [(1usize, "rest"), (2, "active")] {
        out.extend_from_slice(alloc::format!("  {name} runs by ms:").as_bytes());
        for (b, n) in runs[t].iter().enumerate() {
            if *n != 0 {
                out.extend_from_slice(alloc::format!(" <A#{}:A#{}", 1u64 << b, n).as_bytes());
            }
        }
        out.push(b'\n');
    }
    for g in govs.iter() {
        let total: u64 = g.res.iter().sum::<u64>().max(1);
        let mean = g.res.iter().zip(g.opps).map(|(&t, &o)| t * (o / 1000) as u64).sum::<u64>() / total;
        out.extend_from_slice(alloc::format!("{}: asked A#{} MHz (level A#{}), busy last A#{}/256, average A#{}/256, saturated A#{}, A#{} requests, time-weighted mean A#{} MHz\n  ms at each:", match g.domain { 4 => "X3", 3 => "A715", _ => "A510" }, g.opps[g.at] / 1000, g.level / 1000, g.last, g.avg, g.sat, g.asks, mean).as_bytes());
        for (i, &o) in g.opps.iter().enumerate() {
            out.extend_from_slice(alloc::format!(" A#{}:A#{}", o / 1000, ms(g.res[i])).as_bytes());
        }
        out.push(b'\n');
    }
    let b: alloc::vec::Vec<u64> = crate::process::INPUT_TO_FRAME.iter().map(|b| b.load(Relaxed)).collect();
    let n: u64 = b.iter().sum();
    out.extend_from_slice(alloc::format!("frames armed A#{}; input to frame: A#{} measured, mean A#{} us, longest A#{} us; by ms <1:A#{} 1:A#{} 2-3:A#{} 4-7:A#{} 8-15:A#{} 16-31:A#{} 32-63:A#{} 64+:A#{}\n", crate::process::PRESENTS.load(Relaxed), n, crate::process::INPUT_TO_FRAME_SUM.load(Relaxed) * 1_000_000 / f / n.max(1), crate::process::INPUT_TO_FRAME_MAX.load(Relaxed) * 1_000_000 / f, b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]).as_bytes());
}
static mut PT_BUSY_AT: u64 = 0;
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
    // The hottest zone's temperature and which zone it is in one word (facts 10 and 86).
    let hottest = crate::thermal::hottest().map_or(0, |(z, c)| (z as u64) << 8 | c as u64);
    let now = [g[0], charging, ALIVE.load(Relaxed), VAULT_KEYS.load(Relaxed), hottest, crate::thermal::throttled() as u64];
    let shown = unsafe { &mut *(&raw mut FACTS_SHOWN) };
    if now != *shown {
        *shown = now;
        crate::process::with_lock(|| crate::process::bell_count(crate::process::BELL_FACTS));
    }
}

/// The gauge's last sample (facts 1-4: %, hundredths of a degree, cell uV, current uA with charging positive), for the heat trace (thermal.rs `trace_second`), on the service loop's own core as `gauge_poll` writes it.
pub fn gauge_last() -> [u64; 4] {
    unsafe { (*(&raw const GAUGE)).0 }
}

/// From the service loop on the kernel's core: the battery gauge every two seconds, for facts 1-4, and the vault's key count, for fact 7.
fn gauge_poll() {
    let (f, now) = (cntfrq(), cntpct());
    unsafe {
        let g = &mut *(&raw mut GAUGE);
        // Every 2 s while the screen is on, every 64 s while it is dark: nobody reads the battery's share off a dark screen, and each read is a pass of the loop and a bus transfer the dark idle would otherwise sleep thru.
        let period = if clock_state() == 3 { 64 } else { 2 };
        if g.1 == 0 || now.wrapping_sub(g.1) > period * f {
            g.1 = now;
            g.0 = match crate::hsi2c::gauge::sample() {
                Ok(s) => [s.soc as u64, s.temp_cc as i64 as u64, s.vcell_uv as u64, s.current_ua as i64 as u64],
                Err(_) => [crate::process::FACT_UNKNOWN; 4],
            };
            // The vault's key count only every 64 s: it reads the flash, and every 2 s it woke the UFS link out of hibernate (POWER.md).
            if VAULT_KEYS.load(core::sync::atomic::Ordering::Relaxed) == u64::MAX || now.wrapping_sub(VAULT_KEYS_AT.load(core::sync::atomic::Ordering::Relaxed)) > 64 * f {
                VAULT_KEYS_AT.store(now, core::sync::atomic::Ordering::Relaxed);
                if let Some(keys) = vault(|| (*(&raw mut SPAWN_STORE)).as_mut().and_then(|s| s.as_mut()).and_then(|s| s.keys().ok())) {
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
        crate::flashlog::line_once(crate::flashlog::once::USB_TCPC, &alloc::format!("usb: tcpc role G#{role:X} cc G#{cc:X} power G#{pwr:X} usb switch G#{sw:X}{}", if act { " -> closed (G#09)" } else { "" }));
    }
}

pub fn run(mut b: Boot) -> ! {
    // Report now too: if the USB bring-up wedges, the watchdog's reset still shows the vault result (handoff) and the crumb shows how far USB got.
    report(&b, 0xA0);
    let generation = if b.x0 & 1 == 1 { (b.x0 & !crate::board::RELOAD_AKITA) >> 1 } else { 0 };
    let mut store = b.store.take();
    unsafe {
        SPAWN_STORE = &raw mut store;
        // From here other cores reach the vault (the syscalls above), so every read or write of it holds `VAULT` (kvault.rs panics on a block reached without it).
        crate::process::VAULT_SHARED.store(true, core::sync::atomic::Ordering::Release);
        crate::process::CAPSULES = Some(read_capsule);
        crate::process::SETTINGS_WRITE = Some(settings_write);
        crate::process::FACTS = Some(facts);
        crate::process::STORE = Some(store_scoped);
        // The last boot's end, if the panic reporter or the fault sink left a whole record of it (platform.rs `PANIC_RECORD`, `FAULT_RECORD`), into the flash log as a `kernel.panic` entry with typed fields: a reboot should never be silent, and a clean one says nothing.
        if let Some(p) = crate::platform::previous_panic() {
            let mut line = alloc::format!("the kernel panicked in b{} on core A#{} at {}:{}:{}: {} (last crumb G#{:X})", p.boot, p.core, p.file, p.line, p.column, p.message, p.crumb);
            // The kernel heap's last refused allocation (`report_panic`, r[6..8]), in the words: a measure of the heap, not a field a reader acts on.
            if p.heap[0] != 0 {
                line.push_str(&alloc::format!("; the kernel heap had refused A#{} bytes at alignment A#{} on core A#{}, for G#{:X}, the largest free block then A#{}", p.heap[0] & 0xFF_FFFF_FFFF_FFFF, 1u64 << (p.heap[2] >> 56), p.heap[0] >> 56, p.heap[1], p.heap[2] & 0xFF_FFFF_FFFF_FFFF));
            }
            let d = ferros_ledger::death::Death { end: ferros_ledger::death::END_KERNEL_PANIC, message: Some(p.message), file: Some(p.file), line: Some(p.line), column: Some(p.column), of_boot: Some(p.boot), core: Some(p.core), crumb: Some(p.crumb), ..Default::default() };
            crate::flashlog::kernel_death(&line, d);
        }
        if let Some([elr, far, esr]) = crate::platform::previous_fault() {
            let d = ferros_ledger::death::Death { end: ferros_ledger::death::END_KERNEL_FAULT, esr: Some(esr), far: Some(far), pc: Some(elr), frames: alloc::vec![elr], ..Default::default() };
            crate::flashlog::kernel_death(&alloc::format!("the kernel faulted early in the boot before: ESR G#{esr:X}, FAR G#{far:X}, ELR G#{elr:X}"), d);
        }
        crate::process::STORE_REMOVE = Some(store_remove);
        crate::process::STORE_KEYS = Some(store_keys);
        crate::process::STORE_ADDR = Some(store_addr);
        crate::process::DOMAIN_BASE = Some(domain_base);
        crate::process::UNLOCK = Some(store_unlock);
        crate::process::EXPORT = Some(store_export);
        crate::process::TAG = Some(store_tag);
        crate::platform::BEFORE_RESET = Some(lock_before_reset);
    }
    let mut home_started = !matches!(b.kind, BootKind::Cold);
    let t_boot = cntpct();
    // The jumped session's 60 s note, written once (below, in the loop).
    #[cfg(not(ferros_production))]
    let mut escape_noted = false;
    let exc_start = crate::exception_count();
    platform::mark(0x34);

    // Boot screen: after a --live handoff DECON is still running, so show where this generation stands before USB even comes up (show() touches nothing when the DPU is powered down).
    if !crate::checklist::active() {
        let _ = crate::display::show(&status_text(b.kind, generation, b.probe.as_ref()), cntfrq());
    }
    platform::mark(0x35);

    // The USB DMA master on physical addresses (the HSI0 S2MPU bypassed, every USB SYSMMU context off), reported by DIAG; after a jump the path is denied instead until the `usb` switch has reset the controller (platform::close_usb_dma_path). USB itself is the USB process's (payloads/usb, USB.md): the kernel brings up no controller of its own and readies the link only when the process asks (`power usb on`, ptlink.rs).
    let dma = if platform::jumped() { platform::close_usb_dma_path() } else { open_usb_dma_path() };
    platform::mark(0x36);

    // The worker cores (smp.rs): up once the kernel is settled on its own core, before the first slice is handed out.
    crate::smp::launch_all();
    platform::mark(0x39);
    // The wall clock (rtc.rs): read now, so the first process to ask has it.
    let _ = crate::rtc::sync();
    let freq = cntfrq();
    let mut t_heartbeat = cntpct();
    // The kernel heap's next mark for the flash log (`ferros_heap::PEAK` past it: a line from five eighths up).
    let mut heap_mark = 0usize;
    // The stalls already in the flash log (`platform::stalls`).
    let mut stalls_logged = 0u64;
    // The AoC's start, idle until the `sensors` switch asks for it (sensorlink.rs START).
    let mut sensor_boot = SensorBoot::Done;
    let mut st = PtStats::default();
    // A command's reply as it is built (raw: the USB process frames it), and the reply being built when the next command came, put back after (ptlink.rs); whether the command waits to act once its reply has gone out.
    let mut pt_out_data: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    let mut proc_restore: Option<alloc::vec::Vec<u8>> = None;
    let mut proc_act = false;
    // A bridge `store` command the vault process is answering (`VaultJob`), or a part of a kist offered to stow, and the command it came in, kept for a put's value (ptlink takes no other until its reply is taken).
    let mut vault_job: Option<VaultJob> = None;
    let mut vault_cmd: Option<crate::ptlink::Submitted> = None;
    // A long command (ferros_pt::command::held, docs/USB.md "Long commands"): such a job's command is answered STILL at its lease's end (`job_due`, on the counter) and the job carries on detached (`job_detached`); its own answer then goes to the host's held read (`held_read`, waiting until its own lease's end), or is kept for it (`job_end`) when it ends with none waiting.
    // Why (2026-11-13, the 8a): the last part of Photon's 92 MB capsule committed the whole value on a busy vault and ran past the USB driver's 16 s, so the stow failed twice; no answer may depend on one fixed window.
    let mut job_due = 0u64;
    let mut job_detached = false;
    let mut job_end: Option<alloc::vec::Vec<u8>> = None;
    let mut held_read: Option<u64> = None;
    let lease = ferros_pt::command::held::LEASE_SECONDS * freq;
    // A kist offered to stow (`offer_command`): the capsule being carried in, the offset its next part starts at and whether its last part came; a part that came while a kist job held the kernel's channel, started once it is free; a host's read of the offer's state held until it changes or its lease ends (the capsule, the state it last saw, the deadline).
    let mut offer_rx: Option<([u8; 32], u64, bool)> = None;
    let mut offer_part_waiting = false;
    let mut vault_for_offer = false;
    let mut offer_hold: Option<([u8; 32], u8, u64)> = None;
    // When the part being kept came (the console's line for each part says how long the vault took: the USB driver gives up on a reply at 16 s).
    let mut offer_part_at = 0u64;
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
    // Not a development door (docs/USB.md "Offering a kist"): a production build answers it too.
    let cap_offer = ferros_pt::command::dev_cap(ferros_pt::command::caps::OFFER);
    let cap_held = ferros_pt::command::dev_cap(ferros_pt::command::caps::HELD);
    // RELOAD and RUN share STAGE; at most one of these is set, so a payload can never be jumped to as a kernel or vice versa.
    let mut reload_ok = false;
    let mut reload_size = 0usize;
    let mut run_ok = false;
    let mut run_size = 0usize;
    // A capsule streamed straight into its arc (the spawn Write pieces): the arc, the capsule's length, the bytes received, the code's offset and length inside it.
    let mut spawn_stage: Option<(crate::process::Arc, usize, usize, usize, usize)> = None;
    // `diag frames test`'s run so far: the next frame, frames tested, words wrong, frames with one wrong, the first wrong word's address.
    let mut frames_test = (0usize, 0usize, 0usize, 0usize, 0usize);
    // `diag frames trip`'s pass so far: which (0 arm, 1 check, 2 off), the next frame, frames acted on, words changed; the runs a check found and the changed lines past the most it keeps; when the last arm was done.
    let mut trip = (0u8, 0usize, 0usize, 0usize);
    let mut trip_runs: alloc::vec::Vec<crate::frames::TripRun> = alloc::vec::Vec::new();
    let mut trip_more = 0usize;
    let mut trip_armed_at = 0u64;

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
    // The chords (chord.rs): each arms once its keys are not all down, so the chord that jumped here from Android (or started this restart), or the gate's volume down, still held as this loop starts, fires nothing; a hold fires once.
    let mut chords = crate::chord::Chords::new();
    let mut ipc_marked = false;
    // The radios switched off (preference 15) as the loop last applied them: init started only the ones left on.
    let mut radios_applied = crate::process::pref(crate::process::PREF_RADIOS_OFF) & 0x1F;
    // Cellular or WiFi switched off before this boot (a restart keeps the chips as the last session left them): taken down once now, as the switch does.
    for i in 0..2 {
        if radios_applied >> i & 1 == 1 {
            let r = radio_down(&mut store, i);
            crate::flashlog::line(&alloc::format!("radio {} off at start: power {}", ["modem", "pcie1"][i], if r { "down" } else { "left" }));
        }
    }
    // The GPS driver started for the claims held now (process.rs CLAIMS): set as the first claim starts it, cleared as the last one goes and stops it. A driver that ends by itself while claimed is not started again until the claims go to none and back.
    let mut gps_on = false;
    // ARC_FREES when the driver's spawn was last refused for want of an arc, as the stower's below: tried again once an arc has come back.
    let mut gps_refused_at: Option<u64> = None;
    // The stower (payloads/kists) started for the person present.
    let mut kists_started = false;
    // ARC_FREES when the stower's spawn was last refused for want of an arc (the 8a, 2026-11-10: "arc pool exhausted" at attest, beside the wallpaper's start, and no stower for the rest of the boot): tried again once an arc has come back since.
    let mut kists_refused_at: Option<u64> = None;
    // The kept processes (process.rs `KEPT`) whose restart was refused for want of an arc, and ARC_FREES then: tried again once an arc has come back, as the stower is.
    let mut kept_room: u32 = 0;
    let mut kept_room_at: u64 = 0;
    // Restart notes waiting for a person to post them to (a note is the person's): what process.rs queued (`notices_take`) and the restarts this loop made.
    let mut notices: alloc::vec::Vec<(&'static [u8], ferros_kist::restart::End, bool)> = alloc::vec::Vec::new();
    // The on-demand servers whose start was refused for a crash loop, said once each until a restart by hand clears it (bits by `ON_DEMAND` index).
    let mut on_demand_looped: u32 = 0;
    // Whether the flash log has had the background wipe's line (frames.rs `wipe_report`).
    let mut wipe_logged = false;
    // The panel's VCI as ABL or Android left it (SYS_FACTS 81): the display server reads it to know a panel it must power up (state B).
    crate::panel_power::read_state();
    // The display engine fenced to its own regions when the DPU is up already (ABL's splash, or a jump from a lit Android); a DPU that is down is fenced by the `dpu` switch as it powers it (s2mpu::fence_dpu). Before init starts the display server, so it never runs unfenced.
    if crate::dpu_power::all_on() {
        let r = crate::s2mpu::fence_dpu();
        crate::flashlog::line(&alloc::format!("dpu: S2MPUs fenced at boot: G#{r:X} (1 = fenced)"));
    }
    // The CoreSight system's bus master (the debug port's memory access, STM, the trace sinks) cut off from all memory on a cold boot: ferros uses none of it, and ABL leaves its S2MPU open. Not the cores' path: deny-all on it with ferros running left every core, the display and USB running (2026-11-04). Not after a jump: closed there, the jumped ferros froze right after its vault opened (most likely Android's trace still running on the cores, its sink cut off, backing up into them); that waits for tracing switched off on every core first.
    // The modem's link fenced to its shared memory before init starts its driver (s2mpu::fence_modem), after a jump too: the hv's deny-all would refuse the CP the boot image, and the driver takes Android's CP down before it brings one up (payloads/modem `off`).
    // Between this and that crowbar a CP Android left linked could write only into the carve-outs, which hold nothing of ferros's until the driver loads its image after the CP is down; the jump of 2026-11-15 found the link suspended (the ch0 PHY powered down), so no DMA at all.
    let r = crate::s2mpu::fence_modem();
    crate::flashlog::line(&alloc::format!("modem: HSI1's S2MPU fenced to the shared-memory windows: G#{r:X} (1 = fenced)"));
    // A reload is a jump from ferros's own last session: the WiFi chip and the CP may be running, their links up or suspended, their DMA going, Bluetooth's core on. Both links' endpoints are held in reset and Bluetooth switched off with its pins driven before init starts any driver, after a jump too (pcie_guard.rs `crowbar`); each driver's own `off` then takes its chip down.
    // The reload of 2026-11-16 (b908) wedged the WiFi chip: its driver's power cycle ran under a Bluetooth core whose off had never driven its pin, and the chip's next BAR read never completed (bt.rs `power`).
    if b.x0 & 1 == 1 || platform::jumped() {
        crate::pcie_guard::crowbar(if b.x0 & 1 == 1 { "a reload" } else { "a jump" });
    }
    if !platform::jumped() {
        let r = crate::s2mpu::deny_all(crate::s2mpu::CPUCL0_S2MPU);
        crate::flashlog::line(&alloc::format!("cpucl0: the debug system's S2MPU denied all: G#{r:X} (1 = done)"));
        // BLK_MISC's bus masters (PDMA1, SPDMA1, DIT), none of them ferros's: denied all, as the hv denies them at the jump.
        let r = crate::s2mpu::deny_all(crate::s2mpu::MISC_S2MPU);
        crate::flashlog::line(&alloc::format!("misc: BLK_MISC's S2MPU denied all: G#{r:X} (1 = done)"));
    } else {
        // After a jump (HV.md "The high bank after a jump", 2026-11-15): every bus master Android left that the hv's deny-alls and the domains cut leave able to write, stopped before init starts anything.
        // Trace's sinks first (coresight.rs: the ETR would write a buffer Linux allocated, now ferros's memory), the TMCs only. Measured on the jumped Pro (b879, 2026-11-15): every TMC off and the ETR never given a buffer. The kernel of b880 also read every core's trace unit here and stalled the bus (APC watchdog), so those are left alone.
        // CPUCL0's S2MPU stays open after a jump for now: its deny froze the jump of 2026-11-04, and with trace proven off that freeze is unexplained; it is tried alone, by hand (`diag s2mpu deny cpucl0`), before it joins these.
        let mut s = alloc::string::String::new();
        crate::coresight::report(&mut s);
        crate::coresight::stop(&mut s);
        for l in s.lines() {
            crate::flashlog::line(&alloc::format!("trace after the jump: {l}"));
        }
        // Android's GPU (or the last ferros session's, after a restart), perhaps left powered with its page tables and its MCU running: cut off from memory at once.
        let r = crate::s2mpu::deny_gpu();
        crate::flashlog::line(&alloc::format!("gpu: G3D's S2MPU denied all after the jump: G#{r:X} (1 = done, E4 = pd_g3d off)"));
        // The AoC, still in Android's session: held to its own carve-out.
        let r = crate::s2mpu::fence_aoc();
        crate::flashlog::line(&alloc::format!("aoc: its S2MPU fenced to its carve-out after the jump: G#{r:X} (1 = fenced)"));
    }
    // The GPU at every boot from its reset, then held to its own memory (gpu-mem) before any process can program it (its page tables are a process's): hard reset and fenced (pd.rs `gpu_fresh`; a GPU found with a domain down stays down and denied until its power-on is proven, b879 found g3d off after a jump), the GPU service then starting below and loading the MCU's firmware afresh. After a jump this is the crowbar (owner, 2026-11-21: "if we can bring up GPU and all hardware after a jump, that would be excellent"; the jump is a killswitch: nothing of Android's GPU adopted); after a reload or a restart it ends the last session's MCU and page tables; on a cold boot ABL's GPU is at reset already and the reset costs nothing. A GPU that does not come to reset stays denied and no service starts: a call on `gpu` is then answered ERR (process.rs `gpu_absent`).
    let mut out = alloc::vec::Vec::new();
    let fresh = crate::pd::gpu_fresh(&mut out, false);
    for l in out.split(|&c| c == b'\n').filter(|l| !l.is_empty()) {
        crate::flashlog::line(&alloc::format!("gpu fresh: {}", core::str::from_utf8(l).unwrap_or("?")));
    }
    if fresh {
        let r = crate::s2mpu::fence_gpu();
        crate::flashlog::line(&alloc::format!("gpu: G3D's S2MPU fenced to gpu-mem: G#{r:X} (1 = fenced)"));
        GPU_FRESH.store(r == 1, core::sync::atomic::Ordering::Relaxed);
    }
    // The device's opt-in to Android (ANDROID_ALLOWED), once.
    if let Some(st) = unsafe { (*(&raw mut SPAWN_STORE)).as_mut() }.and_then(|s| s.as_mut()) {
        let yes = vault(|| matches!(st.get_by_key("android.allowed"), Ok(Some(v)) if v.as_slice() == [1]));
        ANDROID_ALLOWED.store(if yes { 2 } else { 1 }, core::sync::atomic::Ordering::Relaxed);
    }
    platform::mark(0x3A);
    loop {
        platform::pet_watchdogs();
        // The service loop's pass crumbs (2026-11-08: the reset left G#B4, woken from the idle `wfe`, as the X3's last word, and nothing after it said where the pass went): G#D3 the pass's top, G#D4 a command taken from the USB process, G#D5 with its capability and op (bits 8-39 and 40-47), G#D6 the polls, G#D7 the clock policy, G#D8 past the UFS hibernate and the thermal poll; then G#D1 a slice, G#D2 back, G#B3 into the idle `wfe`, G#B4 out of it.
        platform::crumb(0xD3);
        // A core stuck in an access to a PCIe link's window ended by that link going down (pcie_guard.rs), first in the pass: what follows may take the kernel lock, which a stuck access on the locked path holds.
        crate::pcie_guard::release_stuck();
        // The power switches a process left on when it ended (one shot dead, no cleanup of its own): turned off here, on the kernel's core.
        crate::process::power_offs_apply();
        platform::crumb(0xF0);
        // Frames given back, wiped and freed with the kernel lock let go (frames.rs `reclaim_step`): up to 2^3 a pass (about a millisecond of this core), the idle workers taking the rest; with none up (the dark, alone) this is the only taker.
        for _ in 0..1 << 3 {
            if !crate::frames::reclaim_step() {
                break;
            }
        }
        // A person unlocked: wallgen, once (`spawn_wallgen`).
        if let Some(seed) = unsafe { (*(&raw mut WALLGEN_SEED)).take() } {
            spawn_wallgen(&mut store, &seed);
        }
        // A status circle tapped (preference 15, owner 2026-11-07): a radio switched off has its driver stopped and, where the kernel holds its power switch (Bluetooth, NFC, GPS), its chip powered down; switched on, its driver starts again from its capsule. Cellular and WiFi have no kernel switch: their drivers stop, then each starts once with its power-down input (`radio_down`), which takes the chip down and ends (2026-11-13: the chips left powered with no driver drew more than with the radios on, 968 mW dark on the 8a against 782-859).
        let radios_off = crate::process::pref(crate::process::PREF_RADIOS_OFF) & 0x1F;
        if radios_off != radios_applied {
            const DRIVERS: [&[u8]; 5] = [b"modem", b"pcie1", b"bt", b"nfc", b"gps"];
            for (i, name) in DRIVERS.iter().enumerate() {
                let bit = 1u8 << i;
                if radios_off & bit != 0 && radios_applied & bit == 0 {
                    let killed = crate::process::kill(name);
                    let powered_down = match i {
                        0 | 1 => radio_down(&mut store, i),
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
        // The vault, usb and display up once init has run 16 s, or said on the glass (glass.rs): two loads and a compare a pass until then.
        // No usb up then and no init running (it died, or crash-looped, before it started usb): the kernel's own usb (`usb_rescue`), once, at that deadline.
        if crate::glass::boot_watch() && !crate::process::running(b"init") {
            usb_rescue(&alloc::format!("init is not running and no usb is up A#{} s after init started", 1u32 << crate::glass::BOOT_DEADLINE_SHIFT), true);
        }
        // The vault process ended having served, or was killed with no successor (process.rs `vault_ended`, `VAULT_ON`): started again from its capsule at once, the store calls queued for it waiting meanwhile, never answered by the kernel's vault, which no longer holds what it keeps. One that will not start leaves the vault gone, its waiting calls refused.
        if crate::process::vault_wanted() && crate::process::with_lock(|| crate::process::named_alive_locked(&[b"vault"])) == 0 {
            let mut rep = alloc::vec::Vec::new();
            restart(&mut store, b"vault", &mut rep);
            let said = core::str::from_utf8(&rep).unwrap_or("").trim_end();
            if rep.ends_with(b" started\n") {
                crate::flashlog::line(&alloc::format!("vault process started again: {said}"));
                // Its death (not a kill) noted for the person (docs/KERNEL.md "Restarts").
                if let Some(end) = crate::process::vault_end_take() {
                    notices.push((b"vault", end, false));
                }
            } else {
                let n = crate::process::with_lock(crate::process::vault_refuse_locked);
                crate::flashlog::line(&alloc::format!("vault process not started again ({said}): gone, A#{n} waiting calls refused"));
            }
        }
        let person = crate::kvault::PERSON.load(core::sync::atomic::Ordering::Acquire);
        // The deck is up: idle workers may now zero the frames Android left (frames.rs, after a jump only), and the flash log says once when they are done.
        if person {
            crate::frames::wipe_allow();
        }
        if !wipe_logged && crate::frames::wipe_done() {
            wipe_logged = true;
            crate::flashlog::line(crate::frames::wipe_report().trim_end());
        }
        // The stower (payloads/kists), once each time a person is in: it serves the map's Kists page and fetches back the capsules a restored list names. Its grants are its own (`kists_grants`), init's page being full.
        let frees = crate::process::ARC_FREES.load(core::sync::atomic::Ordering::Acquire);
        if !person {
            // The person left: an offer they had not answered is gone, never shown to whoever attests next.
            if kists_started {
                crate::process::offer_drop();
            }
            kists_started = false;
            kists_refused_at = None;
        } else if !kists_started && kists_refused_at != Some(frees) {
            kists_started = true;
            let running = crate::process::with_lock(|| crate::process::named_alive_locked(&[b"kists"])) != 0;
            if !running {
                let mut rep = alloc::vec::Vec::new();
                restart(&mut store, b"kists", &mut rep);
                crate::flashlog::line(&alloc::format!("kists after attest: {}", core::str::from_utf8(&rep).unwrap_or("").trim_end()));
                // Refused for room, and only that (a stower that died by a panic or a fault is started again as a kept process, below): the next arc back (a process ending) brings another try.
                if rep.windows(18).any(|w| w == b"arc pool exhausted") {
                    kists_started = false;
                    kists_refused_at = Some(frees);
                }
            }
        }
        // The kept processes that died by a panic or a fault (process.rs `KEPT`, `crash_answer`; owner, 2026-11-21, after the deck panicked on the Pro and nothing answered a touch until the bridge restarted it): each started again at once from its capsule, as `diag restart` does, in `KEPT`'s order (the display before the deck). The death woke this loop (`sev`), never a timer; one that died again too soon was not asked for (a crash loop, said and noted where it was counted).
        // One alive already (started meanwhile by hand, or by init's keeper) is left alone. What reads the person's space is refused as `restart` refuses it (the deck until the device is bound, the identity service and the stower until a person attests), which one that was running had passed already. One refused for room is tried again once an arc has come back.
        let mut want = crate::process::kept_take();
        if kept_room != 0 && frees != kept_room_at {
            want |= kept_room;
            kept_room = 0;
        }
        if want != 0 {
            for (i, name) in crate::process::KEPT.iter().enumerate() {
                if want >> i & 1 == 0 {
                    continue;
                }
                let nm = core::str::from_utf8(name).unwrap_or("?");
                // `running`, not `named_alive_locked`: the one that died may still be ending (a thread of it on another core until its next trap), and that is no successor.
                if crate::process::running(name) {
                    crate::flashlog::line(&alloc::format!("restarts: {nm} is running again already"));
                    continue;
                }
                // NFC is a radio: switched off by its status circle, it stays off.
                if *name == b"nfc" && radios_off >> 3 & 1 == 1 {
                    crate::flashlog::line("restarts: nfc not started again: switched off");
                    continue;
                }
                let end = crate::process::kept_end(i);
                let mut rep = alloc::vec::Vec::new();
                restart_once(&mut store, name, &mut rep);
                let said = alloc::string::String::from_utf8_lossy(&rep);
                let said = said.trim_end();
                if rep.starts_with(name) && rep.ends_with(b" started\n") {
                    crate::flashlog::line(&alloc::format!("restarts: {nm} died by {} and was started again", end.word()));
                    crate::process::push_console(alloc::format!("restarts: {nm} started again after {}\n", end.word()).as_bytes());
                    notices.push((name, end, false));
                    // The identity service holds the person's roots in RAM alone unless unlock at boot is on (payloads/fgtw), so a new one may hold none: the attest screen started again after it asks the service's state and, finding no roots, comes up over whatever runs so the handle typed hands them over (AUTH.md "Never stuck"; ahau_core `attest_screen`). With roots kept it shows nothing and stays as the keyboard.
                    if *name == b"fgtw" {
                        let mut rep = alloc::vec::Vec::new();
                        restart(&mut store, b"ahau", &mut rep);
                        crate::flashlog::line(&alloc::format!("restarts: ahau started again after fgtw, to ask for the roots if it holds none: {}", core::str::from_utf8(&rep).unwrap_or("").trim_end()));
                    }
                } else if rep.ends_with(b"running already\n") {
                    crate::flashlog::line(&alloc::format!("restarts: {nm} is running again already (started meanwhile)"));
                } else if rep.windows(18).any(|w| w == b"arc pool exhausted") {
                    kept_room |= 1 << i;
                    kept_room_at = frees;
                    crate::flashlog::line(&alloc::format!("restarts: {nm} not started again yet, no arc free: tried again once one comes back"));
                } else {
                    crate::flashlog::line(&alloc::format!("restarts: {nm} died by {} and was not started again: {said}", end.word()));
                }
            }
            // The GPU service's restart tried (not one put off for room, which is still coming): calls waiting on `gpu` get ERR if it is not running now (process.rs `gpu_absent`).
            if let Some(g) = crate::process::KEPT.iter().position(|n| *n == b"gpu") {
                if want >> g & 1 == 1 && kept_room >> g & 1 == 0 {
                    let n = crate::process::gpu_restart_tried();
                    if n != 0 {
                        crate::flashlog::line(&alloc::format!("restarts: gpu not running: A#{n} calls waiting on it answered ERR"));
                    }
                }
            }
        }
        // The restart notes, posted once a person is present (a note is theirs; nobody present, they wait here): the Deck button glows and the about page lists them (payloads/map notes.rs).
        notices.extend(crate::process::notices_take());
        if person && !notices.is_empty() {
            for (name, end, stopped) in notices.drain(..) {
                let words = ferros_kist::restart::notice_words(name, end, stopped);
                let r = crate::process::note_post_notice(name, &words);
                if r == crate::process::ERR || r == crate::process::NOTHING {
                    crate::flashlog::line(&alloc::format!("restarts: the note \"{words}\" was not kept (G#{r:X})"));
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
                // Input 01, the on-chip replay (GPS.md "No fix, 2026-11-18": the stored `gps.replay` is a host-mode session that can never give a fix); the driver takes no input the same way since that day, so either side alone is enough.
                restart_with(&mut store, b"gps", &[1], &mut rep, false);
                crate::flashlog::line(&alloc::format!("gps claimed (A#{} claims): {}", crate::process::gps_claims(), core::str::from_utf8(&rep).unwrap_or("").trim_end()));
                // Not started: the GPS mailbox's readers woken to a dead mailbox (`mailbox_ring_dead`), as a driver's end would wake them, so none waits on a publish that will not come.
                if !(rep.starts_with(b"gps") && rep.ends_with(b" started\n")) {
                    crate::process::with_lock(|| crate::process::mailbox_ring_dead(ferros_mailbox::GPS));
                }
                // Refused for room, as the stower's above: the next arc back brings another try while the claim stands.
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
        // Servers started on demand (process.rs ON_DEMAND: the torch): a call found none receiving, so one is started from its capsule; the call waits for it (a live call has had its ERR already, and the next one finds it). Already alive (spawned, not yet at its first receive) is left alone. One that cannot start answers its waiting calls ERR, so none waits on a server that is not coming, and is tried again at the next call.
        platform::crumb(0xF1);
        let wanted = crate::process::on_demand_take();
        for (i, name) in crate::process::ON_DEMAND.iter().enumerate() {
            if wanted >> i & 1 == 0 || crate::process::with_lock(|| crate::process::named_alive_locked(&[name])) != 0 {
                continue;
            }
            let nm = core::str::from_utf8(name).unwrap_or("?");
            // In a crash loop (process.rs `death_counted`; docs/KERNEL.md "Restarts"): not started until a restart by hand, its waiting calls answered ERR; said once.
            if crate::process::restart_stopped(name) {
                let n = crate::process::with_lock(|| crate::process::on_demand_refuse_locked(name));
                if on_demand_looped >> i & 1 == 0 {
                    on_demand_looped |= 1 << i;
                    crate::flashlog::line(&alloc::format!("{nm} on demand: not started, it kept crashing (until `diag restart {nm}`); A#{n} waiting calls answered ERR"));
                }
                continue;
            }
            on_demand_looped &= !(1 << i);
            let mut rep = alloc::vec::Vec::new();
            restart(&mut store, name, &mut rep);
            if rep.starts_with(name) && rep.ends_with(b" started\n") {
                crate::flashlog::line(&alloc::format!("{nm} started on demand"));
            } else {
                let n = crate::process::with_lock(|| crate::process::on_demand_refuse_locked(name));
                crate::flashlog::line(&alloc::format!("{nm} on demand: not started ({}); A#{n} waiting calls answered ERR", core::str::from_utf8(&rep).unwrap_or("").trim_end()));
            }
        }
        // Released (a completed departure): restart, so the device comes back at the attest screen with nothing of the identity left.
        // First the person's domains in the vault process are dropped, a call a pass through the kernel's channel (none while a bridge command holds it); a drop that fails, or no vault to send it to, is logged and the restart goes ahead: the renewed device-only secret already makes those entries unopenable.
        let channel_free = vault_job.is_none() && kist_job.is_none();
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
                        // A removal ends in the killswitch's power off (owner, 2026-11-22); a completed departure restarts.
                        if RELEASE_OFF.load(core::sync::atomic::Ordering::Acquire) {
                            crate::flashlog::line("identity removed: powering off; the next power-on comes up at the attest screen");
                            platform::power_off();
                        }
                        crate::flashlog::line("identity released: restarting at the attest screen");
                        platform::reboot();
                    }
                }
            }
        }
        // The person's answer to Android's recovery request (the Android page, SYS_UNLOCK op 13 requests 7 and 8; docs/BOOT.md "Recovery requests from Android"), taken here where a reset may be ordered and UFS written. Checked again: only with a person present.
        #[cfg(not(shim_handoff))]
        match RECOVERY_ANSWER.swap(0, core::sync::atomic::Ordering::AcqRel) {
            0 => {}
            _ if !crate::kvault::PERSON.load(core::sync::atomic::Ordering::Acquire) => crate::flashlog::line("recovery: an answer with no one present: not taken"),
            1 => recovery_yes(),
            _ => recovery_no(),
        }
        // The person's standing choice, once, at the first attest of a boot nobody answered (`START_INTO`): Android when they chose it and this device allows it.
        #[cfg(not(shim_handoff))]
        if START_INTO.load(core::sync::atomic::Ordering::Relaxed) && crate::kvault::PERSON.load(core::sync::atomic::Ordering::Acquire) {
            START_INTO.store(false, core::sync::atomic::Ordering::Relaxed);
            let android = crate::settings::START_ANDROID.load(core::sync::atomic::Ordering::Relaxed);
            let allowed = ANDROID_ALLOWED.load(core::sync::atomic::Ordering::Relaxed) == 2;
            if android && allowed {
                crate::flashlog::line("start into: Android, the person's choice, after the attest");
                ANDROID_REQ.store(true, core::sync::atomic::Ordering::Release);
            } else if android {
                crate::flashlog::line("start into: Android chosen, but this device does not allow Android: ferros");
            }
        }
        // A launch of Android the attested identity chose (the "Start into" setting, "start Android now" on the Settings page, the `android` switch): the gate's Android path, as `diag android` takes it. The person is checked again here; the reset scrubs ferros's memory on its way (platform::before_reset).
        // Android's own recovery request first (a factory reset, RescueParty, an update left in misc; owner, 2026-11-22): the launch waits until the person has answered it on the Android page, and is never the answer itself, so a wipe they asked for in Android is never cleared on the way past. Their no clears it and the launch goes on; their yes resets into recovery instead.
        #[cfg(not(shim_handoff))]
        if ANDROID_REQ.load(core::sync::atomic::Ordering::Acquire) {
            let held = || {
                if !ANDROID_HELD_SAID.swap(true, core::sync::atomic::Ordering::Relaxed) {
                    crate::flashlog::line("android: launch held: Android's recovery request waits on the person's answer (the Android page)");
                }
            };
            if !crate::kvault::PERSON.load(core::sync::atomic::Ordering::Acquire) {
                ANDROID_REQ.store(false, core::sync::atomic::Ordering::Release);
            } else if RECOVERY_ASK.load(core::sync::atomic::Ordering::Acquire) != ferros_hal::bcb::ASK_NONE {
                held();
            } else {
                // misc read again at the launch: a request the boot's read missed (misc unreadable then) is asked, not cleared.
                match crate::slot::misc_request() {
                    Ok((ferros_hal::bcb::ASK_NONE, _)) => {
                        ANDROID_REQ.store(false, core::sync::atomic::Ordering::Release);
                        ANDROID_HELD_SAID.store(false, core::sync::atomic::Ordering::Relaxed);
                        // Under the UFS lock (slot.rs takes it), as every UFS command: devinfo is written while processes still run.
                        let r = crate::slot::set_active(0);
                        crate::flashlog::line(&alloc::format!("android: launching (slot a {r})"));
                        // The word only with the reset, so none waits for a later boot.
                        if r == crate::slot::MARKED {
                            crate::platform::android_once();
                            platform::crumb(0xC7);
                            platform::reboot();
                        }
                    }
                    Ok((ask, _)) => {
                        recovery_ask_set(ask);
                        crate::flashlog::line(&alloc::format!("android: misc asks for {} after all", recovery_ask_words(ask)));
                        held();
                    }
                    Err(e) => {
                        ANDROID_REQ.store(false, core::sync::atomic::Ordering::Release);
                        crate::flashlog::line(&alloc::format!("android: not launching: misc not read (G#{e:X}), so whether Android asks for recovery is unknown"));
                    }
                }
            }
        }
        // A part of a kist offered to stow that came while a kist job held the kernel's channel: its put started now that the channel is free (the part's command kept in `vault_cmd`); its reply waits for the put, as a store command's does.
        if offer_part_waiting && kist_job.is_none() && vault_job.is_none() {
            offer_part_waiting = false;
            let params = vault_cmd.as_ref().and_then(|c| ferros_pt::command::parse(c.payload.bytes())).map(|c| c.params).unwrap_or(&[]);
            match offer_part(params) {
                Ok(job) => {
                    vault_job = Some(job);
                    vault_for_offer = true;
                }
                Err(resp) => {
                    offer_rx = None;
                    vault_cmd = None;
                    crate::process::with_lock(|| crate::ptlink::hold_big(false));
                    if !job_detached {
                        queue_pt_response(&mut pt_out_data, &resp);
                    } else if held_read.take().is_some() {
                        job_detached = false;
                        queue_pt_response(&mut pt_out_data, &resp);
                    } else {
                        job_detached = false;
                        job_end = Some(resp);
                    }
                }
            }
        }
        // A host's read of the offer's state, held until the stower tells another state or its lease ends (ferros_kist::offer::HOLD_SECONDS, inside the USB driver's 16 s): then answered, with the state as it stands.
        let offer_hold_waits = match offer_hold {
            Some((capsule, seen, until)) => {
                let s = crate::process::offer_state(&capsule);
                if s[0] != seen || cntpct() >= until {
                    queue_pt_response(&mut pt_out_data, &s);
                    offer_hold = None;
                    false
                } else {
                    true
                }
            }
            None => false,
        };
        // A bridge `store` command, or a part of a kist offered to stow, the vault process is answering: its next step; its reply goes out below once the last call is answered.
        let job_busy = match vault_job.as_mut() {
            Some(job) => {
                let params = vault_cmd.as_ref().and_then(|c| ferros_pt::command::parse(c.payload.bytes())).map(|c| c.params).unwrap_or(&[]);
                match vault_job_step(job, params, &mut store) {
                    Some(resp) => {
                        // An offered part the vault did not keep: the host starts the capsule again from its first part.
                        if vault_for_offer {
                            let ms = (cntpct() - offer_part_at) * 1000 / freq;
                            match (resp.first(), offer_rx) {
                                (Some(&0), Some((_, end, true))) => crate::flashlog::line(&alloc::format!("kist offer: carried in whole, A#{end} bytes (its last part kept in A#{ms} ms)")),
                                (Some(&0), Some((_, end, false))) => crate::process::push_console(alloc::format!("kist offer: a part kept, A#{end} bytes in, in A#{ms} ms\n").as_bytes()),
                                (Some(&s), _) => crate::flashlog::line(&alloc::format!("kist offer: a part not kept (status A#{s}, in A#{ms} ms): the host starts again from the first")),
                                (None, _) => crate::flashlog::line(&alloc::format!("kist offer: a part not kept (no status, in A#{ms} ms): the host starts again from the first")),
                            }
                        }
                        if resp.first() != Some(&0) && core::mem::take(&mut vault_for_offer) {
                            offer_rx = None;
                        }
                        vault_for_offer = false;
                        // Its own answer: to its command, or once that was answered STILL, to the host's held read (kept for it when none waits).
                        if job_detached {
                            crate::flashlog::line(&alloc::format!("long command: its work ended A#{} ms after it came (status A#{})", (cntpct() - (job_due - lease)) * 1000 / freq, resp.first().copied().unwrap_or(u8::MAX)));
                            job_detached = false;
                            if held_read.take().is_some() {
                                queue_pt_response(&mut pt_out_data, &resp);
                            } else {
                                job_end = Some(resp);
                            }
                        } else {
                            queue_pt_response(&mut pt_out_data, &resp);
                        }
                        crate::process::with_lock(|| crate::ptlink::hold_big(false));
                        vault_job = None;
                        vault_cmd = None;
                        false
                    }
                    None => true,
                }
            }
            None => false,
        } || offer_part_waiting;
        // Still at work at its lease's end: its command answered STILL, the job carried on (ferros_pt::command::held). A put still reading its value from the kernel's command region keeps that region from the next command (ptlink `hold_big`).
        if job_busy && !job_detached && cntpct() > job_due {
            crate::flashlog::line("long command: still at work at its lease's end, answered STILL; its end goes to the host's held read");
            queue_pt_response(&mut pt_out_data, &held_status(ferros_pt::command::held::STILL, "still at work: its end is the answer to ferros.pt.held"));
            job_detached = true;
            let big = matches!(vault_cmd.as_ref().map(|c| &c.payload), Some(crate::ptlink::Payload::Big(_)));
            crate::process::with_lock(|| crate::ptlink::hold_big(big));
        }
        // A held read whose own lease ended with the job still at work: STILL again, and the host asks again.
        if let Some(due) = held_read {
            if cntpct() > due {
                queue_pt_response(&mut pt_out_data, &held_status(ferros_pt::command::held::STILL, "still at work: its end is the answer to ferros.pt.held"));
                held_read = None;
            }
        }
        let vault_waits = (job_busy && !job_detached) || held_read.is_some() || offer_hold_waits;
        // A kist launch init asked for (KISTS.md): started once the kernel's channel is free (no bridge store command or offered part, no release under way), then a read a pass until the capsule is in its arc (`kist_step`).
        let bridge_holds_channel = vault_job.is_some();
        if kist_job.is_none() && !bridge_holds_channel && !RELEASE_RESET.load(core::sync::atomic::Ordering::Acquire) {
            // A launch first (a person waiting at a circle), then a stow (the stower waits on its word).
            let why = crate::process::kist_take().map(KistWhy::Launch).or_else(|| crate::process::stow_take().map(KistWhy::Stow));
            if let Some(why) = why {
                let capsule = match &why {
                    KistWhy::Launch(l) => l.capsule,
                    KistWhy::Stow(i) => i.capsule,
                };
                // The seals a masked kist's launch is checked against, read once for the job under the vault's lock, as every vault read.
                let sealed = seals_for_launch(&mut store);
                let job = KistJob { why, capsule, sealed, total: 0, head: alloc::vec::Vec::new(), arc: None, got: 0, t0: cntpct(), waiting: false, checked: None, read: false, told: false, twin: None, hash_ticks: 0 };
                if kist_call(&job) {
                    kist_tell(&job, crate::process::STOW_READING, 0);
                    kist_job = Some(job);
                } else {
                    match &job.why {
                        KistWhy::Launch(l) => {
                            crate::process::push_console(alloc::format!("kist {}: not started: no vault process, or the kernel's channel to it is busy\n", core::str::from_utf8(&l.name).unwrap_or("?")).as_bytes());
                            crate::process::kist_not_started(&l.capsule, why::READ);
                            crate::process::kist_done();
                        }
                        KistWhy::Stow(i) => {
                            crate::process::push_console(b"kist stow: not started: no vault process, or the kernel's channel to it is busy\n");
                            crate::process::stow_tell(i, crate::process::STOW_REFUSED, crate::process::STOW_WHY_READ);
                        }
                    }
                }
            }
        }
        platform::crumb(0xF2);
        move_seals(&mut store);
        if let Some(job) = kist_job.as_mut() {
            if let Some(r) = kist_step(job) {
                let ms = (cntpct() - job.t0) * 1000 / cntfrq();
                let line = match (&job.why, r) {
                    // Where the launch's time went (2026-11-17): the hashing as the bytes came (one pass, both hashes), then each check after the read, the build last.
                    (KistWhy::Launch(l), Ok(_)) => {
                        let us = |t: u64| t * 1_000_000 / cntfrq();
                        let k = |i: usize| us(crate::process::KIST_TICKS[i].load(core::sync::atomic::Ordering::Relaxed));
                        alloc::format!(
                            "kist {}: started from the vault ({} bytes read in {ms} ms, {} B a call): hashed as it came {} us; after the read the address {} us, the gate {} us, the seal {} us, the build {} us\n",
                            core::str::from_utf8(&l.name).unwrap_or("?"),
                            job.total,
                            crate::process::kernel_get_max(),
                            us(job.hash_ticks),
                            k(crate::process::KT_ADDRESS),
                            k(crate::process::KT_GATE),
                            k(crate::process::KT_SEAL),
                            k(crate::process::KT_BUILD)
                        )
                    }
                    // Kept with its reason for the map, which init told "started" (`kist_not_started`): its circle says why. One that started under the same name meanwhile runs, and nothing is kept.
                    (KistWhy::Launch(l), Err((code, e))) => {
                        if code != crate::process::KIST_ALREADY {
                            crate::process::kist_not_started(&l.capsule, code);
                        }
                        alloc::format!("kist {}: not started: {e}\n", core::str::from_utf8(&l.name).unwrap_or("?"))
                    }
                    // Checked and sealed (SANDBOX.md "Lifecycle"): the seal joins the vault's `sealed` list, which every masked spawn reads; the stower is told once it is kept.
                    (KistWhy::Stow(i), Ok(KistDone::Sealed(seal))) => {
                        let kept = vault(|| {
                            let Some(s) = store.as_mut() else { return false };
                            seal_add(s, &seal).is_ok()
                        });
                        crate::process::stow_tell(i, if kept { crate::process::STOW_DONE } else { crate::process::STOW_REFUSED }, if kept { 0 } else { crate::process::STOW_WHY_KEPT });
                        if kept {
                            alloc::format!("kist stow: checked and sealed to this device ({} bytes, {ms} ms)\n", job.total)
                        } else {
                            alloc::format!("kist stow: checked, but the vault did not keep its seal\n")
                        }
                    }
                    (KistWhy::Stow(_), Ok(KistDone::Launched)) => alloc::string::String::new(),
                    // A stow's native code on a development build: nothing to check or seal (a launch runs it on a development build only, as the bridge's development door's).
                    (KistWhy::Stow(i), Ok(KistDone::Native)) => {
                        crate::process::stow_tell(i, crate::process::STOW_DONE, crate::process::STOW_NATIVE);
                        alloc::format!("kist stow: native code, taken unchecked on this development build ({} bytes)\n", job.total)
                    }
                    (KistWhy::Stow(i), Err((code, e))) => {
                        // A refusal the checker or the seal told already; anything else (the vault, the arc pool) is told here, by the job's reason.
                        if !job.told {
                            let told = if matches!(code, why::ROOM | why::OTHER) { crate::process::STOW_WHY_ROOM } else { crate::process::STOW_WHY_READ };
                            crate::process::stow_tell(i, crate::process::STOW_REFUSED, told);
                        }
                        alloc::format!("kist stow: not stowed: {e}\n")
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
        platform::crumb(0xF3);
        crate::keys::serve();
        if !keys_settled && cntpct() > keys_settle {
            crate::keys::forget_missing();
            keys_settled = true;
        }
        // The killswitch needs IRQs unmasked whenever the kernel isn't deliberately masking them (only the RELOAD handover does): re-assert it every pass, so no path back from a process or a fault can leave it disarmed.
        unsafe { core::arch::asm!("msr daifclr, #2") };
        // The keys as their pins read this pass (chord.rs): either volume key's edge and the power key's press are interrupts, which wake this loop.
        let keys = crate::chord::Keys { power: crate::killswitch::power_held(), vol_up: crate::killswitch::vol_up_held(), vol_down: crate::killswitch::vol_down_held() };
        // The power key turns the screen off and on (owner, 2026-10-22): a preference every process sees at once (process.rs PREF_SCREEN_OFF); the display server darkens the panel. The wake policy (proximity) comes later.
        // A press made with a volume key already down is a chord's (power + volume up restarts, power + volume down stops the app in front), never the screen's.
        if unsafe { core::ptr::read_volatile(&raw const crate::__power_pressed) } != 0 {
            unsafe { core::ptr::write_volatile(&raw mut crate::__power_pressed, 0) };
            let was = crate::process::pref(crate::process::PREF_SCREEN_OFF);
            let off = was ^ 1;
            // The proximity wake (owner's rule): a press lights the screen only if nothing is near the sensor. Fails open: with no proximity reading yet (sensors not up, or the AoC failed), the press wakes as before, so a sensor fault never locks the owner out.
            let refused = off == 0 && crate::process::pref(crate::process::PREF_PROX_WAKE) == 1 && crate::sensorlink::prox_near() == Some(true);
            if !crate::chord::Chords::press_is_screens(keys) {
                crate::flashlog::line("power pressed with a volume key down: a chord's, not the screen's");
            } else if refused {
                crate::flashlog::line("wake refused: proximity near");
            } else {
                crate::process::set_pref(crate::process::PREF_SCREEN_OFF, off);
                chords.toggled(was);
            }
        }
        #[cfg(not(shim_handoff))]
        match chords.step(keys) {
            // Power + volume up: ferros restarts, as the chord leaves Android, with no bootloader (owner, 2026-11-10: "Kill everything and reboot but jump style"; restart.rs).
            crate::chord::Act::Restart => crate::restart::restart(b.stub),
            // Power + volume down: the app in front ended at once, as the deck's Stop ends a kist (owner, 2026-11-18: "power plus vol down kills"; process.rs `stop_front`, WINDOWS.md "Stop"). The press that began it toggled the screen before volume down joined: the screen back as it was.
            crate::chord::Act::Stop { screen_back } => {
                if let Some(was) = screen_back {
                    crate::process::set_pref(crate::process::PREF_SCREEN_OFF, was);
                }
                let line = crate::process::stop_front();
                crate::flashlog::line(&line);
                crate::process::push_console(alloc::format!("{line}\n").as_bytes());
            }
            crate::chord::Act::None => {}
        }
        let now = cntpct();
        // A command from the USB process waiting: no sleep and no time slice before it runs.
        // Not while the last command is still being answered (`proc_restore`: a vault put, a held read of an offer): its USB process ended and a new one sent this one (ptlink.rs `reset`). It waits in the inbox until the last is done, its reply dropped (`ptlink::reply`), so a job never meets another on the kernel's one channel to the vault, and the processes keep their slices meanwhile (the vault process answers the put).
        let inbox_full = crate::ptlink::INBOX_FULL.load(core::sync::atomic::Ordering::Relaxed) && proc_restore.is_none();
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
        platform::crumb(0xF4);
        crate::odpm::poll();
        // The SoC's unused power domains off, once, as the clock policy starts (pd.rs). After a jump at once, on the first pass: the jump is a killswitch, and a domain Android left on may hold a bus master still running Android's work (the GPU, a codec, the camera's chain, the TPU); its power cut stops it dead, through EL3 as every domain switch goes, where its S2MPU could not be touched safely (owner, 2026-11-04: "kill the rails").
        crate::pd::gate_at_boot(now.wrapping_sub(t_boot), if platform::jumped() { 0 } else { 16 * cntfrq() });
        platform::crumb(0xD7);
        clock_policy(false, now.wrapping_sub(t_boot));
        platform::crumb(0xF5);
        // The UFS link into hibernate once it has been idle 16 ms (ferros_hal ufs `h8_enter_if_idle`), under the UFS lock as every command is; every command brings it back.
        if ferros_hal::ufs::H8_ENABLED.load(core::sync::atomic::Ordering::Relaxed) {
            crate::process::UFS.with(|| ferros_hal::ufs::UfsController::new(0x1320_0000).h8_enter_if_idle(cntfrq() / 64));
        }
        // The SoC's heat and the thermal policy, once a second (thermal.rs); the facts' bell after each read.
        platform::crumb(0xF6);
        if crate::thermal::poll() {
            bell_facts();
        }
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
            // Under the vault's lock, as every store access once processes run: the USB and other processes drive the flash from the other cores (unlocked, this write hung the device black on 2026-11-05).
            if let Some(doc) = crate::checklist::span_document() {
                let _ = vault(|| store.as_mut().map(|s| s.set(crate::checklist::SPAN_KEY, &doc).is_ok()));
            }
        }

        // The kernel lock's alarm (a hold of 2^-7 s or more since the last pass) into the flash log: the line that survives a reset (devsched.rs).
        crate::devsched::alarm_lines();
        // A stall the catcher caught since the last pass, into the flash log at once (2026-11-20: the Pro was power-cycled after a hang and its stall record, kept in DRAM, read back as noise). Written here, by the service loop, never by the catching core: the stalled core may hold the storage lock the flash log takes.
        let caught = platform::stalls();
        if caught != stalls_logged {
            stalls_logged = caught;
            for l in stall_lines(freq).lines().filter(|l| l.starts_with("stall (this boot") || l.starts_with("lock: held longest")) {
                crate::flashlog::line(l);
            }
        }
        if now.wrapping_sub(t_heartbeat) > 64 * freq {
            t_heartbeat = now;
            let sl: alloc::vec::Vec<u64> = crate::process::SLICES.iter().skip(4).take(5).map(|s| s.load(core::sync::atomic::Ordering::Relaxed)).collect();
            // The log region (64 MiB) holds months of these.
            // With the retry lines counted since they were last kept (flashlog `once`: the USB link's state, the link readied, the USB process's end), so the count is in the log even while nothing new is said.
            let again: alloc::string::String = crate::flashlog::once_pending().iter().map(|(s, n)| alloc::format!(", retry line {s} again A#{n}")).collect();
            let heap = crate::heap_report().map(|r| alloc::format!(", kernel heap A#{} of A#{} bytes in use (peak A#{})", r.in_use, r.size, r.peak)).unwrap_or_default();
            crate::flashlog::line(&alloc::format!("heartbeat: up {} s, commands from the USB process {}, slices C4-C8 {:?}{again}{heap}", now.wrapping_sub(t_boot) / freq, st.proc_cmds, sl));
        }
        // The kernel heap's peak said in the flash log as it passes each eighth of the heap from five eighths up (2026-11-22, b1131: the first sign of a heap running out was the panic itself): an atomic read a pass, the heap's report only when the peak has moved past the last mark.
        let peak = ferros_heap::PEAK.load(core::sync::atomic::Ordering::Relaxed);
        if peak > heap_mark {
            if let Some(r) = crate::heap_report() {
                let eighth = r.size >> 3;
                if peak >= eighth * 5 {
                    crate::flashlog::line(&alloc::format!("kernel heap: peak A#{peak} of A#{} bytes (past A#{}/8), A#{} in use now, the largest free block A#{}; `diag heap` has the rest", r.size, peak / eighth, r.in_use, r.largest));
                }
                heap_mark = (peak / eighth + 1) * eighth;
            }
        }
        // The GPU's clock follows its service (owner, 2026-11-05: the attest mark within a 120 Hz frame): at its top while a `gpu` process lives, at its floor otherwise; looked at only when the process table changed (process.rs PROCS_GEN), on this core, where every ACPM call is made. The L2 first going up and last going down (GPU.md "The GPU's clock").
        let table = crate::process::PROCS_GEN.load(core::sync::atomic::Ordering::Acquire);
        if table != unsafe { PROCS_SEEN } {
            unsafe { PROCS_SEEN = table };
            // The process count for the facts' bell (the About page's), counted only when the table changed.
            ALIVE.store(crate::process::alive() as u64, core::sync::atomic::Ordering::Relaxed);
            bell_facts();
            let live = crate::process::running(b"gpu");
            // A GPU service that drives its clock by its queue (the `gpu-clock` switch, `gpu_clock`) owns it from its first use; this rule is for a service from before.
            if !GPU_BY_SWITCH.load(core::sync::atomic::Ordering::Relaxed) && live != unsafe { GPU_CLOCKED } {
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
        // The AoC's start, when the `sensors` switch has asked for it and the boot's first seconds are past (the deck first).
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
            // The vault's lock alone (`vault`): a piece of the AoC's firmware is 2 MiB, 43 to 130 ms of reads that held the kernel lock until 2026-11-18, every 64 s on a reloaded kernel where the AoC never comes online (`diag lock`, the camera's 96 a second dying behind them).
            let mut get = |k: &str| vault(|| unsafe { (*st).as_mut().and_then(|s| s.get_by_key(k).ok().flatten()) });
            sensor_boot = sensor_step(core::mem::replace(&mut sensor_boot, SensorBoot::Done), now, &mut get);
        }
        // Give one runnable process a time slice (it comes back at the latest when the slice timer fires), unless a command from the USB process waits: that runs first. `ran` is false when no process could run (all asleep or blocked): then the core sleeps below until the next event-stream tick instead of spinning the pass. With the shell up, processes are always alive, so "alive == 0" never let it sleep: the X3 spun at 2.9 GHz the whole time (owner, 2026-10-23: "the phone is a smidge warm").
        platform::crumb(0xD1);
        // Only when the queue holds a thread this core may run, a sleeper's deadline is due or a device line is pending (`run_slice` wakes their waiters, wherever they run: with every worker busy this core is the one that looks), or no worker is up (every thread is this core's); otherwise `run_slice` would take the lock to find nothing, which every worker's `sev` made it do (2026-11-19).
        let for_here = for_this_core();
        let ran = if !inbox_full && for_here { crate::process::run_slice() } else { false };
        platform::crumb(0xD2);
        // Dark and alone (the screen off, no worker up: every thread runs here and every interrupt comes here, the radio agent's routing): tickless (idle.rs). Woken by the next thread's deadline, the loop's own quarter-second duties (the watchdog's pet, the keys' check, the polls), a power window's sample, the UFS link's hibernate, and any interrupt.
        let dark_alone = !inbox_full && !ran && crate::idle::TICKLESS.load(core::sync::atomic::Ordering::Relaxed) && clock_state() == 3 && crate::smp::UP.load(core::sync::atomic::Ordering::Acquire) == 0;
        if dark_alone {
            let t = cntpct();
            platform::crumb(0xB5);
            if !backstop_ppi {
                crate::killswitch::enable_ppi(26);
                backstop_ppi = true;
            }
            let mut next = crate::process::NEXT_WAKE.load(core::sync::atomic::Ordering::Acquire).min(t + (cntfrq() >> crate::idle::TICK_SHIFT));
            // A due time already behind the counter is one the pass just handled (or could not act on: a link that is down never hibernates), never a reason to wake at once.
            for d in [power_due(), ferros_hal::ufs::UfsController::h8_awake_since().map(|u| u + cntfrq() / 64)].into_iter().flatten() {
                if d > t {
                    next = next.min(d);
                }
            }
            // `wfi` with the EL2 timer at the deadline, IRQs masked from the look to the `wfi` so an interrupt in between ends it at once. No event stream: `wfi` does not wake for it.
            unsafe { core::arch::asm!("msr daifset, #2") };
            let busy = unsafe { read_volatile(&raw const crate::process::__irq_any) } != 0 || crate::process::RUNNABLE_N.load(core::sync::atomic::Ordering::Acquire) != 0;
            if !busy {
                // WHY: the deadline can be behind the counter by now (a thread's, lowered while this pass ran); a wait of zero is still a timer that fires at once, but the subtraction must not wrap.
                let wait = next.saturating_sub(cntpct());
                unsafe { core::arch::asm!("msr cnthp_tval_el2, {t}", "mov {c}, #1", "msr cnthp_ctl_el2, {c}", "isb", "wfi", "msr cnthp_ctl_el2, xzr", "isb", t = in(reg) wait, c = out(reg) _) };
                crate::idle::SHALLOW.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            }
            unsafe { core::arch::asm!("msr daifclr, #2") };
            let slept = cntpct().wrapping_sub(t);
            platform::crumb(0xB6);
            asleep += slept;
            X3_IDLE_TICKS.fetch_add(slept, core::sync::atomic::Ordering::Relaxed);
            X3_SLEEPS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            X3_ASLEEP.fetch_add(slept, core::sync::atomic::Ordering::Relaxed);
        } else if !inbox_full && !ran {
            let t = cntpct();
            platform::crumb(0xB3);
            // A backstop under the event stream: the EL2 timer armed 2 ms out, so a GIC interrupt ends this `wfe` even if the event stream cannot. On battery at the rest clocks the X3 went into this `wfe` and never came out — the last crumb G#B3, its event stream still on (2026-10-25, three times) — as if the core had been taken down while it waited, where only an interrupt brings a core back. Taken at EL2 the tick only stops the timer (.Lirq_tick). The event stream wakes it first every time it can, so the timer is disarmed before it fires; a sleep that ran past 1.5 ms was ended by the backstop, and DIAG counts those.
            if !backstop_ppi {
                crate::killswitch::enable_ppi(26);
                backstop_ppi = true;
            }
            // Armed at the next sleeping thread's deadline when that is sooner (process::NEXT_WAKE), so deadlines land on time however slow the event stream: with the stream every 2.7 ms (bit 15, 15-25 mW under bit 12's 333 us with the screen off, 2026-10-25) a display waiting on its next flip would otherwise wake up to a stream period late.
            // The doze (`DOZE_LOG2`): a wake that brings nothing for this core (another core's `sev` for its own queue) within 2^-n s of the sleep's start goes back to sleep without a pass.
            // Measured 2026-11-19 (KERNEL.md "The service loop's lock holds"): under a Lumis save the eight workers' `sev`s woke this core about 4,800 times a second, and each wake ran a whole pass whose crumbs (a clean to DRAM and a barrier each) took 28 % of the X3, its lock holds 25 % before they were made lock-free.
            // What a pass looks at that no check here sees (a claim, an on-demand server, a launch) waits at most the doze's 2^-9 s, under the event stream's period, so at rest nothing changes: every event-stream wake is past the bound and runs its pass.
            let doze = cntfrq() >> DOZE_LOG2.load(core::sync::atomic::Ordering::Relaxed);
            loop {
                let next = crate::process::NEXT_WAKE.load(core::sync::atomic::Ordering::Acquire);
                let w = cntpct();
                let backstop = next.saturating_sub(w).clamp(cntfrq() / 20_000, cntfrq() / 500);
                unsafe { core::arch::asm!("msr cnthp_tval_el2, {t}", "mov {c}, #1", "msr cnthp_ctl_el2, {c}", "isb", "wfe", "msr cnthp_ctl_el2, xzr", "isb", t = in(reg) backstop, c = out(reg) _) };
                if cntpct().wrapping_sub(w) > cntfrq() / 500 * 3 / 4 {
                    BACKSTOP_WAKES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                }
                if DOZE_LOG2.load(core::sync::atomic::Ordering::Relaxed) == 0 || for_this_core() || crate::ptlink::INBOX_FULL.load(core::sync::atomic::Ordering::Relaxed) || cntpct().wrapping_sub(t) >= doze {
                    break;
                }
                X3_DOZES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            }
            platform::crumb(0xB4);
            let slept = cntpct().wrapping_sub(t);
            asleep += slept;
            X3_IDLE_TICKS.fetch_add(slept, core::sync::atomic::Ordering::Relaxed);
            X3_SLEEPS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            X3_ASLEEP.fetch_add(slept, core::sync::atomic::Ordering::Relaxed);
        }
        // Development: a jumped session that the host has not reached in 60 s (no command from the USB process) leaves a note, once, and runs on.
        // Until 2026-11-13 it also reset, so the next boot could read the pstore ring (the twenty-seventh jump, 2026-11-02: ferros up, no USB, the screen still Android's).
        // That reset was the owner's "works for a minute or two, then reboots" (2026-11-13, the Pro, boot b863): every jump nobody talked to over USB within 60 s ended there, about 57 s after the jump's first flash-log line.
        // And it kept nothing: after a jump the bootloader read the reset as a crash (RST_STAT PIN_RESET, reason G#CECA), ran its debug-dump stage (about 9 s in BL2) and power-cycled (PIN_RESET | PO_RESET), so DRAM decayed and the pstore ring and the report page with it.
        // The flash log on UFS is what survives now, so the note goes there too.
        #[cfg(not(ferros_production))]
        if !escape_noted && platform::from_android() && st.proc_cmds == 0 && cntpct().wrapping_sub(t_boot) > 60 * cntfrq() {
            escape_noted = true;
            crate::flashlog::line(&alloc::format!("jump: no host in 60 s (USB process commands 0); workers up G#{:X}, x3 parked {}; running on, the thread report in the pstore ring", crate::smp::up_mask(), crate::x3::parked()));
            platform::pstore(b"ferros: the jump's session: no host in 60 s, running on\n");
            // Every thread and where it waits, so the log says what the session was stuck on (the twenty-ninth jump: the display server and the attest screen ran, init, touch and USB printed nothing).
            let mut threads = alloc::vec::Vec::new();
            crate::process::threads_report(&mut threads);
            platform::pstore(&threads);
            let w: alloc::vec::Vec<u64> = crate::smp::IDLE_WAKES.iter().map(|w| w.load(core::sync::atomic::Ordering::Relaxed)).collect();
            platform::pstore(alloc::format!("workers: up G#{:X} (bit = core), idle wakes by core {:?}; x3 parked {}\n", crate::smp::up_mask(), w, crate::x3::parked()).as_bytes());
            // Per core, the hand-off words the next cold boot overwrites (the fourth jump of 2026-11-03: the X3 and two workers took their entries and were never heard from): the worker's progress (G#FD60_0B00, low byte the step: 1 arrived, 2 GIC armed, 3 EL0 on, 4-6 loop passes, 8 first slice), its arrival in Rust (0B80), and the hv's two trace words (0C00, 0C80), all in counter ticks.
            for c in 0..9usize {
                let r = |base: usize| unsafe { core::ptr::read_volatile((base + c * 8) as *const u64) };
                platform::pstore(alloc::format!("core {}: step {} at G#{:X}, arrived G#{:X}, hv G#{:X} G#{:X}\n", c, r(0xFD60_0B00) & 0xFF, r(0xFD60_0B00) & !0xFF, r(0xFD60_0B80), r(0xFD60_0C00), r(0xFD60_0C80)).as_bytes());
            }
            // The hand-over blocks the X3 (G#819F_F100) and the workers (G#819F_F200 + (core XOR 4) * G#80) read with their MMU off, beside this core's own registers: the jump freezes Android's CPUs but not its DMA masters, so a block that differs from what was written says a device wrote into ferros's memory (the theory for the 2026-11-03 jumps whose cores died before their first Rust line).
            let (mair, tcr, ttbr, sctlr, hcr): (u64, u64, u64, u64, u64);
            unsafe { core::arch::asm!("mrs {a}, mair_el2", "mrs {b}, tcr_el2", "mrs {c}, ttbr0_el2", "mrs {d}, sctlr_el2", "mrs {e}, hcr_el2", a = out(reg) mair, b = out(reg) tcr, c = out(reg) ttbr, d = out(reg) sctlr, e = out(reg) hcr) };
            platform::pstore(alloc::format!("live: mair G#{:X} tcr G#{:X} ttbr G#{:X} sctlr G#{:X} hcr G#{:X}\n", mair, tcr, ttbr, sctlr, hcr).as_bytes());
            for (name, base) in [("x3", 0x819F_F100usize), ("c4", 0x819F_F200), ("c5", 0x819F_F280), ("c6", 0x819F_F300), ("c7", 0x819F_F380), ("c0", 0x819F_F400), ("c1", 0x819F_F480), ("c2", 0x819F_F500), ("c3", 0x819F_F580)] {
                let r = |o: usize| unsafe { core::ptr::read_volatile((base + o) as *const u64) };
                platform::pstore(alloc::format!("{}: mair G#{:X} tcr G#{:X} ttbr G#{:X} sctlr G#{:X} hcr G#{:X} vbar G#{:X} cptr G#{:X} stack G#{:X} cnthctl G#{:X}\n", name, r(0), r(8), r(16), r(24), r(32), r(40), r(48), r(56), r(88)).as_bytes());
            }
            // The console is not drained into the ring any more: the session runs on, and `diag console` is the host's when it comes.
        }
        // A command the USB process handed over (ptlink.rs, USB.md): the one dispatch below. Its handlers end with `continue`, so what follows — the raw reply handed back — runs at the top of the next pass (`proc_restore`).
        if proc_restore.is_some() {
            continue;
        }
        // The lock only when the inbox says it holds a command (set with it under the lock): every pass took it before 2026-11-19, up to 4,800 a second under a Lumis save.
        if !crate::ptlink::INBOX_FULL.load(core::sync::atomic::Ordering::Relaxed) {
            continue;
        }
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
        // A large command that came while a long command's put still reads its value from the kernel's command region: never copied over it (ptlink `hold_big`), and refused.
        if matches!(proc_cmd.payload, crate::ptlink::Payload::Refused) {
            queue_pt_response(&mut pt_out_data, &held_status(3, "the last command is still at work: ask ferros.pt.held for its end first"));
            continue;
        }
        if let Some(cmd) = ferros_pt::command::parse(payload) {
            st.last_cmd = u32::from_le_bytes([cmd.cap[0], cmd.cap[1], cmd.cap[2], cmd.cap[3]]) as u64 | (cmd.op as u64) << 56;
            platform::crumb(0xD5 | (st.last_cmd & 0xFFFF_FFFF) << 8 | (cmd.op as u64) << 40);
            // The end of a long command is the answer to the host's next command only when that is its held read: a host that moved on to anything else has let it go.
            if cmd.cap != cap_held {
                job_end = None;
            }
            // Production builds (FERROS_PRODUCTION=1) close the development doors over USB, for the locked bootloader (BOOT.md): a cable must not be able to reload the kernel, run code, read or write memory or the vault, read the screen, spawn, or drive a diagnostic, since ABL checks only what it boots. Open: a reboot, and DIAG with no parameters (status, no secrets). The portal will grant the rest to the owner's identity (STATUS.md 7); until then they are closed outright.
            // And a kist offered to stow (`ferros.kist.offer`, docs/USB.md "Offering a kist"): the person checks it on the glass, so it is no door around them.
            // And the end of a long command (`ferros.pt.held`): only the host's own last command's answer, which an offer's part needs.
            let open = !cfg!(ferros_production) || (cmd.cap == cap_reboot && cmd.op == ferros_pt::Op::Exec) || (cmd.cap == cap_diag && cmd.op == ferros_pt::Op::Read && cmd.params.is_empty()) || cmd.cap == cap_offer || cmd.cap == cap_held;
            if !open {
                queue_pt_response(&mut pt_out_data, b"closed: a production build answers only a reboot, a plain DIAG and a kist offered to stow over USB\n");
            } else if cmd.cap == cap_held {
                // The end of the command answered STILL: kept already, or held until the job ends or this read's lease does (the top of the loop answers it), or NOTHING when no command is held.
                if let Some(resp) = job_end.take() {
                    queue_pt_response(&mut pt_out_data, &resp);
                } else if job_detached {
                    held_read = Some(cntpct() + lease);
                } else {
                    queue_pt_response(&mut pt_out_data, &held_status(ferros_pt::command::held::NOTHING, "no command is held"));
                }
            } else if job_detached && (cmd.cap == cap_store || (cmd.cap == cap_offer && cmd.op != ferros_pt::Op::Read)) {
                // The kernel's one channel to the vault is the long command's until it ends: a command that needs it waits for the host to read that end.
                queue_pt_response(&mut pt_out_data, &held_status(3, "the last command is still at work: ask ferros.pt.held for its end first"));
            } else if cmd.cap == cap_offer {
                // A kist offered to stow (docs/KISTS.md "Stowing"): its parts kept in `capsules.`, then offered to the stower, then its state read by the host, held. Every answer but a state is [status][words]: 0 done, 1 nobody present, 2 another offer is waiting, 3 a part or an offer not as the protocol says, 4 no stower.
                // Each refusal on the flash log too, one line: what the host was told is where an offer stopped.
                let say = |status: u8, words: &str| {
                    crate::flashlog::line(&alloc::format!("kist offer refused (A#{status}): {words}"));
                    let mut r = alloc::vec![status];
                    r.extend_from_slice(words.as_bytes());
                    r
                };
                let person = crate::kvault::PERSON.load(core::sync::atomic::Ordering::Acquire);
                // WHY each length check: the parameters are the host's (external input).
                let capsule: Option<[u8; 32]> = cmd.params.get(..32).map(|c| c.try_into().unwrap());
                match (cmd.op, capsule) {
                    (_, None) => queue_pt_response(&mut pt_out_data, &say(3, "the capsule's 32-byte address comes first")),
                    // Its state, held until it changes or the lease ends (the top of the loop answers it).
                    (ferros_pt::Op::Read, Some(c)) => {
                        let seen = cmd.params.get(32).copied().unwrap_or(u8::MAX);
                        let s = crate::process::offer_state(&c);
                        if s[0] != seen || ferros_kist::offer::finished(s[0]) {
                            queue_pt_response(&mut pt_out_data, &s);
                        } else {
                            offer_hold = Some((c, seen, cntpct() + ferros_kist::offer::HOLD_SECONDS * freq));
                        }
                    }
                    _ if !person => queue_pt_response(&mut pt_out_data, &say(1, "nobody is present: attest on the device first")),
                    _ if crate::process::offer_open() => queue_pt_response(&mut pt_out_data, &say(2, "another kist is offered and waiting on the person")),
                    (ferros_pt::Op::Write, Some(c)) => {
                        let (offset, more) = match (cmd.params.get(32..40), cmd.params.get(40)) {
                            (Some(o), Some(&m)) if cmd.params.len() > 41 => (u64::from_le_bytes(o.try_into().unwrap()), m != 0),
                            _ => {
                                queue_pt_response(&mut pt_out_data, &say(3, "a part is [capsule 32][offset u64][more u8][bytes]"));
                                continue;
                            }
                        };
                        // In order, from the first part: a part at 0 starts the capsule afresh (a host starting again).
                        let next = match offer_rx {
                            Some((rc, n, false)) if rc == c => n,
                            _ => 0,
                        };
                        if offset != 0 && offset != next {
                            queue_pt_response(&mut pt_out_data, &say(3, "parts come in order, from the first"));
                        } else {
                            offer_rx = Some((c, offset + (cmd.params.len() - 41) as u64, !more));
                            offer_part_at = cntpct();
                            if offset == 0 {
                                crate::flashlog::line(&alloc::format!("kist offer: a capsule carried in over USB, its first part come (G#{:02X}{:02X}{:02X}{:02X}...)", c[0], c[1], c[2], c[3]));
                            }
                            // The kernel's channel to the vault is one call at a time: a kist job reading a capsule holds it, so the part waits for it (its reply with it).
                            job_due = cntpct() + lease;
                            if kist_job.is_some() {
                                offer_part_waiting = true;
                            } else {
                                match offer_part(cmd.params) {
                                    Ok(job) => {
                                        vault_job = Some(job);
                                        vault_for_offer = true;
                                    }
                                    Err(resp) => {
                                        offer_rx = None;
                                        queue_pt_response(&mut pt_out_data, &resp);
                                    }
                                }
                            }
                        }
                    }
                    (ferros_pt::Op::Exec, Some(c)) => {
                        let len = cmd.params.get(32..40).map(|l| u64::from_le_bytes(l.try_into().unwrap()));
                        if len.is_none() || offer_rx != Some((c, len.unwrap(), true)) {
                            queue_pt_response(&mut pt_out_data, &say(3, "not carried in whole: every part first, in order, the last with more 0"));
                        } else if !crate::process::running(b"kists") {
                            queue_pt_response(&mut pt_out_data, &say(4, "the stower is not running"));
                        } else {
                            offer_rx = None;
                            match crate::process::offer_put(c, len.unwrap()) {
                                Ok(()) => {
                                    crate::flashlog::line(&alloc::format!("kist offered over USB to stow: A#{} bytes", len.unwrap()));
                                    queue_pt_response(&mut pt_out_data, &[0]);
                                }
                                Err(e) => queue_pt_response(&mut pt_out_data, &say(2, e)),
                            }
                        }
                    }
                }
            } else if cmd.cap == cap_diag && cmd.op == ferros_pt::Op::Read {
                let mut resp: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
                // Development tools: not in a production build at all (FERROS_PRODUCTION=1, build.rs), which answers only the plain DIAG below and a reboot.
                #[cfg(not(ferros_production))]
                {
                if cmd.params.starts_with(b"lun ") {
                    // Hex dump of UFS blocks: `diag lun <lun> <first lba> <count>`, non-zero 16-byte rows only (LU3: devinfo LBA 6, pinfo 8, blenv 10; LU0 partitions at the LBAs `diag ufsdesc` prints) — for finding where the bootloader keeps the active slot besides devinfo (2026-10-17). devinfo carries the device's identity (serials, IMEIs, MACs): don't paste it anywhere.
                    let mut it = cmd.params[4..].split(|&c| c == b' ').filter_map(|t| core::str::from_utf8(t).ok()?.parse::<u32>().ok());
                    let (lun, first, count) = (it.next().unwrap_or(3).min(7) as u8, it.next().unwrap_or(6), it.next().unwrap_or(1).min(8));
                    // Under the UFS lock (process.rs `UFS`), as every UFS command: processes on the other cores drive the flash too (the vault process's blocks, the partitions' readers).
                    crate::process::UFS.with(|| {
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
                    // Under the UFS lock (process.rs `UFS`), as every UFS command: processes on the other cores drive the flash too (the vault process's blocks, the partitions' readers).
                    crate::process::UFS.with(|| {
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
                    // Only with a person present (owner, 2026-11-21: Android starts only when the attested identity chose it). These devices unlock at boot, so the development door still works for whoever holds them; refused before anything is answered or written.
                    if !crate::kvault::PERSON.load(core::sync::atomic::Ordering::Acquire) {
                        queue_pt_response(&mut pt_out_data, b"android: refused: no one has attested\n");
                        continue;
                    }
                    // From the USB process: reply first, and reset when it asks again with its reply sent (ptlink.rs).
                    if from_proc && !pt_act {
                        queue_pt_response(&mut pt_out_data, b"android: resetting once this reply is out\n");
                        proc_act = true;
                        continue;
                    }
                    // The gate's Android path from the host, for bisecting it: slot a active in devinfo, then the plain PSCI reset. The bootloader boots the active slot. `android here` leaves the slot as it is, so the gate of the slot that booted answers, as volume down there does (how Android from slot b is tested, 2026-10-28).
                    // Android's own recovery request in misc is the person's to answer first, as the service loop's launch waits for it (owner, 2026-11-22): never cleared on the way past. A request there: no reset, the reply says how to answer.
                    let m = match crate::slot::misc_request() {
                        Ok((ferros_hal::bcb::ASK_NONE, _)) => crate::slot::ALREADY,
                        Ok((ask, _)) => {
                            recovery_ask_set(ask);
                            resp.extend_from_slice(alloc::format!("misc asks for {}: answer it first (the Android page, or `diag recovery clear` for no)\n", recovery_ask_words(ask)).as_bytes());
                            crate::slot::NOT_ACTIVE
                        }
                        Err(e) => e,
                    };
                    let r = if m != crate::slot::ALREADY { m } else if cmd.params == b"android here" { crate::slot::MARKED } else { crate::slot::set_active(0) };
                    // Tell slot a's boot gate (ferros_hv/src/gate.rs) to answer "Android" this once: only when resetting, so no word waits for a later boot.
                    if r == crate::slot::MARKED {
                        crate::platform::android_once();
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
                // `diag "start into ferros"` / `"start into android"`: what this device starts into (settings.rs `START_ANDROID`), as the Settings page's op 13 sets it, kept in the device document. Android only while a person is present on a device that allows Android; ferros always.
                if cmd.params == b"start into ferros" || cmd.params == b"start into android" {
                    let on = cmd.params == b"start into android";
                    let ok = !on || (crate::kvault::PERSON.load(core::sync::atomic::Ordering::Acquire) && ANDROID_ALLOWED.load(core::sync::atomic::Ordering::Relaxed) == 2);
                    let kept = ok && {
                        crate::settings::START_ANDROID.store(on, core::sync::atomic::Ordering::Relaxed);
                        keep_settings_now()
                    };
                    let say = alloc::format!("start into: {}\n", match (ok, kept) { (false, _) => "refused (Android needs a person present on a device that allows it)", (true, true) => if on { "Android, kept" } else { "ferros, kept" }, (true, false) => "set, but the document was NOT kept" });
                    crate::flashlog::line(say.trim_end());
                    queue_pt_response(&mut pt_out_data, say.as_bytes());
                    continue;
                }
                // `diag recovery clear`: Android's recovery request in misc answered "no" (owner, 2026-11-02 and 2026-11-21): the bootloader message's command and arguments zeroed, nothing else in misc (slot.rs `misc_clear_recovery`). Only with a person present, as the launch that does the same.
                #[cfg(not(shim_handoff))]
                if cmd.params == b"recovery clear" {
                    let r = if crate::kvault::PERSON.load(core::sync::atomic::Ordering::Acquire) { Some(crate::slot::misc_clear_recovery()) } else { None };
                    if matches!(r, Some(crate::slot::MARKED | crate::slot::ALREADY)) {
                        recovery_ask_set(ferros_hal::bcb::ASK_NONE);
                    }
                    crate::flashlog::line(&match r {
                        Some(crate::slot::MARKED) => alloc::string::String::from("recovery clear: Android's recovery request in misc cleared (the person present said no)"),
                        Some(crate::slot::ALREADY) => alloc::string::String::from("recovery clear: misc held no recovery request"),
                        Some(e) => alloc::format!("recovery clear: misc not cleared (G#{e:X})"),
                        None => alloc::string::String::from("recovery clear: refused: no one has attested"),
                    });
                    let say = match r {
                        Some(crate::slot::MARKED) => alloc::string::String::from("recovery clear: cleared\n"),
                        Some(crate::slot::ALREADY) => alloc::string::String::from("recovery clear: nothing to clear\n"),
                        Some(e) => alloc::format!("recovery clear: failed (G#{e:X})\n"),
                        None => alloc::string::String::from("recovery clear: refused: no one has attested\n"),
                    };
                    queue_pt_response(&mut pt_out_data, say.as_bytes());
                    continue;
                }
                // `diag recovery ask`: Android's recovery request as the kernel holds it (the question the Android page shows) and as misc holds it now, its arguments as text. Read only.
                #[cfg(not(shim_handoff))]
                if cmd.params == b"recovery ask" {
                    let held = RECOVERY_ASK.load(core::sync::atomic::Ordering::Acquire);
                    resp.extend_from_slice(alloc::format!("held: {} (A#{held}), the person may allow it alone: {}\n", recovery_ask_words(held), if recovery_allowable(held) { "yes" } else { "no" }).as_bytes());
                    match crate::slot::misc_request() {
                        Ok((ask, args)) => {
                            let args: alloc::string::String = args.iter().map(|&c| if (0x20..0x7F).contains(&c) { c as char } else { '|' }).collect();
                            resp.extend_from_slice(alloc::format!("misc now: {} (A#{ask}), arguments \"{args}\"\n", recovery_ask_words(ask)).as_bytes());
                        }
                        Err(e) => resp.extend_from_slice(alloc::format!("misc not read (G#{e:X})\n").as_bytes()),
                    }
                    resp.extend_from_slice(alloc::format!("pKVM signs from the bootloader this boot: {}\n", unsafe { core::str::from_utf8(&*(&raw const crate::modem::KVM_SIGNS)).unwrap_or("(not text)") }).as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                // `diag recovery test`: a harmless recovery request written into misc from ferros's side, for testing the question on a device (development builds, a person present): the command "boot-recovery" with the arguments "recovery\n--just_exit\n", which asks recovery to do nothing and is classed as one the person cannot allow alone (only no). Never a wipe. Held at once, as a boot would find it.
                // `diag recovery pretend <A#1..4>`: the kernel's held request set to that class in RAM alone (misc untouched), to see the Android page's question for a wipe or a rescue without one in misc: a yes then reads misc again, finds nothing like it, and resets nothing.
                #[cfg(all(not(shim_handoff), not(ferros_production)))]
                if cmd.params == b"recovery test" || cmd.params.starts_with(b"recovery pretend ") {
                    let say = if !crate::kvault::PERSON.load(core::sync::atomic::Ordering::Acquire) {
                        alloc::string::String::from("refused: no one has attested")
                    } else if cmd.params == b"recovery test" {
                        match crate::slot::misc_write_request(b"recovery\n--just_exit\n") {
                            crate::slot::MARKED => {
                                if let Ok((ask, _)) = crate::slot::misc_request() {
                                    recovery_ask_set(ask);
                                }
                                alloc::format!("written; held: {}", recovery_ask_words(RECOVERY_ASK.load(core::sync::atomic::Ordering::Acquire)))
                            }
                            e => alloc::format!("not written (G#{e:X})"),
                        }
                    } else if !matches!(crate::slot::misc_request(), Ok((ferros_hal::bcb::ASK_NONE, _))) {
                        // Pretend only over an empty misc: over a real request a pretended wipe's yes would find misc asking it and really wipe.
                        alloc::string::String::from("refused: misc holds a request (or was not read); pretend is for an empty misc")
                    } else {
                        match &cmd.params[17..] {
                            b"1" | b"2" | b"3" | b"4" => {
                                let ask = cmd.params[17] - b'0';
                                recovery_ask_set(ask);
                                alloc::format!("held (RAM only, misc untouched): {}", recovery_ask_words(ask))
                            }
                            _ => alloc::string::String::from("pretend takes A#1 (wipe), 2 (rescue), 3 (remote), 4 (changes what boots)"),
                        }
                    };
                    crate::flashlog::line(&alloc::format!("diag {}: {say}", core::str::from_utf8(cmd.params).unwrap_or("recovery")));
                    resp.extend_from_slice(say.as_bytes());
                    resp.push(b'\n');
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"recovery" {
                    // Only with a person present: a wipe or a rescue of Android's data needs an attest (owner, 2026-11-02, android_behind_attestation); recovery is Android's, which starts only by the attested identity's hand (2026-11-21).
                    if !crate::kvault::PERSON.load(core::sync::atomic::Ordering::Acquire) {
                        queue_pt_response(&mut pt_out_data, b"recovery: refused: no one has attested\n");
                        continue;
                    }
                    // `diag recovery`: Android's recovery, once. The gate refuses a recovery boot unless this word is there (ferros_hv gate.rs RECOVERY_ONCE: no reflash without the handle, owner 2026-11-01); ferros leaves it, cleaned to DRAM, then resets into the bootloader's recovery mode. A development door for now; the attest-gated setting comes with the UI.
                    if from_proc && !pt_act {
                        queue_pt_response(&mut pt_out_data, b"recovery: resetting into recovery once this reply is out\n");
                        proc_act = true;
                        continue;
                    }
                    let r = recovery_once_reset();
                    resp.extend_from_slice(alloc::format!("recovery: the EL3 monitor refused the reboot mode ({r})\n").as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"devinfo" {
                    // The bootloader's slot record as it stands (slot.rs): both words, retries byte and flags byte (b0 unbootable, b1 successful, b2 active, b3 fastboot-ok).
                    // Under the UFS lock (process.rs `UFS`), as every UFS command: processes on the other cores drive the flash too (the vault process's blocks, the partitions' readers).
                    crate::process::UFS.with(|| {
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
                    // Under the UFS lock (process.rs `UFS`), as every UFS command: processes on the other cores drive the flash too (the vault process's blocks, the partitions' readers).
                    crate::process::UFS.with(|| {
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
                if cmd.params == b"heap" {
                    // `diag heap`: the kernel heap in full (main.rs `heap_text`): its regions, in use and peak, free bytes and blocks and the largest, the free blocks by size, and the last refused allocation (its size, alignment, caller and the heap then).
                    resp.extend_from_slice(crate::heap_text().as_bytes());
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
                    // `diag key stop`: what power + volume down does, the stop of the app in front (process.rs `stop_front`), for testing the stop without hands: the chord itself is two pins' levels and cannot be injected, so this runs its action, not its keys.
                    if cmd.params == b"key power" {
                        if crate::killswitch::volume_key_down() {
                            resp.extend_from_slice(b"key power refused: a volume key is held, and a press now would be a chord\n");
                        } else {
                            unsafe { core::ptr::write_volatile(&raw mut crate::__power_pressed, 1) };
                            resp.extend_from_slice(b"key power pressed\n");
                        }
                    } else if cmd.params == b"key stop" {
                        let line = crate::process::stop_front();
                        crate::flashlog::line(&line);
                        resp.extend_from_slice(line.as_bytes());
                        resp.push(b'\n');
                    } else {
                        resp.extend_from_slice(b"key: `key power`, or `key stop` (the stop chord's action); the volume keys are read as levels (the killswitch, the gate, the chords) and nothing queues their presses\n");
                    }
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"console kept" || cmd.params.starts_with(b"console kept ") {
                    // `diag "console kept [KiB]"`: the console's last KiB (default 256, at most 256: a reply's room, and the whole ring, ferros_console RING) as kept, drained or not, without draining it (process.rs `console_kept`).
                    let kib = core::str::from_utf8(&cmd.params[12..]).ok().and_then(|t| t.trim().parse::<usize>().ok()).unwrap_or(256).clamp(1, 256);
                    resp.extend_from_slice(&crate::process::console_kept(kib * 1024));
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"console" {
                    // What processes have sent their consoles since the last read (background slices included), after a line for each writer whose text the ring overwrote before this read (ferros_console), and how many are alive.
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
                        // Under the UFS lock (process.rs `UFS`), as every UFS command: processes on the other cores drive the flash too (the vault process's blocks, the partitions' readers).
                        crate::process::UFS.with(|| {
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
                if cmd.params == b"clock" || cmd.params.starts_with(b"clock ") {
                    // `diag clock`: the adaptive clock (clock_diag).
                    clock_diag(&cmd.params, &mut resp);
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
                                crate::thermal::note_asked(d, khz);
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
                    // At least 20, so the step policy's release point (10 under) stays above zero.
                    crate::thermal::AT.store(c.max(20), core::sync::atomic::Ordering::Relaxed);
                    resp.extend_from_slice(alloc::format!("step: cap at A#{} C, release below A#{} C; smooth: every trip at most A#{} C (takes effect within a second)\n", c.max(20), c.max(20) - 10, c.max(20)).as_bytes());
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
                if cmd.params.starts_with(b"thermal") && !cmd.params.starts_with(b"thermal at ") {
                    // `diag thermal`: every TMU zone (now, peak, failed reads), the hottest named, the kernel's core's clock against the firmware's rate, the slow seconds, the clock dividers, the policy and its caps (thermal.rs `report`). `thermal init` sends TMU_IPC_INIT first (Linux does once at probe); `thermal hw` the dividers' configuration, the PMIC's warning levels and the firmware governor's targets and trip counts, read only; `thermal reset` starts the peaks and counts again; `thermal policy step|smooth|off` (docs/THERMAL.md).
                    match &cmd.params[7..] {
                        b" init" => resp.extend_from_slice(alloc::format!("init: {:?}\n", crate::acpm::tmu_init().map(|c| alloc::format!("caps G#{c:X}")).map_err(|e| alloc::format!("G#{e:X}"))).as_bytes()),
                        b" hw" => crate::thermal::report_hw(&mut resp),
                        b" reset" => crate::thermal::reset(),
                        b" policy step" => crate::thermal::set_policy(crate::thermal::STEP),
                        b" policy smooth" => crate::thermal::set_policy(crate::thermal::SMOOTH),
                        b" policy off" => crate::thermal::set_policy(crate::thermal::OFF),
                        b"" => {}
                        t if t.starts_with(b" trace") => {
                            // `diag thermal trace [n]`: the last n seconds of the hardware's clocks and throttles (thermal.rs `report_trace`), then nothing else.
                            let n = core::str::from_utf8(&t[6..]).ok().and_then(|s| s.trim().parse::<usize>().ok()).unwrap_or(usize::MAX);
                            crate::thermal::report_trace(&mut resp, n);
                            queue_pt_response(&mut pt_out_data, &resp);
                            continue;
                        }
                        _ => resp.extend_from_slice(b"thermal [init | hw | reset | trace [n] | policy step|smooth|off | at <C>]\n"),
                    }
                    crate::thermal::report(&mut resp);
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
                // `diag devinfo kv <BLAKE3 of the block now, 64 hex> <the block wanted, 8192 hex>`: devinfo's key-value block (LBA 7) put back, only while it still hashes to what the host read (slot.rs set_kv_block). For the bootloader's EIO record (avb.managed_verity_mode) after a failed Android disk check; never the identity block.
                #[cfg(not(shim_handoff))]
                if let Some(rest) = cmd.params.strip_prefix(b"devinfo kv ") {
                    let hex = |s: &[u8]| -> Option<alloc::vec::Vec<u8>> {
                        if s.len() % 2 != 0 {
                            return None;
                        }
                        s.chunks(2).map(|p| u8::from_str_radix(core::str::from_utf8(p).ok()?, 16).ok()).collect()
                    };
                    let mut it = rest.split(|&c| c == b' ').filter(|w| !w.is_empty());
                    let now = it.next().and_then(hex).and_then(|v| <[u8; 32]>::try_from(v.as_slice()).ok());
                    let want = it.next().and_then(hex);
                    match (now, want) {
                        (Some(now), Some(want)) => resp.extend_from_slice(alloc::format!("devinfo kv: G#{:X}\n", crate::slot::set_kv_block(&now, &want)).as_bytes()),
                        _ => resp.extend_from_slice(b"devinfo kv <blake3 now, 64 hex> <block wanted, 8192 hex>\n"),
                    }
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
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
                            (Some(s), Some(r), Some(f)) => resp.extend_from_slice(alloc::format!("set_word: G#{:X}\n", crate::slot::set_word(s, r, f)).as_bytes()),
                            _ => resp.extend_from_slice(b"slot word <a|b> <retries hex> <flags hex>\n"),
                        }
                    }
                    match crate::slot::words() {
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
                if cmd.params == b"keys" || cmd.params == b"keys sleep" || cmd.params == b"keys time" || cmd.params == b"keys flip" || cmd.params == b"keys idtime" {
                    // `diag keys [sleep|time|flip|idtime]`: where the ira and wairua are held, the self-test and check, a power-down rehearsal, the cost of a use, a flipped bit caught and repaired, the identity's held-key paths timed against the software ones with test values (keys.rs).
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
                if cmd.params == b"crumbs ring" {
                    // `diag "crumbs ring"`: the kernel's core's last long stretches in time order (platform.rs `CRUMB_RING`): when each ended (us on the counter), us, from which crumb to which.
                    let f = cntfrq();
                    let n = platform::CRUMB_RING_N.load(core::sync::atomic::Ordering::Relaxed) as usize;
                    resp.extend_from_slice(alloc::format!("crumb stretches of 2^7 us or more (not D1-D2, B3-B4, B5-B6): A#{n}, the last A#{} (end us on the counter:us:from-to):", n.min(platform::CRUMB_RING)).as_bytes());
                    for k in n.saturating_sub(platform::CRUMB_RING)..n {
                        let e = &platform::CRUMB_RING_E[k & (platform::CRUMB_RING - 1)];
                        let (t, w) = (e[0].load(core::sync::atomic::Ordering::Relaxed), e[1].load(core::sync::atomic::Ordering::Relaxed));
                        if (k & 7) == 0 {
                            resp.push(b'\n');
                        }
                        resp.extend_from_slice(alloc::format!(" {}:{}:{:02X}-{:02X}", t * 1_000_000 / f, (w & ((1 << 48) - 1)) * 1_000_000 / f, w >> 56, w >> 48 & 0xFF).as_bytes());
                    }
                    resp.push(b'\n');
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"crumbs" || cmd.params == b"crumbs reset" {
                    // `diag crumbs`: the kernel's core's longest stretch ending at each service-loop crumb (platform.rs `CRUMB_GAP`): us, the crumb it began at, when it ended (us on the counter), and how many were 2^9 us or longer. `diag "crumbs reset"` zeroes them first.
                    let f = cntfrq();
                    if cmd.params == b"crumbs reset" {
                        platform::CRUMB_RING_N.store(0, core::sync::atomic::Ordering::Relaxed);
                    }
                    for (k, e) in platform::CRUMB_GAP.iter().enumerate() {
                        if cmd.params == b"crumbs reset" {
                            for w in e.iter() {
                                w.store(0, core::sync::atomic::Ordering::Relaxed);
                            }
                            continue;
                        }
                        let w: [u64; 4] = core::array::from_fn(|j| e[j].load(core::sync::atomic::Ordering::Relaxed));
                        if w[0] != 0 {
                            resp.extend_from_slice(alloc::format!("crumb G#{k:02X}: longest A#{} us from G#{:02X}, ended at A#{} us; A#{} of 2^9 us or more\n", w[0] * 1_000_000 / f, w[1], w[2] * 1_000_000 / f, w[3]).as_bytes());
                        }
                    }
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params.starts_with(b"irqs runs") || cmd.params.starts_with(b"irqs freeze ") {
                    // `diag "irqs runs"`: the armed actions' last runs, each with what its operations read (irqarm.rs `runs_report`); `diag "irqs runs reset"` clears them; `diag "irqs freeze <us>"` keeps the runs around the first gap between acted runs longer than that (0 off).
                    if cmd.params == b"irqs runs reset" {
                        crate::irqarm::runs_reset_all();
                    }
                    if let Some(n) = cmd.params.strip_prefix(b"irqs freeze short ").and_then(|d| core::str::from_utf8(d).ok()).and_then(|d| d.trim().parse::<u64>().ok()) {
                        crate::irqarm::FREEZE_SHORT_US.store(n, core::sync::atomic::Ordering::Relaxed);
                    } else if let Some(n) = cmd.params.strip_prefix(b"irqs freeze ").and_then(|d| core::str::from_utf8(d).ok()).and_then(|d| d.trim().parse::<u64>().ok()) {
                        crate::irqarm::FREEZE_US.store(n, core::sync::atomic::Ordering::Relaxed);
                    }
                    crate::irqarm::runs_report(&mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"irqs" || cmd.params.starts_with(b"irqs storm ") {
                    // `diag irqs`: the interrupt storm guard's measure (process.rs `irqs_report`): each granted line's unmasks, those that found it still asserted, the longest run of those, whether it stormed.
                    // `diag "irqs storm N"`: the run that counts as a storm, until the next boot (for measuring traffic with the guard out of the way, and for testing it).
                    if let Some(n) = cmd.params.strip_prefix(b"irqs storm ").and_then(|d| core::str::from_utf8(d).ok()).and_then(|d| d.trim().parse::<u32>().ok()) {
                        crate::process::IRQ_STORM_RUN.store(n, core::sync::atomic::Ordering::Relaxed);
                    }
                    crate::process::irqs_report(&mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"pets" {
                    // `diag pets`: the process watchdog's measurements (process.rs pets_report): each process's longest stretch between pets.
                    crate::process::pets_report(&mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"lock" || cmd.params.starts_with(b"lock ") {
                    // `diag lock`: every hold of the kernel lock by length, by syscall and by site, the long holds with what held them (devsched.rs). `diag "lock reset"` zeroes the counts first; `diag "lock long <us>"` sets the long mark.
                    let arg = cmd.params.strip_prefix(b"lock").unwrap_or(b"").trim_ascii();
                    if arg == b"reset" {
                        crate::devsched::lock_reset();
                    }
                    if let Some(us) = arg.strip_prefix(b"long ").and_then(|d| core::str::from_utf8(d).ok()).and_then(|d| d.trim().parse::<u64>().ok()) {
                        // WHY the max: 0 is the unset mark (devsched `hold` puts its default there), so 0 us asks for every hold.
                        crate::devsched::LONG_TICKS.store((us * cntfrq() / 1_000_000).max(1), core::sync::atomic::Ordering::Relaxed);
                    }
                    crate::devsched::lock_report(&mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"dev" || cmd.params.starts_with(b"dev ") {
                    // `diag dev`: device threads (devsched.rs): the switches and each granted line's interrupt to its waiter's run. `diag "dev reset"`, `diag "dev first on|off"`, `diag "dev follow on|off"`, `diag "dev core <k>|off"`, `diag "dev freeze <intid> <us>"` (the scheduler trace stops at the first run of that line's waiter later than that).
                    use core::sync::atomic::Ordering::Relaxed;
                    let arg = cmd.params.strip_prefix(b"dev").unwrap_or(b"").trim_ascii();
                    let num = |s: &[u8]| core::str::from_utf8(s).ok().and_then(|d| d.trim().parse::<u64>().ok());
                    match arg {
                        b"reset" => crate::devsched::lat_reset(),
                        b"first on" => crate::devsched::FIRST.store(true, Relaxed),
                        b"first off" => crate::devsched::FIRST.store(false, Relaxed),
                        b"follow on" => crate::devsched::FOLLOW.store(true, Relaxed),
                        b"follow off" => crate::devsched::FOLLOW.store(false, Relaxed),
                        b"core off" => crate::devsched::CORE.store(0, Relaxed),
                        _ => {
                            if let Some(k) = arg.strip_prefix(b"core ").and_then(num) {
                                // WHY: a core index the mask's sixteen bits cannot name is refused, not shifted out.
                                if k < 16 {
                                    crate::devsched::CORE.store(1 << k, Relaxed);
                                }
                            } else if let Some(rest) = arg.strip_prefix(b"freeze ") {
                                let mut it = rest.split(|&c| c == b' ').filter(|s| !s.is_empty());
                                if let (Some(i), Some(us)) = (it.next().and_then(num), it.next().and_then(num)) {
                                    crate::devsched::FREEZE_TICKS.store(us * cntfrq() / 1_000_000, Relaxed);
                                    crate::devsched::FREEZE_INTID.store(i as usize, Relaxed);
                                    crate::devsched::FROZEN.store(false, Relaxed);
                                }
                            }
                        }
                    }
                    let names: alloc::vec::Vec<(&str, u32)> = crate::process::IRQS.iter().map(|l| (l.name, l.intid)).collect();
                    crate::devsched::lat_report(&mut resp, &names);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"sched old on" || cmd.params == b"sched old off" {
                    // `diag "sched old on|off"`: the scheduler's behaviour before 2026-11-17 back, or not (process.rs `OLD`, KERNEL.md Order item 8), to measure both on one boot.
                    let on = cmd.params == b"sched old on";
                    crate::process::OLD.store(on, core::sync::atomic::Ordering::Relaxed);
                    resp.extend_from_slice(if on { b"sched: as before 2026-11-17\n" } else { b"sched: as built 2026-11-17\n" });
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"sched x3 on" || cmd.params == b"sched x3 off" {
                    // `diag "sched x3 on|off"`: unpinned threads may also run on the kernel's core while it is free (process.rs `X3_SHARE`).
                    let on = cmd.params == b"sched x3 on";
                    crate::process::X3_SHARE.store(on, core::sync::atomic::Ordering::Relaxed);
                    resp.extend_from_slice(if on { b"sched: unpinned threads may run on the kernel's core\n" } else { b"sched: unpinned threads on the workers only\n" });
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if let Some(n) = cmd.params.strip_prefix(b"sched slice ") {
                    // `diag "sched slice <n>"`: the slice timer at 2^-n s, 0 for the default 10 ms (process.rs `SLICE_LOG2`). WHY the range: the number is the bridge's; a slice longer than 2^-6 s (15.6 ms, past the default) or shorter than 2^-12 s (244 us, a few switches' worth) is refused, not clamped.
                    match core::str::from_utf8(n).ok().and_then(|s| s.trim().parse::<u32>().ok()) {
                        Some(n) if n == 0 || (6..=12).contains(&n) => {
                            crate::process::SLICE_LOG2.store(n, core::sync::atomic::Ordering::Relaxed);
                            resp.extend_from_slice(alloc::format!("sched: slice {}\n", if n == 0 { alloc::string::String::from("10 ms (the default)") } else { alloc::format!("2^-{n} s") }).as_bytes());
                        }
                        _ => resp.extend_from_slice(b"sched slice <n>: 0 (10 ms) or 6 to 12 (2^-n s)\n"),
                    }
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"sched" || cmd.params == b"sched reset" || cmd.params == b"sched all" {
                    // `diag sched`: the scheduler's counts (wake to run per kind, the lock's wait per core, register syscalls by path, yields, cores asked to make way) and its trace (process.rs): who called whom, who waited, for how long. `diag "sched reset"` zeroes the counts first; `diag "sched all"` lists the whole ring, not its last 512 events.
                    if cmd.params == b"sched reset" {
                        crate::process::sched_reset();
                    }
                    crate::process::sched_report(&mut resp, cmd.params == b"sched all");
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
                        // The number base: this device's own (base.rs), and whether it follows the identity's.
                        ("base", Some(v)) if (0..=255).contains(&v) && crate::base::valid(v as u64) => crate::process::with_lock(|| crate::base::set_device(v as u8, || true)),
                        ("base_follow", Some(v @ (0 | 1))) => crate::process::with_lock(|| crate::base::set_own(v == 0, || true)),
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
                    let kept = ok && keep_settings_now();
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
                // `diag notify`: the notes waiting on the person and the kists alive (docs/NOTIFY.md). `diag "notify <live|personal|status> <kist> [words]"`: a note from the system on that kist's behalf, as the system decides it (a personal one names a stand-in person), so each kind can be seen on the device with no kist that posts (NOTIFY.md "Seeing it"); `<kist>` a live kist's name or its capsule's address in hex (whole, or its first bytes as `diag notify` shows them). `diag "notify clear <kist>"`: its notes gone, as if the person had looked.
                if cmd.params == b"notify" || cmd.params.starts_with(b"notify ") {
                    let mut words = cmd.params.split(|&b| b == b' ').filter(|w| !w.is_empty()).skip(1);
                    let (what, who) = (words.next(), words.next());
                    let rest: alloc::vec::Vec<u8> = words.collect::<alloc::vec::Vec<_>>().join(&b' ');
                    let kind = match what {
                        Some(b"status") => Some(ferros_kist::note::STATUS),
                        Some(b"personal") => Some(ferros_kist::note::PERSONAL),
                        Some(b"live") => Some(ferros_kist::note::LIVE),
                        _ => None,
                    };
                    let source = who.and_then(crate::process::note_kist_named);
                    match (what, kind, source) {
                        (None, _, _) => resp.extend_from_slice(crate::process::notes_report().as_bytes()),
                        (Some(b"clear"), _, Some(s)) => resp.extend_from_slice(alloc::format!("notify: A#{} notes cleared\n", crate::process::notes_clear(s)).as_bytes()),
                        (_, Some(kind), Some(s)) => {
                            let text = if rest.is_empty() { alloc::format!("a {} note", crate::process::note_kind_name(kind)) } else { alloc::string::String::from_utf8_lossy(&rest).into_owned() };
                            let r = crate::process::note_post_system(s, kind, kind != ferros_kist::note::STATUS, &text);
                            resp.extend_from_slice(match r {
                                crate::process::ERR => alloc::format!("notify: refused (words not ASCII, or past A#{} bytes)\n", ferros_kist::note::WORDS_MAX),
                                crate::process::NOTHING => alloc::string::String::from("notify: refused (nobody present, or that kist is at its count)\n"),
                                r => alloc::format!("notify: note A#{} posted, {}\n", r & 0xFF, crate::process::note_kind_name((r >> 8) as u8)),
                            }.as_bytes());
                        }
                        _ => resp.extend_from_slice(b"notify: say `notify`, `notify <live|personal|status> <kist> [words]` or `notify clear <kist>`, the kist by its name while it runs or its capsule's address in hex\n"),
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
                // `diag move <name>…`: each named process's arc moved to other frames (ARCS.md step 3), at once or as its last thread leaves a core; the console line says how long the copy and the whole pause took (`diag console`).
                if let Some(names) = cmd.params.strip_prefix(b"move ") {
                    for name in names.split(|&b| b == b' ').filter(|n| !n.is_empty()) {
                        resp.extend_from_slice(name);
                        resp.extend_from_slice(if crate::process::move_named(name) { b": held for its move (the console says when it moved)\n" } else { b": no such process\n" });
                    }
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                // `diag frames`: the frame pool's memory map on this board (frames.rs, BOARDS.md "Memory map").
                if cmd.params == b"frames" {
                    resp.extend_from_slice(crate::frames::map_report().as_bytes());
                    resp.extend_from_slice(crate::process::with_lock(crate::frames::pool_report).as_bytes());
                    resp.extend_from_slice(crate::frames::wipe_report().as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                // `diag frames release`: the frames a `FERROS_FRAMES_JUMP_HOLD=1` build held after a jump given to the pool (frames.rs `release_held`), once the tripwire and the fences say Android left nothing writing there.
                if cmd.params == b"frames release" {
                    let n = crate::process::with_lock(crate::frames::release_held);
                    resp.extend_from_slice(alloc::format!("frames release: A#{n} held frames (A#{} MiB) given to the pool\n", n * 2).as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                // `diag trace`: zuma's CoreSight trace sinks as they stand (coresight.rs: the TMCs and the ETR's buffer; not the per-core trace units, whose read stalled a jumped kernel), read only; `diag trace stop` stops a sink that captures.
                if cmd.params == b"trace" || cmd.params == b"trace stop" {
                    let mut s = alloc::string::String::new();
                    if cmd.params == b"trace stop" {
                        crate::coresight::stop(&mut s);
                    }
                    crate::coresight::report(&mut s);
                    resp.extend_from_slice(s.as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                // `diag frames test`: the free frames written, read back and wiped (frames.rs `test_frame`), a hold of the kernel lock a frame and 256 frames (512 MiB) a call, the next call going on from there until the pool is done; then the whole run's count. Never the whole pool in one call: the service loop busy for akita's 5 GiB (1.2 s, 2026-11-12) reset the 8a about 25 s later, every time, with nothing on the console, where calls of up to 0.7 s did not.
                if cmd.params == b"frames test" {
                    let total = crate::process::with_lock(crate::frames::total);
                    let (at, n, bad, bad_frames, first) = &mut frames_test;
                    let mut tested = 0;
                    while *at < total && tested < 1 << 8 {
                        if let Some((wrong, w)) = crate::process::with_lock(|| crate::frames::test_frame(*at)) {
                            if *bad == 0 && wrong != 0 {
                                *first = w;
                            }
                            tested += 1;
                            *n += 1;
                            *bad += wrong;
                            *bad_frames += (wrong != 0) as usize;
                        }
                        *at += 1;
                    }
                    let tail = if *bad == 0 { alloc::string::String::new() } else { alloc::format!(", the first at G#{first:X}") };
                    if *at < total {
                        resp.extend_from_slice(alloc::format!("frames test: A#{at} of A#{total} frames so far, A#{n} free ones written and read back, A#{bad} words wrong in A#{bad_frames} frames{tail}; again for the next\n").as_bytes());
                    } else {
                        resp.extend_from_slice(alloc::format!("frames test done: A#{n} free frames written and read back, A#{bad} words wrong in A#{bad_frames} frames{tail}\n").as_bytes());
                        frames_test = (0, 0, 0, 0, 0);
                    }
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                // `diag frames trip arm|check|off`: the tripwire (frames.rs `trip_arm`): every free or held frame written with a pattern naming each word's own address, read back on command for writes no CPU made, then wiped. A bus master Android left running after a jump (or any device writing where it should not) shows as changed runs with their address, extent and words. 256 frames (512 MiB) a call and a hold of the kernel lock a frame, as `frames test`; call again until it says done. A check re-arms what it found changed, so each reports the writes since the one before.
                if let Some(op) = cmd.params.strip_prefix(b"frames trip ") {
                    let kind = match op {
                        b"arm" => 0u8,
                        b"check" => 1,
                        b"off" => 2,
                        _ => 3,
                    };
                    if kind == 3 {
                        resp.extend_from_slice(b"frames trip: arm, check or off\n");
                        queue_pt_response(&mut pt_out_data, &resp);
                        continue;
                    }
                    if trip.0 != kind {
                        trip = (kind, 0, 0, 0);
                        trip_runs.clear();
                        trip_more = 0;
                    }
                    let total = crate::process::with_lock(crate::frames::total);
                    let t0 = cntpct();
                    let mut acted = 0;
                    while trip.1 < total && acted < 1 << 8 {
                        let f = trip.1;
                        let did = crate::process::with_lock(|| match kind {
                            0 => crate::frames::trip_arm(f).then_some(0),
                            1 => crate::frames::trip_check(f, &mut trip_runs, 1 << 6, &mut trip_more),
                            _ => crate::frames::trip_off(f).then_some(0),
                        });
                        if let Some(words) = did {
                            acted += 1;
                            trip.2 += 1;
                            trip.3 += words;
                        }
                        trip.1 += 1;
                    }
                    let us = cntpct().wrapping_sub(t0) * 1_000_000 / cntfrq();
                    let (at, n, words) = (trip.1, trip.2, trip.3);
                    if at < total {
                        resp.extend_from_slice(alloc::format!("frames trip {}: A#{at} of A#{total} frames so far, A#{n} acted on (this call A#{us} us); again for the next\n", core::str::from_utf8(op).unwrap_or("")).as_bytes());
                    } else {
                        match kind {
                            0 => {
                                trip_armed_at = cntpct();
                                resp.extend_from_slice(alloc::format!("frames trip armed: A#{n} frames (A#{} MiB), every free and held frame\n", n * 2).as_bytes());
                            }
                            1 => {
                                let s = cntpct().wrapping_sub(trip_armed_at) / cntfrq();
                                resp.extend_from_slice(alloc::format!("frames trip check: A#{n} armed frames read back, A#{s} s after the arm, A#{words} words changed in A#{} runs{}\n", trip_runs.len(), if trip_more == 0 { alloc::string::String::new() } else { alloc::format!(" (and A#{trip_more} changed lines past those)") }).as_bytes());
                                // Each word as read, and where it is a pattern word from elsewhere, the address it names (`p:`).
                                let word = |w: u64| {
                                    let x = w ^ crate::frames::TRIP_SALT;
                                    if x >> 36 == 0 && x & 7 == 0 { alloc::format!("p:G#{x:X}") } else { alloc::format!("G#{w:X}") }
                                };
                                for r in &trip_runs {
                                    let sample: alloc::vec::Vec<alloc::string::String> = r.sample.iter().map(|&w| word(w)).collect();
                                    resp.extend_from_slice(alloc::format!("G#{:X}-G#{:X}: A#{} bytes, A#{} words changed; G#{:X}: {}\n", r.start, r.end, r.end - r.start, r.words, r.line, sample.join(" ")).as_bytes());
                                }
                            }
                            _ => resp.extend_from_slice(alloc::format!("frames trip off: A#{n} frames wiped\n").as_bytes()),
                        }
                        trip = (kind, 0, 0, 0);
                        trip_runs.clear();
                        trip_more = 0;
                    }
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"vault fresh process yes" {
                    // `diag "vault fresh process yes"` (development builds; the owner, 2026-11-23: "you can nuke all. I do not care about data on this device"): the vault process alone starts over, its own fresh start (its spawn input bit 0: it zeroes the start of each of its two mirrors and opens them blank, payloads/vault `FRESH`).
                    // What goes is everything the vault process keeps: the person's data, the identity service's space copy, the stowed kists and their list, the device's firmware and calibration scopes (export them first: `store get`). What stays is the kernel's own vault (every shell capsule, init, the seals, the device settings) and the ledger, so the device comes back with its whole shell, unlike `vault fresh yes`.
                    #[cfg(not(ferros_production))]
                    {
                        crate::flashlog::line("vault fresh process: the owner's fresh start of the vault process alone");
                        restart_with(&mut store, b"vault", &[1], &mut resp, false);
                    }
                    #[cfg(ferros_production)]
                    resp.extend_from_slice(b"vault fresh process: not on a production build\n");
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if let Some(names) = cmd.params.strip_prefix(b"restart ") {
                    // A restart by hand clears each name's count of deaths, and with it a crash loop's stop (docs/KERNEL.md "Restarts").
                    for name in names.split(|&b| b == b' ').filter(|n| !n.is_empty()) {
                        if crate::process::restart_clear(name) {
                            resp.extend_from_slice(b"restarts: the crash loop's stop cleared for ");
                            resp.extend_from_slice(name);
                            resp.push(b'\n');
                        }
                    }
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
                if cmd.params == b"vault fresh yes" {
                    // `diag "vault fresh yes"` (development builds; the owner's word, 2026-11-18: "wipe both devices' vaults and start fresh", "You can nuke both"): every vault and the ledger start over.
                    // Zeroed, under the kernel lock so nothing else reaches the flash in between: the ledger's ring (both copies), the kernel vault's two mirror regions (manifestus genesises a blank pair at the next open), and the first MIRROR blocks of each half of the vault process's window (its own fresh start, payloads/vault FRESH).
                    // Then a plain reset at once, inside the lock: nothing may write to a zeroed region before the next boot opens it afresh (a commit there would leave a region manifestus neither opens nor genesises).
                    // No reply: the host sees the device go and come back; the next boot's flash log says "kvault: genesis value absent, writing it".
                    // What it destroys is everything stored on this device, the person's data and every capsule; the next boot has no init, so it brings up only the USB process the kernel carries (start_home), for the bridge to store the shell again.
                    #[cfg(not(ferros_production))]
                    {
                        crate::process::push_console(b"vault fresh: zeroing the ledger, the kernel vault and the vault process's region, then a reset\n");
                        crate::flashlog::line("vault fresh: the owner's fresh start; zeroing the ledger, the kernel vault and the vault process's region, then a reset");
                        // Since 2026-11-18 the flash and the vault have locks of their own: all of them held to the reset (the kernel's, the vault's, the flash log's, each block's zeroing under the UFS lock), and the vault process's block calls refused from here (`BLOCKS_SHUT`), which no longer take the kernel lock.
                        crate::process::BLOCKS_SHUT.store(true, core::sync::atomic::Ordering::SeqCst);
                        crate::process::with_lock(|| {
                            crate::process::VAULT.with(|| {
                                crate::process::LOG.with(|| {
                                    let (l, ln) = crate::flashlog::ring_region();
                                    let (k, kn) = crate::kvault::regions();
                                    let w = crate::process::WINDOWS[0].offset;
                                    // The vault process's two halves (payloads/vault: HALF 2^21 blocks apart, MIRROR 2^17 blocks zeroed by its own FRESH).
                                    // A refused run stops the zeroing where it is; the reset follows either way, and the next boot says what it found (a vault that neither opens nor genesises names itself in the flash log).
                                    let _ = crate::process::UFS.with(|| crate::process::zero_blocks(l, ln) && crate::process::zero_blocks(k, kn) && crate::process::zero_blocks(w, 1 << 17) && crate::process::zero_blocks(w + (1 << 21), 1 << 17));
                                    platform::reboot()
                                })
                            })
                        });
                    }
                    #[cfg(ferros_production)]
                    {
                        resp.extend_from_slice(b"vault fresh: not on a production build\n");
                        queue_pt_response(&mut pt_out_data, &resp);
                        continue;
                    }
                }
                if cmd.params == b"kv selftest" {
                    // `diag kv selftest`: one large put through the manifestus vault, with its block I/O counted and every appended block re-read (kvault.rs).
                    match vault(|| store.as_mut().map(|s| s.selftest())) {
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
                if cmd.params == b"panic test" {
                    // `diag panic test` (development builds; compiled out of a production build, which answers only a plain DIAG): a kernel panic on purpose, to see the panic screen (glass.rs): the text on the glass, held until a press of power or 32 s, then the report and the warm reset as for any panic.
                    // No reply comes: the panic is the answer.
                    crate::process::push_console(b"diag panic test: panicking on purpose\n");
                    panic!("diag panic test: a panic asked for on purpose (a development build), to show the panic screen");
                }
                #[cfg(not(ferros_production))]
                if let Some(text) = cmd.params.strip_prefix(b"glass ") {
                    // `diag glass <text>` (development builds): a notice on the glass as a dead process's line goes there (glass.rs `notice`), drawn even while the display serves; `\n` in the text is a line return.
                    // The display redraws over it only where its own content changes.
                    let mut t = alloc::vec::Vec::with_capacity(text.len());
                    let mut i = 0;
                    while i < text.len() {
                        // WHY `get`: the text is the host's, and a backslash may be its last byte.
                        if text[i] == b'\\' && text.get(i + 1) == Some(&b'n') {
                            t.push(b'\n');
                            i += 2;
                        } else {
                            t.push(text[i]);
                            i += 1;
                        }
                    }
                    let shown = crate::process::with_lock(|| {
                        crate::glass::notice(crate::glass::Note::words(&alloc::string::String::from_utf8_lossy(&t)), true);
                        crate::display::scanned().map(|s| (s.width, s.height, s.stride, s.iova, s.translates()))
                    });
                    resp.extend_from_slice(match shown {
                        Some((w, h, st, a, tr)) => alloc::format!("glass: A#{w} x A#{h}, stride A#{st}, layer at G#{a:X}{}\n", if tr { ", translated" } else { "" }),
                        None => alloc::string::String::from("glass: no buffer on the glass the console can write (the DPU off, or a compressed layer)\n"),
                    }.as_bytes());
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
                if cmd.params == b"gpu fence" {
                    // The GPU's fence's latched fault (2026-11-20, docs/GPU-KISTS.md "Measured on the 8a"): every VID's address (both words), its info decoded as pKVM's s2mpu.h does (type 23:21, 2 a permission; bit 20 a write; length 19:16; id 15:0) and info2, then cleared (INTERRUPT_CLEAR), so the next run's is its own.
                    resp.extend_from_slice(crate::s2mpu::gpu_fault_take().as_bytes());
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
                if cmd.params.starts_with(b"store gets") {
                    // `diag "store gets [reset]"` (2026-11-11): the forwarded store calls the vault process answered: how many, the bytes handed back, the time from each call to its reply (the stamp, the wait for the vault's thread to be given a core, its serving) on average and at the longest, and the replies' copies into the callers. With the vault's own read report (its serving and the time between its requests) and the reader's own count of its calls, it says where a slow read waits.
                    use core::sync::atomic::Ordering::Relaxed;
                    if &cmd.params[..] == b"store gets reset" {
                        crate::process::STORE_GETS.iter().chain(crate::process::STORE_LONGEST.iter()).chain(crate::process::STORE_BY_CLOCK.iter()).for_each(|s| s.store(0, Relaxed));
                    }
                    // The longest call: whose, and what (op 0 set, 1 get, 2 delete, 3 list, 5 export; a set's `more`; length; offset). The clock states the calls were answered under, and the X3's clock now.
                    let (who, what) = (crate::process::STORE_LONGEST[0].load(Relaxed), crate::process::STORE_LONGEST[1].load(Relaxed));
                    let c = |i: usize| crate::process::STORE_BY_CLOCK[i].load(Relaxed);
                    let longest = alloc::format!("  the longest: {}, op A#{}{}, A#{} bytes at A#{}; answered at the boot's clocks A#{}, rest A#{}, active A#{}, dark A#{}; X3 now A#{} kHz (state A#{})\n", alloc::string::String::from_utf8_lossy(&who.to_le_bytes()).trim_end_matches('\0'), what & 0x7F, if what & 0x80 != 0 { " (more)" } else { "" }, (what >> 8) & 0xFF_FFFF, what >> 32, c(0), c(1), c(2), c(3), crate::acpm::dvfs_get(4).unwrap_or(0), clock_state());
                    let s = |i: usize| crate::process::STORE_GETS[i].load(Relaxed);
                    let us = |t: u64| t * 1_000_000 / cntfrq();
                    // WHY the max: no calls yet would divide by zero.
                    let calls = s(0);
                    resp.extend_from_slice(alloc::format!("store gets: A#{} calls, A#{} bytes back; call to reply A#{} ms in all, A#{} us a call, A#{} us the longest; copying replies A#{} us in all\n", calls, s(1), us(s(2)) / 1000, us(s(2)) / calls.max(1), us(s(3)), us(s(4))).as_bytes());
                    resp.extend_from_slice(longest.as_bytes());
                    resp.extend_from_slice(alloc::format!("pin off: {}\n", { let k = crate::process::PIN_OFF.load(Relaxed); if k == 0 { alloc::string::String::from("none") } else { alloc::string::String::from_utf8_lossy(&k.to_le_bytes()).trim_end_matches('\0').into() } }).as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params.starts_with(b"pin off ") || &cmd.params[..] == b"pin on" {
                    // `diag "pin off <name>"`: that process's SYS_PIN is ignored from now (its threads stay on the workers, or go back there at their next pin); `diag "pin on"` ends it. For measuring the vault's bulk reads on the X3 and on the workers on one boot (2026-11-11).
                    use core::sync::atomic::Ordering::Relaxed;
                    let key = if &cmd.params[..] == b"pin on" { 0 } else { crate::process::pin_key(&cmd.params[8..]) };
                    crate::process::PIN_OFF.store(key, Relaxed);
                    resp.extend_from_slice(alloc::format!("pin off: {}\n", if key == 0 { "none".into() } else { alloc::string::String::from_utf8_lossy(&cmd.params[8..]).into_owned() }).as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params.starts_with(b"ufs link") {
                    // `diag "ufs link [wb on|wb off]"` (docs/RAW-VIDEO.md, 2026-11-19): the link as the UniPro layer has it now (gear, series, lanes, power mode, read from the host's own PA attributes) and the device's WriteBooster (the SLC cache a sustained write runs into: its descriptor fields, the enable flags, the buffer's size and wear); `wb on`/`wb off` set or clear fWriteBoosterEn and the flush flags, which Linux sets at every probe and ferros never did.
                    // Under the UFS lock (process.rs `UFS`), as every UFS command: the vault process drives the flash from other cores.
                    crate::process::UFS.with(|| {
                        let ufs = ferros_hal::ufs::UfsController::new(0x1320_0000);
                        ufs.h8_wake_now();
                        let wb = &cmd.params[8..];
                        if wb == b" wb on" || wb == b" wb off" {
                            let on = wb == b" wb on";
                            // fWriteBoosterEn G#0E, fWBBufferFlushEn G#0F, fWBBufferFlushDuringHibernate G#10 (JEDEC UFS 3.1).
                            let r: [bool; 3] = core::array::from_fn(|k| if on { ufs.set_flag(0x0E + k as u8) } else { ufs.clear_flag(0x0E + k as u8) });
                            resp.extend_from_slice(alloc::format!("writebooster {}: accepted {:?}\n", if on { "on" } else { "off" }, r).as_bytes());
                        }
                        let pa = |a: u32| match ufs.dme_get(a, 0) { Ok(v) => alloc::format!("A#{v}"), Err(e) => alloc::format!("err G#{e:X}") };
                        let mode = ufs.dme_get(0x1571, 0).unwrap_or(0xFF);
                        let modes = |m: u32| match m { 1 => "fast", 2 => "slow", 4 => "fast auto", 5 => "slow auto", 3 => "hibernate", 7 => "unchanged", _ => "?" };
                        resp.extend_from_slice(alloc::format!("link: tx gear {} rx gear {} (device's rx most {}), series {} (1 A, 2 B), lanes tx {} of {} rx {} of {}, power mode G#{mode:02X} (rx {}, tx {}), termination tx {} rx {}\n", pa(0x1568), pa(0x1583), pa(0x1587), pa(0x156A), pa(0x1560), pa(0x1561), pa(0x1580), pa(0x1581), modes(mode >> 4 & 7), modes(mode & 7), pa(0x1569), pa(0x1584)).as_bytes());
                        let (d, n, ocs) = ufs.read_descriptor(0x00, 0);
                        let be32 = |b: &[u8], at: usize| u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
                        // Device descriptor (JEDEC UFS 3.1): wSpecVersion G#10, bQueueDepth G#21, dExtendedUFSFeaturesSupport G#4F (bit 8 WriteBooster), bWriteBoosterBufferPreserveUserSpaceEn G#53, bWriteBoosterBufferType G#54 (0 per LU, 1 shared), dNumSharedWriteBoosterBufferAllocUnits G#55.
                        resp.extend_from_slice(alloc::format!("device descriptor len A#{n} ocs A#{ocs}: spec G#{:02X}{:02X}, queue depth A#{}, extended features G#{:08X}, writebooster preserve user space A#{}, type A#{}, shared units A#{}\n", d[0x10], d[0x11], d[0x21], be32(&d, 0x4F), d[0x53], d[0x54], be32(&d, 0x55)).as_bytes());
                        let (g, gn, gocs) = ufs.read_descriptor(0x07, 0);
                        // Geometry descriptor: qSegmentSize G#0D (512-byte units), bAllocationUnitSize G#11 (segments), dWriteBoosterBufferMaxNAllocUnits G#4F, bDeviceMaxWriteBoosterLUs G#53, bWriteBoosterBufferCapAdjFac G#54.
                        resp.extend_from_slice(alloc::format!("geometry descriptor len A#{gn} ocs A#{gocs}: segment A#{} bytes, allocation unit A#{} segments, writebooster most A#{} units, capacity adjustment factor A#{}\n", (be32(&g, 0x0D) as u64) << 9, g[0x11], be32(&g, 0x4F), g[0x54]).as_bytes());
                        let fmt = |v: Option<u32>| match v { Some(x) => alloc::format!("A#{x}"), None => alloc::string::String::from("?") };
                        let fmtb = |v: Option<bool>| match v { Some(x) => alloc::format!("{}", x as u8), None => alloc::string::String::from("?") };
                        // Flags G#0E-G#10 as above; attributes bWBBufferFlushStatus G#1C, bAvailableWriteBoosterBufferSize G#1D (tenths free), bWriteBoosterBufferLifeTimeEst G#1E, dCurrentWriteBoosterBufferSize G#1F (units).
                        resp.extend_from_slice(alloc::format!("writebooster: enabled {} flush {} flush in hibernate {}; flush status {}, available {} tenths, life used {} (1 is under a tenth), current size {} units\n", fmtb(ufs.read_flag(0x0E)), fmtb(ufs.read_flag(0x0F)), fmtb(ufs.read_flag(0x10)), fmt(ufs.read_attribute(0x1C, 0)), fmt(ufs.read_attribute(0x1D, 0)), fmt(ufs.read_attribute(0x1E, 0)), fmt(ufs.read_attribute(0x1F, 0))).as_bytes());
                        let cw = crate::process::CLIPS_WB_SET.load(core::sync::atomic::Ordering::Relaxed);
                        resp.extend_from_slice(alloc::format!("clips: writebooster {} this boot (flags accepted G#{:X} of G#7)\n", if cw >> 8 & 1 != 0 { "asked" } else { "not yet asked" }, cw & 7).as_bytes());
                    });
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params.starts_with(b"ufs durable") {
                    // `diag "ufs durable [sync|fua|off|reset]"` (the owner, 2026-11-22: a cache flush per commit, measured): how persistent writes pass the flash's own write cache (ferros_hal `DURABLE`), the SYNCHRONIZE CACHE commands sent and their time, and LU 0's caching mode page (WCE 1: the device has a volatile write cache).
                    use core::sync::atomic::Ordering::Relaxed;
                    match &cmd.params[..] {
                        b"ufs durable sync" => ferros_hal::ufs::DURABLE.store(0, Relaxed),
                        b"ufs durable fua" => ferros_hal::ufs::DURABLE.store(1, Relaxed),
                        b"ufs durable off" => ferros_hal::ufs::DURABLE.store(2, Relaxed),
                        b"ufs durable reset" => ferros_hal::ufs::SYNC_STATS.iter().for_each(|s| s.store(0, Relaxed)),
                        _ => {}
                    }
                    let page = crate::process::UFS.with(|| ferros_hal::ufs::UfsController::new(0x1320_0000).caching_page(0));
                    let mode = ["sync (SYNCHRONIZE CACHE at each flush)", "fua (every write FUA, flushes skipped)", "off (no flush: development)"];
                    let (n, t) = (ferros_hal::ufs::SYNC_STATS[0].load(Relaxed), ferros_hal::ufs::SYNC_STATS[1].load(Relaxed) * 1_000_000 / cntfrq());
                    // WHY the max: no flush yet is no time, and the mean would divide by zero.
                    resp.extend_from_slice(alloc::format!("ufs durable: {}
  flushes A#{n}, A#{} ms in all, A#{} us each
  LU 0 caching page: {}
", mode[(ferros_hal::ufs::DURABLE.load(Relaxed) as usize).min(2)], t / 1000, t / n.max(1), match page { Some((wce, rcd)) => alloc::format!("WCE {} RCD {}", wce as u8, rcd as u8), None => alloc::string::String::from("did not read") }).as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params.starts_with(b"ufs runs") {
                    // `diag "ufs runs [on|off|reset]"`: SYS_BLOCK's multi-block commands (a run a command, ferros_hal `read_run_lun`) and what the vault process's block calls cost: calls, blocks, time inside them, MB/s, reads and writes. `off` sends every run a block a command again, to measure both on one boot; `reset` zeroes the counts.
                    use core::sync::atomic::Ordering::Relaxed;
                    match &cmd.params[..] {
                        b"ufs runs on" | b"ufs runs off" => ferros_hal::ufs::RUNS_ON.store(cmd.params == b"ufs runs on", Relaxed),
                        b"ufs runs reset" => {
                            crate::process::BLOCK_STATS.iter().for_each(|s| s.store(0, Relaxed));
                            ferros_hal::ufs::CMD_TICKS.iter().for_each(|s| s.store(0, Relaxed));
                        }
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
                    // The reads' time split (2026-11-11): the UFS commands alone (the link's wake, the cache upkeep, the doorbell to completion) and the rest (the copy to the caller), and how many were a large value's read-ahead (MOVING_BLOCKS or more).
                    // The command's ticks are a part of the call's (both from the same start, added under the kernel lock), so the subtraction holds. WHY the max: no reads yet is no time, and the rate would divide by zero.
                    let (calls, blocks, inside, cmd) = (s(0), s(1), s(2) * 1_000_000 / f, s(6) * 1_000_000 / f);
                    resp.extend_from_slice(alloc::format!("    in the commands A#{} ms (A#{} MB/s), copying out A#{} ms; A#{} of A#{} calls a read-ahead\n", cmd / 1000, (blocks << 12) / cmd.max(1), (inside - cmd) / 1000, s(7), calls).as_bytes());
                    // Every UFS command's split (the kernel's own vault and flash log included): the cache upkeep around it, and the doorbell to completion.
                    let h = |i: usize| ferros_hal::ufs::CMD_TICKS[i].load(Relaxed) * 1_000_000 / f / 1000;
                    resp.extend_from_slice(alloc::format!("  every command: cache upkeep A#{} ms, doorbell to completion A#{} ms\n", h(0), h(1)).as_bytes());
                    resp.extend_from_slice(line("writes", 3).as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"idle" || cmd.params.starts_with(b"idle ") {
                    // `diag idle [tickless on|off]`: the kernel core's dark idle (idle.rs).
                    crate::idle::diag(core::str::from_utf8(&cmd.params[4..]).unwrap_or("").trim(), &mut resp);
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"smp dark on" || cmd.params == b"smp dark off" {
                    DARK_WORKERS_OFF.store(cmd.params == b"smp dark on", core::sync::atomic::Ordering::Relaxed);
                    resp.extend_from_slice(alloc::format!("workers off while dark: {}\n", cmd.params == b"smp dark on").as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"smp little off" || cmd.params == b"smp little on" {
                    // `diag smp little off|on`: the A510s switched off (and kept off by later launches) or started again, for their idle cost; threads pinned there run on the other cores meanwhile (process.rs `may_run`: a mask with no core up runs anywhere).
                    let on = cmd.params == b"smp little on";
                    crate::smp::LITTLE_ON.store(on, core::sync::atomic::Ordering::Relaxed);
                    if on {
                        crate::smp::launch_all();
                    } else {
                        crate::smp::stop(crate::smp::LITTLE);
                    }
                    let (up, _) = crate::smp::report();
                    resp.extend_from_slice(alloc::format!("workers up G#{up:X} (bit = core)\n").as_bytes());
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
                if cmd.params == b"identity remove" {
                    // `diag "identity remove"`: the kernel's half of a removal's dry run (AUTH.md "Remove this device"): what it would take, nothing changed. The kists' own domains are named by the identity service (fgtw op 33's dry run, which asks the kernel for this same account with them).
                    let t = crate::process::with_lock(|| crate::process::remove_dry_text(&[]));
                    resp.extend_from_slice(t.as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"identity" || cmd.params == b"identity release" {
                    // `diag identity`: the identity class's state (AUTH.md); `diag "identity release"`: every identity entry deleted, the binding and any kept seed gone, the device-only secret renewed — what a completed departure does, for a developer starting the device over.
                    // Under the kernel lock, as every UFS command (release writes the vault; the state reads it).
                    let s = if cmd.params == b"identity release" { crate::process::with_lock(|| store_unlock(4, None)).map(|_| "released") } else { Ok("") };
                    let st = crate::process::with_lock(|| store_unlock(0, None)).unwrap_or(0);
                    // The person key only as held or not, and the alias derivation against fgtw's known answer (FACETS.md): never a byte of either.
                    resp.extend_from_slice(alloc::format!("identity: {} unlocked {}, bound {}, person key held {}, alias known answer {}, aliases given A#{}\n", s.unwrap_or("release failed"), st & 1 == 1, st & 2 == 2, st & 4 == 4, if crate::kvault::alias_known_answer() { "ok" } else { "WRONG" }, crate::process::ALIASES_GIVEN.load(core::sync::atomic::Ordering::Relaxed)).as_bytes());
                    queue_pt_response(&mut pt_out_data, &resp);
                    continue;
                }
                if cmd.params == b"log" || cmd.params.starts_with(b"log ") {
                    // The flash log (flashlog.rs, LEDGER.md "Reading it"). Until signed bridge commands exist (BRIDGE.md B2) this needs no right: whoever holds the cable reads the log opened; it should need the bridge right once B2 is built.
                    let arg = core::str::from_utf8(&cmd.params[3..]).unwrap_or("").trim();
                    let mut w = arg.split_whitespace();
                    let num = |t: Option<&str>| t.and_then(|t| t.parse::<u32>().ok());
                    match w.next() {
                        // `diag "log legacy <first> [count]"`: the old text log's raw blocks, for the bridge's one-shot converter; `diag "log legacy erase <first> [count]"`: those blocks zeroed and read back.
                        // WHY the clamp on count: the host's number; a reply holds 2^6 blocks (256 KiB), and an erase step holds the kernel lock per block, 2^6 at a time.
                        Some("legacy") => match w.next() {
                            Some("erase") => {
                                let first = num(w.next()).unwrap_or(0);
                                let n = crate::flashlog::legacy_erase(first, num(w.next()).unwrap_or(crate::flashlog::LEGACY_STEP).clamp(1, crate::flashlog::LEGACY_STEP));
                                resp.extend_from_slice(alloc::format!("old log: A#{n} blocks from A#{first} zeroed and read back\n").as_bytes());
                            }
                            first => resp.extend_from_slice(&crate::flashlog::legacy_blocks(num(first).unwrap_or(0), num(w.next()).unwrap_or(crate::flashlog::LEGACY_STEP).clamp(1, crate::flashlog::LEGACY_STEP))),
                        },
                        Some("bench") => resp.extend_from_slice(crate::flashlog::bench().as_bytes()),
                        // `diag "log deaths [n] [blocks] [exits]"`: the newest n deaths (panics and faults, plain exits too with `exits`) (2^4 if none asked) in the newest `blocks` blocks of the ring (2^10 if none asked, about ten thousand entries; the whole ring is 2^14), as VSF documents for `ferros-bridge deaths`. Nothing is clamped: a count past the ring's ends at the head, and the reply's room cuts the answer.
                        Some("deaths") => {
                            let (n, blocks) = (num(w.next()).unwrap_or(1 << 4) as usize, num(w.next()).unwrap_or(1 << 10));
                            resp.extend_from_slice(&crate::flashlog::deaths(n, blocks, (1 << 18) - (1 << 10), w.next() == Some("exits")));
                        }
                        // `diag log [n]`: the newest n entries (2^8 if none asked), as VSF documents the bridge renders; at most a reply's room (256 KiB less a KiB), the newest kept.
                        // WHY the clamp: the host's number; the ring holds about 2^17 entries a copy, and the room cuts the answer anyway.
                        n => resp.extend_from_slice(&crate::flashlog::answer(num(n).unwrap_or(1 << 8).clamp(1, 1 << 17) as usize, (1 << 18) - (1 << 10))),
                    }
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
                    // The full report's steps that touch hardware or the firmware say themselves as crumbs first (G#D9 plus the step above bit 8: 1 this PSCI call, 2 the thermal lines, 3 the USB controller's words, 4 the S2MPUs, 5 the reboot word thru EL3), so a stall inside it names its step (b1119, 2026-11-22: a plain `diag` on the jumped Pro froze the kernel's core here, its last crumb only G#D5, the DIAG read).
                    platform::crumb(0xD9 | 1 << 8);
                    let _ = platform::take_serror();
                    unsafe { core::arch::asm!("smc #0", inout("x0") 0x8400_0004u64 => r, in("x1") 0u64, in("x2") 0u64) };
                    append_hex(&mut resp, b"CPU0_AFFINITY=", r as u32);
                }
                append_hex(&mut resp, b"BACKSTOP_WAKES=", BACKSTOP_WAKES.load(core::sync::atomic::Ordering::Relaxed) as u32);
                append_hex(&mut resp, b"X3_SLEEPS=", X3_SLEEPS.load(core::sync::atomic::Ordering::Relaxed) as u32);
                append_hex(&mut resp, b"X3_DOZES=", X3_DOZES.load(core::sync::atomic::Ordering::Relaxed) as u32);
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
                // The heat (thermal.rs `diag`): the hottest zone, °C, and which; the X3's uncapped rate in MHz while the policy caps it (0 = not); the kernel's core's clock measured, the firmware's rate for the X3, the slow and firmware-capped seconds, the X3's divider bits seen.
                platform::crumb(0xD9 | 2 << 8);
                crate::thermal::diag(&mut resp);
                for k in 0..9 {
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
                    for (k, s) in crate::process::SLICES.iter().enumerate().filter(|(k, _)| *k <= 8) {
                        append_hex(&mut resp, alloc::format!("SLICES_C{k}=").as_bytes(), s.load(core::sync::atomic::Ordering::Relaxed) as u32);
                    }
                    append_hex(&mut resp, b"LOCK_SPINS=", crate::process::LOCK_SPINS.load(core::sync::atomic::Ordering::Relaxed) as u32);
                    for (k, w) in crate::smp::IDLE_WAKES.iter().enumerate() {
                        append_hex(&mut resp, alloc::format!("IDLE_WAKES_C{}=", k).as_bytes(), w.load(core::sync::atomic::Ordering::Relaxed) as u32);
                    }
                    // Why idle cores take the lock: the queue count, the interrupt flag, and how far off the next deadline is (signed ms; negative = already passed, which would make every core look on every event-stream tick).
                    append_hex(&mut resp, b"IDLE_RUNNABLE_N=", crate::process::RUNNABLE_N.load(core::sync::atomic::Ordering::Relaxed) as u32);
                    append_hex(&mut resp, b"IDLE_RUNNABLE_LITTLE=", crate::process::RUNNABLE_LITTLE.load(core::sync::atomic::Ordering::Relaxed) as u32);
                    // CTR_EL0 as the weakest of every core that has run ferros (cachemap `CTR_ALL`: IDC bit 28, DIC bit 29).
                    append_hex(&mut resp, b"SMP_CTR_ALL=", crate::cachemap::CTR_ALL.load(core::sync::atomic::Ordering::Relaxed) as u32);
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
                platform::crumb(0xD9 | 3 << 8);
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
                    platform::crumb(0xD9 | 4 << 8);
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
                // The kernel heap now (`diag heap` has it in full): its size, free bytes and blocks, the largest (what one allocation can get), in use and its peak, refusals since boot.
                if let Some(h) = crate::heap_report() {
                    append_hex(&mut resp, b"HEAP_SIZE=", h.size as u32);
                    append_hex(&mut resp, b"HEAP_FREE=", h.free as u32);
                    append_hex(&mut resp, b"HEAP_FREE_BLOCKS=", h.free_blocks as u32);
                    append_hex(&mut resp, b"HEAP_LARGEST_FREE=", h.largest as u32);
                    append_hex(&mut resp, b"HEAP_REFUSED=", h.fails as u32);
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
                platform::crumb(0xD9 | 5 << 8);
                // An SError left waiting by any read above would bounce this firmware call until EL3 panics (the EL3 poison, platform.rs `take_serror`): taken first.
                let _ = platform::take_serror();
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
                    Some(Ok(job)) => {
                        vault_job = Some(job);
                        job_due = cntpct() + lease;
                    }
                    Some(Err(resp)) => queue_pt_response(&mut pt_out_data, &resp),
                    // Under the vault's lock alone, the kernel lock taken only to apply a settings document (`store_host`).
                    None => {
                        let resp = store_host(&mut store, cmd.op, cmd.params);
                        // Absent from the kernel's vault, a name of a scope this kernel does not know may be in the vault process (`foreign_job`).
                        match if resp == [1] { foreign_job(cmd.op, cmd.params) } else { None } {
                            Some(job) => {
                                vault_job = Some(job);
                                job_due = cntpct() + lease;
                            }
                            None => queue_pt_response(&mut pt_out_data, &resp),
                        }
                    }
                }
                }
            } else if cmd.cap == cap_display && cmd.op == ferros_pt::Op::Write {
                // A development door: compiled out of a production build (its `open` check refuses it there anyway).
                #[cfg(not(ferros_production))]
                {
                // A leading G#01 byte = hold (bridge `display --hold`). The DSI tools — G#02 a packet script, G#03 the DSIM dump, G#04 a DCS read, G#06 the refresh rate, G#08 the raster, G#09 the idle floor, G#0A the modeset — left with dpu.rs and dsim.rs on 2026-11-01 (KERNEL-OUT.md decision 6): the display server drives the panel.
                // The painter's dev tools went on 2026-11-22 (KERNEL-OUT.md move 6, decision 4's buckets): G#05 the TE rate (payloads/tecount reads the pin from EL0), G#07 DECON as Android (the display server's modeset sets that state), G#0C the fill, G#11 the patch, G#12 the 10-bit patch and G#14 its raw words (the display server's op 52 mode 8: any 10-bit disc on any surround, full field at radius 0), G#0F own framebuffer or ABL's, G#10 the DQE dither word and G#13 DECON's background (their questions answered: DISPLAY.md, DISPLAY-DESIGN.md).
                // Each re-adopted the pipe, fighting a running display server.
                // What stays: the painter itself (text, the boot status screen's path), G#15 the ruler (the board's key rows for the kernel's own gate), and G#0B, G#0D, G#0E, which are hsi2c_11's (KERNEL-OUT.md decision 5), not the display's. Anything else is text for the painter.
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
                    // G#15 = the binary ruler: every row its own number in binary across the width (bridge `display ruler`).
                    Some((21, _)) => {
                        let rep = crate::display::ruler(freq);
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
                    crate::process::UFS.with(|| ferros_hal::ufs::UfsController::new(0x1320_0000).h8_wake_now());
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
                        // The arc starts with the image's first heap and grows to the rest (`stream_room`), and holds the whole capsule while it is staged. WHY the bound: the code offset is the host's; an image header past the first piece is not read (the arc is then the host's sum).
                        let (size, ceiling) = if co <= piece.len() && piece.len() - co >= 56 { crate::process::stream_room(need, &piece[co..co + 56]) } else { (need, need) };
                        let size = size.max(crate::process::CODE + total + crate::process::STACK);
                        // Its frames made clean first with the kernel lock let go (frames.rs `prepare`), so the take under it zeroes none.
                        crate::frames::prepare(crate::process::arc_frames(size, false));
                        if let Some(arc) = crate::process::with_lock(|| crate::process::arc_alloc_room(size, ceiling, false)) {
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
                            18 => crate::process::Grant::Items,
                            _ => crate::process::Grant::Use(g),
                        });
                    }
                    let input = &pr[at.min(pr.len())..];
                    // Signed capsules only: the gate (oracle.rs) must pass, or spawn never completes.
                    let sealed = seals_for_launch(&mut store);
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
                        (Ok(seal), Some(store)) => match vault(|| seal_add(store, &seal)) {
                            Ok(()) => {
                                let mut r = alloc::vec![0u8];
                                r.extend_from_slice(&seal);
                                r
                            }
                            Err(why) => {
                                let mut r = alloc::vec::Vec::from(&b"ERR:install: "[..]);
                                r.extend_from_slice(why.as_bytes());
                                r
                            }
                        },
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
                    // From a jumped session the stamped mode never reaches the bootloader (platform.rs `reboot_to_bootloader`; 2026-11-15, the Pro's b875: the ROM's USB recovery, then ferros again): the wish goes into the vault and a plain reset lands in ferros cold, which takes the cold path at once (main.rs, vault_boot `next_boot_take`).
                    let jumped = platform::jumped();
                    let kept = jumped && vault(|| store.as_mut().map(|s| crate::vault_boot::next_boot_keep(s, platform::REBOOT_MODE_BOOTLOADER))) == Some(true);
                    crate::flashlog::line(if !jumped {
                        "reboot fastboot: a cold session, the mode G#FC stamped thru EL3, then the watchdogs"
                    } else if kept {
                        "reboot fastboot: a jumped session, the wish kept in the vault for the next boot, then a plain reset"
                    } else {
                        "reboot fastboot: a jumped session, and the vault did not keep the wish: the cold path anyway"
                    });
                    unsafe {
                        let dctl = read_volatile((DWC3 + 0xC704) as *const u32);
                        write_volatile((DWC3 + 0xC704) as *mut u32, dctl & !(1 << 31));
                    }
                    if kept {
                        platform::reboot();
                    }
                    // Only returns on failure: platform::WATCHDOG_DID_NOT_FIRE, or the EL3 monitor's refusal (nonzero, negative = error). Kept in report slot r[23] so the exit report says which.
                    FASTBOOT_FAIL.store(platform::reboot_to_bootloader(), core::sync::atomic::Ordering::Relaxed);
                }
                leave(&b, EXIT_REBOOT, &st);
            }
        }
        // A put the vault process takes over later passes reads its value from the command: it stays with the job (and with an offered part waiting for the channel).
        if (matches!(vault_job, Some(VaultJob::Put { .. })) || offer_part_waiting) && vault_cmd.is_none() {
            vault_cmd = Some(proc_cmd);
        }
    }
}
