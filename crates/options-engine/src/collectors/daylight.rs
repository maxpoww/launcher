//! Layer 4 (part) — daylight: is the sun below the horizon *here*, right now?
//!
//! "Golem should know when sunset is" — and it can, entirely locally:
//!
//! 1. **Where is here?** The system timezone (`/etc/localtime` resolves into
//!    the tzdb) names a city; the tzdb's own `zone1970.tab` carries that
//!    city's coordinates. City-level accuracy moves a sunset by under a
//!    minute — plenty. No network, no GPS, no configuration.
//! 2. **Where is the sun?** The standard low-precision solar-position
//!    algorithm (Meeus/NOAA approximation, good to ~0.01°) gives the sun's
//!    elevation for a UTC instant at those coordinates. "After sunset" is
//!    simply elevation below the horizon (−0.833°, the civil sunset zenith:
//!    refraction plus the solar radius) — which stays true through the night
//!    and turns itself off at sunrise, with no per-day scheduling at all.
//!
//! The same poll also senses whether a `hyprsunset` process is already
//! running (a `/proc` comm scan, no subprocess), so the mind's offer can
//! withdraw itself the moment eye protection is on — however it was started.
//!
//! If no location can be resolved (no `/etc/localtime`, a zone missing from
//! the table), the collector emits nothing and the [`Layer::Daylight`] health
//! stays dark — the mind then never surfaces daylight affordances, rather
//! than being told a false "daytime".

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::{mpsc, watch};

use crate::collector::{Collector, CollectorFuture};
use crate::message::{ContextDelta, Update};
use crate::state::{ContextState, DaylightState, Layer};

/// How often to look. The elevation drifts by ~0.02°/5s at these latitudes and
/// the hyprsunset check should feel responsive after a click, so this is the
/// snappier of the two needs; both reads are trivially cheap (pure math + one
/// /proc sweep).
const POLL: Duration = Duration::from_secs(5);

/// Civil sunset elevation: atmospheric refraction (~0.567°) plus the solar
/// radius (~0.266°) — the sun's centre is this far below the geometric horizon
/// when its upper limb visually disappears.
const SUNSET_ELEVATION_DEG: f64 = -0.833;

pub struct DaylightCollector;

impl DaylightCollector {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self
    }
}

impl Collector for DaylightCollector {
    fn name(&self) -> &'static str {
        "daylight"
    }
    fn layer(&self) -> Layer {
        Layer::Daylight
    }
    fn run(
        self: Box<Self>,
        _ctx: watch::Receiver<ContextState>,
        tx: mpsc::Sender<Update>,
    ) -> CollectorFuture {
        Box::pin(async move {
            // Location resolves once: the timezone changing mid-session is a
            // travel-day rarity, and the next daemon start picks it up.
            let Some((lat, lon)) = system_location() else {
                tracing::info!("daylight: no location from timezone — staying dark");
                return Ok(());
            };
            tracing::info!("daylight: location from timezone: {lat:.2}, {lon:.2}");
            loop {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0);
                let state = DaylightState {
                    after_sunset: solar_elevation_deg(now, lat, lon) < SUNSET_ELEVATION_DEG,
                    eye_protection_on: hyprsunset_running(),
                };
                if tx
                    .send(Update::Delta(Layer::Daylight, ContextDelta::Daylight(state)))
                    .await
                    .is_err()
                {
                    return Ok(()); // aggregator gone
                }
                tokio::time::sleep(POLL).await;
            }
        })
    }
}

/// This machine's coordinates, resolved from its timezone: `/etc/localtime`'s
/// symlink target names the zone; the tzdb's `zone1970.tab` locates it.
fn system_location() -> Option<(f64, f64)> {
    let zone = system_zone()?;
    // NixOS exposes the tzdb at /etc/zoneinfo; most other distros under /usr.
    for tab in [
        "/etc/zoneinfo/zone1970.tab",
        "/usr/share/zoneinfo/zone1970.tab",
    ] {
        if let Ok(text) = std::fs::read_to_string(tab) {
            if let Some(c) = coords_for_zone(&text, &zone) {
                return Some(c);
            }
        }
    }
    None
}

/// The IANA zone name (`America/La_Paz`) from `/etc/localtime`'s target path,
/// which ends `…/zoneinfo/<zone>` wherever the tzdb itself lives.
fn system_zone() -> Option<String> {
    let target = std::fs::read_link("/etc/localtime").ok()?;
    let s = target.to_str()?;
    let idx = s.find("/zoneinfo/")?;
    Some(s[idx + "/zoneinfo/".len()..].to_string())
}

/// Find `zone`'s coordinates in `zone1970.tab` text. Pure, so the tab-file
/// parsing is unit-testable. Lines are `codes<TAB>coords<TAB>zone[<TAB>comment]`;
/// coordinates are ISO 6709 `±DDMM±DDDMM` or `±DDMMSS±DDDMMSS`.
fn coords_for_zone(tab: &str, zone: &str) -> Option<(f64, f64)> {
    for line in tab.lines() {
        if line.starts_with('#') {
            continue;
        }
        let mut fields = line.split('\t');
        let (_codes, coords, name) = (fields.next()?, fields.next()?, fields.next()?);
        if name == zone {
            return parse_iso6709(coords);
        }
    }
    None
}

/// Parse an ISO 6709 lat+lon pair (`-1630-06809`): the longitude begins at the
/// second sign; degrees are 2 digits for latitude, 3 for longitude, then MM
/// and optionally SS.
fn parse_iso6709(s: &str) -> Option<(f64, f64)> {
    let lon_at = s[1..].find(['+', '-'])? + 1;
    let lat = parse_dms(&s[..lon_at], 2)?;
    let lon = parse_dms(&s[lon_at..], 3)?;
    Some((lat, lon))
}

/// One signed ISO 6709 coordinate with `deg_digits` degree digits.
fn parse_dms(s: &str, deg_digits: usize) -> Option<f64> {
    let (sign, digits) = match s.as_bytes().first()? {
        b'+' => (1.0, &s[1..]),
        b'-' => (-1.0, &s[1..]),
        _ => return None,
    };
    if digits.len() < deg_digits + 2 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let deg: f64 = digits[..deg_digits].parse().ok()?;
    let min: f64 = digits[deg_digits..deg_digits + 2].parse().ok()?;
    let sec: f64 = if digits.len() >= deg_digits + 4 {
        digits[deg_digits + 2..deg_digits + 4].parse().ok()?
    } else {
        0.0
    };
    Some(sign * (deg + min / 60.0 + sec / 3600.0))
}

/// The sun's elevation above the horizon (degrees) at a UTC instant (unix
/// seconds) and location — the standard low-precision solar position
/// (Astronomical Almanac / NOAA approximation, ~0.01° accuracy). Pure, so the
/// astronomy is unit-testable against known sun positions.
fn solar_elevation_deg(unix_secs: f64, lat_deg: f64, lon_deg: f64) -> f64 {
    // Days since J2000.0 (2000-01-01 12:00 UTC = unix 946728000).
    let d = unix_secs / 86400.0 - 10957.5;
    // Mean anomaly and mean longitude of the sun.
    let g = (357.529 + 0.985_600_28 * d).to_radians();
    let q = 280.459 + 0.985_647_36 * d;
    // Ecliptic longitude (equation of centre), obliquity of the ecliptic.
    let l = (q + 1.915 * g.sin() + 0.020 * (2.0 * g).sin()).to_radians();
    let e = (23.439 - 0.000_000_36 * d).to_radians();
    // Equatorial coordinates.
    let ra = (e.cos() * l.sin()).atan2(l.cos());
    let decl = (e.sin() * l.sin()).asin();
    // Hour angle from local sidereal time.
    let gmst_hours = 18.697_374_558 + 24.065_709_824_419_08 * d;
    let lst = (gmst_hours * 15.0 + lon_deg).to_radians();
    let ha = lst - ra;
    let lat = lat_deg.to_radians();
    (lat.sin() * decl.sin() + lat.cos() * decl.cos() * ha.cos())
        .asin()
        .to_degrees()
}

/// Whether a `hyprsunset` process exists — a `/proc/*/comm` sweep, no
/// subprocess spawned.
fn hyprsunset_running() -> bool {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return false;
    };
    for entry in dir.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().filter(|n| n.bytes().all(|b| b.is_ascii_digit())) else {
            continue;
        };
        if let Ok(comm) = std::fs::read_to_string(format!("/proc/{pid}/comm")) {
            if comm.trim_end() == "hyprsunset" {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAB: &str = "# comment line\n\
                       AD\t+4230+00131\tEurope/Andorra\n\
                       BO\t-1630-06809\tAmerica/La_Paz\n\
                       AQ\t-6448-06406\tAntarctica/Palmer\tPalmer\n";

    #[test]
    fn finds_and_parses_zone_coordinates() {
        let (lat, lon) = coords_for_zone(TAB, "America/La_Paz").unwrap();
        assert!((lat - -16.5).abs() < 0.01, "lat {lat}");
        assert!((lon - -68.15).abs() < 0.01, "lon {lon}");
        let (lat, lon) = coords_for_zone(TAB, "Europe/Andorra").unwrap();
        assert!(lat > 42.0 && lon > 0.0);
        assert!(coords_for_zone(TAB, "Mars/Olympus").is_none());
    }

    #[test]
    fn parses_seconds_precision_coordinates() {
        // zone1970.tab also carries ±DDMMSS±DDDMMSS rows.
        let (lat, lon) = parse_iso6709("+481407-0113000").unwrap();
        assert!((lat - (48.0 + 14.0 / 60.0 + 7.0 / 3600.0)).abs() < 1e-6);
        assert!((lon - -(11.0 + 30.0 / 60.0)).abs() < 1e-6);
    }

    // 2026-09-07 00:00 UTC.
    const SEP7: f64 = 1_788_739_200.0;
    const LA_PAZ: (f64, f64) = (-16.5, -68.15);

    #[test]
    fn sun_is_up_at_la_paz_midday_and_down_at_night() {
        // 16:00 UTC = noon-ish local (UTC-4): high sun.
        let noon = solar_elevation_deg(SEP7 + 16.0 * 3600.0, LA_PAZ.0, LA_PAZ.1);
        assert!(noon > 40.0, "midday elevation {noon}");
        // 04:00 UTC = local midnight: deep below the horizon.
        let midnight = solar_elevation_deg(SEP7 + 4.0 * 3600.0, LA_PAZ.0, LA_PAZ.1);
        assert!(midnight < -40.0, "midnight elevation {midnight}");
    }

    #[test]
    fn sunset_boundary_lands_in_the_early_evening() {
        // Early September in La Paz the sun sets ≈18:15 local (22:15 UTC):
        // still up at 17:30 local, clearly set by 19:00 local.
        let before = solar_elevation_deg(SEP7 + 21.5 * 3600.0, LA_PAZ.0, LA_PAZ.1);
        assert!(before > SUNSET_ELEVATION_DEG, "17:30 local: {before}");
        let after = solar_elevation_deg(SEP7 + 23.0 * 3600.0, LA_PAZ.0, LA_PAZ.1);
        assert!(after < SUNSET_ELEVATION_DEG, "19:00 local: {after}");
    }
}
