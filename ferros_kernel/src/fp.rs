//! Fingerprint-sensor reachability probe (SENSORS.md §Fingerprint, FINGERPRINT.md).
//!
//! The Goodix under-display sensor's normal-world driver (gf_spi.c) does no SPI at all: it binds as a platform_driver, only toggles a reset (gpp1-0) and forwards an IRQ (gpa7-3) over netlink, and carries zero register access. Capture and matching live in the Trusty TA, and the sensor's SPI controller is left out of the normal-world device tree entirely. The pin map puts that bus on USI1 of PERIC0 (spi_1, G#1088_0000: CLK/MOSI/MISO on gpp2-0/1/2, CS on gpp2-3), a controller the tree leaves `status = "disabled"` and no board file re-enables - the shape of a peripheral handed to the secure world.
//!
//! This answers one question: is PERIC0 USI1 reachable from ferros at NS-EL2, or has the bootloader locked it to the secure world (TZPC/TZASC)? That decides every path to raw frames. ferros already drives the identical s3c64xx IP on the HSI0 bus (spi.rs: touch, the amps, GNSS), so if USI1 reads back from NS, ferros-the-OS can drive the sensor itself once Trusty is gone, and a stage-2 MMIO trap could watch a normal-world owner. If it faults, only a logic analyser on the flex can see the bus.
//!
//! Read-only. The USI personality (which of UART/SPI/I2C a USI is wired to) lives in sysreg_peric0 (G#1082_1000), a syscon the normal world owns - always clocked, reads cleanly, tells us whether USI1 is even in SPI mode. The SPI block itself (G#1088_0000) is the one that may be secure-locked, so every read of it goes through a fault-safe probe. On the handoff build the synchronous-abort fault sink is terminal by design (main.rs .Lexc_recover warm-resets to preserve the report), so it is disarmed for the window of the probe and restored right after: a secure-locked read then skip-and-counts, and probe32 reports it as a fault instead of resetting the phone.

use crate::platform::append_hex;
use alloc::vec::Vec;

/// USI SW_CONF words in sysreg_peric0 (zuma-usi.dtsi usi-offset): USI1 @ +0, USI2 +4, USI3 +8, USI4 +C, USI5 +10.
const SYSREG_PERIC0: usize = 0x1082_1000;
/// The earlycon console (serial_0 = PERIC0 USI0): a known normal-world, clocked PERIC0 MMIO block, the control that proves a read can succeed.
const USI0_UART: usize = 0x1087_0000;
/// The fingerprint candidate (spi_1 = PERIC0 USI1).
const USI1_SPI: usize = 0x1088_0000;

// s3c64xx SPI register offsets (same IP as spi.rs).
const CH_CFG: usize = 0x00;
const MODE_CFG: usize = 0x08;
const SLAVE_SEL: usize = 0x0C;
const STATUS: usize = 0x14;
const USI_CON: usize = 0xC4;

/// USI SW_CONF personality (spi-s3c64xx.c USI_SW_CONF_MASK = 0x7: bit0 UART, bit1 SPI, bit2 I2C).
fn mode_name(sw: u32) -> &'static str {
    match sw & 0x7 {
        0x1 => "UART",
        0x2 => "SPI",
        0x4 => "I2C",
        0 => "reset/none",
        _ => "mixed?",
    }
}

/// A fault-safe read that is also safe on the handoff build: the terminal sync-abort sink is disarmed for the read so a secure-locked access skip-and-counts (main.rs .Lexc_recover) and comes back as None, not a warm reset. SError external aborts are counted regardless of the sink.
fn probe(a: usize) -> Option<u32> {
    let saved = unsafe { core::ptr::read_volatile(&raw const crate::__exc_sink) };
    unsafe { core::ptr::write_volatile(&raw mut crate::__exc_sink, 0) };
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    let r = crate::aoc::probe32(a);
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    unsafe { core::ptr::write_volatile(&raw mut crate::__exc_sink, saved) };
    r
}

/// One labelled register read: value or FAULTED. Returns whether it read back.
fn reg(out: &mut Vec<u8>, label: &[u8], a: usize) -> bool {
    match probe(a) {
        Some(v) => {
            append_hex(out, label, v);
            true
        }
        None => {
            out.extend_from_slice(label);
            out.extend_from_slice(b"FAULTED\n");
            false
        }
    }
}

/// `diag fp`: is the fingerprint's SPI controller reachable from ferros at NS-EL2?
pub fn report(out: &mut Vec<u8>) {
    // The PERIC0 USI personality map, from the syscon the normal world owns (always clocked).
    out.extend_from_slice(b"PERIC0 USI modes (sysreg_peric0):\n");
    let names = [b"  USI1=".as_slice(), b"  USI2=", b"  USI3=", b"  USI4=", b"  USI5="];
    let mut usi1_spi = false;
    for (i, nm) in names.iter().enumerate() {
        out.extend_from_slice(nm);
        match probe(SYSREG_PERIC0 + i * 4) {
            Some(v) => {
                out.extend_from_slice(mode_name(v).as_bytes());
                append_hex(out, b" (SW_CONF G#", v);
                // append_hex ends the line; close the note on the next.
                let last = out.len() - 1;
                out[last] = b')';
                out.push(b'\n');
                if i == 0 && v & 0x7 == 0x2 {
                    usi1_spi = true;
                }
            }
            None => out.extend_from_slice(b"FAULTED\n"),
        }
    }

    // Control: a known normal-world, clocked PERIC0 block must read, or the probe itself is the thing that is broken.
    let control = reg(out, b"CONTROL USI0-UART[0] (console, known NS)=", USI0_UART);

    // The decider: the USI1 SPI block. FAULTED here with the control reading back = secure-locked (or this USI's clock gated); reads back = normal-world reachable.
    out.extend_from_slice(b"USI1 SPI block (G#10880000):\n");
    let a = reg(out, b"  USI_CON=", USI1_SPI + USI_CON);
    let b = reg(out, b"  CH_CFG=", USI1_SPI + CH_CFG);
    let c = reg(out, b"  MODE_CFG=", USI1_SPI + MODE_CFG);
    let d = reg(out, b"  SLAVE_SEL=", USI1_SPI + SLAVE_SEL);
    let e = reg(out, b"  STATUS=", USI1_SPI + STATUS);
    let reached = a || b || c || d || e;

    out.extend_from_slice(b"VERDICT: ");
    if !control {
        out.extend_from_slice(b"inconclusive - the control read faulted, so the probe path, not USI1, is suspect\n");
    } else if reached {
        out.extend_from_slice(b"USI1 SPI is NORMAL-WORLD REACHABLE - ferros can drive the sensor itself (new spi.rs Port) and a stage-2 trap could watch a NS owner");
        if !usi1_spi {
            out.extend_from_slice(b"; note USI1 is not currently in SPI mode, so this may not be the fingerprint's bus");
        }
        out.push(b'\n');
    } else {
        out.extend_from_slice(b"USI1 SPI is NOT reachable from NS-EL2 - secure-locked (TZPC/TZASC) or its USI clock is gated; only a logic analyser on the flex can see this bus, and ferros-native driving would need the secure assignment cleared first\n");
    }
}
