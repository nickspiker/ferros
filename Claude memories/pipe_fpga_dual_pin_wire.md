---
name: pipe-fpga-dual-pin-wire
description: PIPE wire on FPGA side uses two separate pins (TX out + RX in) bridged externally — not one bidirectional pin. Bridge GP0 is the single bidirectional end.
metadata: 
  node_type: memory
  type: project
  originSessionId: 8a47d2cf-0516-4c75-ab36-e6a43da45e08
---

The PIPE wire test rig is asymmetric at the pin level:

- **Bridge (RP2040)**: GP0 is a single bidirectional GPIO. PIO0 SM0 reads it (RX), PIO0 SM1 drives it (TX). Funcsel switching + pad OE controls.
- **FPGA (ECP5)**: two separate pins — one TX output, one RX input — bridged together externally on the breadboard to form the single wire that connects to bridge GP0 (through the 470 Ω series).

**Why:** Single-wire bidirectional half-duplex is the protocol model (mirror-XOR), but the FPGA's internal logic wants distinct TX and RX paths so the codec can XOR them cleanly. Putting them on the same physical pin via bidirectional IO buffer is harder than just using two pins and shorting them outside the chip.

**How to apply:**

- **Silent-FPGA test bitstream**: to truly silence the wire for bridge TX validation, tristate the FPGA TX pin (TRELLIS_IO with T=1, or just don't drive it). The current `top_pipe_silent_oled.v` assigns `pipe_pad = 1'b0` which is a hard CMOS LOW drive — it fights the bridge through the 470 Ω resistor. RX pin can still be active; doesn't drive the wire.

- **Mirror-XOR protocol implementation**: each side respects slot ownership — enclave drives during enclave slots, host drives during host slots. With dual TX/RX pins on the FPGA, the protocol can drive TX in enclave slots and float TX (high-Z) in host slots, while RX always listens. No CMOS contention when both sides obey the slot convention.

- **Bridge GP0 handling stays single-pin**: PIO0 SM1 drives via funcsel switching during host-owned slots, releases via funcsel back to SIO + OE clear during enclave-owned slots. The bridge already has this infrastructure (`pio_tx_start`/`pio_tx_stop`).

**Origin**: noted at the end of the 2026-05-25 session after `wire_loopback` proved bridge TX drives the wire end-to-end but couldn't lock due to contention with `top_pipe_silent_oled`'s hard LOW drive. Confirmed by `wire_tx_diag` showing 207 PIO transitions per 4096 SIO samples (TX working) alongside SIO direct-toggle baseline of 1024/1024.
