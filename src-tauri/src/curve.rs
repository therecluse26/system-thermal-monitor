//! Fan curves: the PWM duty a fan gets at each temperature of one sensor.

use serde::{Deserialize, Serialize};

/// °C the temperature must fall before the duty follows it down. Stops the fan hunting.
pub const HYSTERESIS: f64 = 3.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CurvePoint {
    /// °C.
    pub temp: f64,
    /// PWM duty in percent.
    pub duty: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FanCurve {
    /// Id of the sensor whose temperature drives the fan.
    pub sensor: String,
    /// In temperature order. Between two points the duty is a straight line.
    pub points: Vec<CurvePoint>,
}

impl FanCurve {
    /// Duty in percent at `temp`. Below the first point the first duty applies,
    /// above the last point the last duty.
    pub fn duty_at(&self, temp: f64) -> f64 {
        let (Some(first), Some(last)) = (self.points.first(), self.points.last()) else {
            return 100.0;
        };
        if temp <= first.temp {
            return first.duty;
        }
        if temp >= last.temp {
            return last.duty;
        }
        let i = self.points.iter().position(|p| p.temp > temp).unwrap_or(1);
        let (a, b) = (self.points[i - 1], self.points[i]);
        a.duty + (temp - a.temp) / (b.temp - a.temp) * (b.duty - a.duty)
    }

    /// The duty that follows `last`. It rises with the temperature at once, but falls
    /// only to the curve's duty at `HYSTERESIS` °C above the temperature.
    pub fn next_duty(&self, temp: f64, last: Option<f64>) -> f64 {
        let now = self.duty_at(temp);
        match last {
            Some(last) if now < last => self.duty_at(temp + HYSTERESIS).min(last).max(now),
            _ => now,
        }
    }
}
