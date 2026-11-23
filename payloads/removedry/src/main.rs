//! The dry run of removing this device from its identity (docs/AUTH.md "Remove this device"; payloads/fgtw remove.rs): asks the identity service for it (op 33 [1]) and says its answer. The identity service puts every check and what each step would take on the console (`fgtw: remove (dry run): ...`), the kernel's half included; nothing is changed, no handle is typed, nothing of the identity is printed but device names and kist names.
//!
//! Build it with `cargo el0 -p ferros-payload-removedry` (spelled out in the farm), run it with `ferros-bridge spawn target/el0/aarch64-unknown-none/release/removedry.bin --name removedry --use fgtw`, and read its answer with `ferros-bridge diag console | grep -a "remove (dry run)"`.
#![no_std]
#![no_main]

/// The `--use fgtw` grant: the first after the console.
const S_FGTW: u64 = 1;

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let mut r = [0u8; 8];
    let n = ferros_sys::call_live(ferros_sys::cap(grants, S_FGTW), &[33, 1], &mut r);
    ferros_sys::console(
        grants,
        match (n, r[0]) {
            (n, _) if n == ferros_sys::ERR || n == ferros_sys::NOTHING || n < 1 => b"removedry: the identity service did not answer\n".as_slice(),
            (_, 0) => b"removedry: the dry run is on the console (fgtw: remove (dry run))\n",
            (_, 1) => b"removedry: no identity in hand (nobody attested): nothing to remove\n",
            _ => b"removedry: the identity service has no dry run (one before 2026-11-22?)\n",
        },
    );
    ferros_sys::exit(0);
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_report(env!("CARGO_BIN_NAME"), info)
}

// The entry and its image header (ferros_sys::entry!: the kernel sizes the arc from the image and its .bss).
ferros_sys::entry!(payload_main);
