//! Hardware access that the window, the fan helper and the fan daemon share: sensors,
//! their thresholds, fans, fan control, fan curves and the saved settings.
//!
//! It has no Tauri and no NVML code, so a process that runs as root needs neither.

use std::time::Duration;

pub mod control;
pub mod curve;
pub mod daemon;
pub mod fans;
pub mod helper;
pub mod sensors;
pub mod settings;
#[cfg(test)]
mod testsys;
pub mod thresholds;

/// Time between two samples of the window, and between two ticks of the fan helper.
pub const INTERVAL: Duration = Duration::from_secs(1);
/// Look for sensors and fans that appeared after start-up (a woken dGPU, a hot-plugged drive).
pub const RESCAN_EVERY: u64 = 30;
