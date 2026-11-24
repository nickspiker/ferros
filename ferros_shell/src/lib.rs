//! The shell's processes and the grants each is started with: one table, read by init when it starts them at boot (payloads/init) and by the kernel when it starts one again (ferros_kernel service.rs `shell_grants`: an approve's attest screen, a restart at a death, `diag restart`).
//! Until 2026-11-24 each kept its own copy, and they drifted: the kernel's attest screen had 13 grants where init's had 14, the 14th `use nfc`, so every attest screen the kernel started again had no NFC and a kept tag never attested there (docs/NFC.md "A tag that did nothing at the attest screen").
//!
//! A slot number is a place in init's own grants, counted from 1 (slot 0 is the console), in the order the kernel's `home_grants` (service.rs) lists them; the process receives them as its slots 1, 2, ... in the order given here, the order its own header expects.
//! The kernel's special cases stay the kernel's: the attest screen's `surfaces` passed as one `surface` each, the arena as a 32 MiB block of its own, the identity service's facets after its last slot, the torch after the map's, and the processes init does not start (torch, audio, camera, init itself).
#![no_std]

/// Each process: its name, its vault key, and which of init's slots it receives, in the order its own grant slots expect them (payloads/display, payloads/touch, payloads/map headers).
/// init starts them in this order.
pub const SHELL: [(&[u8], &[u8], &[u64]); 12] = [
    // What runs before a person is present (AUTH.md, the device class): the screen, touch and the attest screen, then the network and the radios.
    (b"display", b"display", &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 21, 22, 25, 26, 27, 97, 98, 99, 100, 101, 102, 103, 20]),
    (b"touch", b"touch", &[14, 15, 16, 23, 91]),
    // The attest screen, before anything else draws: 1 use display, 2 use touch, 3 a surface (init's `surfaces` passes as one of its own), 4 the vault scope `ahau.`, 5 the vault scope `fgtw.`, 6 the ira key, 7 settings (its layout switch writes the device's layout), 8 use fgtw (the identity service's verdict on the identity, and the mint), 9 the identity unlock right, 10 a second surface (the keyboard's back buffer; `surfaces` again), 11-13 the GPU service, the layer arena, use wlan, 14 use nfc (attest with a tag, AUTH.md) (payloads/ahau).
    (b"ahau", b"ahau", &[17, 18, 22, 51, 48, 49, 19, 47, 85, 22, 24, 68, 44, 80]),
    // The USB driver (payloads/usb, USB.md): it runs the controller and serves Photon Transport to the bridge, handing each command to the kernel. 1 the controller's registers, 2 its DMA memory, 3 its interrupt, 4 the kernel's PT commands, 5 the `usb` power switch. The kernel has no USB of its own: should the driver end, the keeper below starts it again.
    (b"usb", b"usb", &[86, 87, 88, 89, 90]),
    // The WiFi driver's slots in its own order: 1-12 hardware, 13 entropy, 14 DMA, 15 serve wlan, 16 the vault scope, 17 the ira key (its per-network MAC), 18 Bluetooth's power (off before WL_REG_ON is cycled) (payloads/pcie1 header). It brings the chip up now and joins once the identity's network credential can be read.
    (b"pcie1", b"pcie1", &[29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 28, 43, 49, 83]),
    // 1 use wlan, 2 serve udp, 3 entropy, 4 use rmnet (payloads/ip).
    (b"ip", b"ip", &[44, 45, 41, 52]),
    // The radios: the NFC driver 1 serve nfc, 2 hsi2c_0, 3 gpio-alive, 4 its power (payloads/nfc); the Bluetooth driver 1 serve bt, 2 the APM UART, 3 vendor_a (the patch), 4 its power, 5 entropy (payloads/bt).
    // The sensors process (payloads/sensors): the USF conversation with the AoC once the kernel has the AoC online; it publishes the samples the kernel answers SYS_FACTS and SYS_LIGHT from. 1 the AoC's IPC block, 2 its mailboxes, 3 the vault scope `aoc.`, 4 the Sensors right, 5 the `sensors` switch. Should it fail, the kernel keeps the conversation.
    (b"sensors", b"sensors", &[92, 93, 94, 95, 96]),
    (b"nfc", b"nfc", &[77, 78, 67, 79]),
    (b"bt", b"bt", &[81, 82, 76, 83, 41]),
    // The cellular modem's driver (payloads/modem): it powers the CP, registers and serves the data call on rmnet; ip takes it as its second link.
    (b"modem", b"modem", &[53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 40, 69, 70, 71, 72, 73, 74, 75, 76]),
    // What reads the person's space (the identity class), each when it may (`wait_person`). fgtw, the identity service, once an identity is unlocked: it is part of attest, looking the identity up and joining its fleet (its chain and key; not the Photon app, which a person stows): 1 use udp, 2 the vault scope `fgtw.`, 3 the ira key, 4 serve fgtw, 5 entropy (the fleet-key minting), 6 use nfc, 7 use bt, 8 the identity unlock right, 9 the vault scope `map.` and 10 the vault scope `kists.` (the person's identity space, joined to their fleet's: payloads/fgtw space.rs). The map only once the device is also bound to that identity (owner, 2026-11-04: "if it's not attached to an identity, not usable"): its arrangement, 1 use display, 2 use touch, 3 settings, 4 the vault scope `map.`, 5 use fgtw (the Fleet page), 6 the vault scope `kists.`, 7 use launch, 8 use kists, 9 use nfc (the NFC page's "add a tag", AUTH.md "Attest with a tag"; the kernel puts the torch after it).
    (b"fgtw", b"fgtw", &[50, 48, 49, 46, 41, 80, 84, 85, 20, 107]),
    // The map: 1 use display, 2 use touch, 3 settings, 4 the vault scope `map.`, 5 use fgtw, 6 the vault scope `kists.`, 7 use launch, 8 use kists, 9 use nfc, 10 use wlan (the WiFi page's picker, 2026-11-18: payloads/map wifi.rs); the kernel adds the torch after them.
    (b"map", b"map", &[17, 18, 19, 20, 47, 107, 106, 108, 80, 44]),
];

/// The slots `name` is started with, or None for a process not in the shell.
pub fn slots(name: &[u8]) -> Option<&'static [u64]> {
    SHELL.iter().find(|s| s.0 == name).map(|s| s.2)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No two entries share a name: `slots` and init's keepers find a process by its name, and the second would never be found.
    #[test]
    fn names_unique() {
        for (k, a) in SHELL.iter().enumerate() {
            assert!(SHELL[k + 1..].iter().all(|b| b.0 != a.0), "{:?} twice", core::str::from_utf8(a.0));
        }
    }

    /// Every slot is one of init's grants: 0 is the console, never a grant to pass on.
    #[test]
    fn slots_from_one() {
        for s in SHELL.iter() {
            assert!(s.2.iter().all(|&n| n >= 1), "{:?} passes slot 0", core::str::from_utf8(s.0));
        }
    }

    /// The attest screen keeps its NFC grant (slot 80, use nfc) wherever it is started: the grant whose absence from the kernel's copy hid every kept tag on an approve (2026-11-24).
    #[test]
    fn attest_screen_has_nfc() {
        assert_eq!(slots(b"ahau").map(|s| s.last()), Some(Some(&80)));
    }
}
