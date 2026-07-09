//! Configuration types, loaded from
//! `$XDG_CONFIG_HOME/waverunner/config.toml` (default `~/.config/...`).
//!
//! Every field has a default so a missing or partial file always yields a
//! usable configuration. Unknown keys are rejected to catch typos early.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Errors that can occur while loading the configuration file.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file exists but could not be read.
    #[error("failed to read {path}: {source}")]
    Io {
        /// Path of the offending file.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// The file was read but is not valid TOML / does not match the schema.
    #[error("failed to parse {path}: {source}")]
    Parse {
        /// Path of the offending file.
        path: PathBuf,
        /// Underlying TOML error.
        source: toml::de::Error,
    },
}

/// Top-level configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Popup geometry.
    pub window: WindowConfig,
    /// Animation curves and timings.
    pub animation: AnimationConfig,
    /// Colors and shapes.
    pub theme: ThemeConfig,
    /// Pointer/gesture behavior.
    pub input: InputConfig,
}

impl Config {
    /// Compute the config file path from `XDG_CONFIG_HOME`, falling back
    /// to `~/.config`.
    pub fn default_path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("waverunner").join("config.toml")
    }

    /// Load configuration from [`Config::default_path`].
    ///
    /// A missing file is not an error: defaults are returned. A present
    /// but malformed file *is* an error — silently ignoring a broken
    /// config confuses users more than failing loudly.
    pub fn load() -> Result<Self, ConfigError> {
        Self::load_from(Self::default_path())
    }

    /// Load configuration from an explicit path (missing file ⇒ defaults).
    pub fn load_from(path: PathBuf) -> Result<Self, ConfigError> {
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => return Err(ConfigError::Io { path, source }),
        };
        toml::from_str(&text).map_err(|source| ConfigError::Parse { path, source })
    }
}

/// Card geometry, in logical pixels (scale factor applied by the daemon).
///
/// The UI is one fixed-size rounded card that slides up from below the
/// bottom screen edge. Docked, only its top `input_bar_height` pixels
/// peek above the edge; open, the whole card floats `bottom_margin`
/// pixels above it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowConfig {
    /// Width of the card.
    pub width: u32,
    /// Full height of the card (input bar + result list).
    pub height: u32,
    /// Height of the card's top sliver shown in the docked state.
    pub input_bar_height: u32,
    /// Gap between the fully risen card and the bottom screen edge.
    pub bottom_margin: u32,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            width: 720,
            height: 560,
            input_bar_height: 42,
            bottom_margin: 12,
        }
    }
}

/// Animation configuration for the open and close transitions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AnimationConfig {
    /// Curve used while opening (Hidden -> Open).
    pub open: CurveConfig,
    /// Curve used while closing (Open -> Hidden).
    pub close: CurveConfig,
}

/// The shape of a single animation curve.
///
/// `kind` selects the curve; `duration_ms` applies to the eased kinds,
/// while the `spring_*` fields apply to `kind = "spring"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CurveConfig {
    /// One of: `spring`, `ease-out-quart`, `ease-out-cubic`, `ease-in-cubic`.
    pub kind: CurveKind,
    /// Duration in milliseconds (eased kinds only).
    pub duration_ms: u32,
    /// Spring stiffness (spring kind only).
    pub spring_stiffness: f32,
    /// Spring damping (spring kind only).
    pub spring_damping: f32,
    /// Spring mass (spring kind only).
    pub spring_mass: f32,
}

impl Default for CurveConfig {
    fn default() -> Self {
        Self {
            kind: CurveKind::Spring,
            duration_ms: 220,
            spring_stiffness: 550.0,
            spring_damping: 42.0,
            spring_mass: 1.0,
        }
    }
}

impl CurveConfig {
    /// Default close curve: faster, ease-in-cubic, per the design notes.
    pub fn default_close() -> Self {
        Self {
            kind: CurveKind::EaseInCubic,
            duration_ms: 140,
            ..Self::default()
        }
    }
}

impl Default for AnimationConfig {
    fn default() -> Self {
        Self {
            open: CurveConfig::default(),
            close: CurveConfig::default_close(),
        }
    }
}

/// Enumeration of supported curve kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CurveKind {
    /// Damped spring (slight settle, no big overshoot).
    Spring,
    /// Ease-out quartic.
    EaseOutQuart,
    /// Ease-out cubic.
    EaseOutCubic,
    /// Ease-in cubic.
    EaseInCubic,
}

/// Pointer/gesture behavior.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InputConfig {
    /// Natural (content-follows-fingers) scroll direction for the dock
    /// expand/collapse gesture: scrolling *down* on the dock opens the
    /// popup, scrolling *up* collapses it. Set to `false` for classic
    /// wheel direction (up opens).
    pub natural_scroll: bool,
    /// Reveal the dock when the pointer touches the bottom screen edge
    /// (within the dock's horizontal span). While hidden, a thin
    /// `edge_reveal_px` strip stays pointer-sensitive.
    pub edge_reveal: bool,
    /// Height of the edge-reveal strip, in logical pixels. Keep a few px
    /// of margin: some pointer stacks (e.g. VM tablet integration) clamp
    /// the cursor short of the true last screen row.
    pub edge_reveal_px: u32,
    /// Hide the dock again when the pointer leaves it.
    pub autohide: bool,
    /// Grace period before auto-hiding, in milliseconds. Re-entering
    /// within this window cancels the hide.
    pub autohide_delay_ms: u32,
    /// Keep the dock visible while no window overlaps its zone and only
    /// hide when one does (macOS dodge-windows). Needs Hyprland IPC;
    /// silently falls back to always-auto-hide elsewhere.
    pub intellihide: bool,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            natural_scroll: true,
            edge_reveal: true,
            edge_reveal_px: 5,
            autohide: true,
            autohide_delay_ms: 300,
            intellihide: true,
        }
    }
}

/// Colors and shape parameters. Colors are `#rrggbb` or `#rrggbbaa` hex.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeConfig {
    /// Card background color.
    pub background: String,
    /// Radius of the card corners, in logical pixels.
    pub corner_radius: f32,
    /// Primary text color (app names).
    pub text: String,
    /// Hover-highlight fill behind dock icons and list rows.
    pub highlight: String,
    /// Icon theme name for `freedesktop-icons` lookup (falls back to
    /// `hicolor` automatically).
    pub icon_theme: String,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            background: "#1e1e2ecc".to_owned(),
            corner_radius: 24.0,
            text: "#e6e6efff".to_owned(),
            highlight: "#ffffff26".to_owned(),
            icon_theme: "hicolor".to_owned(),
        }
    }
}

impl ThemeConfig {
    /// Parse [`ThemeConfig::background`] into linear-ish RGBA floats in
    /// `0.0..=1.0`. Invalid values fall back to an opaque dark grey rather
    /// than erroring per frame.
    pub fn background_rgba(&self) -> [f32; 4] {
        parse_hex_rgba(&self.background).unwrap_or([0.12, 0.12, 0.18, 1.0])
    }

    /// Parse [`ThemeConfig::text`], falling back to near-white.
    pub fn text_rgba(&self) -> [f32; 4] {
        parse_hex_rgba(&self.text).unwrap_or([0.9, 0.9, 0.94, 1.0])
    }

    /// Parse [`ThemeConfig::highlight`], falling back to faint white.
    pub fn highlight_rgba(&self) -> [f32; 4] {
        parse_hex_rgba(&self.highlight).unwrap_or([1.0, 1.0, 1.0, 0.15])
    }
}

/// Parse `#rgb`, `#rrggbb`, or `#rrggbbaa` into RGBA floats.
fn parse_hex_rgba(s: &str) -> Option<[f32; 4]> {
    let hex = s.strip_prefix('#')?;
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    match hex.len() {
        6 => Some([
            byte(0)? as f32 / 255.0,
            byte(2)? as f32 / 255.0,
            byte(4)? as f32 / 255.0,
            1.0,
        ]),
        8 => Some([
            byte(0)? as f32 / 255.0,
            byte(2)? as f32 / 255.0,
            byte(4)? as f32 / 255.0,
            byte(6)? as f32 / 255.0,
        ]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let c = Config::default();
        assert!(c.window.input_bar_height < c.window.height);
        assert_eq!(c.animation.close.kind, CurveKind::EaseInCubic);
    }

    #[test]
    fn partial_toml_fills_defaults() {
        let c: Config = toml::from_str("[window]\nwidth = 800\n").unwrap();
        assert_eq!(c.window.width, 800);
        assert_eq!(c.window.height, WindowConfig::default().height);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(toml::from_str::<Config>("[window]\nwdith = 800\n").is_err());
    }

    #[test]
    fn hex_colors_parse() {
        assert_eq!(parse_hex_rgba("#ff0000"), Some([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(parse_hex_rgba("#00000000"), Some([0.0, 0.0, 0.0, 0.0]));
        assert_eq!(parse_hex_rgba("nope"), None);
    }
}
