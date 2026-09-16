//! Optional schema-1 profiling handshake; never substitutes callbacks for presentation.
use anyhow::{Context as _, Result, ensure};
use std::{
    path::Path,
    time::{Duration, Instant},
};

const GATE_TIMEOUT: Duration = Duration::from_secs(90);

fn gate_ready(path: &Path, elapsed: Duration) -> Result<bool> {
    if path
        .try_exists()
        .with_context(|| format!("read start gate {}", path.display()))?
    {
        return Ok(true);
    }
    ensure!(
        elapsed < GATE_TIMEOUT,
        "start gate {} did not arrive within 90 seconds",
        path.display()
    );
    Ok(false)
}

pub async fn wait_for_gate(path: &Path) -> Result<()> {
    let waiting = Instant::now();
    while !gate_ready(path, waiting.elapsed())? {
        gpui::Timer::after(Duration::from_millis(25)).await;
    }
    Ok(())
}

#[cfg(any(target_os = "macos", test))]
fn ticks_to_ns(ticks: u64, numer: u32, denom: u32) -> Result<u64> {
    ensure!(denom != 0, "invalid mach timebase denominator");
    u64::try_from(u128::from(ticks) * u128::from(numer) / u128::from(denom))
        .context("mach host time exceeds UInt64 nanoseconds")
}

#[cfg(target_os = "macos")]
pub const HOST_CLOCK: &str = "mach_absolute_time";

#[cfg(target_os = "macos")]
pub fn host_time_ns() -> Result<u64> {
    #[repr(C)]
    struct Timebase {
        numer: u32,
        denom: u32,
    }
    unsafe extern "C" {
        fn mach_absolute_time() -> u64;
        fn mach_timebase_info(info: *mut Timebase) -> i32;
    }
    let mut info = Timebase { numer: 0, denom: 0 };
    // libSystem functions, valid output pointer; no wall-clock conversion.
    ensure!(
        unsafe { mach_timebase_info(&mut info) } == 0,
        "mach_timebase_info failed"
    );
    ticks_to_ns(unsafe { mach_absolute_time() }, info.numer, info.denom)
}

#[cfg(not(target_os = "macos"))]
pub const HOST_CLOCK: &str = "process-relative-std-Instant-not-Mach-host-time";

#[cfg(not(target_os = "macos"))]
pub fn host_time_ns() -> Result<u64> {
    static ORIGIN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    Ok(ORIGIN
        .get_or_init(Instant::now)
        .elapsed()
        .as_nanos()
        .try_into()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_gate_waits_until_exact_deadline_and_existing_gate_opens() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("start");
        assert!(!gate_ready(&path, Duration::ZERO).unwrap());
        assert!(!gate_ready(&path, Duration::from_millis(89_999)).unwrap());
        assert!(
            gate_ready(&path, Duration::from_secs(90))
                .unwrap_err()
                .to_string()
                .contains("90 seconds")
        );
        std::fs::write(&path, b"").unwrap();
        assert!(gate_ready(&path, Duration::ZERO).unwrap());
    }

    #[test]
    fn mach_conversion_preserves_integer_precision_without_intermediate_overflow() {
        assert_eq!(ticks_to_ns(5, 125, 3).unwrap(), 208);
        assert_eq!(ticks_to_ns(u64::MAX, 3, 3).unwrap(), u64::MAX);
        assert!(ticks_to_ns(1, 1, 0).is_err());
        assert!(ticks_to_ns(u64::MAX, 2, 1).is_err());
    }
}
