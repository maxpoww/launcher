//! ACCESSIBILITY (Max, 2026-11-06, the Controls gap audit: every settings
//! app has a page for it and Golem had none): Controls → ACCESSIBILITY —
//! bigger text, the pointer's size, the screen magnified round the
//! pointer, less motion, and a screen reader.
//!
//! TEXT SIZE is one factor the apps and the shell share: dconf's
//! `text-scaling-factor` (GTK reads it live, Qt through the portal) and the
//! shell's own zooms (`zoom.rs`: the bar, the menus, the file windows, the
//! quick card, the desktop) set to the same number on the UI thread
//! (`App::control_pick`, key `access-text`). THE MAGNIFIER is the
//! compositor's zoom about the pointer (`cursor.zoom_factor`), kept as the
//! input choices are. LESS MOTION is the shell's own flag
//! (`animation::reduce_motion`: every animation snaps), the compositor's
//! animations off (`settings.lua` turns the leaves off too, as the stage
//! does; turned back on, the config is reloaded, which restores every
//! leaf's own curve), and dconf's `enable-animations` for the apps. THE
//! SCREEN READER is Orca: on this machine, a switch that runs it; absent,
//! installed as any app is (the dock's own install).

use crate::controls::{choice_of, group_of, line_of, on_off_of, ControlView, Group};
use crate::display::GolemSettings;
use crate::i18n::tr;

/// Text sizes: the factor every surface and app is scaled by.
const TEXT_SIZES: [(f64, &str); 4] = [(1.0, "Normal"), (1.15, "Large"), (1.3, "Larger"), (1.5, "Largest")];

/// The magnifier's steps (1 = off).
const MAGNIFIERS: [(f64, &str); 4] = [(1.0, "Off"), (1.5, "1.5×"), (2.0, "2×"), (3.0, "3×")];

/// Orca, as nixpkgs and its desktop file name it.
pub(crate) const READER_ATTR: &str = "orca";
pub(crate) const READER_ID: &str = "org.gnome.Orca.desktop";

/// The reader's install is under way (set by the UI thread as it starts it).
pub(crate) static INSTALLING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn nearest(stops: &[f64], value: f64) -> usize {
    stops.iter().enumerate().min_by(|a, b| (a.1 - value).abs().total_cmp(&(b.1 - value).abs())).map_or(0, |(i, _)| i)
}

pub(crate) fn view(view: &mut ControlView) {
    let s = GolemSettings::load();
    view.subtitle = tr("Bigger, calmer, read aloud.").to_owned();
    let stops: Vec<f64> = TEXT_SIZES.iter().map(|t| t.0).collect();
    let lit = nearest(&stops, s.text_scale.unwrap_or(1.0));
    let sizes = TEXT_SIZES.iter().enumerate().map(|(i, (v, name))| choice_of(tr(name), i == lit, v.to_string())).collect();
    view.groups.push(Group { labelled: true, ..group_of(Some("Text size"), "access-text", sizes) });
    view.groups.push(crate::input_page::cursor_group(&s));
    let stops: Vec<f64> = MAGNIFIERS.iter().map(|t| t.0).collect();
    let lit = nearest(&stops, s.magnifier.unwrap_or(1.0));
    let zooms = MAGNIFIERS.iter().enumerate().map(|(i, (v, name))| choice_of(tr(name), i == lit, v.to_string())).collect();
    view.groups.push(Group { labelled: true, ..group_of(Some("Magnifier"), "magnifier", zooms) });
    view.groups.push(on_off_of(Some("Reduce motion"), "access-motion", s.reduce_motion.unwrap_or(false)));
    if crate::launch::on_path("orca") {
        view.groups.push(on_off_of(Some("Screen reader"), "access-reader", reader_running()));
    } else {
        let installing = INSTALLING.load(std::sync::atomic::Ordering::Relaxed);
        let word = if installing { tr("Installing the screen reader…") } else { tr("Install a screen reader (Orca)") };
        view.groups.push(Group { buttons: true, ..group_of(None, "access-reader-install", vec![choice_of(word, false, READER_ATTR)]) });
    }
    view.groups.push(Group { centred: true, ..line_of(tr("For more contrast, choose a high-contrast palette in Colours.").to_owned()) });
}

/// Whether Orca is running for this session.
fn reader_running() -> bool {
    crate::controls::out_of("pgrep", &["-x", "orca"]).is_some_and(|o| !o.trim().is_empty())
}

/// A pick on the worker. True when the key was this page's.
pub(crate) fn apply(key: &str, value: &str) -> bool {
    match key {
        "magnifier" => {
            if let Ok(z) = value.parse::<f64>() {
                GolemSettings::change(|s| {
                    s.magnifier = Some(z.clamp(1.0, 8.0));
                    s.store_input();
                });
            }
        }
        "access-reader" => {
            if value == "on" {
                if !reader_running() {
                    let _ = std::process::Command::new("orca")
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn();
                }
            } else {
                crate::controls::run_of("pkill", &["-x", "orca"]);
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
    fn the_sizes_light_the_nearest_stop() {
        let stops: Vec<f64> = TEXT_SIZES.iter().map(|t| t.0).collect();
        assert_eq!(nearest(&stops, 1.2), 1);
        assert_eq!(nearest(&stops, 1.6), 3);
        let stops: Vec<f64> = MAGNIFIERS.iter().map(|t| t.0).collect();
        assert_eq!(nearest(&stops, 1.0), 0);
    }
}
