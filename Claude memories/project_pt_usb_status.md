---
name: PT over USB status and DWC3 issues
description: Current state of Photon Transport over USB, what works, what doesn't, DWC3 quirks
type: project
---

## What works (2026-03-15)
- PT blast mode: SPEC→ACK→DATA blast→COMPLETE, no per-packet ACK
- Per-chunk BLAKE3 hash verification, lean EWE encoding, u64 bitmaps
- Single-shot diag: 15KB boot log over 33 DATA packets, reliable
- Multi-session diag: 20/20 consecutive transfers on same boot
- DWC3 ISP_IMI flag for short bulk IN packet completion
- 2 transfer resources on ep3 (avoids resource leak in TransferComplete handler)
- Outbound response: no SPEC ACK needed, no FIN needed, kernel auto-blasts
- Log buffer cleared after diag (prevents heap exhaustion with bump allocator)

## What doesn't work yet
- **Hot-reload (151 packets inbound)**: SPEC+ACK succeed, but DATA blast hangs
  - Bridge's `link.send().await` blocks on one of the 151 OUT transfers
  - Kernel's single-TRB ep2 can only receive one packet at a time
  - Each packet: ep2 TransferComplete → process → bulk_out_arm → next packet
  - With 151 packets blasted rapidly, the host retries on NAK but something stalls
  - Diag works (1 packet inbound) — the issue is multi-packet inbound blast only

## DWC3 quirks discovered
- **ISP_IMI required on bulk IN TRBs** — without it, short packets don't generate TransferComplete
- **STARTTRANSFER from within TransferComplete works** but leaks transfer resources — need 2 resources per endpoint
- **Host reconnect silently invalidates pending STARTTRANSFER** — no bus event, no TransferComplete. ep2 stays "armed" but DWC3 stops responding to OUT tokens
- **Periodic re-arm with ENDTRANSFER interferes with active transfers** — can't force-end during data reception
- **DSB barrier needed between cache clean and STARTTRANSFER** — DMA coherency

## Next steps for hot-reload
1. **TRB ring** instead of single static TRB — let DWC3 chain multiple receives
2. Or: **pace the bridge's blast** — add small delay between OUT sends so kernel can keep up
3. Or: **flow control** — kernel sends a "ready" signal after each N packets
4. The Linux DWC3 gadget driver uses UPDATETRANSFER with chained TRBs — study that approach

## Architecture notes
- Bridge side: nusb async, each send/recv is one USB transfer
- Kernel side: poll_event() loop, single-threaded, no interrupts
- Bump allocator (256KB) — never frees, Vec::clear() reuses capacity
- Screen debug (log.screen=true) blocks USB poll loop — keep disabled during transfers
