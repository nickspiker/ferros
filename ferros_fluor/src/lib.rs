//! A fluor app on ferros in the person's app scale and the device's theme (owner, 2026-11-20: "if the user sets the RU high, they want all their kists to render that way"; every kist follows the app scale and the device theme by default, an implicit right, never asked).
//!
//! - **Scale**: preference 14 is 64ths of an octave (ferros_sys `PREF_SCALE`); the app's RU multiplier is 2^(s/64). The app starts at it, and a change (the Settings page's slider, another kist's pinch) is followed with the layout run again about the frame's centre, as a pinch's step runs it.
//! - **Theme**: preference 5 picks a row of ferros_sys::theme::THEMES, the device's one table (the map, the display and the pucks draw from it too); it becomes fluor's look (`fluor::theme::set_look`), so every fluor widget draws in it and the app reads the rest (`look().ground`, `accent`, `ring`) for its own drawing.
//! - **A pinch inside the kist** is the person's app scale (owner, 2026-11-20: "settings pages should modify that, same with apps"): at its end the scale it reached is written as preference 14 (SYS_SET_PREF with the display grant, which the kernel takes for this preference alone), for every kist; a kernel from before that keeps the pinch in the kist and it says so once.
//!
//! Reading a preference is a plain read of the process's own grants page, which the kernel writes in place for every process (ferros kernel process.rs: "Reading them needs no capability"); there is no doorbell for a preference, and none is needed: the host's loop passes once a frame while the kist is in front, and the read is two bytes. Behind, nothing is drawn, and the first pass in front again reads them.



use fluor::coord::Coord;
use fluor::theme::{dark, fmt, from_gamma2, vsf, Look};

#[cfg(target_os = "ferros")]
pub use run::run_app;

/// A G#RRGGBB VSF RGB code at gamma 2 (every display op's, ferros_sys::theme's) as fluor's buffer stores it: opaque, α and darkness, in the buffer's working primaries.
fn ink(gamma2: u32) -> u32 {
    dark(fmt(vsf(from_gamma2(gamma2 & 0x00FF_FFFF))))
}

/// A device theme as fluor's look: the words in its text colours, the controls in its button row (fill, held, the two edges), a field on its plane, black while it is being typed in (as fluor's TEXTBOX_ACTIVE on every other platform; owner, 2026-11-21), and the app's ground, accent and ring.
pub fn look_of(t: &ferros_theme::Theme) -> Look {
    let [fill, held, light, shadow] = t.button;
    Look {
        text: ink(t.text),
        label: ink(t.text2),
        field: ink(t.plane),
        field_hover: ink(t.plane),
        field_active: ink(0),
        light_edge: ink(light),
        shadow_edge: ink(shadow),
        selection: ink(held),
        button: ink(fill),
        button_hover: ink(held),
        button_active: ink(fill),
        button_held: ink(held),
        ground: ink(0),
        accent: ink(t.lit),
        ring: ink(t.ring),
    }
}

/// The RU multiplier the person's app scale gives (2^(s/64)).
pub fn scale_ru(s: i8) -> Coord {
    (2f32).powf(s as Coord * (1. / (1 << 6) as Coord))
}


/// The run loop, on ferros alone.
#[cfg(target_os = "ferros")]
mod run {
    use super::{look_of, scale_ru};
    use fluor::coord::Coord;
    use fluor::host::app::FluorApp;
    use fluor::host::ferros::{run_app_with, Caps, Contact, FerrosShell};
    use fluor::host::panel::PanelPixel;

    /// A scale as the preference keeps it: the nearest 64th of an octave.
    fn scale_of(ru: Coord) -> i8 {
        // WHY the bound: the kernel takes only ±SCALE_REACH (64, an octave each way; process.rs `pref_valid`); a pinch past it is written as the reach, the kernel's own range.
        (ru.log2() * (1 << 6) as Coord).round().clamp(-64., 64.) as i8
    }

    fn theme_now() -> u8 {
        ferros_sys::pref(ferros_sys::PREF_THEME)
    }

    fn scale_now() -> i8 {
        ferros_sys::pref(ferros_sys::PREF_SCALE) as i8
    }

    /// fluor's ferros host for `app`, in the person's app scale and the device's theme, following both. `scale_token` is the grant a pinch's scale is written with (the display's slot today: a kist holds no Settings; see the module's words).
    pub fn run_app<P: PanelPixel, A: FluorApp<P> + 'static>(app: A, caps: Caps, size: Option<(u32, u32)>, scale_token: u64) -> Result<(), String> {
        let mut theme_seen = theme_now();
        fluor::theme::set_look(look_of(ferros_sys::theme::current()));
        let mut scale_seen = scale_now();
        let mut ru_set = scale_ru(scale_seen);
        let mut refused = false;
        run_app_with(app, caps, ru_set, size, move |shell: &mut FerrosShell<A, P>, contacts: &[Contact]| {
            if !contacts.is_empty() {
                shell.on_contacts(contacts);
            }
            let tm = theme_now();
            if tm != theme_seen {
                theme_seen = tm;
                fluor::theme::set_look(look_of(ferros_sys::theme::current()));
                shell.request_redraw();
            }
            let sc = scale_now();
            if sc != scale_seen {
                scale_seen = sc;
                ru_set = scale_ru(sc);
                shell.set_zoom(ru_set);
            } else if !refused && !shell.pinching() && shell.zoom() != ru_set {
                // A pinch ended at a scale of its own: the person's app scale, for every kist.
                let s = scale_of(shell.zoom());
                if ferros_sys::set_pref(scale_token, ferros_sys::PREF_SCALE, s as u8) == 0 {
                    scale_seen = s;
                    ru_set = scale_ru(s);
                    shell.set_zoom(ru_set);
                } else {
                    refused = true;
                    println!("ferros_fluor: the pinch's scale stays in this kist: the kernel took no app scale from it (a kernel from before the display grant wrote preference 14, or a scale out of its reach)");
                }
            }
        })
    }
}
