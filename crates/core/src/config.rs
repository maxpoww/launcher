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
    /// Application launch behavior.
    pub launch: LaunchConfig,
    /// The "OPTIONS" topbar — a reserved strip at the top of the screen.
    pub options: OptionsConfig,
}

/// Application launch behavior.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LaunchConfig {
    /// Command prefix used to run `Terminal=true` apps inside a terminal
    /// emulator. The app's command line is appended as `sh -c '<exec>'`,
    /// which suits terminals that accept a trailing command directly
    /// (foot, kitty, wezterm); terminals that need a flag include it
    /// here, e.g. `terminal = "alacritty -e"`.
    pub terminal: String,
}

impl Default for LaunchConfig {
    fn default() -> Self {
        Self {
            terminal: "foot".to_owned(),
        }
    }
}

/// The "OPTIONS" topbar: a second layer-shell surface anchored to the top
/// edge, reserving its own strip (exclusive zone) so windows and the dock
/// lay out beneath it. This is the foundation for glanceable status and
/// quick-setting modules; for now it is just a glass bar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OptionsConfig {
    /// Whether the topbar is created at all.
    pub enabled: bool,
    /// Height of the reserved strip, in logical pixels.
    pub height: u32,
    /// Integer supersampling factor, matching [`WindowConfig::render_scale`]
    /// so text and icons stay crisp on HiDPI/fractional outputs.
    pub render_scale: u32,
}

impl Default for OptionsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            height: 28,
            render_scale: 2,
        }
    }
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
    /// Integer supersampling factor: the daemon renders its buffer at
    /// `logical × render_scale` physical pixels and lets the compositor
    /// downscale to the (possibly fractional) output scale. `2` keeps
    /// icons and text crisp on HiDPI/fractional displays; `1` renders at
    /// logical resolution (sharp only on scale-1 outputs).
    pub render_scale: u32,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            width: 720,
            // Tall enough for the three popup sections: the 6×3 app
            // grid plus the single-row Install and Files strips.
            height: 680,
            input_bar_height: 42,
            bottom_margin: 8,
            render_scale: 2,
        }
    }
}

/// Animation configuration for the open and close transitions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AnimationConfig {
    /// Curve for the box growing (Dock -> Open, or straight to Open).
    pub open: CurveConfig,
    /// Curve for the box shrinking (Open -> Dock/Hidden).
    pub close: CurveConfig,
    /// Curve for the autohide dock sliding up (Hidden -> Dock).
    pub dock_reveal: CurveConfig,
    /// Curve for the autohide dock sliding down (Dock -> Hidden).
    pub dock_hide: CurveConfig,
    /// Duration in milliseconds of the app-box open/close grow (eased);
    /// deliberately slow so the box is seen expanding to fill the grid.
    pub group_expand_ms: u32,
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
            // Box open: lightly underdamped (ζ≈0.85) so it overshoots the
            // top a touch and settles back — a subtle bounce. Tuned live
            // to ~480ms so the content ride reads without dragging.
            spring_stiffness: 622.0,
            spring_damping: 42.0,
            spring_mass: 1.0,
        }
    }
}

impl CurveConfig {
    /// Default close curve: a lightly-underdamped spring so the box dips
    /// a touch past the dock rest before settling — a subtle bounce.
    pub fn default_close() -> Self {
        Self {
            // Box close: critically damped and fast (ζ≈1, near-instant) —
            // the card snaps onto the dock rest and stops. No dip.
            kind: CurveKind::Spring,
            spring_stiffness: 5000.0,
            spring_damping: 141.0,
            spring_mass: 1.0,
            ..Self::default()
        }
    }
}

impl Default for AnimationConfig {
    fn default() -> Self {
        Self {
            open: CurveConfig::default(),
            close: CurveConfig::default_close(),
            // Dock autohide slide, tuned independently of the box: a
            // crisp spring up (small settle), a quick ease down. Same
            // feel as the earlier 30/9 · 1800ms tuning, at real speed.
            dock_reveal: CurveConfig {
                // Calling the dock is a plain eased rise — no spring, so it
                // can never overshoot or bounce; the bar just slides up and
                // stops. An eased curve also reports zero velocity, so the
                // AGUA icon water stays flat on a summon (only the box
                // open/close spring sloshes it).
                kind: CurveKind::EaseOutCubic,
                duration_ms: 200,
                ..CurveConfig::default()
            },
            dock_hide: CurveConfig {
                kind: CurveKind::EaseInCubic,
                duration_ms: 95,
                ..CurveConfig::default()
            },
            group_expand_ms: 190,
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
    /// Radius of the dock/main-card corners, in logical pixels. Kept modest
    /// for a macOS-dock look; the pop-out folder boxes round more than this.
    pub corner_radius: f32,
    /// Primary text color (app names).
    pub text: String,
    /// Hover-highlight fill behind dock icons and list rows.
    pub highlight: String,
    /// Icon theme name for `freedesktop-icons` lookup (falls back to
    /// `hicolor` automatically).
    pub icon_theme: String,
    /// Frosted squircle plate behind every icon: trims each icon to its
    /// content, scales it to a uniform size and centers it on an identical
    /// translucent rounded tile, so a mix of circular and square theme
    /// icons reads as one cohesive dock (macOS/iOS style). Baked once per
    /// icon (cached), so there is no per-frame cost.
    pub icon_plate: bool,
    /// Superellipse corner-mask exponent, used only when `icon_plate` is
    /// off: ~5 ≈ Apple corners, lower is rounder (2 ≈ circle), higher is
    /// squarer. `0` disables it. It only trims a tile's extreme corners,
    /// so it rounds full-bleed square icons but leaves circular or padded
    /// ones unchanged — hence plates are the default for real cohesion.
    pub icon_squircle: f32,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            background: "#05070980".to_owned(),
            corner_radius: 16.0,
            text: "#ffffffff".to_owned(),
            highlight: "#05070920".to_owned(),
            icon_theme: "hicolor".to_owned(),
            icon_plate: true,
            icon_squircle: 0.0,
        }
    }
}

impl ThemeConfig {
    /// Parse [`ThemeConfig::background`] into linear-ish RGBA floats in
    /// `0.0..=1.0`. Invalid values fall back to an opaque dark grey rather
    /// than erroring per frame.
    pub fn background_rgba(&self) -> [f32; 4] {
        parse_hex_rgba(&self.background).unwrap_or([0.02, 0.027, 0.035, 0.502])
    }

    /// Parse [`ThemeConfig::text`], falling back to white.
    pub fn text_rgba(&self) -> [f32; 4] {
        parse_hex_rgba(&self.text).unwrap_or([1.0, 1.0, 1.0, 1.0])
    }

    /// Parse [`ThemeConfig::highlight`], falling back to a faint dark tint.
    pub fn highlight_rgba(&self) -> [f32; 4] {
        parse_hex_rgba(&self.highlight).unwrap_or([0.02, 0.027, 0.035, 0.125])
    }
}


/// Parse `#rrggbb` or `#rrggbbaa` into RGBA floats.
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
        // Box close is a lightly-underdamped spring (subtle settle bounce)
        // by design — see CurveConfig::default_close.
        assert_eq!(c.animation.close.kind, CurveKind::Spring);
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
