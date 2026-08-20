---
name: pipe-oled-fb-ce-gating
description: "PIPE OLED greyscale driver advances per-CE — the consumer's framebuffer read MUST be gated by the same CE pulse or byte order wraps."
metadata: 
  node_type: memory
  type: feedback
  originSessionId: 6adb97ac-ad4e-416a-9091-e8b2900d78dc
---

`rtl/oled_greyscale_driver.v` advances its gather FSM per-CE (every CLK_DIV cycles), so its combinational `fb_raddr` output also updates only per-CE. The consuming top's framebuffer read MUST be gated by the same CE pulse — `always @(posedge clk) if (ce_pulse) oled_rdata <= fb[oled_raddr]`. If you do `always @(posedge clk) oled_rdata <= fb[oled_raddr]` ungated, fb_rdata races ahead each clock and the driver captures bytes in order 1,2,3,4,5,6,7,0 instead of 0,1,2,3,4,5,6,7. The shift-right packing maps this to bit positions mirror-reflected around bit 4 in each OLED page byte.

**Why:** Caught while integrating the AA glyph renderer into top_pipe_ira for ira hex display. The bug ALSO exists in top_pipe_ring_scope.v but is invisible there because ring scope displays random entropy — byte-order wrap on random data produces no perceptible artifact. Only structured data (gradients, glyphs) makes it visible.

**How to apply:**
- Whenever you instantiate `oled_greyscale_driver` (or any module that takes a combinational raddr output but runs its own state per-CE), replicate its CLK_DIV divider in the top and gate the consumer's BRAM read with that CE pulse. Example for CLK_DIV=7:
  ```
  localparam integer OLED_CLK_DIV = 7;
  reg [2:0] ce_cnt = 0;
  wire ce_at_top = (ce_cnt == OLED_CLK_DIV - 1);
  always @(posedge clk) ce_cnt <= ce_at_top ? 0 : ce_cnt + 1;
  reg ce_pulse = 0;
  always @(posedge clk) ce_pulse <= ce_at_top;

  always @(posedge clk) if (ce_pulse) oled_rdata <= fb[oled_raddr];
  ```
- Symptom signature for this bug: byte-position-mirror around bit 4 within each OLED page byte. Test by displaying a known per-row brightness pattern (top_oled_diag does this cleanly).
- If you ever rewrite oled_greyscale_driver, consider exposing the CE pulse as an output port so consumers don't have to replicate the divider.

Related: [[pipe-slow-is-security]] (the slow clock domain is intentional throughout PIPE).
