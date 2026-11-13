//! Photon on ferros: the app Photon's library builds, run through fluor's ferros host over the display server's app surface and the touch server. Launched as a kist (tools/photon-capsule.sh) with, in slot order, the display and touch endpoints, the app surface, the udp endpoint, its own vault domain, the ira and entropy (placements of the implicit set, KISTS.md), then be you and the person's people (asked, docs/FACETS.md); the GPU's three and sound out after them. The desktop main (photon's src/main.rs) does the winit things; this one does the ferros things.

use photon_messenger::crypto::photon_keys::PhotonKeys;
use photon_messenger::network::fgtw::derive_device_keypair;
use photon_messenger::network::fgtw::fleet::SigningBundle;
use photon_messenger::ui::photon_app::PhotonApp;

/// The ira's grant slot, in the spawn's order above (tools/photon-capsule.sh declares it sixth: "to tie its keys to this device"). The ira is one of the implicit set since 2026-11-12 (docs/KISTS.md), never asked; Photon's capsule still declares it, so it stays in this slot. The alias comes through the same grant (SYS_IRA x4 bit 1).
const IRA: u64 = 6;
/// Be you's grant slot (the capsule's eighth want): a send right on the identity service's `facets` endpoint, or an empty slot where the person said no or has not been asked.
const BE_YOU: u64 = 8;
/// The person's people's grant slot (the ninth want): the same endpoint, stamped by the kernel with what the person granted.
const PEOPLE: u64 = 9;

/// The kernel, as this process reaches it (ferros KERNEL.md, the process ABI): the ira and the alias, and a call on an endpoint. Inlined as Mahere's, fluor's and kete's are: ferros_sys is no_std and not a published crate.
mod sys {
    use core::arch::asm;

    const CALL: u64 = 1;
    const IRA: u64 = 23;
    /// ERR, the kernel's refusal (ferros_sys::ERR).
    const ERR: u64 = u64::MAX;

    #[inline(always)]
    fn svc(n: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
        let r: u64;
        unsafe { asm!("svc #0", inlateout("x0") a0 => r, in("x1") a1, in("x2") a2, in("x3") a3, in("x4") a4, in("x8") n, options(nostack)) };
        r
    }

    /// The arc base (x28, the kernel's, never written by compiled code on this target).
    #[inline(always)]
    fn arc_base() -> u64 {
        let b: u64;
        unsafe { asm!("mov {}, x28", out(reg) b, options(nomem, nostack, preserves_flags)) };
        b
    }

    #[inline(always)]
    fn off<T: ?Sized>(p: *const T) -> u64 {
        (p as *const u8 as u64).wrapping_sub(arc_base())
    }

    /// The grants page is arc offset 0: the handle table in its first 512 bytes, the u32 count at G#200, the 32-byte tokens after it (ferros SANDBOX.md, "Handles").
    fn cap(slot: u64) -> u64 {
        0x204 + 32 * slot
    }

    /// A 32-byte key for `domain` rooted in the device's ira (ferros_sys `ira_key`): the same on every boot, never the ira itself; for a kist the kernel binds it to the kist's gate identity, so no other kist and no system service derives it (KISTS.md, "A kist's ira keys are its own"). None without an Ira grant (the person said no) or an ira this boot.
    pub fn ira_key(slot: u64, domain: &str) -> Option<[u8; 32]> {
        let mut out = [0u8; 32];
        let r = svc(IRA, cap(slot), off(domain.as_ptr()), domain.len() as u64, off(out.as_mut_ptr()), 0);
        (r == 32).then_some(out)
    }

    /// Photon's alias (ferros_sys `alias`; docs/FACETS.md "The alias"): the seed the kernel derives for this kist alone from the person key and Photon's gate identity, the same on every device of the person's. None before attest, before the identity service handed the kernel the person key, or on a kernel from before 2026-11-12.
    pub fn alias(slot: u64) -> Option<[u8; 32]> {
        let mut out = [0u8; 32];
        let r = svc(IRA, cap(slot), 0, 0, off(out.as_mut_ptr()), 2);
        (r == 32).then_some(out)
    }

    /// A call on the endpoint in `slot`, its answer in `reply`: the answer's length, or None when the kernel refused (an empty slot: the person said no).
    pub fn call(slot: u64, msg: &[u8], reply: &mut [u8]) -> Option<usize> {
        let r = svc(CALL, cap(slot), off(msg.as_ptr()), msg.len() as u64, off(reply.as_mut_ptr()), reply.len() as u64);
        (r != ERR).then(|| (r as usize).min(reply.len()))
    }
}

/// The person as Photon's messenger knows them on ferros (docs/PHOTON.md "Photon's own key: the migration"): its alias from the kernel, and from the identity service the handle proof, the pinned identity key and the binding of the alias to the person (be you, op 0 on `facets`). None without the alias or without be you: Photon then runs as before, on its own handle screen (PHOTON.md option C). Never the identity seed: ferros hands no kist a root, so `legacy_seed` is None.
fn photon_keys() -> Option<PhotonKeys> {
    let Some(alias) = sys::alias(IRA) else {
        println!("photon: no alias from the kernel (before attest, or the person key not handed in yet)");
        return None;
    };
    let signer = fgtw::alias::bundle(&alias);
    let public = signer.public();
    let bundle = public.to_bytes();
    let eggs = fgtw::binding::possession(&signer, &public);
    let mut req = Vec::with_capacity(3 + bundle.len() + eggs.len());
    req.push(0);
    req.extend_from_slice(&(bundle.len() as u16).to_le_bytes());
    req.extend_from_slice(&bundle);
    req.extend_from_slice(&eggs);
    let mut reply = vec![0u8; 1 << 16];
    let Some(n) = sys::call(BE_YOU, &req, &mut reply) else {
        println!("photon: be you not granted: Photon runs on its own handle screen");
        return None;
    };
    if n < 65 || reply[0] != 0 {
        println!("photon: the identity service refused be you ({})", reply.first().copied().unwrap_or(0xFF));
        return None;
    }
    let mut hp = [0u8; 32];
    let mut identity = [0u8; 32];
    hp.copy_from_slice(&reply[1..33]);
    identity.copy_from_slice(&reply[33..65]);
    let binding = reply[65..n].to_vec();
    // The binding names this alias and this handle proof, or it is not taken: a check of the service's answer against what this process holds, never the chain's (that is a contact's, against the chain it fetches).
    match fgtw::binding::Binding::from_vsf_bytes(&binding) {
        Ok(b) if b.alias_key() == public.ed25519() && b.handle_proof == hp => {}
        _ => {
            println!("photon: the identity service's binding does not name this alias");
            return None;
        }
    }
    println!("photon: be you: the alias and its binding in hand ({} bytes)", binding.len());
    Some(PhotonKeys::new(alias, hp, identity, binding, None))
}

/// The person's people Photon may know (people, op 1 on `facets`): all of them or the ones the person chose, without their fleets. Empty without the grant: no lying, nobody is nobody.
fn people() -> Vec<fgtw::people::Person> {
    let mut reply = vec![0u8; 1 << 18];
    match sys::call(PEOPLE, &[1], &mut reply) {
        Some(n) if n >= 1 && reply[0] == 0 => fgtw::people::from_doc(&reply[1..n]).unwrap_or_default(),
        Some(_) => {
            println!("photon: the identity service refused the people ({})", reply[0]);
            Vec::new()
        }
        None => Vec::new(),
    }
}

fn main() {
    photon_messenger::init_logging();
    photon_messenger::log_version();
    std::panic::set_hook(Box::new(|info| {
        let location = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
        println!("PANIC at {location}: {info}");
    }));
    // The device key from this kist's ira (docs/PHOTON.md, "Photon's device key on ferros", 2026-11-12). Photon's own fallback reads the machine fingerprint, which on ferros (no files) was tohu's last resort, the bytes "unknown": every ferros device's Photon had one device key, derivable by anyone.
    // No ira, no Photon (the ira is handed to every kist since 2026-11-12, KISTS.md "The implicit set", so this is a kernel from before, where a person could say no to it, or no ira this boot): without it Photon has no key of its own to be a device with, and the shared one is the bug above. It ends at once, as it did before with every want off (KISTS.md, "Measured on the Pro").
    let Some(root) = sys::ira_key(IRA, "photon device") else {
        println!("photon: no ira key at slot {IRA} (the ira right was not granted); Photon has no device key of its own and does not start");
        return;
    };
    // The person, as facets (docs/FACETS.md; PHOTON.md "ferros's part"): Photon's alias and its binding, and the person's people. Asked once at start, before the first frame.
    let keys = photon_keys();
    let people = people();
    println!("photon: {} of the person's people handed in", people.len());
    let mut app = PhotonApp::new();
    app.set_device_keypair(derive_device_keypair(&root));
    app.set_signing_bundle(SigningBundle::derive(&root));
    // The person, handed in before Photon starts (photon 47f8b360): with PhotonKeys Photon opens its vault under the alias and lands on its contacts, no handle screen; it refuses a binding that does not name this alias and this handle proof (it says "PHOTON KEYS: refused") and runs its handle screen as before. The people merge into its contacts once its vault is open.
    if let Some(keys) = keys {
        app.set_photon_keys(keys);
    }
    app.set_people(people);
    // No window controls strip (minimise, maximise, close: a desktop window manager's, nothing on ferros), as Photon's Android build has none: since 2026-11-12 the frame reaches the glass's edge under the console (WINDOWS.md "Drawn under the controls"), and the strip, cut away with the corner until then, would show in the top right under the radios' circles, which take every touch there.
    fluor::paint::DEBUG_SKIP_CONTROLS.store(true, std::sync::atomic::Ordering::Relaxed);
    let caps = fluor::host::ferros::Caps { display: 1, touch: 2, surface: 3 };
    match fluor::host::ferros::run_app(app, caps, 1.0, None) {
        Ok(()) => println!("photon: closed"),
        Err(e) => println!("photon: {e}"),
    }
}
