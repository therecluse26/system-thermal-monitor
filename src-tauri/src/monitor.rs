//! Samples every sensor on a fixed interval and keeps a rolling history.

use std::collections::VecDeque;
use std::time::Duration;

use serde::Serialize;

use crate::fans::{self, Fan, FanReading};
use crate::sensors::{self, Sensor, SysPaths};

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
        let sensors = sensors::discover(&paths);
        let series = sensors.iter().map(|_| VecDeque::new()).collect();
        let fans = fans::discover(&paths);
        let fan_readings = fans.iter().map(fans::read).collect();
        Self {
            paths,
            sensors,
            fans,
            generation: 1,
            ticks: 0,
            times: VecDeque::new(),
            series,
            fan_readings,
        }
    }

    /// Adds sensors and fans that were not there before. Known ones keep their slot
    /// even when they stop reading, so their history and settings survive.
    fn rescan(&mut self) {
        let mut added = false;
        for sensor in sensors::discover(&self.paths) {
            if self.sensors.iter().all(|s| s.id != sensor.id) {
                self.sensors.push(sensor);
                self.series.push(self.times.iter().map(|_| None).collect());
                added = true;
            }
        }
        for fan in fans::discover(&self.paths) {
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

        let values: Vec<Option<f64>> = self.sensors.iter().map(sensors::read).collect();
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

        self.fan_readings = self.fans.iter().map(fans::read).collect();

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

    pub fn sensor_count(&self) -> usize {
        self.sensors.len()
    }

    pub fn fan_count(&self) -> usize {
        self.fans.len()
    }
}
