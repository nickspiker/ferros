//! The chords under ferros (HV.md "The chords", WINDOWS.md "Stop"), read by the service loop from the keys' pins each pass (either volume key's edge and the power key's press are interrupts that wake it): pure, no hardware, so tools/chords tests it on the host.
//! - Power + volume up: ferros restarts (restart.rs; owner, 2026-11-10).
//! - Power + volume down: the app in front stops (owner, 2026-11-18: "power plus vol down kills"; process.rs `stop_front`).
//! - Volume up + volume down: the kill switch, answered by the interrupt itself (main.rs `.Lirq`), never here; with all three down it has already cut the power.
//!
//! Each chord arms only once its keys are not all down, so a hold that began before the loop started (the gate's volume down, the chord that jumped here from Android or started this restart) never fires, and a hold fires once.
//! A power press made with a volume key already down is a chord's, never the screen's; one that toggled the screen and is then joined by volume down before power lifts is the stop chord's after all, and the screen goes back as it was.

/// The three keys as their pins read now (active low on the always-on bank, killswitch.rs): true held.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Keys {
    pub power: bool,
    pub vol_up: bool,
    pub vol_down: bool,
}

/// What a pass must do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Act {
    None,
    /// Power + volume up: restart.
    Restart,
    /// Power + volume down: stop the app in front, and first put the screen back to this value when the press that began the chord had toggled it.
    Stop { screen_back: Option<u8> },
}

/// The chords' state across passes.
pub struct Chords {
    restart_armed: bool,
    stop_armed: bool,
    /// The screen as it was before a power press toggled it, while that press is still held.
    screen_before: Option<u8>,
}

impl Chords {
    /// Disarmed: nothing fires until each chord's keys have been seen not all down.
    pub const fn new() -> Self {
        Self { restart_armed: false, stop_armed: false, screen_before: None }
    }

    /// Whether a power press taken now is the screen's (it toggles it): not when a volume key is down, where it is a chord's.
    pub fn press_is_screens(k: Keys) -> bool {
        !k.vol_up && !k.vol_down
    }

    /// A power press toggled the screen from `was`: kept while the press is held, for a volume down that joins it.
    pub fn toggled(&mut self, was: u8) {
        self.screen_before = Some(was);
    }

    /// One pass, after any power press was taken: the chord that fires, if one does.
    pub fn step(&mut self, k: Keys) -> Act {
        let restart = k.power && k.vol_up;
        let stop = k.power && k.vol_down && !k.vol_up;
        let mut act = Act::None;
        if restart && self.restart_armed {
            act = Act::Restart;
        }
        self.restart_armed |= !restart;
        if stop && self.stop_armed {
            self.stop_armed = false;
            act = Act::Stop { screen_back: self.screen_before.take() };
        }
        self.stop_armed |= !stop;
        if !k.power {
            self.screen_before = None;
        }
        act
    }
}
