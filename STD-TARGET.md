# ferros as a Rust `std` target

Started 2026-10-27. Owner: "You don't need the filesystem layer, you can strip that out. Photon speaks native vault, and yes, there was a reason I had you bring up all the cores." The goal is the real Photon app (fluor UI, tokio, the fgtw chain) running on the phone as a ferros process, not a no_std re-implementation beside it.

## Shape

- **Target:** `targets/aarch64-unknown-ferros.json` — `target_os = "ferros"`, static PIE (the kernel loads a process anywhere in its arc, `ferros_sys::relocate` fixes the RELA table), `has-thread-local` (TPIDR_EL0 is the thread pointer), panic = abort, x24–x28 reserved (x28 is the arc base the kernel hands every thread).
- **Kernel:** the trap `Frame` carries TPIDR_EL0 (`tpidr` at `[sp, #816]`, 832-byte frame) since 2026-10-27, so each thread of a process keeps its own thread pointer across entries and slices. Nothing else in the kernel changes for std: threads are `SYS_THREAD_SPAWN`, blocking is `WAIT`/`WAKE` on an address (the futex), time is `cntvct_el0`, memory is the arc.
- **Runtime crate `ferros_rt`** (this repo, no_std, `rustc-dep-of-std` feature as the motor port's `moto_rt`): syscalls, futex, thread spawn with a TLS block per thread and a join futex, the allocator (ported from `ferros_heap`), console stdio (grant slot 0), entropy (`fill_bytes`), exit, and the crt0 `_entry` that relocates, sets TPIDR_EL0 for the main thread and calls C `main`.
- **std fork:** `/mnt/Harbor/Code/ferros-rust/library` — rustc 1.97.0-nightly (d7f14d3d8 2026-05-15) rust-src, with `sys/pal/ferros` and a `target_os = "ferros"` arm in each `sys/<area>/mod.rs` dispatch (pal, thread, sync futex, thread_local native, time, alloc, random, stdio, exit). Everything a process has no business with on ferros — fs, net sockets, process, env, args, pipes, fds — is `unsupported`: Photon speaks the vault natively and its sockets are the `ip` server's endpoints behind a ferros `mio` backend.
- **Build:** `cargo +nightly build -Zbuild-std=core,alloc,std,panic_abort --target targets/aarch64-unknown-ferros.json` with `__CARGO_TESTS_ONLY_SRC_ROOT=/mnt/Harbor/Code/ferros-rust/library`.

## Build and run

```bash
tools/std-build.sh std/hello            # std from the fork, the ferros target, objcopy → stdhello.bin (65 KB)
cargo run -p ferros-bridge -- spawn std/hello/target/aarch64-unknown-ferros/release/stdhello.bin --name stdhello --entropy
```

The first std program ran 2026-10-27: `println!`, a `.tdata` thread-local read back exactly in main and in four spawned threads, joins, `Mutex` + `Condvar`, eight detached threads, `HashMap`, a 100 ms sleep that took 100 ms, `SystemTime` from the PMIC clock, exit 0 — 150 ms end to end.

## How the pieces fit

- **The image header** (`ferros_rt/src/start.rs`, `ferros_rt/ferros.ld`): offset 0 is `b __ferros_start`; offset 8 is `ferrosRT` and five absolute words the linker script computed — the zeroed tail (`.bss` + 16 + the heap), the TLS image size, the TLS block size, the TLS alignment, the heap size. The kernel reads the tail before the process runs (`process.rs image_tail`) and grows the arc by it, with the spawn input moved past it; the runtime reads the rest after. A flat image cannot carry `.bss`, and the input used to land right behind the code — this is how a std image gets its zero memory without sending it over USB. `--defsym __ferros_heap=<bytes>` sets the heap (2 MiB default).
- **The heap** is the tail after `.bss` up to the input, plus the arc's power-of-two slack past the input up to the kernel's 64 KiB entry stack. The main thread runs on a 1 MiB stack from the heap (`MAIN_STACK`); thread stacks and TLS blocks come from it too.
- **TLS** is native (`has-thread-local`, local-exec after lld's relaxation): per thread a block in TPIDR_EL0 laid out as lld computes TPOFF for AArch64 — 16 bytes of TCB, then the image at `16 + ((p_vaddr − 16) & (p_align − 1))`, `.tdata` copied in, `.tbss` zero. The kernel saves and restores TPIDR_EL0 in the trap frame.
- **Threads** end in assembly with no stack access after the state word settles (1 joinable-exited, 3 detached-exited), so the joiner or the next spawn's reaper can free the stack without racing a thread still on it.
- **The futex** is the kernel's WAIT/WAKE on a 64-bit word: `Futex = AtomicU64` for both widths.
- **Entropy**: the entry probes the grants for an Entropy capability (`--entropy`); without one `std::random` falls back to a counter-seeded mixer, fine for `HashMap`, not for keys.

## Traps met

- Under `relocation-model = pic` rustc reaches an extern static through a GOT entry — and the GOT entries are exactly what `relocate()` has yet to fix. `relocate()` finds its three linker symbols with PC-relative `adrp` in asm (`sys::pc_rel!`); the first hello faulted in the heap setup on an unrelocated `__bss_end`.
- cargo's build-std does not notice edits to a path dependency of std: after changing `ferros_rt`, clear `target/aarch64-unknown-ferros/release/{.fingerprint,deps}` of `ferros-rt-*` and `std-*` (or `rm -rf target`).
- `panic_immediate_abort` is a panic strategy now, not a std feature; `compiler-builtins-mem` must be in `build-std-features` or `memset` is undefined at link.
- The handle-guard pre-commit hook takes the better part of an hour over the 58 MB first commit of the std fork.

## The substrate, piece by piece (all 2026-10-27)

- **tokio** multi-thread runtime (`rt`, `rt-multi-thread`, `sync`, `time`, `macros`; no `net`, which needs mio and socket2 and a libc): `std/probe` ran 8 tasks over 4 workers with timers in 71 ms. Photon's sockets will be a ferros socket type over the `ip` server, not tokio's.
- **kete** over the kernel vault (`kete/src/ferros.rs`): the process's Store scope, every entry by the 32-byte address kete already derives, passed to the kernel as those bytes (SYS_STORE bit 5), values in 4 KiB pieces (staged writes, delete, list). The probe wrote a 10 KB value, read it back exact, listed, deleted. No `dirs`, no files.
- **getrandom** 0.2/0.3/0.4 custom backends in `ferros_rt` (`--cfg getrandom_backend="custom"` and the 0.2 `custom` feature from the program's side) — the kernel's entropy or an error, never the mixer.
- **fluor** `host-ferros` (`fluor/src/host/ferros/mod.rs`): the Android shell's twin over the display server's app surface (op 22, 16-bit gamma-2 working values from fluor's gamma-2.2 darkness bytes through one table) and the touch server's contact records (the first finger is the pointer; pinch waits for the input work). `std/fluordemo` draws text and counts taps in a 448 × 996 frame; spawn it with `--use display --use touch --dma app-surface --entropy`.
- **Memory**: `cachemap::POOL_BIG`, 512 MiB at G#A000_0000 in 2 MiB blocks (nothing in the device tree reserves G#9800_0000-G#C000_0000), a second buddy pool for arcs of 16 MiB and up (and small ones when the page pool is full); masked arcs stay in the page pool for their per-page W^X. The demo's 64 MiB arc (3.3 MB image + a 32 MiB heap by `--defsym __ferros_heap=`) is the first from it.
- **C in the graph**: `tools/std-build.sh` points the cc crate at `aarch64-linux-gnu-gcc` freestanding with `tools/std-sysroot/include` (string, stdlib, assert, stdio, inttypes, math, unistd, features: declarations only, the mem routines are compiler_builtins'), and the PQClean KEM suites build and run — ML-KEM-768 round-trips on the phone in 572 µs. Crates that spell C types through `libc` (pqcrypto-internals, rav1d) take the fork at `/mnt/Harbor/Code/ferros-libc` (libc 0.2.183 plus a `ferros` arm of scalar types, no functions): `[patch.crates-io] libc = { path = … }` and `cargo update -p libc`. fluor's `icon` feature stays off on ferros until rav1d is tried through that fork.

## Order

1. Kernel TPIDR_EL0 + the target JSON — done, flashed.
2. `ferros_rt` + the std platform layer; a std hello on the phone — done.
3. kete on the vault — done. 4. fluor host — done (the owner's eyes on the panel pending). 5. tokio — the runtime runs; a socket type over `ip` is next with Photon.
6. Photon with `cfg(target_os = "ferros")`: its sockets over `ip`, and out go reqwest/rustls/tungstenite/cpal/rav1e/rav1d/opus/winit/softbuffer/arboard/thread-priority/libc/dirs/memmap2 and tokio's `net` (`cargo tree` on the target lists them all); the PQClean C suites need a freestanding libc header set or a Rust port.
7. STOP at the input device: the owner goes over the text-input idea first.

## Encryption, as the owner laid it out

Drivers and key apps are encrypted by the ira on disk. Once the user attests, the identity (Photon's keys from the handle) unlocks Photon messages, Lumis settings, and the rest: a lock screen, nothing works before attesting. The display driver and the identity-keyed data meet there; design before building.
