---
name: payload-stack-on-bss
description: the kernel sizes a no_std payload's arc from its flat image without .bss (image_tail reads only a ferros_rt header), so the 64 KiB stack at the arc's top can sit on the last statics; display crashed three ways on the 8a (2026-11-08); fixed by keeping .bss clear plus a start-up gap check; real fix is a kernel change
metadata:
  type: project
---

`process.rs build`: `need = CODE + code.len() + tail + inline + STACK`, `tail = image_tail(code)`, which is non-zero only for images carrying the `ferrosRT` header (std processes). A no_std payload's flat binary (`objcopy -O binary`) has no .bss, so the arc (next power of two) can end inside .bss, and the first thread's stack (64 KiB at the arc's top) overwrites the last statics silently.

**Found 2026-11-08 on the 8a:** the display's .bss ended 23 KB inside its stack (4 MiB arc). Crashes moved with every build (band table, glyph caches, an orange screen of repeated rows), each in the top-of-.bss statics. Dead ends that night: a separate 512 KiB static stack (pushed .bss past 4 MiB, masked addresses wrapped: worse); folding .bss into the image (the capsule went over the kernel vault's 3 MiB put limit and never stored, so every later test ran the old build: check for "stored" after every store put).

**Fix in place:** LOAD_STACK 256 → 128 KiB (image end 172 KB under the stack's top), `__image_end` in payload.ld, and `_entry` refuses to start, loudly, if the gap is under 64 KiB.

**Why:** the symptoms look like logic bugs and move with layout, so feature bisects mislead.

**How to apply:** random crashes in a payload's top-of-.bss statics: compare `__image_end` (or `nm -n` last symbol) + CODE + 64 KiB with the arc's power of two. Proper fix (kernel flash, owner's OK): the kernel learns each payload's memsz (capsule field from the ELF, or a header like ferros_rt's) and a guard page under STACK.

**Fixed in the kernel 2026-11-09 (70ad26a, flashed on the 8a):** every payload carries the header through `ferros_sys::entry!` and `__ferros_tail = ADDR(.bss) + SIZEOF(.bss) - __file_end + 16` (the alignment gap counted); the kernel refuses an image without it ("the image has no header") and puts a kernel-only guard page under every first-thread stack (payloads/stackprobe faults in it). `tools/payload-check.sh` runs before storing. **Before flashing this kernel on any device, re-store every process capsule from a current build** (`store ls`, then put each): an old capsule without the header will not start.
**The Pro too (2026-11-09):** 20 process capsules re-stored from current builds (all passed payload-check), booted on the old kernel, then the kernel flashed (flash_a_full: hv build, AVB sign, boot_a): all 12 processes up, the stripped image refused, stackprobe faulting in its guard.
