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
   - `usb-dma`: non-cacheable EL0 memory (`USB_DMA`): 2 MiB at G#8B20_0000 at first, 8 MiB at G#8B80_0000 since step 4.
   - `usb-dwc3`: the controller's interrupt, INTID 434.
   - Verified with `payloads/usb p`: GSNPSID read at EL0 (G#33313130, "3110"), a DMA word written and read back.
3. **[done] The kernel's PT commands behind an endpoint** (`ptlink.rs`, `SYS_PT` 25, the `--pt` grant). The process hands each assembled command to the kernel and gets the raw reply; the kernel runs the same dispatch its own USB feeds (`service.rs` `run`, now at the end of each pass).
   - Op 0 hands a command over (up to 1 MiB on the heap; up to 6 MiB in the PT receive region, only while the process owns the controller). Op 1 takes the reply from an offset: its full length, bit 32 set when the command waits to act, bit 33 when it gave no reply at all (as the kernel's own USB gives none after a reload or reboot), NOTHING while it runs.
   - Reload, reboot and `diag android` are "reply, then act": they answer first, and act when the process hands the same command over again with op 3, after its reply has gone out.
   - The `usb` power switch hands the controller over. On: the kernel stops touching DWC3, closes the Type-C USB switch, brings the eUSB2 PHY up again (the host sees the kernel's device leave) and opens the DMA path. Off: the kernel re-initialises its own USB on the next pass. A process named `usb` gone while it holds the controller is noticed within 4 s, and the kernel takes it back the same way.
   - DIAG: `KERNEL_USB` (1 the kernel drives the controller, 0 the process), `PT_PROC_CMDS`.
   - Verified on a hot-reloaded kernel (`payloads/usb` inputs `d`, `h`, `k`): a `diag` through the endpoint, 3,671 bytes back in 2.1 ms; the controller taken for 2 s and given back, the phone back on the bus about 2 s later; taken by a process that then exited, reclaimed in the same pass; the kernel's own PT unchanged (2 MiB vault round trip byte-exact, 0.7 s each way; reload to the next generation).
4. **`payloads/usb`** (input `s`). First cut done: the process takes the controller (`power usb on`), resets and runs it with `ferros_hal::usb` unchanged (no allocator; the module no longer needs one), and serves PT. Each command is assembled in its DMA area and handed to the kernel by address (`SYS_PT` takes `DMA_ADDR | address` inside a DMA region the caller holds; an arc offset cannot name memory outside the arc). Reload and reboot act after the COMPLETE is out. While it settles, the process gives the controller back when the host never configures it (four tries), when the kernel refuses a command, or when a reply takes over 16 s. Verified: DIAG through it (`KERNEL_USB=0`); a 2 MiB vault round trip byte-exact, put 1.5 s and get 4.4 s (one 512-byte packet a pass, a sleep while the endpoint is busy); a kernel reload through it, back to kernel USB at the next generation.
   - **[done] Runs.** After a SPEC is acknowledged, the OUT endpoint is armed for up to 1024 packets at once, straight into one of two 512 KiB run buffers (the next run fills while the last is decoded) (`ferros_hal::usb` `bulk_out_arm_at`). Replies go out the same way (`bulk_in_send_at`). The bridge writes and reads 1 MiB per bulk transfer (`RUN_BYTES`); the kernel's own USB takes the same transfers a packet at a time.
   - **[done] Cached command area.** Measured per phase (the driver writes a console line per large command): decoding straight out of non-cacheable memory cost 2.2 s of CPU per 2 MiB. Now only the controller's block and the run buffer stay non-cacheable (`USB_DMA_NC`, 2 MiB), the command area is cached, and each packet moves between the run buffer and the stack 16 bytes a load.
   - **Then** (before the host fixes below): a 2 MiB vault round trip, byte-exact.

     | | Process USB | Kernel USB |
     |---|---|---|
     | put | 0.64 s (wire 238 ms; the kernel's seal and write 266 ms) | 0.56 s |
     | get | 0.43 s (reply out 239 ms) | 0.78 s |

     About 8.8 MB/s each way on the wire; the CPU's share is 30-45 ms.
   - **[done] The 8.8 MB/s ceiling was the host.** Ruled out first:
     - the CPU;
     - the FIFOs: GTXFIFOSIZ1 G#253 and GRXFIFOSIZ0 G#209 words, about 9 packets each;
     - the bus bursts: GSBUSCFG0 G#2222_0001, undefined-length INCR.

     What it was, measured one change at a time on 2 MiB each way:

     | Change | In | Out |
     |---|---|---|
     | Before | 271 ms | 239 ms |
     | The bridge keeps four 128 KiB transfers in flight (usb.rs `send_queued`, `recv_queued`) | 271 ms | 107 ms |
     | The driver arms the next 512 KiB run in a second buffer before decoding the last | 240 ms | 107 ms |
     | BLAKE3 and `ferros_pt` built for speed in the bridge's dev builds (Cargo.toml) | 100 ms | 108 ms |

     About 20 MB/s each way. A 2 MiB vault round trip was then put 0.38 s (the kernel's seal and write 265 ms of it) and get 0.19 s; the kernel's own USB took 0.56 s and 0.78 s.
   - **[done] The clocks.**
     - The driver's 78 ms of CPU per 2 MiB received was the rest clock: the kernel's clock policy counted only its own PT transfers as activity, so the workers sat at 402 MHz (and stop when the screen is dark).
     - The driver now says a transfer is moving (`SYS_PT` op 2, `ptlink::MOVING_AT`) at each SPEC and each run, and the policy counts it.
     - Before that, 64-byte vector loads for the non-cacheable copies took the CPU only from 78 to 72 ms. The USB DMA is not cache-coherent here (no `dma-coherent` in zuma's device tree), so the run buffers stay non-cacheable.

     | Measured on 2 MiB | Before | After |
     |---|---|---|
     | In | 99 ms (CPU 72 ms) | 73 ms (CPU 28 ms) |
     | Out | 108 ms | 72 ms |
     | The kernel's vault write | 265-544 ms | 154 ms |

     About 29 MB/s each way. A vault round trip is now put 0.25 s and get 0.13 s.
   - **[done] init starts it.** It runs from the vault capsule `usb`, with init's slots 86-90 (registers, DMA, interrupt, PT commands, power), and `diag "restart usb"` starts it again. Slot b carries this kernel since 2026-10-31. A cold boot comes up with the process serving USB (`KERNEL_USB=0`), the kernel's own USB only until init starts it. The dev switch is the vault itself: with no `usb` capsule, the kernel keeps USB; a driver that fails gives the controller back.
   - **[done] Interrupt wake-ups.** With no event in a pass and no reply awaited, the driver acks the controller's line and waits on it (`irq_wait`, a quarter-second deadline for its own checks) instead of sleeping 122 µs at a time. Measured: 56-65 wake-ups by the interrupt over a 2 MiB round trip, a `diag` round trip 31 ms, the wire unchanged.
   - Still to do: the controller from cold (today the kernel's `usb` switch readies the PHY).
   - Hazard: a hot reload issued 8 s into a fresh boot left the new kernel hung with the kernel lock held by a worker core (PREV_NOTE_0 = 8). The same reload a minute later was fine. Not yet explained; probably the AoC/Trusty start still running.
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
