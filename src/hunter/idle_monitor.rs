/// System idle detection for opportunistic scan scheduling.
///
/// Instead of scanning on a fixed timer, the hunter node can wait for
/// the system to become idle before starting a scan.  This avoids
/// competing with the user's workload and keeps the node invisible
/// during active use.
///
/// # How it works
///
/// On Linux, CPU idle percentage is read from /proc/stat by comparing
/// two snapshots of the cumulative CPU counters.  On macOS, the same
/// information comes from host_processor_info().  We avoid pulling in
/// a large dependency like `sysinfo` by reading these values directly
/// -- the parsing is trivial and platform-specific code is isolated
/// behind a clean interface.
///
/// The scheduler checks CPU usage on a configurable polling interval
/// (default 30s).  When the system has been below the idle threshold
/// for a sustained period (default: 3 consecutive checks), it signals
/// readiness via a `tokio::sync::Notify`.  The scan loop awaits this
/// signal instead of a fixed timer.
///
/// # Fallback
///
/// On unsupported platforms, or if /proc/stat is unreadable, the
/// scheduler falls back to the fixed-interval timer so scans still
/// happen regardless.

use std::time::Duration;
use std::sync::Arc;

use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Configuration for idle-aware scheduling.
#[derive(Clone, Debug)]
pub struct IdleConfig {
    /// CPU usage percentage below which the system is considered idle.
    /// Range 0.0 to 100.0.  Default: 15.0 (system is 85%+ idle).
    pub idle_threshold: f64,
    /// How often to sample CPU usage.  Default: 30 seconds.
    pub sample_interval: Duration,
    /// How many consecutive samples must be below the threshold before
    /// the system is considered "sustainably idle."  Default: 3.
    /// This prevents triggering on brief pauses between user actions.
    pub required_consecutive: u32,
    /// Maximum time to wait for idle before forcing a scan anyway.
    /// Prevents indefinite deferral on busy systems.  Default: 4 hours.
    pub max_wait: Duration,
    /// If true, use idle detection.  If false, fall back to fixed
    /// interval scheduling.
    pub enabled: bool,
}

impl Default for IdleConfig {
    fn default() -> Self {
        Self {
            idle_threshold: 15.0,
            sample_interval: Duration::from_secs(30),
            required_consecutive: 3,
            max_wait: Duration::from_secs(4 * 3600),
            enabled: true,
        }
    }
}

/// Snapshot of CPU time counters used to compute usage between two points.
#[derive(Clone, Debug)]
struct CpuSnapshot {
    /// Total CPU time across all states (user, nice, system, idle, etc.)
    total: u64,
    /// CPU time spent idle.
    idle: u64,
}

/// Read a CPU snapshot from /proc/stat (Linux).
///
/// The first line of /proc/stat looks like:
///   cpu  user nice system idle iowait irq softirq steal guest guest_nice
///
/// We sum all fields for total, and use the idle + iowait fields for idle.
#[cfg(target_os = "linux")]
fn read_cpu_snapshot() -> Option<CpuSnapshot> {
    use std::fs;
    let content = fs::read_to_string("/proc/stat").ok()?;
    let first_line = content.lines().next()?;
    if !first_line.starts_with("cpu ") {
        return None;
    }
    let fields: Vec<u64> = first_line
        .split_whitespace()
        .skip(1) // skip "cpu"
        .filter_map(|s| s.parse().ok())
        .collect();
    if fields.len() < 4 {
        return None;
    }
    let total: u64 = fields.iter().sum();
    // idle is field[3], iowait is field[4] (if present)
    let idle = fields[3] + fields.get(4).unwrap_or(&0);
    Some(CpuSnapshot { total, idle })
}

/// Read a CPU snapshot on macOS using host_processor_info.
#[cfg(target_os = "macos")]
fn read_cpu_snapshot() -> Option<CpuSnapshot> {
    use std::mem;
    use std::ptr;

    // These are from mach/host_info.h and mach/processor_info.h
    const HOST_CPU_LOAD_INFO: i32 = 3;
    const CPU_STATE_USER: usize = 0;
    const CPU_STATE_SYSTEM: usize = 1;
    const CPU_STATE_IDLE: usize = 2;
    const CPU_STATE_NICE: usize = 3;

    #[repr(C)]
    struct HostCpuLoadInfo {
        ticks: [u32; 4], // user, system, idle, nice
    }

    extern "C" {
        fn mach_host_self() -> u32;
        fn host_statistics(
            host: u32,
            flavor: i32,
            info: *mut HostCpuLoadInfo,
            count: *mut u32,
        ) -> i32;
    }

    unsafe {
        let mut info: HostCpuLoadInfo = mem::zeroed();
        let mut count = (mem::size_of::<HostCpuLoadInfo>() / mem::size_of::<u32>()) as u32;
        let result = host_statistics(
            mach_host_self(),
            HOST_CPU_LOAD_INFO,
            &mut info as *mut _,
            &mut count,
        );
        if result != 0 {
            return None;
        }
        let total = info.ticks.iter().map(|t| *t as u64).sum();
        let idle = info.ticks[CPU_STATE_IDLE] as u64;
        Some(CpuSnapshot { total, idle })
    }
}

/// Fallback for unsupported platforms -- always returns None so the
/// scheduler falls back to fixed-interval mode.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn read_cpu_snapshot() -> Option<CpuSnapshot> {
    None
}

/// Compute the CPU usage percentage between two snapshots.
/// Returns a value between 0.0 and 100.0.
fn cpu_usage_between(before: &CpuSnapshot, after: &CpuSnapshot) -> f64 {
    let total_delta = after.total.saturating_sub(before.total);
    if total_delta == 0 {
        return 0.0;
    }
    let idle_delta = after.idle.saturating_sub(before.idle);
    let busy_delta = total_delta - idle_delta;
    (busy_delta as f64 / total_delta as f64) * 100.0
}

/// Wait until the system is idle, then signal via the returned Notify.
///
/// This is the main entry point for idle-aware scheduling.  It runs in
/// a loop, sampling CPU usage at the configured interval.  When the
/// required number of consecutive idle samples is reached, it notifies
/// once and resets the counter.
///
/// If idle detection is disabled or unsupported, this falls back to
/// waiting for `fallback_interval` and then notifying.
pub async fn wait_for_idle(
    config: &IdleConfig,
    fallback_interval: Duration,
    token: &CancellationToken,
) -> bool {
    // If idle detection is disabled, just sleep the fallback interval
    if !config.enabled {
        tokio::select! {
            _ = token.cancelled() => return false,
            _ = tokio::time::sleep(fallback_interval) => return true,
        }
    }

    // Try to get an initial snapshot.  If that fails, fall back.
    let mut prev_snapshot = match read_cpu_snapshot() {
        Some(s) => s,
        None => {
            warn!("CPU idle detection unavailable on this platform, using fixed interval");
            tokio::select! {
                _ = token.cancelled() => return false,
                _ = tokio::time::sleep(fallback_interval) => return true,
            }
        }
    };

    let mut consecutive_idle: u32 = 0;
    let deadline = tokio::time::Instant::now() + config.max_wait;

    loop {
        tokio::select! {
            _ = token.cancelled() => return false,
            _ = tokio::time::sleep(config.sample_interval) => {
                // Check if we have exceeded the max wait time
                if tokio::time::Instant::now() >= deadline {
                    info!(
                        "max idle wait ({:?}) exceeded, forcing scan",
                        config.max_wait
                    );
                    return true;
                }

                match read_cpu_snapshot() {
                    Some(current) => {
                        let usage = cpu_usage_between(&prev_snapshot, &current);
                        prev_snapshot = current;

                        if usage < config.idle_threshold {
                            consecutive_idle += 1;
                            debug!(
                                "system idle: {:.1}% usage ({}/{} consecutive)",
                                usage, consecutive_idle, config.required_consecutive
                            );
                            if consecutive_idle >= config.required_consecutive {
                                info!(
                                    "system idle for {} consecutive samples, scan approved",
                                    consecutive_idle
                                );
                                return true;
                            }
                        } else {
                            if consecutive_idle > 0 {
                                debug!(
                                    "idle streak broken at {:.1}% usage (was {}/{})",
                                    usage, consecutive_idle, config.required_consecutive
                                );
                            }
                            consecutive_idle = 0;
                        }
                    }
                    None => {
                        warn!("failed to read CPU snapshot, skipping sample");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_usage_calculation() {
        let before = CpuSnapshot { total: 1000, idle: 800 };
        let after = CpuSnapshot { total: 2000, idle: 1600 };
        // 200 busy out of 1000 total delta = 20% usage
        let usage = cpu_usage_between(&before, &after);
        assert!((usage - 20.0).abs() < 0.01);
    }

    #[test]
    fn cpu_usage_fully_idle() {
        let before = CpuSnapshot { total: 1000, idle: 800 };
        let after = CpuSnapshot { total: 2000, idle: 1800 };
        // 0 busy out of 1000 delta = 0% usage
        let usage = cpu_usage_between(&before, &after);
        assert!((usage - 0.0).abs() < 0.01);
    }

    #[test]
    fn cpu_usage_fully_busy() {
        let before = CpuSnapshot { total: 1000, idle: 800 };
        let after = CpuSnapshot { total: 2000, idle: 800 };
        // 1000 busy out of 1000 delta = 100% usage
        let usage = cpu_usage_between(&before, &after);
        assert!((usage - 100.0).abs() < 0.01);
    }

    #[test]
    fn cpu_usage_zero_delta() {
        let snap = CpuSnapshot { total: 1000, idle: 800 };
        let usage = cpu_usage_between(&snap, &snap);
        assert_eq!(usage, 0.0);
    }

    #[test]
    fn default_config_values() {
        let config = IdleConfig::default();
        assert_eq!(config.idle_threshold, 15.0);
        assert_eq!(config.required_consecutive, 3);
        assert!(config.enabled);
    }

    #[tokio::test]
    async fn fallback_when_disabled() {
        let config = IdleConfig {
            enabled: false,
            ..Default::default()
        };
        let token = CancellationToken::new();
        // With a very short fallback, this should return quickly
        let result = wait_for_idle(
            &config,
            Duration::from_millis(10),
            &token,
        ).await;
        assert!(result);
    }

    #[tokio::test]
    async fn cancellation_returns_false() {
        let config = IdleConfig {
            enabled: false,
            ..Default::default()
        };
        let token = CancellationToken::new();
        token.cancel();
        let result = wait_for_idle(
            &config,
            Duration::from_secs(3600),
            &token,
        ).await;
        assert!(!result);
    }
}
