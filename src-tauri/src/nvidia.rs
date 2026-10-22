//! NVIDIA GPUs through NVML. The proprietary driver exposes no hwmon sensors.
//!
//! NVML is loaded at run time (`libnvidia-ml.so.1`). Without the NVIDIA driver,
//! `Nvidia::init` returns `None` and the app has no NVIDIA devices.

use std::collections::HashMap;
use std::path::PathBuf;

use nvml_wrapper::enum_wrappers::device::{TemperatureSensor, TemperatureThreshold};
use nvml_wrapper::{Device, Nvml};

use crate::fans::{Fan, FanInput, FanReading, ShareKind};
use crate::sensors::{Category, Input, Sensor};
use crate::thresholds;

pub struct Nvidia {
    nvml: Nvml,
}

fn threshold(device: &Device, kind: TemperatureThreshold) -> Option<f64> {
    device
        .temperature_threshold(kind)
        .ok()
        .map(f64::from)
        .filter(|c| thresholds::believable(*c))
}

impl Nvidia {
    pub fn init() -> Option<Self> {
        Nvml::init().ok().map(|nvml| Self { nvml })
    }

    /// One temperature sensor per GPU, and each GPU fan whose speed can be read.
    pub fn discover(&self) -> (Vec<Sensor>, Vec<Fan>) {
        let mut sensors = Vec::new();
        let mut fans = Vec::new();
        let count = self.nvml.device_count().unwrap_or(0);
        let mut names: HashMap<String, u32> = HashMap::new();

        for index in 0..count {
            let Ok(device) = self.nvml.device_by_index(index) else {
                continue;
            };
            let name = device.name().unwrap_or_else(|_| "NVIDIA GPU".to_string());
            let short = name.strip_prefix("NVIDIA ").unwrap_or(&name).to_string();
            *names.entry(short.clone()).or_default() += 1;
            // The UUID belongs to the board, so settings survive moving it to another slot.
            let uuid = device.uuid().unwrap_or_else(|_| format!("index{index}"));
            let instance = Some(format!("GPU {index}"));

            if device.temperature(TemperatureSensor::Gpu).is_ok() {
                sensors.push(Sensor {
                    id: format!("nvml/{uuid}/temp"),
                    legacy_id: format!("nvml/{uuid}/temp"),
                    chip: "nvml".to_string(),
                    category: Category::Gpu,
                    device: short.clone(),
                    label: "Core".to_string(),
                    instance: instance.clone(),
                    detail: name.clone(),
                    thresholds: thresholds::nvidia(
                        threshold(&device, TemperatureThreshold::AcousticCurr),
                        threshold(&device, TemperatureThreshold::GpuMax),
                        threshold(&device, TemperatureThreshold::Slowdown),
                        threshold(&device, TemperatureThreshold::Shutdown),
                    ),
                    input: Input::Nvml(index),
                    kernel_name: format!("GPU {index}"),
                    hwmon: PathBuf::from(format!("nvml/{uuid}")),
                });
            }

            for fan in 0..device.num_fans().unwrap_or(0) {
                if device.fan_speed(fan).is_err() {
                    continue;
                }
                fans.push(Fan {
                    id: format!("nvml/{uuid}/fan{fan}"),
                    legacy_id: format!("nvml/{uuid}/fan{fan}"),
                    chip: "nvml".to_string(),
                    device: short.clone(),
                    label: format!("Fan {}", fan + 1),
                    rated_max: None,
                    share_kind: Some(ShareKind::Driver),
                    controllable: false,
                    input: FanInput::Nvml { gpu: index, fan },
                    control: None,
                });
            }
        }

        // Show "GPU 0" and "GPU 1" only when two cards have the same name.
        for s in &mut sensors {
            if names[&s.device] < 2 {
                s.instance = None;
            }
        }
        (sensors, fans)
    }

    pub fn temperature(&self, gpu: u32) -> Option<f64> {
        let device = self.nvml.device_by_index(gpu).ok()?;
        device.temperature(TemperatureSensor::Gpu).ok().map(f64::from)
    }

    pub fn fan(&self, gpu: u32, fan: u32) -> Option<FanReading> {
        let device = self.nvml.device_by_index(gpu).ok()?;
        let share = f64::from(device.fan_speed(fan).ok()?).min(100.0);
        Some(FanReading {
            // Needs a recent driver; without it the card shows the share only.
            rpm: device.fan_speed_rpm(fan).ok(),
            target: None,
            share: Some(share),
        })
    }
}
