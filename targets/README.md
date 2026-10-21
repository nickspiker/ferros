# Target specs

Rust target JSON takes no comments, so the reasoning lives here.

## `aarch64-ferros.json` — EL0 payloads

The stock `aarch64-unknown-none` spec with one field changed: `features` carries the process ABI's reserved registers and the CPU's measured feature set in addition to the stock `+v8a,+strict-align,+neon`.

**Why it is a target and not a flag.** `-C target-feature` applies to the crates cargo compiles, but a payload also links `core`, `alloc` and `compiler_builtins`. Reserving x24-x28 is only sound if *every* instruction in the payload honours it: KERNEL.md's masking invariant is that x28 (arc base) and x27 (mask) hold at every instruction, so a jump landing anywhere still cannot escape the arc. Callee-saved does not give that — it only promises the value is restored before a function returns, and says nothing about the instructions in between. So the precompiled `core` is not good enough, `-Z build-std` is required, and the cleanest way to make the rules reach the rebuilt `core` is to put them in the target the whole graph is built for.

`tools/ferros-masked-build.sh` used to scrape the feature string out of `.cargo/config.toml` with `grep | sed`. That worked, but a change to the shape of that line would have silently yielded a different feature set, and the failure mode is a payload whose `core` scratches x28 — the same class of bug as 2026-10-15, where compiled code used x28 as a scratch register and a process reading its arc base mid-call got garbage.

**Verified** (2026-10-20, `hello_el0`): `compiler_builtins` compiled for this target contains zero references to x24-x28, and the payload's only reference is its own inline-asm read of x28. The image passes the sandbox checker, 0 refused.

**Three copies, two naming domains.** The same feature set is written three times, and only two of them can be merged:
- this target's `features` — rustc spelling, covers the payload and everything `build-std` rebuilds;
- `.cargo/config.toml` `rustflags` — rustc spelling, still needed for the kernel and for plain (unmasked) builds on stock `aarch64-unknown-none`;
- `ATTR` in `tools/ferros-masked-build.sh` — **LLVM** spelling, for `llvm-mc` when the masked assembly is assembled. LLVM and rustc disagree on several names (`+fullfp16` for `+fp16`, `+fp16fml` for `+fhm`, `+complxnum` for `+fcma`, `+fptoint` for `+frintts`), so this one is deliberately separate.

## Caveats

- Target-spec JSON is an unstable format and needs `-Z json-target-spec` (and `-Z build-std`, since a custom target has no precompiled `core`). Pinning the nightly in a `rust-toolchain.toml` is the next step if the format shifts under us.
- The kernel stays on stock `aarch64-unknown-none`: it runs at EL2 and the x27/x28 process ABI does not apply to it, so it keeps the faster precompiled `core`.
- Plain `cargo build` payload builds still use stock `aarch64-unknown-none` and its precompiled `core`, so they do **not** get this guarantee. That is the already-documented native-dev-capsule gap (KERNEL.md: "native dev capsules don't follow it yet"); masked builds are the ones that must be sound.

## Regenerating

```sh
rustc +nightly -Z unstable-options --target=aarch64-unknown-none --print target-spec-json
```
then re-apply the `features` line and the `metadata` block.
