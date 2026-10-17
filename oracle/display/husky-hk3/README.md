# husky display oracle — google-hk3 on DSIM0, as Android drives it

Captured 2026-10-16 from the phone running Android (Magisk root), by `tools/sweep.sh` and `tools/sweep-retry.sh` over adb, decoded by `tools/decode.py`. This is the register oracle for ferros's display driver (DISPLAY-DESIGN.md): what the stock driver sends to the panel and what the DPU looks like, in every state. Nothing here is secret.

Each state directory holds:
- `dsi-commands.txt` — every DSI packet of the transition into that state (ftrace `dsi_tx`: payload bytes, packet type when not a long write, the driver's wait after it) with the time in ms from the first packet; `-- pps begin/end`, `-- init_cmd begin/end` and TE2 notes are the driver's own labels.
- `dpu-regdump.txt` — the DPU register dump after the transition (DECON0 main + shadow, WIN/WINCON 0–13, DPP 0–9 + DMA, HDR/HDRCOMM, SRAMC, DSIM, DSIM DPHY, DSIM LINK), from debugfs `crtc-0/reg_dump`.
- `panel.txt` — the panel's sysfs after the transition (refresh_rate, op_hz, power_state, panel_idle, min_vrefresh, te2_timing, backlight level, hbm_mode, dimming).
- `mode.txt` — SurfaceFlinger's active mode. `rates.txt` — DECON TE and frame-done interrupt counts over 2 s (these count DPU activity, not the panel's own TE: with the DPU hibernating on a still screen they fall to zero).

States: modes pinned through SurfaceFlinger (`service call SurfaceFlinger 1035 i32 N`: 0 WQHD 60, 1 WQHD 120, 2 FHD 120, 3 FHD 60), with panel idle off (`*_noidle`: the mode itself) and on (`*_idle`: the self-refresh / auto-rate state Android rests in, 7 s of stillness); idle floors `IDLE_MIN10` / `IDLE_MIN30` (min_vrefresh); `OPHZ60_NS` / `OPHZ120_HS` (op_hz, the panel's normal-speed 60 Hz mode and back); brightness 2 / 140 / 1000 nits through the framework, HBM on (4095) / 3700 / off through the backlight sysfs, dimming on/off; `AOD_enter` / `AOD_settled` / `AOD_exit` (doze: LP mode, 30 Hz, bin "low"); `OFF` / `OFF_settled` / `ON` (the full power-off and power-on with every delay — `ON` is the complete init sequence).

The sweeps restore the phone's state. `dphy.txt` is the DPHY tuning (cap peaking, de-emphasis, HS/CLK prepare counts, 400 mV reference, driver resistors). Battery current was sampled too but the screen content was not controlled, so it is not a display power profile; that needs a fixed full-screen pattern and is still to do.
