//! Attest to approve (owner, 2026-11-24; docs/AUTH.md "Approve on the attest screen"): a protective change, and a change to a setting under a "require attest" guard, is approved on the attest screen itself, never on a second handle-entry screen.
//!
//! The owner: asking for such a setting "should immediately lock the device anyways and drop to the attest screen, pending success to apply the setting and the button instead of attest ... should say 'approve' or similar and keep the user locked out if fail."
//! And: "for any debatable settings you could add a toggle that requires attest, and to untoggle, of course requires an attest but then after that your kid can change it all day long until you toggle 'require attest' on again."
//!
//! Shared by the kernel, which holds the request across the lock and applies it, the attest screen (ahau), which shows what it approves, and the settings pages (payloads/map), which ask.
//! A request is a kind and one number, never text: the screen says what it approves in the person's own words from the kind (ferros_lang), so the requester cannot put words of its own on the attest screen.

/// No request.
pub const NONE: u8 = 0;
/// Attest at boot on: the kernel keeps the seed of the handle typed at the approve and turns the setting on.
pub const ATTEST_AT_BOOT_ON: u8 = 1;
/// Attest at boot off.
pub const ATTEST_AT_BOOT_OFF: u8 = 2;
/// Android on for this device: the person's acceptance of the disclaimer kept (if it is not yet) and Android allowed.
pub const ANDROID_ON: u8 = 3;
/// Remove this device from its identity (docs/AUTH.md "Remove this device"): approved here, carried out by the identity service, which the kernel lets erase only with this approval held.
pub const REMOVE_DEVICE: u8 = 4;
/// A "require attest" guard off: the number is the guard (`guard`).
pub const GUARD_OFF: u8 = 5;
/// The battery's charge cap, under its guard: the number is the cap in mV.
pub const CHARGE_CAP: u8 = 6;
/// What this device starts into, under its guard: the number is 0 for ferros, 1 for Android.
pub const START_INTO: u8 = 7;
/// Developer mode on, under its guard: approved here, carried out by the settings pages, which take the approval from the kernel.
pub const DEVELOPER: u8 = 8;
/// Bind a new device to the identity (owner, 2026-11-24: "the fleet page does nothing unique until you press 'bind new device' or similar, then it does the scan/tap enable, all that"): approved here, the add flow opened by the identity service once it holds the roots again, which takes the approval from the kernel as it opens it (one approve, one opening).
pub const FLEET_ADD: u8 = 9;

/// The "require attest" guards (each a bit of the device setting `guards`, kept in the device's settings document): which setting each protects.
/// Turning a guard on needs nothing (tightening is always safe); turning it off is an approve (`GUARD_OFF`); while it is on, changing its setting is an approve too.
pub mod guard {
    /// The battery's charge cap (`super::CHARGE_CAP`).
    pub const CHARGE_CAP: u8 = 0;
    /// What this device starts into (`super::START_INTO`).
    pub const START_INTO: u8 = 1;
    /// Developer mode (`super::DEVELOPER`).
    pub const DEVELOPER: u8 = 2;
    /// Every guard there is, as bits.
    pub const ALL: u64 = 1 << CHARGE_CAP | 1 << START_INTO | 1 << DEVELOPER;

    /// Whether `g` names a guard.
    pub const fn known(g: u64) -> bool {
        g < 64 && ALL >> g & 1 == 1
    }

    /// Whether guard `g` is on in `bits`.
    pub const fn on(bits: u64, g: u8) -> bool {
        bits >> g & 1 == 1
    }
}

/// One request: what is to change, and its number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    pub kind: u8,
    pub arg: u64,
}

/// The largest number a request carries: the word keeps the kind in its low byte.
pub const ARG_MAX: u64 = u64::MAX >> 8;

impl Request {
    /// A request, when the kind is one and its number is one that kind takes; None otherwise.
    /// The kernel checks again what only it can know (a cap the battery takes, Android allowed here).
    pub const fn new(kind: u8, arg: u64) -> Option<Request> {
        let ok = match kind {
            ATTEST_AT_BOOT_ON | ATTEST_AT_BOOT_OFF | ANDROID_ON | REMOVE_DEVICE | DEVELOPER | FLEET_ADD => arg == 0,
            GUARD_OFF => guard::known(arg),
            // A cap in mV: the battery's own range is the kernel's check (process.rs `charge_cv_ok`); here only that it is a number a word carries.
            CHARGE_CAP => arg != 0 && arg <= ARG_MAX,
            START_INTO => arg <= 1,
            _ => false,
        };
        if ok { Some(Request { kind, arg }) } else { None }
    }

    /// The request as one word: the kind in bits 0-7, the number above.
    pub const fn word(self) -> u64 {
        self.kind as u64 | self.arg << 8
    }

    /// A word read back; None for 0 (no request) and for anything `new` refuses.
    pub const fn from_word(w: u64) -> Option<Request> {
        Request::new(w as u8, w >> 8)
    }

    /// Whether the kernel carries the change out itself at the approve's unlock.
    /// The others (the removal, developer mode, the add flow's opening) are carried out by a process, which the kernel lets act only while the approval is held for it (`kernel` false: held until taken or until the next lock).
    pub const fn kernel(self) -> bool {
        !matches!(self.kind, REMOVE_DEVICE | DEVELOPER | FLEET_ADD)
    }

    /// The guard this request is the change of, when it is a guarded setting's: changing it needs the approve only while that guard is on.
    pub const fn guard(self) -> Option<u8> {
        match self.kind {
            CHARGE_CAP => Some(guard::CHARGE_CAP),
            START_INTO => Some(guard::START_INTO),
            DEVELOPER => Some(guard::DEVELOPER),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every request a settings page makes reads back from its word as it was made.
    #[test]
    fn word_round_trip() {
        let all = [
            Request { kind: ATTEST_AT_BOOT_ON, arg: 0 },
            Request { kind: ATTEST_AT_BOOT_OFF, arg: 0 },
            Request { kind: ANDROID_ON, arg: 0 },
            Request { kind: REMOVE_DEVICE, arg: 0 },
            Request { kind: GUARD_OFF, arg: guard::CHARGE_CAP as u64 },
            Request { kind: GUARD_OFF, arg: guard::START_INTO as u64 },
            Request { kind: GUARD_OFF, arg: guard::DEVELOPER as u64 },
            Request { kind: CHARGE_CAP, arg: 4300 },
            Request { kind: START_INTO, arg: 0 },
            Request { kind: START_INTO, arg: 1 },
            Request { kind: DEVELOPER, arg: 0 },
            Request { kind: FLEET_ADD, arg: 0 },
        ];
        for r in all {
            assert_eq!(Request::new(r.kind, r.arg), Some(r));
            assert_eq!(Request::from_word(r.word()), Some(r));
        }
    }

    /// No request, an unknown kind, and a number its kind does not take are refused.
    #[test]
    fn refused() {
        assert_eq!(Request::from_word(0), None);
        for kind in [NONE, 10, 255] {
            assert_eq!(Request::new(kind, 0), None, "{kind}");
        }
        assert_eq!(Request::new(ATTEST_AT_BOOT_ON, 1), None);
        assert_eq!(Request::new(REMOVE_DEVICE, 7), None);
        assert_eq!(Request::new(GUARD_OFF, 3), None);
        assert_eq!(Request::new(GUARD_OFF, 64), None);
        assert_eq!(Request::new(GUARD_OFF, u64::MAX), None);
        assert_eq!(Request::new(START_INTO, 2), None);
        assert_eq!(Request::new(CHARGE_CAP, 0), None);
        assert_eq!(Request::new(CHARGE_CAP, ARG_MAX + 1), None);
    }

    /// The guarded changes name their guards, the protective ones none, and every guard is one `ALL` holds.
    #[test]
    fn guards() {
        for (kind, arg, g) in [(CHARGE_CAP, 4200, Some(guard::CHARGE_CAP)), (START_INTO, 1, Some(guard::START_INTO)), (DEVELOPER, 0, Some(guard::DEVELOPER)), (ATTEST_AT_BOOT_ON, 0, None), (ANDROID_ON, 0, None), (REMOVE_DEVICE, 0, None), (GUARD_OFF, 0, None)] {
            let r = Request::new(kind, arg).unwrap();
            assert_eq!(r.guard(), g, "{kind}");
            if let Some(g) = g {
                assert!(guard::known(g as u64));
            }
        }
        assert!(guard::on(guard::ALL, guard::DEVELOPER));
        assert!(!guard::on(0, guard::CHARGE_CAP));
        assert!(!guard::known(63));
    }

    /// Only the removal, developer mode and the add flow's opening are a process's to carry out.
    #[test]
    fn who_applies() {
        assert!(!Request::new(REMOVE_DEVICE, 0).unwrap().kernel());
        assert!(!Request::new(FLEET_ADD, 0).unwrap().kernel());
        assert!(!Request::new(DEVELOPER, 0).unwrap().kernel());
        for kind in [ATTEST_AT_BOOT_ON, ATTEST_AT_BOOT_OFF, ANDROID_ON] {
            assert!(Request::new(kind, 0).unwrap().kernel());
        }
        assert!(Request::new(CHARGE_CAP, 4200).unwrap().kernel());
        assert!(Request::new(START_INTO, 1).unwrap().kernel());
        assert!(Request::new(GUARD_OFF, 1).unwrap().kernel());
    }
}
