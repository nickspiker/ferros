//! The chords under ferros and the screenshot's slide on the host: kernel/src/chord.rs and payloads/map/src/pair.rs themselves, tested.
//! `cargo test --manifest-path tools/chords/Cargo.toml`.

#[path = "../../../kernel/src/chord.rs"]
pub mod chord;
#[path = "../../../payloads/map/src/pair.rs"]
pub mod pair;

#[cfg(test)]
mod tests {
    use super::chord::{Act, Chords, Keys};
    use super::pair;

    const NONE: Keys = Keys { power: false, vol_up: false, vol_down: false };
    const POWER: Keys = Keys { power: true, vol_up: false, vol_down: false };
    const DOWN: Keys = Keys { power: false, vol_up: false, vol_down: true };
    const STOP: Keys = Keys { power: true, vol_up: false, vol_down: true };
    const RESTART: Keys = Keys { power: true, vol_up: true, vol_down: false };
    const ALL: Keys = Keys { power: true, vol_up: true, vol_down: true };

    /// A run of passes, each chord fired collected.
    fn run(c: &mut Chords, passes: &[Keys]) -> Vec<Act> {
        passes.iter().map(|&k| c.step(k)).filter(|a| *a != Act::None).collect()
    }

    #[test]
    fn stop_fires_once_a_hold() {
        let mut c = Chords::new();
        assert_eq!(run(&mut c, &[NONE, DOWN, STOP, STOP, STOP, DOWN, NONE]), vec![Act::Stop { screen_back: None }]);
        // Let go and pressed again: a second stop.
        assert_eq!(run(&mut c, &[STOP, STOP, NONE]), vec![Act::Stop { screen_back: None }]);
    }

    #[test]
    fn held_at_start_fires_nothing() {
        // The gate's volume down with power, or a restart's chord, still down as the loop starts.
        let mut c = Chords::new();
        assert_eq!(run(&mut c, &[STOP, STOP, DOWN]), vec![]);
        assert_eq!(run(&mut c, &[STOP]), vec![Act::Stop { screen_back: None }]);
        let mut c = Chords::new();
        assert_eq!(run(&mut c, &[RESTART, RESTART, POWER]), vec![]);
        assert_eq!(run(&mut c, &[RESTART]), vec![Act::Restart]);
    }

    #[test]
    fn restart_unchanged() {
        let mut c = Chords::new();
        assert_eq!(run(&mut c, &[NONE, RESTART]), vec![Act::Restart]);
    }

    #[test]
    fn all_three_never_stops() {
        // Both volume keys are the kill switch, answered by the interrupt before any pass reads the keys. All three never read as the stop; power + volume up is inside them, the restart's keys as before this change.
        let mut c = Chords::new();
        assert!(!run(&mut c, &[NONE, ALL, ALL]).iter().any(|a| matches!(a, Act::Stop { .. })));
    }

    #[test]
    fn press_with_a_volume_key_is_a_chords() {
        assert!(Chords::press_is_screens(POWER));
        assert!(!Chords::press_is_screens(STOP));
        assert!(!Chords::press_is_screens(RESTART));
    }

    #[test]
    fn screen_goes_back_when_volume_down_joins_the_press() {
        // Power first: its press toggled the screen from on (0); volume down then joins while power is held.
        let mut c = Chords::new();
        assert_eq!(c.step(NONE), Act::None);
        c.toggled(0);
        assert_eq!(c.step(POWER), Act::None);
        assert_eq!(c.step(STOP), Act::Stop { screen_back: Some(0) });
    }

    #[test]
    fn a_toggle_whose_press_lifted_is_kept() {
        // Power pressed and let go (the screen off for good), then a stop chord later: the screen is not touched.
        let mut c = Chords::new();
        c.step(NONE);
        c.toggled(0);
        c.step(POWER);
        c.step(NONE);
        assert_eq!(run(&mut c, &[DOWN, STOP]), vec![Act::Stop { screen_back: None }]);
    }

    /// The 8a's and the Pro's screens.
    const SCREENS: [(i64, i64); 2] = [(1080, 2400), (1344, 2992)];

    #[test]
    fn slide_needs_both_fingers_up() {
        for (w, h) in SCREENS {
            let span = (2 * w * h) as f64 / (w + h) as f64;
            let past = (span / 24.) as i64 + 1;
            let short = (span / 24.) as i64 - 1;
            assert!(pair::slid([past, past].into_iter(), w, h));
            assert!(!pair::slid([past, short].into_iter(), w, h));
            assert!(!pair::slid([past].into_iter(), w, h));
            // Down is not up.
            assert!(!pair::slid([-past, -past].into_iter(), w, h));
            // A third finger does not stop it.
            assert!(pair::slid([past, 0, past].into_iter(), w, h));
        }
    }

    #[test]
    fn slide_thresholds() {
        // A 24th of the harmonic span: 62 px on the 8a, 77 on the Pro.
        assert!(pair::slid([63, 63].into_iter(), 1080, 2400));
        assert!(!pair::slid([62, 62].into_iter(), 1080, 2400));
        assert!(pair::slid([78, 78].into_iter(), 1344, 2992));
        assert!(!pair::slid([77, 77].into_iter(), 1344, 2992));
    }
}
