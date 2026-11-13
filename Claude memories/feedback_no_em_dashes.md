---
name: feedback-no-em-dashes
description: "Owner hates em dashes; never write them in code strings, wire text or VSF `a` fields (they broke fgtw.org's error frames 2026-11-09)"
metadata:
  node_type: memory
  type: feedback
  originSessionId: 434f1d40-b023-4c88-80b3-11913c5cd18f
  modified: 2026-11-10T02:27:33.091Z
---

Never put em dashes (U+2014) or other non-ASCII punctuation in strings that go on a wire or into a VSF `a` (ASCII) field; prefer plain ASCII punctuation in new text generally.

**Why:** owner, 2026-11-09: "Fucknig hate those tings." The fgtw.org worker's error details carried em dashes in `a` fields; vsf's reader refuses non-ASCII in `a`, so every such refusal (device_owned among them) reached clients as an unreadable frame read as success. A released ferros 8a was offered mint and join words for another handle because of it.

**How to apply:** use `-`, `:` or a new sentence instead of an em dash in error strings, log lines and wire text. The worker's `to_ascii` now maps them, but don't rely on it. Related: [[feedback-no-web-encodings]].
