//! Android's bootloader control block (AOSP bootable/recovery bootloader_message.h) at the start of the `misc` partition: what recovery Android asked for, read so the person decides on ferros's screen (docs/BOOT.md "Recovery requests from Android"; owner, 2026-11-02 and 2026-11-22).
//!
//! Android's format, external input: `command`, 32 bytes at 0 ("boot-recovery" makes the bootloader start recovery at every boot until recovery clears it), `status`, 32 at 32, `recovery`, 768 at 64: "recovery\n" then one argument a line ("--wipe_data", "--reason=...", ...), NUL-terminated.
//! The request is classed by the proof it needs (owner, 2026-11-02): a wipe of Android's data or a crash rescue needs the attestation; anything that can change what boots needs a second fleet device as well; a remote wipe is refused, as no one chose it at the device.

/// The `command` field.
pub const COMMAND: core::ops::Range<usize> = 0..32;
/// The `recovery` field: the arguments.
pub const RECOVERY: core::ops::Range<usize> = 64..64 + 768;

/// No recovery request (no "boot-recovery" command).
pub const ASK_NONE: u8 = 0;
/// A factory reset: `--wipe_data` (Settings' erase all data, `RecoverySystem.rebootWipeUserData`). Attestation.
pub const ASK_WIPE: u8 = 1;
/// RescueParty's `--prompt_and_wipe_data`: Android could not start; recovery offers "Try again" or "Factory data reset" to the person. Attestation.
pub const ASK_RESCUE: u8 = 2;
/// A wipe a device admin asked for (Find My Device and other remote wipes: `--reason=` names DevicePolicyManager). Refused: no one at the device chose it.
pub const ASK_REMOTE: u8 = 3;
/// Anything that can change what boots: an update or sideload (`--update_package=`, `--sideload`), userspace fastboot (`--fastboot`), a wipe package, the bare recovery menu (which offers all of them), or an argument this list does not know. Attestation and a second fleet device: not built, so only "no" for now.
pub const ASK_CHANGES: u8 = 4;

/// A field's text: up to its first NUL.
fn text(field: &[u8]) -> &[u8] {
    &field[..field.iter().position(|&c| c == 0).unwrap_or(field.len())]
}

/// The arguments Android left, as text (the `recovery` field up to its NUL), for the flash log. `block` is misc's first block (at least the 832 bytes of the command, status and recovery fields).
pub fn args(block: &[u8]) -> &[u8] {
    text(&block[RECOVERY])
}

/// What Android's request asks (`ASK_*`), from misc's first block (at least 832 bytes).
pub fn ask(block: &[u8]) -> u8 {
    if text(&block[COMMAND]) != b"boot-recovery" {
        return ASK_NONE;
    }
    let (mut wipe, mut rescue, mut remote, mut changes) = (false, false, false, false);
    // The first line names recovery itself; the arguments follow, one a line.
    for line in args(block).split(|&c| c == b'\n').skip(1) {
        let a = line.trim_ascii();
        match a {
            b"" => {}
            b"--wipe_data" => wipe = true,
            b"--prompt_and_wipe_data" => rescue = true,
            // What goes with a wipe and changes nothing that boots: the eSIM erased with it (the person's choice in Settings), the screen after, memory tagging kept.
            b"--wipe_esim" | b"--shutdown_after" | b"--keep_memtag_mode" | b"--show_text" | b"--just_exit" => {}
            _ if a.starts_with(b"--locale=") || a.starts_with(b"--retry_count=") => {}
            // Who asked: a device admin's wipe (DevicePolicyManagerService: "DevicePolicyManager.wipeDataWithReason() from <package>") is a remote one.
            _ if a.starts_with(b"--reason=") => remote |= a.windows(19).any(|w| w == b"DevicePolicyManager"),
            _ => changes = true,
        }
    }
    if changes || !(wipe || rescue) {
        ASK_CHANGES
    } else if remote {
        ASK_REMOTE
    } else if rescue {
        ASK_RESCUE
    } else {
        ASK_WIPE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(command: &[u8], recovery: &[u8]) -> [u8; 4096] {
        let mut b = [0u8; 4096];
        b[..command.len()].copy_from_slice(command);
        b[64..64 + recovery.len()].copy_from_slice(recovery);
        // Vendor space and the rest of misc are never read.
        b[2048..2058].copy_from_slice(b"theme-dark");
        b
    }

    #[test]
    fn a_factory_reset_from_settings() {
        assert_eq!(ask(&block(b"boot-recovery", b"recovery\n--wipe_data\n--reason=MainClearConfirm,2026-11-22T10:00:00Z\n--locale=en_NZ\n")), ASK_WIPE);
        assert_eq!(ask(&block(b"boot-recovery", b"recovery\n--wipe_data\n--wipe_esim\n--reason=MainClearConfirm\n--locale=en_US\n")), ASK_WIPE);
    }

    #[test]
    fn rescue_party() {
        assert_eq!(ask(&block(b"boot-recovery", b"recovery\n--prompt_and_wipe_data\n--reason=RescueParty\n--locale=en_US\n")), ASK_RESCUE);
    }

    #[test]
    fn a_remote_wipe() {
        assert_eq!(ask(&block(b"boot-recovery", b"recovery\n--wipe_data\n--reason=DevicePolicyManager.wipeDataWithReason() from com.google.android.gms\n")), ASK_REMOTE);
    }

    #[test]
    fn what_changes_what_boots() {
        assert_eq!(ask(&block(b"boot-recovery", b"recovery\n--update_package=/data/ota_package/update.zip\n")), ASK_CHANGES);
        assert_eq!(ask(&block(b"boot-recovery", b"recovery\n--sideload\n")), ASK_CHANGES);
        assert_eq!(ask(&block(b"boot-recovery", b"recovery\n--fastboot\n")), ASK_CHANGES);
        // A wipe with something unknown beside it is not a plain wipe.
        assert_eq!(ask(&block(b"boot-recovery", b"recovery\n--wipe_data\n--wipe_ab\n--wipe_package_size=4096\n")), ASK_CHANGES);
        // The bare menu: everything recovery offers, sideload among it.
        assert_eq!(ask(&block(b"boot-recovery", b"recovery\n")), ASK_CHANGES);
        assert_eq!(ask(&block(b"boot-recovery", b"")), ASK_CHANGES);
    }

    #[test]
    fn no_request() {
        assert_eq!(ask(&block(b"", b"")), ASK_NONE);
        // What `adb reboot bootloader` leaves (both Pixels' misc, 2026-11-18): not a recovery request.
        assert_eq!(ask(&block(b"bootonce-bootloader", b"")), ASK_NONE);
        // Arguments without the command: the bootloader does not start recovery for them.
        assert_eq!(ask(&block(b"", b"recovery\n--wipe_data\n")), ASK_NONE);
    }
}
