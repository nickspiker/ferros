---
name: feedback-vsf-not-for-framing
description: "VSF is for the assembled payload, not transport framing: encrypted packets go as lean as possible (counter, ciphertext, tag); control messages and what comes out the other side are VSF"
metadata:
  type: feedback
---

Owner, 2026-11-17, on Wāteo's tunnel: "the VSF rule doesn't apply to framing, in fact, if it's encrypted there's no need to frame since the chance of getting a shitttonnnn of 0's or 1's in a row is basically impossible when crypted, thus send it as lean and mean as you can. VSF is for the full meal deal once it comes out the other side fully assembled. Not that we can ditch the TCP framing or the USB ones today but hey."

**Why:** the "complete VSF files only" transport rule (AGENT.md) is about what is stored and what is delivered whole; a per-packet VSF header and hash on an encrypted data plane is pure overhead, and ciphertext needs no line-coding.

**How to apply:** an encrypted data plane (Wāteo's tunnel, any future one) sends the least the receiver needs to open a packet (counter for nonce and replay window, ciphertext, tag). Control messages (hellos, tickets, receipts) and every assembled payload stay complete VSF files. Existing TCP and USB framing stay as they are for now. docs/WATEO.md "The frames". Related: [[wateo_net]].
