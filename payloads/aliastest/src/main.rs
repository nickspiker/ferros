//! The alias's test kist (docs/FACETS.md "The alias"; "To build" step 1, 2026-11-12): asks the kernel for its alias once at launch and says on the console whether it got one, and if so a short one-way fingerprint of it (eight bytes of a BLAKE3 derive_key over the alias), never the alias.
//!
//! Built masked and signed twice with two options (tools/aliastest-kist.sh): two kist identities from the same code, which must get two aliases, each the same after a reboot. Spawned from the bridge as a plain process with an Ira grant it must be refused: it was not launched by the kist job.
#![no_std]
#![no_main]

/// The fingerprint's context: a hash of the alias for a log line, never the alias.
const PRINT_CONTEXT: &str = "ferros 2026-11-12 aliastest fingerprint";

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    match ferros_sys::alias(ferros_sys::cap(grants, ferros_sys::kist::IRA)) {
        Some(mut a) => {
            let mut fp = blake3::derive_key(PRINT_CONTEXT, &a);
            a.fill(0);
            let mut line = *b"aliastest: alias given, fingerprint ................\n";
            let hex = b"0123456789abcdef";
            let at = line.len() - 17;
            for (k, &c) in fp[..8].iter().enumerate() {
                line[at + 2 * k] = hex[(c >> 4) as usize];
                line[at + 2 * k + 1] = hex[(c & 15) as usize];
            }
            fp.fill(0);
            ferros_sys::console(grants, &line);
        }
        None => ferros_sys::console(grants, b"aliastest: no alias (not a kist the kist job launched, nobody present, or no person key yet)\n"),
    }
    ferros_sys::exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}

// The entry and its image header (ferros_sys::entry!: the kernel sizes the arc from the image and its .bss).
ferros_sys::entry!(payload_main);
