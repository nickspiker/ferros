# openh264-sys2 for ferros

The `openh264-sys2` 0.9.8 crate (Ralf Biedert's bindings, BSD-2-Clause), forked for the ferros std target: its `src/` is the crate's own (the bindgen output for OpenH264 2.6's API, which 2.7 keeps unchanged) less the `libloading` paths (a prebuilt library loaded at run time has no ferros meaning), plus the line that links ferros_rt's shims, its `build.rs` builds Cisco's OpenH264 (BSD-2-Clause, LICENSE-openh264) for `aarch64-unknown-ferros` from a pinned checkout, with no C++ standard library, no exceptions and no RTTI; the C++ runtime, POSIX and stdio calls are ferros_rt's `cxx` feature.
An app takes it in place of crates.io's by `[patch.crates-io] openh264-sys2 = { path = ".../ferros/std/openh264-sys2" }` and keeps using the `openh264` crate as on every other platform.
How it is built, what it was measured at and what it needs: ferros docs/STD-TARGET.md, "C++ through ferros_rt".
