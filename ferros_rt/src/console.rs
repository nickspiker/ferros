//! Stdout and stderr: the console capability every process holds in grant slot 0, one CALL per kilobyte.

use crate::sys::{self, nr, off};

/// Say `bytes` on the console. The buffer must be in the arc (stack, heap or image: everything a process has).
pub fn write(bytes: &[u8]) {
    let mut reply = [0u8; 16];
    for chunk in bytes.chunks(1024) {
        sys::svc(nr::CALL, crate::cap(0), off(chunk.as_ptr()), chunk.len() as u64, off(reply.as_mut_ptr()), reply.len() as u64);
    }
}
