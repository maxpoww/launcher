//! TITLE BARS, THE OWNER'S WAY (Max, 2026-11-18: "en tiles quiero estas
//! opciones: barra de titulo = dos togles: on/off en flotante y en auto
//! arrange (tiling) | botones: izquierda/derecha (esto cambian los botones de
//! cerrar, min, max, de lado con los de pin y card) y también en botones
//! quiero: colores (semáforo) = on/off").
//!
//! The bars are the waveview plugin's (its fork of hyprbars); the choice is
//! kept here, in `settings.json`, and told to the plugin with
//! `hl.plugin.waveview.bars(floating, tiled, right, colors)`: at every pick,
//! at the shell's start, and when the plugin asks (`title-bars`, sent as its
//! bars start — a plugin loaded after the shell, or reloaded, wears it too).
//! An older plugin without `bars` refuses the call: it keeps its bars as made.

use serde::{Deserialize, Serialize};

/// What the owner chose; as made = a bar on the floating windows only, the
/// three discs at the left, in colour (not written).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct TitleBars {
    /// A bar on the floating windows.
    pub floating: bool,
    /// A bar on the windows the layout arranges (tiled).
    pub tiled: bool,
    /// Close, minimize and maximize at the RIGHT end of the bar (the pin and
    /// the card then at the left).
    pub right: bool,
    /// The three discs red, orange and green; off, grey in the bar's ink.
    pub colors: bool,
}

impl Default for TitleBars {
    fn default() -> Self {
        Self {
            floating: true,
            tiled: false,
            right: false,
            colors: true,
        }
    }
}

impl TitleBars {
    pub(crate) fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Any window wears a bar (the buttons' choices mean something).
    pub(crate) fn any(&self) -> bool {
        self.floating || self.tiled
    }

    /// A pick on the page: its key and value. False for a key that is not
    /// one of the page's.
    pub(crate) fn set(&mut self, key: &str, value: &str) -> bool {
        let on = value == "on";
        match key {
            "tiles-bars-floating" => self.floating = on,
            "tiles-bars-tiled" => self.tiled = on,
            "tiles-bars-side" => self.right = value == "right",
            "tiles-bars-colors" => self.colors = on,
            _ => return false,
        }
        true
    }
}

/// The plugin's call for a choice.
pub(crate) fn lua(b: &TitleBars) -> String {
    format!(
        "hl.plugin.waveview.bars({}, {}, {}, {})",
        b.floating, b.tiled, b.right, b.colors
    )
}

/// The choice in `settings.json`, told to the plugin now — with the windows'
/// corners (`corners::plugin_lua`): a plugin loaded or reloaded asks once
/// for both.
pub(crate) fn tell() {
    let b = crate::display::GolemSettings::load().title_bars;
    let lua = format!(
        "{} {}",
        lua(&b),
        crate::corners::plugin_lua(crate::corners::windows_square())
    );
    if crate::hypr::eval_ok(&lua) {
        tracing::info!("title bars: {lua}");
    } else {
        tracing::warn!("title bars: the compositor refused {lua} (a plugin without `bars`?)");
    }
}

/// At the shell's start: a choice that is not Golem's own is told (a plugin
/// already loaded has its bars as made; one loading later asks).
pub(crate) fn assert_at_start() {
    if !crate::display::GolemSettings::load()
        .title_bars
        .is_default()
    {
        tell();
    }
}

/// The page's keys.
pub(crate) fn is_key(key: &str) -> bool {
    key.starts_with("tiles-bars-")
}

/// A pick on Controls → Tiles: kept, then told.
pub(crate) fn pick(key: &str, value: &str) {
    crate::display::GolemSettings::change(|s| {
        if s.title_bars.set(key, value) {
            s.store();
        }
    });
    tell();
}

/// Controls → Tiles' part: which windows wear one, then (while any does)
/// the side of the buttons and their colours.
pub(crate) fn view(view: &mut crate::controls::ControlView) {
    use crate::controls::{choice_of, group_of, line_of, on_off_of};
    use crate::i18n::tr;
    let b = crate::display::GolemSettings::load().title_bars;
    view.groups.push(line_of(tr("Title bars").to_owned()));
    view.groups.push(on_off_of(
        Some("Floating windows"),
        "tiles-bars-floating",
        b.floating,
    ));
    view.groups.push(on_off_of(
        Some("Tiled windows"),
        "tiles-bars-tiled",
        b.tiled,
    ));
    if !b.any() {
        return;
    }
    view.groups.push(group_of(
        Some("Buttons"),
        "tiles-bars-side",
        vec![
            choice_of(tr("Left"), !b.right, "left"),
            choice_of(tr("Right"), b.right, "right"),
        ],
    ));
    view.groups.push(on_off_of(
        Some("Button colours"),
        "tiles-bars-colors",
        b.colors,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_made_floating_only_left_in_colour_and_nothing_written() {
        let b = TitleBars::default();
        assert!(b.is_default() && b.floating && !b.tiled && !b.right && b.colors);
        let s: crate::display::GolemSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.title_bars, b);
        assert!(!serde_json::to_string(&s).unwrap().contains("title_bars"));
        assert_eq!(lua(&b), "hl.plugin.waveview.bars(true, false, false, true)");
    }

    #[test]
    fn each_pick_sets_its_own_choice() {
        let mut b = TitleBars::default();
        assert!(b.set("tiles-bars-tiled", "on"));
        assert!(b.set("tiles-bars-floating", "off"));
        assert!(b.set("tiles-bars-side", "right"));
        assert!(b.set("tiles-bars-colors", "off"));
        assert!(!b.set("tiles-gap", "50"));
        assert_eq!(
            b,
            TitleBars {
                floating: false,
                tiled: true,
                right: true,
                colors: false
            }
        );
        assert_eq!(lua(&b), "hl.plugin.waveview.bars(false, true, true, false)");
        let back: TitleBars = serde_json::from_str(&serde_json::to_string(&b).unwrap()).unwrap();
        assert_eq!(back, b);
        // A file that names only one choice keeps the others as made.
        let one: TitleBars = serde_json::from_str(r#"{"tiled":true}"#).unwrap();
        assert!(one.floating && one.tiled && !one.right && one.colors);
        b.set("tiles-bars-side", "left");
        assert!(!b.right);
        assert!(!TitleBars {
            floating: false,
            tiled: false,
            ..b
        }
        .any());
    }
}
