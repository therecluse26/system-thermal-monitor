//! Samples every sensor on a fixed interval and keeps a rolling history.

use std::collections::VecDeque;
use std::time::Duration;

use serde::Serialize;

use crate::fans::{self, Fan, FanInput, FanReading};
use crate::nvidia::Nvidia;
use crate::sensors::{self, Input, Sensor, SysPaths};

pub const INTERVAL: Duration = Duration::from_secs(1);
/// 30 minutes at one sample per second: the longest window the UI offers.
pub const HISTORY_LEN: usize = 1800;
/// Look for sensors and fans that appeared after start-up (a woken dGPU, a hot-plugged drive).
pub const RESCAN_EVERY: u64 = 30;

/// One reading of every sensor and fan. `values` and `fans` line up with the
/// sensor and fan lists of the same `generation`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    pub generation: u64,
    /// Unix time in milliseconds.
    pub t: u64,
    pub values: Vec<Option<f64>>,
    pub fans: Vec<Option<FanReading>>,
}

/// The sensor and fan lists, all temperature history, and the latest fan readings.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub generation: u64,
    pub interval_ms: u64,
    pub history_len: usize,
    pub sensors: Vec<Sensor>,
    pub times: Vec<u64>,
    /// One series per sensor, each aligned with `times`.
    pub series: Vec<Vec<Option<f64>>>,
    pub fans: Vec<Fan>,
    /// Latest reading of each fan, aligned with `fans`.
    pub fan_readings: Vec<Option<FanReading>>,
}

pub struct Monitor {
    paths: SysPaths,
    nvidia: Option<Nvidia>,
    sensors: Vec<Sensor>,
    fans: Vec<Fan>,
    /// Bumps whenever `sensors` or `fans` changes, so the UI knows to fetch a new snapshot.
    generation: u64,
    ticks: u64,
    times: VecDeque<u64>,
    series: Vec<VecDeque<Option<f64>>>,
    fan_readings: Vec<Option<FanReading>>,
}

impl Monitor {
    pub fn new(paths: SysPaths) -> Self {
        let nvidia = Nvidia::init();
        let (sensors, fans) = discover_all(&paths, nvidia.as_ref());
        let series = sensors.iter().map(|_| VecDeque::new()).collect();
        let mut monitor = Self {
            paths,
            nvidia,
            sensors,
            fans,
            generation: 1,
            ticks: 0,
            times: VecDeque::new(),
            series,
            fan_readings: Vec::new(),
        };
        monitor.fan_readings = monitor.fans.iter().map(|f| monitor.read_fan(f)).collect();
        monitor
    }

    fn read_sensor(&self, sensor: &Sensor) -> Option<f64> {
        match &sensor.input {
            Input::Hwmon(path) => sensors::read_hwmon(path),
            Input::Nvml(gpu) => self.nvidia.as_ref()?.temperature(*gpu),
        }
    }

    fn read_fan(&self, fan: &Fan) -> Option<FanReading> {
        match &fan.input {
            FanInput::Hwmon { .. } => fans::read_hwmon(&fan.input),
            FanInput::Nvml { gpu, fan } => self.nvidia.as_ref()?.fan(*gpu, *fan),
        }
    }

    /// Pairs of (id used by earlier versions, current id) for every sensor and fan.
    pub fn aliases(&self) -> Vec<(String, String)> {
        let sensors = self.sensors.iter().map(|s| (s.legacy_id.clone(), s.id.clone()));
        let fans = self.fans.iter().map(|f| (f.legacy_id.clone(), f.id.clone()));
        sensors.chain(fans).filter(|(old, new)| old != new).collect()
    }

    /// Adds sensors and fans that were not there before. Known ones keep their slot
    /// even when they stop reading, so their history and settings survive.
    fn rescan(&mut self) {
        let mut added = false;
        let (found_sensors, found_fans) = discover_all(&self.paths, self.nvidia.as_ref());
        for sensor in found_sensors {
            if self.sensors.iter().all(|s| s.id != sensor.id) {
                self.sensors.push(sensor);
                self.series.push(self.times.iter().map(|_| None).collect());
                added = true;
            }
        }
        for fan in found_fans {
            if self.fans.iter().all(|f| f.id != fan.id) {
                self.fans.push(fan);
                added = true;
            }
        }
        if added {
            self.generation += 1;
        }
    }

    pub fn sample(&mut self, now_ms: u64) -> Sample {
        if self.ticks > 0 && self.ticks.is_multiple_of(RESCAN_EVERY) {
            self.rescan();
        }
        self.ticks += 1;

        let values: Vec<Option<f64>> = self.sensors.iter().map(|s| self.read_sensor(s)).collect();
        self.times.push_back(now_ms);
        for (series, value) in self.series.iter_mut().zip(&values) {
            series.push_back(*value);
        }
        if self.times.len() > HISTORY_LEN {
            self.times.pop_front();
            for series in &mut self.series {
                series.pop_front();
            }
        }

        self.fan_readings = self.fans.iter().map(|f| self.read_fan(f)).collect();

        Sample {
            generation: self.generation,
            t: now_ms,
            values,
            fans: self.fan_readings.clone(),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            generation: self.generation,
            interval_ms: INTERVAL.as_millis() as u64,
            history_len: HISTORY_LEN,
            sensors: self.sensors.clone(),
            times: self.times.iter().copied().collect(),
            series: self.series.iter().map(|s| s.iter().copied().collect()).collect(),
            fans: self.fans.clone(),
            fan_readings: self.fan_readings.clone(),
        }
    }

    pub fn sensors(&self) -> &[Sensor] {
        &self.sensors
    }

    pub fn sensor_count(&self) -> usize {
        self.sensors.len()
    }

    pub fn fan_count(&self) -> usize {
        self.fans.len()
    }
}

/// hwmon sensors and fans, plus NVIDIA GPUs when the NVIDIA driver is present.
fn discover_all(paths: &SysPaths, nvidia: Option<&Nvidia>) -> (Vec<Sensor>, Vec<Fan>) {
    let mut found_sensors = sensors::discover(paths);
    let mut found_fans = fans::discover(paths);
    if let Some(nvidia) = nvidia {
        let (more_sensors, more_fans) = nvidia.discover();
        found_sensors.extend(more_sensors);
        found_fans.extend(more_fans);
        sensors::sort(&mut found_sensors);
    }
    (found_sensors, found_fans)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shows what this machine gives the app, and how saved ids will move:
    /// `cargo test real_monitor -- --ignored --nocapture`.
    #[test]
    #[ignore = "reads the real /sys of the machine running the test"]
    fn real_monitor() {
        let mut monitor = Monitor::new(SysPaths::default());
        println!("NVIDIA (NVML) loaded: {}", monitor.nvidia.is_some());
        println!("{} sensors, {} fans", monitor.sensor_count(), monitor.fan_count());
        for (old, new) in monitor.aliases() {
            println!("moves: {old} -> {new}");
        }
        let sample = monitor.sample(0);
        assert_eq!(sample.values.len(), monitor.sensor_count());
        assert!(sample.values.iter().any(Option::is_some));
    }
}
