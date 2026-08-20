---
name: Number base prefix convention
description: All numeric output must use [base36]#[number] format — never 0x prefix. Hex=G#, decimal=A#, binary=2#, octal=8#.
type: feedback
---

All numbers in output (display, logs, diagnostics) MUST use `[base]#[number]` format where the base is expressed in base-36: G# for hex (16), A# for decimal (10), 2# for binary, 8# for octal.

NEVER use `0x` prefix — it confuses base-36 with hexadecimal (where 16 = G, not X).

**Why:** User considers `0x` a legacy convention that conflates base-36 and hex notation. Strong preference — "NO EXCEPTIONS."

**How to apply:** Any time you write code that formats/displays numbers, use `G#` for hex output instead of `0x`. For any other base, use the base-36 representation of that base followed by `#`. This applies to all output: FB console, UART, pstore, host tools, debug strings. Rust source code literals (`0xFF`) are language syntax and stay as-is.
