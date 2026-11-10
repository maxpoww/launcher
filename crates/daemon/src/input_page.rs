//! THE MOUSE AND TOUCHPAD, AND THE KEYS (Max, 2026-11-06, the Controls gap
//! audit: "quiero todo lo necesario, en Controls"): Controls → MOUSE AND
//! TOUCHPAD — the touchpad's speed and the mouse's own, tap to click, how
//! the right click is made, natural scrolling, the touchpad still while
//! typing, the left hand, the pointer's size — and KEYBOARD LAYOUT grown
//! into the keyboard whole: several layouts at once with the keys that
//! switch them, and how a held key repeats.
//!
//! Every choice is the compositor's, kept the display settings' way
//! ([`GolemSettings`]: live `hl.config`, `settings.json`, the generated
//! `settings.lua` the compositor runs at its start). The mouse's speed goes
//! to the mice BY NAME (`hl.device`: the compositor has one `sensitivity`
//! for every pointer; a device rule is how one kind gets its own), the
//! names read as the speed is set and again at every start — a touchpad
//! shows as a mouse too (`…-mouse` beside `…-touchpad`) and is left out.
//! The pointer's size is the compositor's `setcursor` now and at its start,
//! and dconf's for the apps.

use crate::controls::{choice_of, group_of, on_off_of, ControlView, Group};
use crate::display::GolemSettings;
use crate::i18n::tr;

/// The speeds, as the pointer's page had them: the compositor's -1..1.
pub(crate) const SPEEDS: [(f64, &str); 5] =
    [(-0.6, "Slowest"), (-0.3, "Slow"), (0.0, "Normal"), (0.3, "Fast"), (0.6, "Fastest")];

/// The pointer's sizes, in the cursor theme's pixels (24 is as shipped).
const CURSOR_SIZES: [(u32, &str); 3] = [(24, "Normal"), (32, "Big"), (48, "Bigger")];

/// How fast a held key repeats (a second) and after how long (ms).
const REPEAT_RATES: [(u32, &str); 3] = [(15, "Slow"), (25, "Normal"), (40, "Fast")];
const REPEAT_DELAYS: [(u32, &str); 3] = [(200, "Short"), (400, "Normal"), (700, "Long")];

/// The keys that switch between two layouts, as xkb names them. Not
/// Super + Space (2026-11-09): it is Golem's voice to text, and the layout
/// changed after the bind ran, so every switch started a dictation too; one
/// saved from before shows as no switch picked.
const SWITCHES: [(&str, &str); 3] =
    [("grp:alt_shift_toggle", "Alt + Shift"), ("grp:ctrl_space_toggle", "Ctrl + Space"), ("grp:caps_toggle", "Caps Lock")];

/// The most mice the mouse's speed is kept for (a mouse is renamed by its
/// plug order, `…-1`, and the names are only ever added).
const MICE_KEPT: usize = 16;

fn nearest(stops: &[f64], value: f64) -> usize {
    stops.iter().enumerate().min_by(|a, b| (a.1 - value).abs().total_cmp(&(b.1 - value).abs())).map_or(0, |(i, _)| i)
}

fn speed_group(caption: &'static str, key: &'static str, now: f64) -> Group {
    let stops: Vec<f64> = SPEEDS.iter().map(|s| s.0).collect();
    let lit = nearest(&stops, now);
    let choices = SPEEDS.iter().enumerate().map(|(i, (v, name))| choice_of(tr(name), i == lit, v.to_string())).collect();
    Group { labelled: true, ..group_of(Some(caption), key, choices) }
}

/// The pointer's size row (the Accessibility page shows it too).
pub(crate) fn cursor_group(s: &GolemSettings) -> Group {
    let now = s.cursor_size.unwrap_or(24);
    let choices = CURSOR_SIZES.iter().map(|(px, name)| choice_of(tr(name), *px == now, px.to_string())).collect();
    Group { labelled: true, ..group_of(Some("Pointer size"), "cursor-size", choices) }
}

/// How the right click is made. Unpicked, the corner is lit (2026-11-09):
/// Golem sets no `clickfinger_behavior`, and the compositor's own is the
/// corner — "Two fingers" was lit and did not work.
fn right_click_group(s: &GolemSettings) -> Group {
    let two = s.right_click.as_deref() == Some("two");
    let ways = vec![choice_of(tr("Two fingers"), two, "two"), choice_of(tr("Bottom right corner"), !two, "corner")];
    Group { labelled: true, ..group_of(Some("Right click"), "right-click", ways) }
}

/// The mice plugged in now added to those the mouse's speed is kept for —
/// at every start and at each pick (2026-11-09: never read again at a start
/// whatever the comments said, and a pick replaced the list: a mouse plugged
/// in later, a second one or one renamed by its plug order ran at the
/// touchpad's speed while the page showed the mouse's).
pub(crate) fn remember_mice() {
    if GolemSettings::load().mouse_speed.is_none() {
        return;
    }
    let (mice, pads) = crate::hypr::pointers();
    GolemSettings::change(|s| {
        if add_mice(&mut s.mouse_names, &mice, &pads) {
            s.store();
        }
    });
}

/// `seen` added to `kept`, the oldest going past [`MICE_KEPT`]; a kept name
/// that is a touchpad now (by its name, or among `pads`) is let go. True
/// when the list changed.
fn add_mice(kept: &mut Vec<String>, seen: &[String], pads: &[String]) -> bool {
    let before = kept.clone();
    kept.retain(|n| !crate::hypr::touchpad_name(n) && !pads.contains(n));
    for name in seen {
        if !kept.contains(name) {
            kept.push(name.clone());
        }
    }
    if kept.len() > MICE_KEPT {
        kept.drain(..kept.len() - MICE_KEPT);
    }
    *kept != before
}

/// Controls → Mouse and touchpad.
pub(crate) fn mouse_view(view: &mut ControlView) {
    let s = GolemSettings::load();
    let mice = crate::hypr::mouse_names();
    view.subtitle = tr(if mice.is_empty() { "The touchpad. A mouse gets its own speed when it is plugged in." } else { "The touchpad, and the mouse." }).to_owned();
    view.groups.push(speed_group("Touchpad speed", "speed", s.pointer_speed.unwrap_or(0.0)));
    if !mice.is_empty() {
        view.groups.push(speed_group("Mouse speed", "mouse-speed", s.mouse_speed.unwrap_or(0.0)));
    }
    view.groups.push(on_off_of(Some("Tap to click"), "tap", s.tap_to_click.unwrap_or(true)));
    view.groups.push(right_click_group(&s));
    view.groups.push(on_off_of(Some("Natural scrolling"), "natural", s.natural_scroll.unwrap_or(true)));
    view.groups.push(on_off_of(Some("Still while typing"), "typing-still", s.typing_stills_touchpad.unwrap_or(true)));
    view.groups.push(on_off_of(Some("Left-handed"), "left-handed", s.left_handed.unwrap_or(false)));
    view.groups.push(cursor_group(&s));
}

/// Controls → Keyboard layout: the layouts (several may be lit: a click on
/// one not lit adds it, on a lit one takes it away), the switch between
/// them, the repeat.
pub(crate) fn keyboard_view(view: &mut ControlView) {
    let s = GolemSettings::load();
    let (layouts, variants, options) = match &s.keyboard {
        Some(k) => (k.layout.clone(), k.variant.clone(), k.options.clone()),
        None => {
            let (l, v) = crate::controls::keyboard_now();
            (l, v, String::new())
        }
    };
    let on: Vec<(String, String)> = pairs(&layouts, &variants);
    let known = |l: &str, v: &str| crate::controls::LAYOUTS.iter().find(|(_, kl, kv)| *kl == l && *kv == v);
    let names: Vec<String> = on.iter().map(|(l, v)| known(l, v).map_or_else(|| format!("{l} {v}").trim().to_owned(), |k| k.0.to_owned())).collect();
    view.subtitle = if names.len() > 1 { names.join(" · ") } else { names.first().cloned().unwrap_or_default() };
    let mut choices: Vec<_> = crate::controls::LAYOUTS
        .iter()
        .map(|(name, l, v)| choice_of(*name, on.iter().any(|(ol, ov)| ol == l && ov == v), format!("{l}:{v}")))
        .collect();
    // (A layout picked outside the list, the installer's: shown first, lit.)
    for (l, v) in &on {
        if known(l, v).is_none() {
            choices.insert(0, choice_of(format!("{l} {v}").trim().to_owned(), true, format!("{l}:{v}")));
        }
    }
    view.groups.push(group_of(None, "kb-layouts", choices));
    if on.len() > 1 {
        let switch = SWITCHES.iter().map(|(opt, name)| choice_of(tr(name), options.split(',').any(|o| o == *opt), *opt)).collect();
        view.groups.push(Group { labelled: true, ..group_of(Some("Switch with"), "kb-switch", switch) });
    }
    let rate = s.repeat_rate.unwrap_or(25);
    let rates = REPEAT_RATES.iter().map(|(r, name)| choice_of(tr(name), *r == rate, r.to_string())).collect();
    view.groups.push(Group { labelled: true, ..group_of(Some("Key repeat"), "kb-repeat", rates) });
    let delay = s.repeat_delay.unwrap_or(400);
    let delays = REPEAT_DELAYS.iter().map(|(d, name)| choice_of(tr(name), *d == delay, d.to_string())).collect();
    view.groups.push(Group { labelled: true, ..group_of(Some("Repeat after"), "kb-delay", delays) });
}

/// `"us,es"` and `",cat"` as pairs; a variant list shorter than the layouts'
/// means empty variants.
pub(crate) fn pairs(layouts: &str, variants: &str) -> Vec<(String, String)> {
    let vs: Vec<&str> = variants.split(',').collect();
    layouts
        .split(',')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .enumerate()
        .map(|(i, l)| (l.to_owned(), vs.get(i).map(|v| v.trim()).unwrap_or("").to_owned()))
        .collect()
}

/// The switch between layouts that xkb `options` hold (`grp:…`), if any.
fn switch_of(options: &str) -> Option<&str> {
    options.split(',').map(str::trim).find(|o| o.starts_with("grp:"))
}

/// `options` with the switch put to `switch` (or taken out) and every other
/// option kept (2026-11-09: a pick replaced them all, and `caps:escape` went
/// with the old switch).
fn with_switch(options: &str, switch: Option<&str>) -> String {
    let mut kept: Vec<&str> = options.split(',').map(str::trim).filter(|o| !o.is_empty() && !o.starts_with("grp:")).collect();
    kept.extend(switch);
    kept.join(",")
}

/// A pick on the worker. True when the key was this page's.
pub(crate) fn apply(key: &str, value: &str) -> bool {
    let on = value == "on";
    match key {
        "mouse-speed" => {
            if let Ok(v) = value.parse::<f64>() {
                let (mice, pads) = crate::hypr::pointers();
                GolemSettings::change(|s| {
                    s.mouse_speed = Some(v.clamp(-1.0, 1.0));
                    add_mice(&mut s.mouse_names, &mice, &pads);
                    s.store_input();
                });
            }
        }
        "tap" => GolemSettings::change(|s| {
            s.tap_to_click = Some(on);
            s.store_input();
        }),
        "right-click" if value == "two" || value == "corner" => GolemSettings::change(|s| {
            s.right_click = Some(value.to_owned());
            s.store_input();
        }),
        "typing-still" => GolemSettings::change(|s| {
            s.typing_stills_touchpad = Some(on);
            s.store_input();
        }),
        "left-handed" => GolemSettings::change(|s| {
            s.left_handed = Some(on);
            s.store_input();
        }),
        "cursor-size" => {
            if let Ok(px) = value.parse::<u32>().map(|p| p.clamp(16, 96)) {
                GolemSettings::change(|s| {
                    s.cursor_size = Some(px);
                    s.store();
                });
                crate::hypr::set_cursor_size(px);
                // The apps: dconf's size (the toolkits read it as they start).
                crate::controls::dconf_write("/org/gnome/desktop/interface/cursor-size", &px.to_string());
            }
        }
        "kb-layouts" => {
            let Some((l, v)) = value.split_once(':') else { return true };
            if !crate::controls::valid_xkb(l) || !crate::controls::valid_xkb(v) || l.is_empty() || l.contains(',') {
                return true;
            }
            GolemSettings::change(|s| {
                let (layouts, variants, options) = match &s.keyboard {
                    Some(k) => (k.layout.clone(), k.variant.clone(), k.options.clone()),
                    None => {
                        let (cl, cv) = crate::controls::keyboard_now();
                        (cl, cv, String::new())
                    }
                };
                let mut on = pairs(&layouts, &variants);
                if let Some(i) = on.iter().position(|(ol, ov)| ol == l && ov == v) {
                    // (The last one stays: a keyboard with no layout types nothing.)
                    if on.len() > 1 {
                        on.remove(i);
                    }
                } else {
                    on.push((l.to_owned(), v.to_owned()));
                }
                // (Only the switch is ours to change: `caps:escape` and the
                // like stay, 2026-11-09.)
                let options = if on.len() <= 1 {
                    with_switch(&options, None)
                } else if switch_of(&options).is_none() {
                    with_switch(&options, Some(SWITCHES[0].0))
                } else {
                    options
                };
                s.set_keyboards(&on, &options);
            });
        }
        "kb-switch" => {
            if SWITCHES.iter().any(|(opt, _)| *opt == value) {
                GolemSettings::change(|s| {
                    if let Some(k) = s.keyboard.as_mut() {
                        k.options = with_switch(&k.options, Some(value));
                        s.store_input();
                    }
                });
            }
        }
        "kb-repeat" => {
            if let Ok(r) = value.parse::<u32>().map(|r| r.clamp(5, 100)) {
                GolemSettings::change(|s| {
                    s.repeat_rate = Some(r);
                    s.store_input();
                });
            }
        }
        "kb-delay" => {
            if let Ok(d) = value.parse::<u32>().map(|d| d.clamp(100, 2000)) {
                GolemSettings::change(|s| {
                    s.repeat_delay = Some(d);
                    s.store_input();
                });
            }
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_and_variants_pair_up() {
        assert_eq!(pairs("us,es", ",cat"), vec![("us".to_owned(), String::new()), ("es".to_owned(), "cat".to_owned())]);
        assert_eq!(pairs("us", ""), vec![("us".to_owned(), String::new())]);
        assert_eq!(pairs("us,latam", ""), vec![("us".to_owned(), String::new()), ("latam".to_owned(), String::new())]);
        assert!(pairs("", "").is_empty());
    }

    #[test]
    fn the_nearest_speed_is_lit() {
        let g = speed_group("Touchpad speed", "speed", 0.25);
        assert_eq!(g.choices.iter().position(|c| c.on), Some(3), "{:?}", g.choices);
        assert!(g.labelled);
        let c = cursor_group(&GolemSettings::default());
        assert_eq!(c.choices.iter().position(|c| c.on), Some(0));
    }

    #[test]
    fn the_right_click_is_the_corner_until_two_fingers_are_picked() {
        let lit = |s: &GolemSettings| right_click_group(s).choices.iter().position(|c| c.on);
        assert_eq!(lit(&GolemSettings::default()), Some(1));
        let two = GolemSettings { right_click: Some("two".to_owned()), ..Default::default() };
        assert_eq!(lit(&two), Some(0));
    }

    #[test]
    fn only_the_switch_of_the_options_is_changed() {
        assert_eq!(with_switch("caps:escape", Some("grp:alt_shift_toggle")), "caps:escape,grp:alt_shift_toggle");
        assert_eq!(with_switch("grp:alt_shift_toggle,caps:escape", Some("grp:caps_toggle")), "caps:escape,grp:caps_toggle");
        assert_eq!(with_switch("grp:alt_shift_toggle,caps:escape", None), "caps:escape");
        assert_eq!(with_switch("grp:alt_shift_toggle", None), "");
        assert_eq!(switch_of("caps:escape,grp:win_space_toggle"), Some("grp:win_space_toggle"));
        assert_eq!(switch_of("caps:escape,grp_led:scroll"), None);
        // Voice to text's keys are not offered.
        assert!(SWITCHES.iter().all(|(opt, _)| *opt != "grp:win_space_toggle"));
    }

    #[test]
    fn mice_are_added_never_replaced() {
        let names = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let mut kept = names(&["logitech-usb-receiver", "alpsps/2-alps-glidepoint", "elan0501:00-04f3:3060"]);
        // Unplugged now: kept; a second one: added; touchpads: let go.
        assert!(add_mice(&mut kept, &names(&["logitech-usb-receiver-1"]), &names(&["elan0501:00-04f3:3060"])));
        assert_eq!(kept, names(&["logitech-usb-receiver", "logitech-usb-receiver-1"]));
        assert!(!add_mice(&mut kept, &names(&["logitech-usb-receiver"]), &[]));
        let many: Vec<String> = (0..20).map(|i| format!("mouse-{i}")).collect();
        add_mice(&mut kept, &many, &[]);
        assert_eq!(kept.len(), MICE_KEPT);
        assert_eq!(kept.last().map(String::as_str), Some("mouse-19"));
    }
}
