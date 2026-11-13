---
name: feedback-perl-pipe-delimiter
description: "never run perl s|…|…| when the pattern or replacement holds a pipe (Rust closures |c|, ||); use s{…}{…} or the Edit tool"
metadata:
  node_type: memory
  type: feedback
  originSessionId: a83238a3-4495-4510-b376-d5e6ba884c36
  modified: 2026-11-07T23:49:17.415Z
---

On 2026-11-07, three times in one session, perl substitutions in `s|pattern|replacement|` form mangled a Rust file's head. The pattern held `\|` (Rust closure pipes, or `||`), and perl ended the pattern there. Twice it was kvault.rs and once orb-tune's look.rs. Each time the file had to be restored from HEAD.

**Why:** a pipe in the text ends the pattern early, and the rest lands somewhere unexpected, often line 1.

**How to apply:**
- For multi-line Rust edits, use the Edit tool.
- If perl is needed, use brace delimiters `s{…}{…}` and write a literal pipe as `\x7c`.
- Check `head -1` of the file after any perl edit.
