//! Intel GPU utilisation, read from the **i915 PMU** via `perf_event_open`.
//!
//! Why this way, since it is the least obvious of the three:
//!
//! - **i915 exposes no busy-percent in sysfs.** Only clock frequencies
//!   (`gt_act_freq_mhz`), and a clock is not a load: an idle GPU parked at
//!   0 MHz and a busy one at 300 MHz say nothing about how much work either is
//!   doing. The kernel *does* keep per-engine busy-time counters — it just only
//!   hands them out through perf.
//! - **The discrete card is the wrong thing to ask on a hybrid laptop.**
//!   Querying NVIDIA *wakes it out of runtime-suspend* (measured 2026-09-13:
//!   `nvidia-smi` took 1.84 s and left the GPU `active`), so polling it would
//!   keep a dGPU awake — real battery cost — for a figure that reads 0 whenever
//!   nothing is being offloaded. The chip that actually draws the desktop is the
//!   integrated one.
//!
//! **Needs `kernel.perf_event_paranoid = 0`** (set declaratively in
//! `/etc/nixos/configuration.nix`). Not 1 — the kernel disallows CPU
//! (system-wide) event access at anything above 0, and a device PMU has no
//! per-process form, so `pid = -1` is the only way to ask it. Without it
//! `perf_event_open` returns EACCES,
//! this reports `None`, and the readout simply has no GPU column — which is the
//! honest answer, not a zero.
//!
//! Everything here degrades to `None`: no i915 (a desktop with an AMD card, a
//! VM), no permission, a counter that stops reading. The cost when it works is
//! one `read(2)` of 8 bytes per engine per poll.

use std::fs;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Where the kernel advertises PMUs. The Intel GPU's is `i915` (the classic
/// driver) or `xe` (its successor); on a multi-GPU box the name carries the PCI
/// address, hence the prefix match rather than a fixed path.
const PMU_ROOT: &str = "/sys/bus/event_source/devices";

/// How many poll ticks to wait before trying to open the PMU again after a
/// failure. At the system collector's 3 s cadence this is a minute — enough
/// that a machine without i915 costs nothing, while a machine that just had the
/// sysctl changed starts reporting without needing the daemon restarted.
const RETRY_TICKS: u32 = 20;

/// At most this many engine counters. A GPU has a handful (render, blitter,
/// video decode, video enhance); the cap is a guard against a future kernel
/// exposing dozens, not a real limit.
const MAX_ENGINES: usize = 8;

/// One open engine-busy counter and the last value read from it.
struct Counter {
    fd: OwnedFd,
    prev: u64,
    at: Instant,
}

/// The GPU meter: open counters, plus the retry countdown when there are none.
pub(crate) struct GpuMeter {
    counters: Vec<Counter>,
    retry_in: u32,
}

impl GpuMeter {
    /// A meter that will try to open the PMU on its first sample.
    pub(crate) fn new() -> Self {
        Self {
            counters: Vec::new(),
            retry_in: 0,
        }
    }

    /// Busy percentage since the previous sample, or `None` while the PMU is
    /// unavailable (and on the very first sample after opening, which only
    /// establishes the baseline).
    ///
    /// **The busiest engine, not their sum**: a GPU's render, blitter and video
    /// engines run in parallel, so adding them can pass 100% while none of them
    /// is saturated. "How hard is the GPU working" is the worst-loaded part of
    /// it — the same thing a CPU percentage means about its busiest moment.
    pub(crate) fn sample(&mut self) -> Option<f32> {
        if self.counters.is_empty() {
            self.retry_in = self.retry_in.saturating_sub(1);
            if self.retry_in == 0 {
                self.counters = open_engine_counters();
                if self.counters.is_empty() {
                    self.retry_in = RETRY_TICKS;
                }
            }
            // Just opened: the counters have no baseline to measure against yet.
            return None;
        }
        let now = Instant::now();
        let mut busiest: Option<f32> = None;
        let mut alive = Vec::with_capacity(self.counters.len());
        for mut c in std::mem::take(&mut self.counters) {
            let Some(value) = read_counter(&c.fd) else {
                // A counter that stops reading is dropped; if they all go, the
                // retry path re-opens the lot rather than limping on one.
                continue;
            };
            let elapsed = now.duration_since(c.at).as_nanos();
            // The counter is monotonic nanoseconds-busy, so the delta over the
            // wall time that passed IS the duty cycle. `saturating_sub` because
            // a re-opened counter restarts from zero.
            if elapsed > 0 {
                let busy = value.saturating_sub(c.prev) as f64;
                let pct = (busy / elapsed as f64 * 100.0).clamp(0.0, 100.0) as f32;
                busiest = Some(busiest.map_or(pct, |b: f32| b.max(pct)));
            }
            c.prev = value;
            c.at = now;
            alive.push(c);
        }
        self.counters = alive;
        if self.counters.is_empty() {
            self.retry_in = RETRY_TICKS;
        }
        busiest
    }
}

/// Open one counter per engine-busy event the GPU's PMU advertises.
/// Empty on any failure — no PMU, no permission, nothing to count.
fn open_engine_counters() -> Vec<Counter> {
    let Some(pmu) = find_gpu_pmu() else {
        return Vec::new();
    };
    let Some(ty) = read_u32(&pmu.join("type")) else {
        return Vec::new();
    };
    // Device PMUs are counted on one CPU, named in `cpumask`; asking for any
    // other CPU fails. Default to 0, which is what i915 reports.
    let cpu = fs::read_to_string(pmu.join("cpumask"))
        .ok()
        .and_then(|s| first_cpu(&s))
        .unwrap_or(0);
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(pmu.join("events")) else {
        return out;
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        // `<engine>-busy` is the counter; `.unit` sidecars and the sema/wait
        // counters are not what "busy" means.
        .filter(|n| n.ends_with("-busy"))
        .collect();
    // Deterministic order, so which engine wins a tie never depends on readdir.
    names.sort();
    for name in names.into_iter().take(MAX_ENGINES) {
        let Some(config) = fs::read_to_string(pmu.join("events").join(&name))
            .ok()
            .and_then(|s| parse_config(&s))
        else {
            continue;
        };
        if let Some(fd) = perf_open(ty, config, cpu) {
            out.push(Counter {
                fd,
                prev: 0,
                at: Instant::now(),
            });
        }
    }
    out
}

/// The Intel GPU's PMU directory, whichever driver owns the chip.
fn find_gpu_pmu() -> Option<PathBuf> {
    let dir = fs::read_dir(PMU_ROOT).ok()?;
    let mut found: Option<PathBuf> = None;
    for e in dir.filter_map(|e| e.ok()) {
        let name = e.file_name().into_string().unwrap_or_default();
        if name == "i915" || name == "xe" || name.starts_with("i915_") || name.starts_with("xe_") {
            // Prefer the first in sorted order so a two-GPU box is stable.
            if found.as_ref().is_none_or(|f| e.path() < *f) {
                found = Some(e.path());
            }
        }
    }
    found
}

/// `config=0x2000` → `0x2000`. The kernel writes one `key=value` per event
/// file, sometimes with trailing fields after a comma.
fn parse_config(s: &str) -> Option<u64> {
    let field = s.trim().split(',').find(|f| f.starts_with("config="))?;
    let v = field.trim_start_matches("config=").trim();
    match v.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => v.parse().ok(),
    }
}

/// First CPU in a `cpumask` list (`0` or `0-3` or `0,8`).
fn first_cpu(s: &str) -> Option<u32> {
    s.trim()
        .split([',', '-'])
        .next()
        .and_then(|v| v.trim().parse().ok())
}

fn read_u32(path: &Path) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// The kernel's `perf_event_attr`, version 7 (120 bytes) — the fields we set
/// plus the tail the kernel expects to be there and zero.
#[repr(C)]
#[derive(Default)]
struct PerfEventAttr {
    type_: u32,
    size: u32,
    config: u64,
    sample_period: u64,
    sample_type: u64,
    read_format: u64,
    /// Bitfield (`disabled`, `inherit`, `pinned`, …). All zero: count from the
    /// moment the fd exists, no inheritance, no sampling.
    flags: u64,
    wakeup_events: u32,
    bp_type: u32,
    config1: u64,
    config2: u64,
    branch_sample_type: u64,
    sample_regs_user: u64,
    sample_stack_user: u32,
    clockid: i32,
    sample_regs_intr: u64,
    aux_watermark: u32,
    sample_max_stack: u16,
    __reserved_2: u16,
    aux_sample_size: u32,
    __reserved_3: u32,
}

/// `PERF_FLAG_FD_CLOEXEC` — never leak a counter into a child process.
const PERF_FLAG_FD_CLOEXEC: u64 = 1 << 3;

/// Open one counter on a device PMU: system-wide (`pid = -1`) on the CPU the
/// PMU is counted on, its own group, closed on exec.
fn perf_open(ty: u32, config: u64, cpu: u32) -> Option<OwnedFd> {
    let attr = PerfEventAttr {
        type_: ty,
        size: std::mem::size_of::<PerfEventAttr>() as u32,
        config,
        ..Default::default()
    };
    // SAFETY: `attr` is a correctly-sized, fully-initialised perf_event_attr
    // living until the call returns; the kernel only reads it. The other
    // arguments are plain scalars. A negative return is the error path and
    // creates no fd.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_perf_event_open,
            &attr as *const PerfEventAttr,
            -1i32,
            cpu as i32,
            -1i32,
            PERF_FLAG_FD_CLOEXEC,
        )
    };
    if fd < 0 {
        return None;
    }
    // SAFETY: the syscall returned a fresh fd that nothing else owns.
    Some(unsafe { OwnedFd::from_raw_fd(fd as RawFd) })
}

/// Read a counter's current value (8 bytes, host order).
fn read_counter(fd: &OwnedFd) -> Option<u64> {
    use std::os::fd::AsRawFd;
    let mut buf = [0u8; 8];
    // SAFETY: reading at most 8 bytes into an 8-byte stack buffer from a fd we
    // own; perf counters return exactly one u64 with the default read_format.
    let n = unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
    (n == buf.len() as isize).then(|| u64::from_ne_bytes(buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configs_parse_in_both_spellings() {
        assert_eq!(parse_config("config=0x2000\n"), Some(0x2000));
        assert_eq!(parse_config("config=0\n"), Some(0));
        assert_eq!(parse_config("config=16\n"), Some(16));
        // A sidecar unit file, or anything else, is not a config.
        assert_eq!(parse_config("ns\n"), None);
        assert_eq!(parse_config(""), None);
    }

    #[test]
    fn the_cpumask_gives_the_first_cpu() {
        assert_eq!(first_cpu("0\n"), Some(0));
        assert_eq!(first_cpu("0-3\n"), Some(0));
        assert_eq!(first_cpu("2,8\n"), Some(2));
        assert_eq!(first_cpu("\n"), None);
    }

    /// On a machine with no i915 (CI, a VM, an AMD desktop) the meter must be
    /// silent rather than wrong — and must not spin trying to open it.
    #[test]
    fn no_pmu_reports_nothing_and_backs_off() {
        let mut m = GpuMeter::new();
        let first = m.sample();
        if m.counters.is_empty() {
            assert_eq!(first, None);
            assert_eq!(m.retry_in, RETRY_TICKS, "should back off, not retry hotly");
        }
    }

    /// The size field must name a perf_event_attr version the kernel knows;
    /// getting it wrong is an E2BIG/EINVAL that would look like "no GPU".
    #[test]
    fn the_attr_is_a_version_the_kernel_accepts() {
        assert_eq!(std::mem::size_of::<PerfEventAttr>(), 120);
    }
}



#[cfg(test)]
mod probe {
    use super::*;
    #[test]
    fn open_and_read() {
        let cs = open_engine_counters();
        eprintln!("opened {} counters (paranoid={})", cs.len(),
            std::fs::read_to_string("/proc/sys/kernel/perf_event_paranoid").unwrap_or_default().trim());
        for c in &cs {
            eprintln!("  counter reads {:?}", read_counter(&c.fd));
        }
        let mut m = GpuMeter::new();
        eprintln!("sample 1 = {:?}", m.sample());
        std::thread::sleep(std::time::Duration::from_millis(300));
        eprintln!("sample 2 = {:?}", m.sample());
        std::thread::sleep(std::time::Duration::from_millis(300));
        eprintln!("sample 3 = {:?}", m.sample());
    }
}
