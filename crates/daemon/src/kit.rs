//! THE KIT — one form per job, the same in every surface (Max, 2026-11-05:
//! "quiero full consistencia… formá una consistencia real"; the sheet is
//! https://claude.ai/artifact/QSig7uAfBuGvzASySGNNEg, board 1). Built on the
//! QUIET direction: nothing behind anything at rest; a fill only for a state;
//! the accent only for where you are, what is on and the one main action; red
//! only for what cannot be undone; the palette's warning colour for a problem.
//!
//! Every surface (the banner's boxes, the dock's box, the control panel,
//! menus, Files, Quick Look) takes its sizes and signal colours from here, so
//! they cannot drift apart again. The card is the one exception (Max: never
//! change the card).

/// The slim switch: 36 × 20, a 12 px knob 4 px in. Off: an outline at
/// `SWITCH_LINE` of the ink, its knob at the left at `SWITCH_KNOB_OFF`; on:
/// filled with the accent, the knob at the right in the accent's own
/// contrast. Centred on its row; the whole row switches it.
pub(crate) const SWITCH_W: f32 = 36.0;
pub(crate) const SWITCH_H: f32 = 20.0;
pub(crate) const SWITCH_KNOB: f32 = 12.0;
pub(crate) const SWITCH_LINE: f32 = 0.38;
pub(crate) const SWITCH_KNOB_OFF: f32 = 0.75;

/// A row's corner radius (times the scale).
pub(crate) const ROW_RADIUS: f32 = 7.0;

/// The accent (the palette's own), as sRGB fractions.
pub(crate) fn accent() -> [f32; 4] {
    crate::palette::accent_srgb()
}

/// A row under the pointer or the keys: THE ONE MENU's quiet tint of the
/// accent, as a wash over whatever the row stands on (a menu mixes it into
/// its solid panel; a box's glass takes it as this alpha).
pub(crate) fn row_tint(bright: bool) -> [f32; 4] {
    let a = accent();
    [a[0], a[1], a[2], if bright { 0.16 } else { 0.22 }]
}

/// The switch's knob on the accent: dark on a light accent, light on a dark
/// one.
pub(crate) fn on_accent() -> [f32; 4] {
    let a = accent();
    if 0.2126 * a[0] + 0.7152 * a[1] + 0.0722 * a[2] > 0.55 {
        [0.06, 0.06, 0.07, 1.0]
    } else {
        [0.98, 0.98, 0.98, 1.0]
    }
}

/// A problem (a wrong password, a connection that failed): the palette's
/// warning colour — never the accent (it means "on"), never red (it means
/// "destroys").
pub(crate) fn warn() -> [f32; 4] {
    crate::palette::warn_srgb()
}

/// What cannot be undone, asked: THE ONE MENU's red (`desktop_menu::danger_on`:
/// the palette's own when it is clearly red and reads at 4.5 : 1, else Golem's),
/// for a box of the `dark` or the light look, as sRGB fractions.
pub(crate) fn danger(dark: bool) -> [f32; 4] {
    let ground = if dark { [0.086, 0.09, 0.106, 1.0] } else { [0.97, 0.97, 0.96, 1.0] };
    let (red, _) = crate::desktop_menu::danger_on(to_linear(crate::palette::danger_srgb()), to_linear(ground), dark);
    to_srgb(red)
}

/// The red's quiet ground under a question asked in place ("Click again to …").
pub(crate) fn danger_tint(dark: bool) -> [f32; 4] {
    let r = danger(dark);
    [r[0], r[1], r[2], if dark { 0.14 } else { 0.10 }]
}

fn to_linear(c: [f32; 4]) -> [f32; 4] {
    let f = |v: f32| if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
    [f(c[0]), f(c[1]), f(c[2]), c[3]]
}

fn to_srgb(c: [f32; 4]) -> [f32; 4] {
    let f = |v: f32| {
        if v <= 0.003_130_8 {
            v * 12.92
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        }
    };
    [f(c[0]), f(c[1]), f(c[2]), c[3]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_red_goes_there_and_back() {
        let c = [0.776, 0.157, 0.157, 1.0];
        let back = to_srgb(to_linear(c));
        for i in 0..3 {
            assert!((back[i] - c[i]).abs() < 1e-4);
        }
    }
}
