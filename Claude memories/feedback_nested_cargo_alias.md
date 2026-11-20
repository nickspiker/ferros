---
name: nested-cargo-alias-merge
description: "A build tree under /mnt/Harbor/Code/ferros/target/... reads the main checkout's .cargo/config.toml too: cargo concatenates array aliases (el0 = build build ...), so `cargo el0` and the bridge's `input` (which runs it) fail there"
metadata:
  type: feedback
---

2026-11-20: in a farm under /mnt/Harbor/Code/ferros/target/agent-rawrec/int/farm/ferros, `cargo el0 -p ferros-payload-inject` failed with "unexpected argument 'build'": cargo merges the farm's .cargo/config.toml with the main checkout's (a parent directory) and joins the two `el0` arrays.

**Why:** cargo reads every .cargo/config.toml from the working directory up, and array values (aliases) concatenate.

**How to apply:** in such a farm build payloads with the alias spelled out (`cargo build --release --target aarch64-unknown-none --target-dir target/el0 --config ...`), and for the bridge's `input`, save the script with `--save` and spawn the farm's inject.bin with it (`--use touch --use display`), as the bridge does. Or keep farms outside the ferros tree.
