//! Golem's memory of the screen.
//!
//! A display effect — today the eye-protection warmth, tomorrow the colour
//! filters — is *intent*, not a one-off command. The thing that carries it out
//! is fragile in two different ways, and both of them made the warmth quietly
//! vanish (Max, 2026-09-11: "for some reason the nightlight goes away"):
//!
//! * **The compositor drops it.** `hyprsunset` holds a colour matrix through
//!   `hyprland-ctm-control-v1`, and Hyprland pushes that matrix to the DRM
//!   state only when it *changes* (`Renderer.cpp`, `m_ctmUpdated`). Anything
//!   that rebuilds the connector's atomic state — a VT switch, a modeset, a
//!   monitor re-init — drops the property, and nobody re-pushes it. hyprsunset
//!   is never told, so it keeps reporting a temperature the screen stopped
//!   wearing: the process is alive, `hyprctl hyprsunset temperature` answers
//!   4000, and the screen is cold.
//! * **A reboot loses it outright.** Nothing on the system remembered that the
//!   user had ever asked for warmth.
//!
//! So the intent lives here instead, and the daemon re-asserts it:
//!
//! * [`ScreenState`] is the desired look, persisted to `screen.json` — it
//!   outlives the daemon, the session, and the machine.
//! * [`App::apply_screen_state`] makes the world match it, and is idempotent,
//!   so it can be called as often as we like.
//! * [`App::reassert_screen_state`] is that call at every moment the world can
//!   have drifted underneath us: daemon start (covers reboot and login) and
//!   Hyprland's monitor/config events (cover the DRM rebuilds).
//!
//! Nothing else may set a screen effect. With one owner, "what is the screen
//! wearing?" is answerable from memory rather than by interrogating whichever
//! process happens to be applying it — which is what lets the sunset panel mark
//! its active preset, and what the filters panel will read when it arrives.

use serde::{Deserialize, Serialize};

use crate::App;

/// hyprsunset's neutral identity — the screen untouched. Remembered like any
/// other choice (the user did pick it), but asserting it is a no-op by
/// definition: identity is what the compositor does on its own, so a dropped
/// CTM already *is* neutral and there is nothing to hold.
pub(crate) const NEUTRAL_K: u32 = 6500;

/// What the screen should look like. One field today; colour filters will land
/// beside it, each with its own applier in [`App::apply_screen_state`].
#[derive(Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenState {
    /// Colour temperature in Kelvin. `None` = never chosen; the screen wears
    /// whatever the compositor gives it.
    #[serde(default)]
    pub temperature_k: Option<u32>,
}

impl ScreenState {
    /// Read the remembered state, or start blank. A corrupt/absent file is not
    /// worth a word: a fresh state means "the screen wears its own look", which
    /// is exactly what a machine with no memory of a choice should do.
    pub fn load() -> Self {
        crate::persist::read_json(&crate::persist::data_path("screen.json")).unwrap_or_default()
    }
}

/// Which side of dusk the night light's tick last saw: 0 none yet, 1 day,
/// 2 night. It acts on a change of side only, so a warmth the owner picks by
/// hand meanwhile stands until the next dusk or dawn.
static NIGHT_SIDE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Civil dusk: the sun this far under the horizon.
const DUSK_DEG: f64 = -6.0;

/// The warmth the night gets when none was ever chosen.
pub(crate) const NIGHT_DEFAULT_K: u32 = 4000;

impl App {
    /// THE NIGHT LIGHT BY ITSELF (the Controls gap audit, 2026-11-06: every
    /// settings app turns it on at sunset): with "At night" on, the warmth
    /// last chosen goes on at dusk and off at dawn — the sun's elevation from
    /// the time zone, asked by the lighting's own tick (every 30 s). At the
    /// dock's start the sky's side is taken as it is (a laptop opened at
    /// night comes up warm). "At night" turned on or off is a fresh look.
    pub(crate) fn night_light_tick(&mut self) {
        let s = crate::display::GolemSettings::load();
        if s.night_auto != Some(true) {
            NIGHT_SIDE.store(0, std::sync::atomic::Ordering::Relaxed);
            return;
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
        let Some(elevation) = options_engine::sun_elevation_deg(now) else {
            return;
        };
        let side = if elevation < DUSK_DEG { 2 } else { 1 };
        if NIGHT_SIDE.swap(side, std::sync::atomic::Ordering::Relaxed) == side {
            return;
        }
        let want = if side == 2 { s.night_k.unwrap_or(NIGHT_DEFAULT_K).clamp(1000, NEUTRAL_K) } else { NEUTRAL_K };
        if self.screen.temperature_k.unwrap_or(NEUTRAL_K) != want {
            tracing::info!("screen: night light {} by itself (sun at {elevation:.1}°)", if side == 2 { "on" } else { "off" });
            self.set_screen_temperature(want);
        }
    }

    /// "At night" was turned on or off: the sky is looked at again now.
    pub(crate) fn night_auto_changed(&mut self) {
        NIGHT_SIDE.store(0, std::sync::atomic::Ordering::Relaxed);
        self.night_light_tick();
    }

    /// Remember a screen temperature and put it on. The one way a temperature
    /// is ever set — the sunset prompt's [turn on] and the panel's presets both
    /// come through here, so the memory can never disagree with the screen.
    pub(crate) fn set_screen_temperature(&mut self, k: u32) {
        self.screen.temperature_k = Some(k);
        self.save_screen_state();
        if k == NEUTRAL_K {
            self.clear_screen_temperature();
        }
        self.apply_screen_state();
        self.draw_options();
    }

    /// Put the remembered state back on the screen without changing it. Safe to
    /// call whenever the world may have drifted — if the effect is still
    /// applied this re-sends the same value, which the compositor animates from
    /// a matrix to itself, i.e. no visible change.
    pub(crate) fn reassert_screen_state(&self) {
        self.apply_screen_state();
    }

    /// Make the world match [`Self::screen`]. Idempotent.
    fn apply_screen_state(&self) {
        if let Some(k) = self.screen.temperature_k {
            self.apply_screen_temperature(k);
        }
    }

    /// The temperature applier. `hyprctl` talks to a running hyprsunset; when
    /// none is running it exits non-zero (3) and the fallback starts one — which
    /// is how a remembered temperature survives a reboot, where nothing is
    /// running yet. Detached (`launch`), so it outlives this daemon.
    fn apply_screen_temperature(&self, k: u32) {
        if k == NEUTRAL_K {
            return;
        }
        let cmd = format!("hyprctl hyprsunset temperature {k} || hyprsunset -t {k}");
        if let Err(e) = crate::launch::launch(&cmd, false, &self.config.launch.terminal) {
            tracing::warn!("screen: applying temperature {k} failed: {e:#}");
        }
    }

    /// Turning the warmth OFF is the one moment neutral is not a no-op: a
    /// hyprsunset may be running with a warm matrix on, and the compositor will
    /// not drop it on its own. Set it to neutral — never start one for it
    /// (`|| true`): with no hyprsunset the screen is already neutral. The
    /// process stays, so the sunset prompt does not ask again the same night.
    fn clear_screen_temperature(&self) {
        let cmd = format!("hyprctl hyprsunset temperature {NEUTRAL_K} || true");
        if let Err(e) = crate::launch::launch(&cmd, false, &self.config.launch.terminal) {
            tracing::warn!("screen: clearing the temperature failed: {e:#}");
        }
    }

    fn save_screen_state(&self) {
        crate::persist::write_json(
            "screen",
            &crate::persist::data_path("screen.json"),
            &self.screen,
        );
    }
}
