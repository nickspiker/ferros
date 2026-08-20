---
name: No manual line wraps in comments
description: Don't manually line-wrap doc comments or block comments in source files (Rust, Verilog, shell, etc.)
type: feedback
originSessionId: 6adb97ac-ad4e-416a-9091-e8b2900d78dc
---
NEVER insert manual line breaks inside a sentence or paragraph in comments. None. Zero. Each paragraph is one long line; the editor wraps it. The user is extremely opinionated and will react badly if this is violated.

**Why:** Manual wraps waste rebuild time when you edit them (every wrap edit is a multi-line diff), and they're unnecessary visual clutter — the editor already soft-wraps. The user has called this out forcefully more than once.

**How to apply:** When writing or editing Rust doc comments (`//!`, `///`, `//`), Verilog block comments, shell here-docs, or any prose embedded in code: each sentence stays on ONE line, however long. Multiple sentences in the same paragraph also stay on one line. Only break lines for paragraph boundaries (blank `//` between groups) or for genuinely structural content like bulleted lists or ```text ASCII diagrams where each item is its own logical line.

**Automated enforcement (ferros, as of 2026-08-19):** `ferros/scripts/comment-gate.sh` (ported from photon's `scripts/lib/comment-gate.sh`) is the ratchet — baseline ZERO, flags any comment line >60 cols ending in a word char whose next same-marker line continues in lowercase. Run it standalone or via the pre-commit hook (`scripts/install-hooks.sh` sets `core.hooksPath -> scripts/hooks`; MUST be run once per clone or wraps silently accumulate — that is exactly what happened here). Fix = join the wrapped sentence onto one line; never add an allowlist. Photon also keeps `photon/Claude memories/{feedback_no_comment_wraps.md,no-wrapped-comments.md}`.

**Example — wrong:**
```rust
//! Talks to `top_pipe_codec.v` via FT232H MPSSE in batched form: one big USB
//! write programs an entire run (drive + sample + drive + sample × N) at
//! slot-rate timing using MPSSE "Clock For N Bytes" as a precise delay.
```

**Example — right:**
```rust
//! Talks to `top_pipe_codec.v` via FT232H MPSSE in batched form: one big USB write programs an entire run (drive + sample + drive + sample × N) at slot-rate timing using MPSSE "Clock For N Bytes" as a precise delay.
```
