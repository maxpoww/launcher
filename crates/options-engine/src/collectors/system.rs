//! Layer 4 (part) — system hardware sampling: CPU load, RAM usage, battery.
//!
//! A pure timer-driven sensor reading `/proc` and `/sys` — no external
//! services, no privileges. CPU utilisation needs two samples (the delta of
//! busy vs total jiffies between polls), so the first tick only seeds the
//! baseline. Everything parses from text the kernel exposes; the parsers are
//! pure and unit-tested.

use std::time::Duration;

use tokio::sync::{mpsc, watch};

use crate::collector::{Collector, CollectorFuture};
use crate::message::{ContextDelta, Update};
use crate::state::{ContextState, Layer, SystemMetrics};

/// How often to sample. Cheap reads, but slow-changing data — a few seconds is
/// plenty and keeps this well clear of the high-frequency layers.
const POLL: Duration = Duration::from_secs(3);

#[derive(Default)]
pub struct SystemCollector;

impl SystemCollector {
    pub fn new() -> Self {
        Self
    }
}

impl Collector for SystemCollector {
    fn name(&self) -> &'static str {
        "system"
    }
    fn layer(&self) -> Layer {
        Layer::Hardware
    }
    fn run(
        self: Box<Self>,
        _ctx: watch::Receiver<ContextState>,
        tx: mpsc::Sender<Update>,
    ) -> CollectorFuture {
        Box::pin(async move {
            // Previous (total, idle) jiffies for the CPU delta.
            let mut prev_cpu: Option<(u64, u64)> = None;
            loop {
                let cpu_now = read_cpu_times();
                let cpu_usage_pct = match (prev_cpu, cpu_now) {
                    (Some(prev), Some(now)) => cpu_delta_pct(prev, now),
                    _ => 0.0,
                };
                if let Some(now) = cpu_now {
                    prev_cpu = Some(now);
                }
                let ram_usage_pct = read_ram_used_pct().unwrap_or(0.0);
                let (battery_pct, is_charging) = match read_battery() {
                    Some((pct, charging)) => (Some(pct), charging),
                    None => (None, false),
                };
                let metrics = SystemMetrics {
                    cpu_usage_pct,
                    ram_usage_pct,
                    battery_pct,
                    is_charging,
                };
                if tx
                    .send(Update::Delta(Layer::Hardware, ContextDelta::Metrics(metrics)))
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

/// Read the aggregate `cpu` line from `/proc/stat` as `(total, idle)` jiffies.
/// `/proc` reads are memory-backed (no real IO wait), so a blocking read here
/// is negligible.
fn read_cpu_times() -> Option<(u64, u64)> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    parse_cpu_times(stat.lines().next()?)
}

/// Parse a `/proc/stat` aggregate cpu line into `(total, idle)` jiffies. `idle`
/// folds in iowait; `total` is the sum of all reported counters.
fn parse_cpu_times(line: &str) -> Option<(u64, u64)> {
    let mut it = line.split_whitespace();
    if it.next()? != "cpu" {
        return None;
    }
    let vals: Vec<u64> = it.filter_map(|s| s.parse().ok()).collect();
    if vals.len() < 5 {
        return None;
    }
    let idle = vals[3] + vals[4]; // idle + iowait
    let total: u64 = vals.iter().sum();
    Some((total, idle))
}

/// CPU utilisation percent from two `(total, idle)` samples.
fn cpu_delta_pct(prev: (u64, u64), now: (u64, u64)) -> f32 {
    let total_d = now.0.saturating_sub(prev.0);
    let idle_d = now.1.saturating_sub(prev.1);
    if total_d == 0 {
        return 0.0;
    }
    let busy = total_d.saturating_sub(idle_d) as f32;
    (busy / total_d as f32 * 100.0).clamp(0.0, 100.0)
}

fn read_ram_used_pct() -> Option<f32> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    ram_used_pct(&meminfo)
}

/// Used-memory percent from `/proc/meminfo` (`MemTotal` vs `MemAvailable`).
fn ram_used_pct(meminfo: &str) -> Option<f32> {
    let field = |key: &str| -> Option<u64> {
        meminfo
            .lines()
            .find_map(|l| l.strip_prefix(key))
            .and_then(|v| v.split_whitespace().next())
            .and_then(|n| n.parse::<u64>().ok())
    };
    let total = field("MemTotal:")?;
    let avail = field("MemAvailable:")?;
    if total == 0 {
        return None;
    }
    let used = total.saturating_sub(avail.min(total)) as f32;
    Some((used / total as f32 * 100.0).clamp(0.0, 100.0))
}

/// First real battery's `(capacity%, charging)` from `/sys/class/power_supply`,
/// or `None` if there's no battery (desktop).
fn read_battery() -> Option<(u8, bool)> {
    let dir = std::fs::read_dir("/sys/class/power_supply").ok()?;
    for entry in dir.flatten() {
        let p = entry.path();
        let Ok(kind) = std::fs::read_to_string(p.join("type")) else {
            continue;
        };
        if kind.trim() != "Battery" {
            continue;
        }
        let Ok(cap) = std::fs::read_to_string(p.join("capacity")) else {
            continue;
        };
        let Ok(pct) = cap.trim().parse::<u8>() else {
            continue;
        };
        let status = std::fs::read_to_string(p.join("status")).unwrap_or_default();
        return Some((pct.min(100), charging_from_status(status.trim())));
    }
    None
}

/// A battery `status` string counts as charging only when actively "Charging"
/// ("Full" on AC is not charging; "Discharging"/"Unknown" are not).
fn charging_from_status(status: &str) -> bool {
    status.eq_ignore_ascii_case("Charging")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_stat_cpu_line() {
        // user nice system idle iowait irq softirq steal …
        let (total, idle) = parse_cpu_times("cpu  100 0 100 700 100 0 0 0").unwrap();
        assert_eq!(idle, 800); // idle 700 + iowait 100
        assert_eq!(total, 1000);
    }

    #[test]
    fn rejects_non_cpu_line() {
        assert!(parse_cpu_times("cpu0 1 2 3 4 5").is_none());
        assert!(parse_cpu_times("intr 1 2 3").is_none());
    }

    #[test]
    fn cpu_delta_is_busy_fraction() {
        // 1000 total jiffies elapsed, 800 of them idle ⇒ 20% busy.
        let pct = cpu_delta_pct((1000, 800), (2000, 1600));
        assert!((pct - 20.0).abs() < 1e-3, "got {pct}");
    }

    #[test]
    fn cpu_delta_handles_zero_and_wraparound() {
        assert_eq!(cpu_delta_pct((1000, 800), (1000, 800)), 0.0);
        // idle grew more than total (shouldn't happen) ⇒ clamped, not negative.
        assert_eq!(cpu_delta_pct((1000, 800), (1010, 900)), 0.0);
    }

    #[test]
    fn ram_used_percent_from_meminfo() {
        let mem = "MemTotal:       16000 kB\nMemFree: 1000 kB\nMemAvailable:    4000 kB\n";
        let pct = ram_used_pct(mem).unwrap();
        assert!((pct - 75.0).abs() < 1e-3, "got {pct}"); // (16000-4000)/16000
    }

    #[test]
    fn ram_needs_both_fields() {
        assert!(ram_used_pct("MemTotal: 16000 kB\n").is_none());
    }

    #[test]
    fn charging_only_when_charging() {
        assert!(charging_from_status("Charging"));
        assert!(charging_from_status("charging"));
        assert!(!charging_from_status("Full"));
        assert!(!charging_from_status("Discharging"));
        assert!(!charging_from_status("Unknown"));
    }
}
