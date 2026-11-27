//! A Rust payload over ferros_sys built for wasm32 and compiled by toka (docs/TOKA.md "Intrinsics"): `cargo +nightly build --release --target wasm32-unknown-unknown`, then `toka target/wasm32-unknown-unknown/release/toka_rust_hello.wasm -o hello.bin --reserve 21`, capsule, seal and spawn; three console lines (a greeting, the timer, the arc's size).

#![no_std]

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

fn hex(mut v: u64, out: &mut [u8; 16]) {
    for k in (0..16).rev() {
        out[k] = b"0123456789ABCDEF"[(v & 15) as usize];
        v >>= 4;
    }
}

extern "C" fn payload_main(grants: u64, _input: u64, _len: u64) -> ! {
    ferros_sys::console(grants, b"toka: hello from Rust, ferros_sys built for wasm32\n");
    let mut line = *b"toka: timer G#0000000000000000 at G#0000000000000000 a second\n";
    let mut h = [0u8; 16];
    hex(ferros_sys::now(), &mut h);
    line[14..30].copy_from_slice(&h);
    hex(ferros_sys::freq(), &mut h);
    line[36..52].copy_from_slice(&h);
    ferros_sys::console(grants, &line);
    let (top, ceiling) = ferros_sys::arc::size();
    hex(top as u64 ^ (ceiling as u64).rotate_left(32), &mut h);
    let mut l2 = *b"toka: arc size^ceiling G#0000000000000000\n";
    l2[25..41].copy_from_slice(&h);
    ferros_sys::console(grants, &l2);
    ferros_sys::exit(0)
}

ferros_sys::entry!(payload_main);
