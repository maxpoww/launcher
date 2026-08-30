//! Battery OPTION — v1, deliberately minimal (an SH emergency: dying
//! batteries were hard-killing whole sessions).
//!
//! The first *new* module built through the Spine: it senses **only** via
//! the Brain ([`options_engine::SystemMetrics`], polled from
//! `/sys/class/power_supply` every 3 s by the system collector) — no
//! daemon-side sensing at all.
//!
//! Behavior:
//! - **≤ 10 % and discharging** → a red badge on the topbar clock pill
//!   (`options.rs` draws it from [`crate::App::battery_low`]).
//! - **≤ 5 % and discharging** → `systemctl suspend` (logind allows it for
//!   the active session, no root). The session survives instead of dying
//!   with the battery.
//!
//! Suspend discipline: firing disarms; plugging in or climbing back above
//! the threshold re-arms. If the machine is woken *still* critical and
//! discharging, it re-suspends after [`RESUSPEND_GRACE`] — long enough to
//! reach the charger, short enough that ignoring it can't kill the session.
//!
//! Grow-later (S3, per Max): percent readout, charging state surface,
//! Mind-ranked "plug in" affordance, power profiles.

use std::time::{Duration, Instant};

use options_engine::ContextState;
use tracing::{info, warn};

use crate::App;

/// Discharging at or below this shows the red badge.
pub(crate) const LOW_PCT: u8 = 10;
/// Discharging at or below this suspends the machine.
pub(crate) const SUSPEND_PCT: u8 = 5;
/// Woken while still critical: how long the user has to reach a charger
/// before the next protective suspend.
const RESUSPEND_GRACE: Duration = Duration::from_secs(180);

impl App {
    /// React to a Brain snapshot's battery state. Called from `on_brain`;
    /// cheap and idempotent (acts only on transitions).
    pub(crate) fn check_battery(&mut self, ctx: &ContextState) {
        let Some(pct) = ctx.metrics.battery_pct else {
            // Desktop / no data: never show the badge.
            if self.battery_low {
                self.battery_low = false;
                self.draw_options();
            }
            return;
        };
        let charging = ctx.metrics.is_charging;

        let low = pct <= LOW_PCT && !charging;
        if low != self.battery_low {
            self.battery_low = low;
            info!("battery: {pct}% (charging: {charging}) — low: {low}");
            self.draw_options();
        }

        if pct > SUSPEND_PCT || charging {
            self.battery_suspend_armed = true;
        } else if self.battery_suspend_armed
            || self
                .battery_last_suspend
                .is_none_or(|t| t.elapsed() > RESUSPEND_GRACE)
        {
            self.battery_suspend_armed = false;
            self.battery_last_suspend = Some(Instant::now());
            warn!("battery: {pct}% and discharging — suspending to save the session");
            if let Err(e) = std::process::Command::new("systemctl").arg("suspend").spawn() {
                warn!("battery: suspend failed to spawn: {e}");
            }
        }
    }
}
