//! Photon on ferros: the app Photon's library builds, run through fluor's ferros host over the display server's app surface and the touch server. Spawned by the shell with (slot order) the display and touch endpoints, the app surface, the udp endpoint, the photon vault scope, the ira and entropy: `--use display --use touch --dma surface --use udp --store photon --ira --entropy`. The desktop main (photon's src/main.rs) does the winit things; this one does the ferros things.

use photon_messenger::network::fgtw::derive_device_keypair;
use photon_messenger::network::fgtw::fleet::SigningBundle;
use photon_messenger::ui::photon_app::PhotonApp;

/// The ira's grant slot, in the spawn's order above (tools/photon-capsule.sh declares it sixth: "to tie its keys to this device").
const IRA: u64 = 6;

/// The kernel, as this process reaches it (ferros KERNEL.md, the process ABI): the call for the ira key and the grants page. Inlined as Mahere's, fluor's and kete's are: ferros_sys is no_std and not a published crate.
mod sys {
    use core::arch::asm;

    const IRA: u64 = 23;

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
}

fn main() {
    photon_messenger::init_logging();
    photon_messenger::log_version();
    std::panic::set_hook(Box::new(|info| {
        let location = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
        println!("PANIC at {location}: {info}");
    }));
    // The device key from this kist's ira (docs/PHOTON.md, "Photon's device key on ferros", 2026-11-12). Photon's own fallback reads the machine fingerprint, which on ferros (no files) was tohu's last resort, the bytes "unknown": every ferros device's Photon had one device key, derivable by anyone.
    // A person who said no to the ira gets no Photon: without it Photon has no key of its own to be a device with, and the shared one is the bug above. It ends at once, as it did before with every want off (KISTS.md, "Measured on the Pro").
    let Some(root) = sys::ira_key(IRA, "photon device") else {
        println!("photon: no ira key at slot {IRA} (the ira right was not granted); Photon has no device key of its own and does not start");
        return;
    };
    let mut app = PhotonApp::new();
    app.set_device_keypair(derive_device_keypair(&root));
    app.set_signing_bundle(SigningBundle::derive(&root));
    let caps = fluor::host::ferros::Caps { display: 1, touch: 2, surface: 3 };
    match fluor::host::ferros::run_app(app, caps, 1.0, None) {
        Ok(()) => println!("photon: closed"),
        Err(e) => println!("photon: {e}"),
    }
}
