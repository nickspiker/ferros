# USB out of the kernel

The USB device controller (DWC3 on HSI0) and Photon Transport, the bridge's link, move from the kernel into a process (owner, 2026-10-31). Two reasons:
- **The kernel goal:** the kernel keeps only what a kernel must, plus the killswitch.
- **Throughput:** the data path is being rewritten anyway, from one 512-byte buffer re-armed per packet to a ring of large buffers woken by the controller's interrupt.

## Development flow

- **Boot:** dev builds go to ferros by themselves 1 s after the Ready? gate, so the gate never needs the host.
- **Android:** `diag android` (set the flag, reboot), over the process's link.
- **Later:** handing straight to Android without a reboot is a separate piece of work (booting Linux from ferros).
- **Recovery:** while the driver settles, recovery does not depend on it. The killswitch, fastboot and reflashing slot b all work without ferros's USB.

## Steps

1. **[done, 7d1fb9e] DMA memory supplied by the owner.** `ferros_hal::usb` takes its DMA memory as one `UsbMem` block its owner hands over (`set_usb_mem`), instead of fixed statics.
   - It does no cache maintenance: the block is non-cacheable, and the instructions trap at EL0.
   - `ep_cmd` issues a store barrier before every endpoint command, and `poll_event` a load barrier after the event count. Without them, bulk IN wedged after 442 transfers.
2. **[done] The grants:**
   - `usb-regs`: the controller's registers mapped for EL0, its 2 MiB block in a new level-2 table for the first GiB (`cachemap.rs` `USB_REGS`, `L2_LO`).
   - `usb-dma`: 2 MiB of non-cacheable EL0 memory at G#8B20_0000 (`USB_DMA`).
   - `usb-dwc3`: the controller's interrupt, INTID 434.
   - Verified with `payloads/usb p`: GSNPSID read at EL0 (G#33313130, "3110"), a DMA word written and read back.
3. **[done] The kernel's PT commands behind an endpoint** (`ptlink.rs`, `SYS_PT` 25, the `--pt` grant). The process hands each assembled command to the kernel and gets the raw reply; the kernel runs the same dispatch its own USB feeds (`service.rs` `run`, now at the end of each pass).
   - Op 0 hands a command over (up to 1 MiB on the heap; up to 6 MiB in the PT receive region, only while the process owns the controller). Op 1 takes the reply from an offset: its full length, bit 32 set when the command waits to act, NOTHING while it runs.
   - Reload, reboot and `diag android` are "reply, then act": they answer first, and act when the process hands the same command over again with op 3, after its reply has gone out.
   - The `usb` power switch hands the controller over. On: the kernel stops touching DWC3, closes the Type-C USB switch, brings the eUSB2 PHY up again (the host sees the kernel's device leave) and opens the DMA path. Off: the kernel re-initialises its own USB on the next pass. A process named `usb` gone while it holds the controller is noticed within 4 s, and the kernel takes it back the same way.
   - DIAG: `KERNEL_USB` (1 the kernel drives the controller, 0 the process), `PT_PROC_CMDS`.
   - Verified on a hot-reloaded kernel (`payloads/usb` inputs `d`, `h`, `k`): a `diag` through the endpoint, 3,671 bytes back in 2.1 ms; the controller taken for 2 s and given back, the phone back on the bus about 2 s later; taken by a process that then exited, reclaimed in the same pass; the kernel's own PT unchanged (2 MiB vault round trip byte-exact, 0.7 s each way; reload to the next generation).
4. **`payloads/usb`:**
   - the controller from cold;
   - a ring of large OUT buffers and batched IN;
   - interrupt wake-ups;
   - PT to the bridge exactly as now.

   A dev switch picks kernel or process USB while it settles.
5. **Switch over.** init starts the driver, the kernel stops touching the controller, and the kernel's USB code goes: the event loop, the watchdogs, the gate's host answer and the PT receive. Then the commands move out one at a time, the store first since it is already a clean bytes-to-bytes function.

## Facts

| What | Value |
|---|---|
| Controller | DWC_usb31 at G#1121_0000, 64 KiB |
| USBCON | G#1110_0000 |
| eUSB2 PHY | G#1111_0000 |
| USB SYSMMU | G#1104_0000 |
| HSI0 S2MPU | G#1107_0000 |
| Repeater | hsi2c_11 (G#10CB_0000), target G#3E; the TCPC is G#25 on the same bus |
| Link | High Speed (480 Mbit/s); about 35-40 MB/s of bulk in practice. PT moved 215 KB/s until 2026-10-31 (the bridge's 1 ms pacing) and 3.4 MB/s since. |
| Kernel PT dispatch | One inline chain in `service.rs` `run()`. Only `store_command` is a clean bytes-to-bytes function; reload, reboot and `diag android` drive the controller directly. |
