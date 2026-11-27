//! The `getrandom` crate's custom backend for ferros, so `rand`, the signature crates and everything above them draw from the kernel's entropy without each program registering a backend: getrandom 0.2 looks for `__getrandom_custom` (its `custom` feature), 0.3 and 0.4 for `__getrandom_v03_custom` (`--cfg getrandom_backend="custom"`). A process spawned without an Entropy capability gets an error here, never the hash-seed mixer: keys come from the kernel or not at all.

use core::num::NonZeroU32;

/// getrandom's `Error`: a `#[repr(transparent)]` non-zero 32-bit code in every version (0.3 over `u32`, 0.4 over its `RawOsError`); `Result<(), Error>` is the code in w0, 0 for Ok, and the same layout here makes the `extern "Rust"` signature match.
#[repr(transparent)]
pub struct Error(NonZeroU32);

/// getrandom's `Error::CUSTOM_START`: codes from here up are the backend's own.
const NO_ENTROPY: u32 = (1 << 31) + 1;

fn fill(dest: *mut u8, len: usize) -> bool {
    if !crate::random::is_hardware() {
        return false;
    }
    let buf = unsafe { core::slice::from_raw_parts_mut(dest, len) };
    crate::random::fill_bytes(buf);
    true
}

#[unsafe(no_mangle)]
unsafe extern "Rust" fn __getrandom_custom(dest: *mut u8, len: usize) -> u32 {
    if fill(dest, len) { 0 } else { NO_ENTROPY }
}

#[unsafe(no_mangle)]
unsafe extern "Rust" fn __getrandom_v03_custom(dest: *mut u8, len: usize) -> Result<(), Error> {
    if fill(dest, len) { Ok(()) } else { Err(Error(NonZeroU32::new(NO_ENTROPY).unwrap())) }
}
