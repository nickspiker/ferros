---
name: payload-stack-on-bss
description: a process's first thread gets the kernel's 64 KiB STACK at its arc's top, directly on the image's last .bss statics; a deep payload overflows silently into them (display, akita, 2026-11-08: three starts, three different crashes); deep servers run on their own static stack
metadata:
  type: project
---

The kernel (process.rs `STACK`, 64 KiB) puts a process's first thread at the arc's top, and the arc is sized `CODE + code + tail + inline + STACK`: the stack's floor is the end of .bss, with no guard. A payload whose main thread goes deeper than 64 KiB writes over its own last statics without a fault.

**Found 2026-11-08 on the 8a:** the display server crashed at start three different ways (a layout band missing, then the glyph caches' tables "len 4") and showed every column as the circle's top row (orange in a dev build) after a few KB of new code; the statics at the top of .bss (`nm -n -S`: SLOW, CLOCK_DIRS, BANDS, …, GLYPHS, CON_GLYPHS) were exactly the ones hit, the faulting sp 8 KiB above them. Fixed in 7b6e455: `_entry` spawns the server on `MAIN_STACK` (512 KiB static) and parks.

**Why:** the symptoms look like logic bugs and move with the binary's layout, so bisecting by feature misleads (a change "fixes" it by shrinking or growing the image).

**How to apply:** random crashes in a payload's top-of-.bss statics, or ones that come and go as code is added: check the main thread's depth first (sp against the end of .bss). Any payload with deep frames or big stack buffers runs its work on its own static stack via `ferros_sys::thread_spawn`. A kernel guard page under STACK would make it fault loud instead.
