# ferros as a Rust `std` target

Started 2026-10-27. Owner: "You don't need the filesystem layer, you can strip that out. Photon speaks native vault, and yes, there was a reason I had you bring up all the cores." The goal is the real Photon app (fluor UI, tokio, the fgtw chain) running on the phone as a ferros process, not a no_std re-implementation beside it.

## Shape

- **Target:** `targets/aarch64-unknown-ferros.json` — `target_os = "ferros"`, static PIE (the kernel loads a process anywhere in its arc, `ferros_sys::relocate` fixes the RELA table), `has-thread-local` (TPIDR_EL0 is the thread pointer), panic = abort, x24–x28 reserved (x28 is the arc base the kernel hands every thread).
- **Kernel:** the trap `Frame` carries TPIDR_EL0 (`tpidr` at `[sp, #816]`, 832-byte frame) since 2026-10-27, so each thread of a process keeps its own thread pointer across entries and slices. Nothing else in the kernel changes for std: threads are `SYS_THREAD_SPAWN`, blocking is `WAIT`/`WAKE` on an address (the futex), time is `cntvct_el0`, memory is the arc.
- **Runtime crate `ferros_rt`** (this repo, no_std, `rustc-dep-of-std` feature as the motor port's `moto_rt`): syscalls, futex, thread spawn with a TLS block per thread and a join futex, the allocator (ported from `ferros_heap`), console stdio (grant slot 0), entropy (`fill_bytes`), exit, and the crt0 `_entry` that relocates, sets TPIDR_EL0 for the main thread and calls C `main`.
- **std fork:** `/mnt/Harbor/Code/ferros-rust/library` — rustc 1.97.0-nightly (d7f14d3d8 2026-05-15) rust-src, with `sys/pal/ferros` and a `target_os = "ferros"` arm in each `sys/<area>/mod.rs` dispatch (pal, thread, sync futex, thread_local native, time, alloc, random, stdio, exit). Everything a process has no business with on ferros — fs, net sockets, process, env, args, pipes, fds — is `unsupported`: Photon speaks the vault natively and its sockets are the `ip` server's endpoints behind a ferros `mio` backend.
- **Build:** `cargo +nightly build -Zbuild-std=core,alloc,std,panic_abort --target targets/aarch64-unknown-ferros.json` with `__CARGO_TESTS_ONLY_SRC_ROOT=/mnt/Harbor/Code/ferros-rust/library`.

## Order

1. Kernel TPIDR_EL0 (done, flashed, the Photon client still runs) + the target JSON.
2. `ferros_rt` + the std platform layer; a std hello on the phone: `println!`, threads across the cores, `Mutex`, `thread::sleep`, `Instant`.
3. kete: a native-vault backend replacing its four `std::fs` uses.
4. fluor host over the display server's app surface + touch.
5. tokio over a ferros `mio`: the reactor over the `ip` server (UDP, client TCP).
6. Photon with `cfg(target_os = "ferros")` leaving out reqwest/rustls/tungstenite/cpal/rav1e/rav1d/opus/winit; a process memory region large enough for it.
7. STOP at the input device: the owner goes over the text-input idea first.

## Encryption, as the owner laid it out

Drivers and key apps are encrypted by the ira on disk. Once the user attests, the identity (Photon's keys from the handle) unlocks Photon messages, Lumis settings, and the rest: a lock screen, nothing works before attesting. The display driver and the identity-keyed data meet there; design before building.
