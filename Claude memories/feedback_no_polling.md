---
name: feedback-no-polling
description: No polling, no timers unless PROVEN necessary: push or doorbell (a held call answered on change, an interrupt, a wake); never a loop that asks every N ms
metadata:
  type: feedback
---

Owner, 2026-11-10, after the map's once-a-second fgtw polls froze two-finger gestures and the Fleet page: "Why do we poll on a timer at all???" and "No polling! No timers unless PROVEN necessary! It should be push or doorbell, no constant draining of battery and crowding network resources."

**Why:** a poll costs power and network whether or not anything changed, queues behind whatever the server is doing (fgtw answered in 2+ s and the map's pass sat in it), and still lags the change by up to its period.

**How to apply:**
- Between processes: a watch call held by the server and answered when its state changes (a generation counter in, the new state out), or an interrupt / `ferros_sys::wait` + `wake` doorbell. Never `call` every N ms "to see".
- Inside a process: block on what can arrive (recv with no NOTHING spin, irq_wait, wait on a word); a timer only for a real deadline (a retransmit, a protocol's own timeout), with the proof in a comment.
- Network: send on change; never a periodic "anything new?" to fgtw.org or a peer.
- When touching existing code that polls, flag it and convert it rather than adding another poll. Related: [[feedback-no-animations]] (no timers in the UI), [[feedback-measure-before-fixing]].
