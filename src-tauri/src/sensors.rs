//! Finds and reads the temperature sensors that the kernel exposes through hwmon.
//! NVIDIA GPUs come from `nvidia.rs` instead, as the same `Sensor` type.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::thresholds::{self, Limits, Thresholds};

/// Where the kernel exposes sensor data. Tests point this at a fake tree.
#[derive(Debug, Clone)]
pub struct SysPaths {
    pub hwmon: PathBuf,
    pub thermal: PathBuf,
    pub cpuinfo: PathBuf,
}

impl Default for SysPaths {
    fn default() -> Self {
        Self {
            hwmon: "/sys/class/hwmon".into(),
            thermal: "/sys/class/thermal".into(),
            cpuinfo: "/proc/cpuinfo".into(),
        }
    }
}

/// The UI groups sensors by category, in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Cpu,
    Gpu,
    Memory,
    Storage,
    Board,
    Wireless,
    Battery,
    Acpi,
    Other,
}

/// Where a sensor's reading comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// A hwmon `tempN_input` file.
    Hwmon(PathBuf),
    /// An NVIDIA GPU, by NVML device index.
    Nvml(u32),
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sensor {
    /// Stable across reboots where the hardware allows it. Settings are keyed by it.
    pub id: String,
    /// The id that earlier versions of the app used, so saved settings can move to `id`.
    #[serde(skip)]
    pub legacy_id: String,
    /// hwmon driver name, e.g. "k10temp" or "nvme".
    pub chip: String,
    pub category: Category,
    /// Short device name, e.g. "CPU" or "WD_BLACK SN850X 2000GB".
    pub device: String,
    /// Name of this sensor on the device, e.g. "Tctl" or "Composite".
    pub label: String,
    /// Kernel name of the device, set only when another device has the same name
    /// (two drives of one model), e.g. "nvme0".
    pub instance: Option<String>,
    /// Longer description of the device, e.g. the CPU model or the PCI address.
    pub detail: String,
    pub thresholds: Thresholds,
    #[serde(skip)]
    pub input: Input,
    /// Kernel name of the device (e.g. "nvme0"), used to tell identical devices apart.
    #[serde(skip)]
    pub kernel_name: String,
    /// The hwmon directory, or another value that is unique per physical device.
    #[serde(skip)]
    pub hwmon: PathBuf,
}

/// Reads a sysfs millidegree file as °C.
fn read_celsius(path: &Path) -> Option<f64> {
    let milli: i64 = fs::read_to_string(path).ok()?.trim().parse().ok()?;
    Some(milli as f64 / 1000.0)
}

/// Real sensors in a running PC read above 0 °C and far below 200 °C.
/// Absent sensors often report 0, a negative number or a sentinel instead.
fn plausible(celsius: f64) -> bool {
    celsius > 0.0 && celsius < 200.0
}

/// Current temperature of a hwmon sensor in °C, or `None` when it cannot be read right now.
pub fn read_hwmon(input: &Path) -> Option<f64> {
    read_celsius(input).filter(|c| plausible(*c))
}

/// Reads a limit file and rounds it to 0.1 °C. Rounding the integer avoids
/// 82.85 turning into 82.8 through float error.
fn read_limit(path: &Path) -> Option<f64> {
    let milli: i64 = fs::read_to_string(path).ok()?.trim().parse().ok()?;
    Some((milli as f64 / 100.0).round() / 10.0).filter(|c| thresholds::believable(*c))
}

pub(crate) fn read_trimmed(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Sorted N for every entry of `dir` named `{prefix}{N}{suffix}`,
/// e.g. "hwmon12", "temp3_input" or "trip_point_0_type".
pub(crate) fn numbered(dir: &Path, prefix: &str, suffix: &str) -> Vec<u32> {
    let mut found: Vec<u32> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name();
            name.to_str()?.strip_prefix(prefix)?.strip_suffix(suffix)?.parse().ok()
        })
        .collect();
    found.sort_unstable();
    found
}

fn hw_limits(dir: &Path, n: u32) -> Limits {
    let limit = |suffix: &str| read_limit(&dir.join(format!("temp{n}_{suffix}")));
    Limits {
        max: limit("max"),
        crit: limit("crit"),
        emergency: limit("emergency"),
        passive: None,
    }
}

/// A thermal zone that backs one temperature of a thermal-subsystem hwmon device.
struct Zone {
    index: u32,
    passive: Option<f64>,
    critical: Option<f64>,
}

/// Thermal zones of one type, in index order. The kernel registers them into the
/// shared hwmon device in that order, so zone k backs `temp{k+1}`.
fn zones_of_type(thermal: &Path, kind: &str) -> Vec<Zone> {
    numbered(thermal, "thermal_zone", "")
        .into_iter()
        .map(|index| (index, thermal.join(format!("thermal_zone{index}"))))
        .filter(|(_, dir)| read_trimmed(&dir.join("type")).as_deref() == Some(kind))
        .map(|(index, dir)| {
            let mut zone = Zone {
                index,
                passive: None,
                critical: None,
            };
            for t in numbered(&dir, "trip_point_", "_type") {
                let kind = read_trimmed(&dir.join(format!("trip_point_{t}_type")));
                let temp = read_limit(&dir.join(format!("trip_point_{t}_temp")));
                match kind.as_deref() {
                    Some("passive") => zone.passive = zone.passive.or(temp),
                    Some("critical") => zone.critical = zone.critical.or(temp),
                    _ => {}
                }
            }
            zone
        })
        .collect()
}

fn cpu_model(cpuinfo: &Path) -> Option<String> {
    let text = fs::read_to_string(cpuinfo).ok()?;
    text.lines()
        .find(|l| l.starts_with("model name"))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.trim().to_string())
}

struct Description {
    category: Category,
    device: String,
    label: String,
    detail: String,
}

const ACRONYMS: &[&str] = &[
    "cpu", "apu", "gpu", "vr", "vram", "soc", "pch", "ssd", "ddr", "vrm", "dimm", "pcie", "usb",
];

fn pretty_word(word: &str, first: bool) -> String {
    let lower = word.to_ascii_lowercase();
    if ACRONYMS.contains(&lower.as_str()) {
        return lower.to_ascii_uppercase();
    }
    if !first {
        return lower;
    }
    let mut chars = lower.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => lower,
    }
}

/// "ambient_f75303@4d" → "Ambient", "gpu_vr_f75303@4d" → "GPU VR".
fn ec_label(raw: &str) -> String {
    let base = raw.split('@').next().unwrap_or(raw);
    // Drop a trailing sensor part number such as "_f75303".
    let base = match base.rsplit_once('_') {
        Some((head, tail))
            if tail.chars().any(|c| c.is_ascii_digit())
                && tail.chars().any(|c| c.is_ascii_alphabetic()) =>
        {
            head
        }
        _ => base,
    };
    base.split('_')
        .enumerate()
        .map(|(i, w)| pretty_word(w, i == 0))
        .collect::<Vec<_>>()
        .join(" ")
}

/// "Tccd1" → "CCD 1", "Package id 0" → "Package"; other labels stay as they are.
fn cpu_label(raw: &str) -> String {
    if let Some(n) = raw.strip_prefix("Tccd") {
        return format!("CCD {n}");
    }
    if raw.starts_with("Package id") {
        return "Package".to_string();
    }
    raw.to_string()
}

/// SPD sensors sit at I²C addresses 0x50–0x57, one per slot: "20-0051" → "DIMM 1".
fn dimm_label(kernel_name: &str) -> String {
    kernel_name
        .rsplit_once('-')
        .and_then(|(_, addr)| u16::from_str_radix(addr, 16).ok())
        .filter(|a| (0x50..=0x57).contains(a))
        .map(|a| format!("DIMM {}", a - 0x50))
        .unwrap_or_else(|| "DIMM".to_string())
}

fn is_wireless(chip: &str) -> bool {
    chip.contains("_phy")
        || ["iwlwifi", "ath", "rtw", "mt79", "mt76", "brcmfmac"]
            .iter()
            .any(|p| chip.starts_with(p))
}

fn is_motherboard(chip: &str) -> bool {
    ["nct", "it8", "f71", "w83", "asus", "thinkpad", "dell_smm", "gigabyte", "hp_wmi"]
        .iter()
        .any(|p| chip.starts_with(p))
}

fn describe(
    chip: &str,
    n: u32,
    raw_label: Option<&str>,
    device_dir: Option<&Path>,
    zone: Option<&Zone>,
    cpu: &str,
) -> Description {
    let kernel_name = device_dir
        .and_then(|d| d.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let device_file = |name: &str| device_dir.and_then(|d| read_trimmed(&d.join(name)));
    let label_or = |fallback: String| raw_label.map(str::to_string).unwrap_or(fallback);
    let d = |category, device: &str, label: String, detail: String| Description {
        category,
        device: device.to_string(),
        label,
        detail,
    };

    match chip {
        "k10temp" | "zenpower" | "coretemp" | "cpu_thermal" => d(
            Category::Cpu,
            "CPU",
            raw_label.map(cpu_label).unwrap_or_else(|| format!("temp{n}")),
            cpu.to_string(),
        ),
        "amdgpu" | "radeon" | "nouveau" | "i915" | "xe" => {
            let label = match raw_label {
                Some("edge") => "Edge".to_string(),
                Some("junction") => "Junction".to_string(),
                Some("mem") => "Memory".to_string(),
                Some(other) => pretty_word(other, true),
                None => format!("temp{n}"),
            };
            d(Category::Gpu, "GPU", label, format!("{chip} at {kernel_name}"))
        }
        "nvme" => d(
            Category::Storage,
            &device_file("model").unwrap_or_else(|| "NVMe drive".to_string()),
            label_or("Composite".to_string()),
            format!("NVMe drive {kernel_name}"),
        ),
        "drivetemp" => d(
            Category::Storage,
            &device_file("model").unwrap_or_else(|| "Drive".to_string()),
            "Drive".to_string(),
            format!("SATA drive {kernel_name}"),
        ),
        "spd5118" | "jc42" => d(
            Category::Memory,
            "Memory",
            dimm_label(&kernel_name),
            format!("SPD sensor {kernel_name}"),
        ),
        "cros_ec" => d(
            Category::Board,
            "EC",
            raw_label.map(ec_label).unwrap_or_else(|| format!("temp{n}")),
            "Embedded controller".to_string(),
        ),
        "acpitz" => d(
            Category::Acpi,
            "ACPI",
            format!("Zone {}", zone.map(|z| z.index).unwrap_or(n.saturating_sub(1))),
            "ACPI thermal zone".to_string(),
        ),
        _ if is_wireless(chip) => d(
            Category::Wireless,
            "Wi-Fi",
            chip.split('_').next().unwrap_or(chip).to_string(),
            format!("Wireless adapter {kernel_name}"),
        ),
        _ if chip.starts_with("BAT") || chip == "battery" => d(
            Category::Battery,
            "Battery",
            label_or(chip.to_string()),
            chip.to_string(),
        ),
        _ if is_motherboard(chip) => d(
            Category::Board,
            "Board",
            label_or(format!("temp{n}")),
            format!("Motherboard sensor chip {chip}"),
        ),
        _ => d(
            Category::Other,
            chip,
            label_or(format!("temp{n}")),
            chip.to_string(),
        ),
    }
}

/// Part of the sensor id that names the device. It must not change between boots, so it
/// avoids the numbers the kernel hands out in probe order:
///
/// - NVMe drives use their serial number, because "nvme0" and "nvme1" can swap.
/// - Devices on PCI use their PCI address, which belongs to the slot.
/// - Below a PCI device, numbered names lose their number ("phy0" → "phy"), and I²C
///   clients keep only their address ("20-0050" → "i2c-0050").
/// - Platform devices drop an automatic instance number ("cros-ec-hwmon.6.auto" → "cros-ec-hwmon").
pub(crate) fn device_key(chip: &str, device_dir: Option<&Path>) -> String {
    if chip == "nvme" {
        if let Some(serial) = device_dir.and_then(|d| read_trimmed(&d.join("serial"))) {
            return serial;
        }
    }
    let Some(dir) = device_dir else {
        return chip.to_string();
    };
    let parts: Vec<String> = dir
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let leaf = parts.last().cloned().unwrap_or_default();
    let pci = parts.iter().rev().find(|p| is_pci_address(p));
    match pci {
        Some(pci) if *pci == leaf => leaf,
        Some(pci) => format!("{pci}/{}", stable_leaf(&leaf, true)),
        None => stable_leaf(&leaf, false),
    }
}

/// The driver name as used in ids. Some drivers put an instance number in their name
/// ("mt7921_phy0", "iwlwifi_1"); that number is dropped.
pub(crate) fn id_chip(chip: &str) -> &str {
    let base = chip.trim_end_matches(|c: char| c.is_ascii_digit());
    if base.len() == chip.len() {
        return chip;
    }
    base.strip_suffix("_phy").or_else(|| base.strip_suffix('_')).unwrap_or(chip)
}

/// "0000:c3:00.0": domain, bus, device and function in hex.
fn is_pci_address(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() == 12
        && b[4] == b':'
        && b[7] == b':'
        && b[10] == b'.'
        && name.chars().enumerate().all(|(i, c)| matches!(i, 4 | 7 | 10) || c.is_ascii_hexdigit())
}

fn stable_leaf(leaf: &str, under_pci: bool) -> String {
    if let Some(base) = leaf.strip_suffix(".auto") {
        return base
            .trim_end_matches(|c: char| c.is_ascii_digit())
            .trim_end_matches('.')
            .to_string();
    }
    if let Some((bus, addr)) = leaf.split_once('-') {
        if bus.chars().all(|c| c.is_ascii_digit()) && addr.len() == 4 && addr.chars().all(|c| c.is_ascii_hexdigit()) {
            return format!("i2c-{addr}");
        }
    }
    let base = leaf.trim_end_matches(|c: char| c.is_ascii_digit());
    if under_pci && !base.is_empty() && base.chars().all(|c| c.is_ascii_alphabetic()) {
        return base.to_string();
    }
    leaf.to_string()
}

/// The device part of ids from earlier versions of the app: the plain kernel name.
pub(crate) fn legacy_device_key(chip: &str, hwmon: &Path, device_dir: Option<&Path>) -> String {
    if chip == "nvme" {
        if let Some(serial) = device_dir.and_then(|d| read_trimmed(&d.join("serial"))) {
            return serial;
        }
    }
    device_dir
        .or(Some(hwmon))
        .and_then(|d| d.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Gives a sensor whose id another sensor already has a suffix from its legacy id, so
/// every id stays unique even when two devices share a stable name.
pub(crate) fn make_unique<T>(items: &mut [T], id: fn(&mut T) -> (&mut String, &str)) {
    let mut seen: HashMap<String, usize> = HashMap::new();
    for item in items.iter_mut() {
        let (current, _) = id(item);
        *seen.entry(current.clone()).or_default() += 1;
    }
    for item in items.iter_mut() {
        let (current, legacy) = id(item);
        if seen[current.as_str()] > 1 {
            *current = format!("{current}@{legacy}");
        }
    }
}

/// Finds every temperature sensor that reads a plausible value right now.
pub fn discover(paths: &SysPaths) -> Vec<Sensor> {
    let cpu = cpu_model(&paths.cpuinfo).unwrap_or_else(|| "CPU".to_string());
    let mut sensors = Vec::new();

    for index in numbered(&paths.hwmon, "hwmon", "") {
        let dir = paths.hwmon.join(format!("hwmon{index}"));
        let Some(chip) = read_trimmed(&dir.join("name")) else {
            continue;
        };
        let device_dir = fs::canonicalize(dir.join("device")).ok();
        let from_thermal_zone = device_dir
            .as_deref()
            .is_some_and(|d| d.to_string_lossy().contains("/virtual/thermal/"));
        let zones = if from_thermal_zone {
            zones_of_type(&paths.thermal, &chip)
        } else {
            Vec::new()
        };

        for n in numbered(&dir, "temp", "_input") {
            let input = dir.join(format!("temp{n}_input"));
            if !read_celsius(&input).is_some_and(plausible) {
                continue;
            }
            let raw_label = read_trimmed(&dir.join(format!("temp{n}_label")));
            let zone = (n as usize).checked_sub(1).and_then(|i| zones.get(i));

            let mut limits = hw_limits(&dir, n);
            // NVMe "Sensor N" entries often carry garbage limits; the drive's
            // composite limits apply to the whole drive.
            if chip == "nvme" && limits.max.is_none() {
                limits = hw_limits(&dir, 1);
            }
            if let Some(z) = zone {
                limits.passive = z.passive;
                limits.crit = limits.crit.or(z.critical);
            }

            let desc = describe(&chip, n, raw_label.as_deref(), device_dir.as_deref(), zone, &cpu);
            sensors.push(Sensor {
                id: format!(
                    "{}/{}/temp{n}",
                    id_chip(&chip),
                    device_key(&chip, device_dir.as_deref())
                ),
                legacy_id: format!(
                    "{chip}/{}/temp{n}",
                    legacy_device_key(&chip, &dir, device_dir.as_deref())
                ),
                thresholds: thresholds::resolve(&chip, desc.category, &limits),
                chip: chip.clone(),
                category: desc.category,
                device: desc.device,
                label: desc.label,
                instance: None,
                detail: desc.detail,
                input: Input::Hwmon(input),
                kernel_name: device_dir
                    .as_deref()
                    .and_then(|d| d.file_name())
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                hwmon: dir.clone(),
            });
        }
    }

    make_unique(&mut sensors, |s| (&mut s.id, &s.legacy_id));
    disambiguate(&mut sensors);
    sort(&mut sensors);
    sensors
}

/// Sorts by category, then device; sensors of one device keep their order.
pub fn sort(sensors: &mut [Sensor]) {
    sensors.sort_by(|a, b| (a.category, &a.device).cmp(&(b.category, &b.device)));
}

/// Two drives of the same model would get the same names. Give every sensor of a
/// device whose full sensor names collide with another device's its kernel name.
pub(crate) fn disambiguate(sensors: &mut [Sensor]) {
    let mut dirs_per_name: HashMap<(String, String), HashSet<PathBuf>> = HashMap::new();
    for s in sensors.iter() {
        dirs_per_name
            .entry((s.device.clone(), s.label.clone()))
            .or_default()
            .insert(s.hwmon.clone());
    }
    let clashing: HashSet<String> = dirs_per_name
        .into_iter()
        .filter(|(_, dirs)| dirs.len() > 1)
        .map(|((device, _), _)| device)
        .collect();
    for s in sensors.iter_mut() {
        if clashing.contains(&s.device) && !s.kernel_name.is_empty() {
            s.instance = Some(s.kernel_name.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testsys::FakeSys;

    #[test]
    fn finds_and_names_sensors() {
        let sys = FakeSys::new();
        sys.write("class/hwmon/hwmon9/name", "k10temp\n");
        sys.write("class/hwmon/hwmon9/temp1_input", "57375\n");
        sys.write("class/hwmon/hwmon9/temp1_label", "Tctl\n");
        sys.link_device("hwmon9", "devices/pci/0000:00:18.3");

        sys.write("class/hwmon/hwmon6/name", "cros_ec\n");
        sys.write("class/hwmon/hwmon6/temp4_input", "55850\n");
        sys.write("class/hwmon/hwmon6/temp4_label", "cpu@4c\n");
        sys.write("class/hwmon/hwmon6/temp4_max", "104850\n");
        // Absent sensor: reads -0.15 °C and must be skipped.
        sys.write("class/hwmon/hwmon6/temp5_input", "-150\n");
        sys.write("class/hwmon/hwmon6/temp5_label", "gpu_amb_f75303@4d\n");

        sys.write("class/hwmon/hwmon11/name", "spd5118\n");
        sys.write("class/hwmon/hwmon11/temp1_input", "52750\n");
        sys.write("class/hwmon/hwmon11/temp1_crit", "85000\n");
        sys.link_device("hwmon11", "devices/i2c-20/20-0051");

        let sensors = discover(&sys.paths());
        let names: Vec<String> = sensors
            .iter()
            .map(|s| format!("{} · {}", s.device, s.label))
            .collect();
        assert_eq!(names, ["CPU · Tctl", "Memory · DIMM 1", "EC · CPU"]);

        assert_eq!(sensors[0].detail, "Test CPU 9000");
        assert_eq!(sensors[0].id, "k10temp/0000:00:18.3/temp1");
        assert_eq!(sensors[0].input, Input::Hwmon(sys.path("class/hwmon/hwmon9/temp1_input")));
        assert_eq!(read_hwmon(&sys.path("class/hwmon/hwmon9/temp1_input")), Some(57.375));
        assert_eq!(sensors[1].thresholds.throttle, 85.0);
        assert_eq!(sensors[2].thresholds.throttle, 104.9);
    }

    #[test]
    fn nvme_sensor_with_garbage_limit_uses_composite_limit() {
        let sys = FakeSys::new();
        sys.write("class/hwmon/hwmon0/name", "nvme\n");
        sys.write("class/hwmon/hwmon0/temp1_input", "33850\n");
        sys.write("class/hwmon/hwmon0/temp1_label", "Composite\n");
        sys.write("class/hwmon/hwmon0/temp1_max", "82850\n");
        sys.write("class/hwmon/hwmon0/temp2_input", "33850\n");
        sys.write("class/hwmon/hwmon0/temp2_label", "Sensor 1\n");
        sys.write("class/hwmon/hwmon0/temp2_max", "65261850\n");
        sys.link_device("hwmon0", "devices/pci/nvme/nvme0");
        sys.write("devices/pci/nvme/nvme0/model", "FAST SSD 1TB          \n");
        sys.write("devices/pci/nvme/nvme0/serial", "  SER123  \n");

        let sensors = discover(&sys.paths());
        assert_eq!(sensors.len(), 2);
        assert_eq!(sensors[0].device, "FAST SSD 1TB");
        assert_eq!(sensors[0].id, "nvme/SER123/temp1");
        assert_eq!(sensors[1].label, "Sensor 1");
        assert_eq!(sensors[1].thresholds.throttle, 82.9);
    }

    #[test]
    fn identical_models_get_kernel_names() {
        let sys = FakeSys::new();
        for (hw, nv) in [("hwmon0", "nvme0"), ("hwmon1", "nvme2")] {
            sys.write(&format!("class/hwmon/{hw}/name"), "nvme\n");
            sys.write(&format!("class/hwmon/{hw}/temp1_input"), "31850\n");
            sys.link_device(hw, &format!("devices/{nv}"));
            sys.write(&format!("devices/{nv}/model"), "SAME MODEL\n");
        }
        // Two DIMMs share the device name "Memory", but their labels already differ.
        for (hw, addr) in [("hwmon11", "20-0050"), ("hwmon12", "20-0051")] {
            sys.write(&format!("class/hwmon/{hw}/name"), "spd5118\n");
            sys.write(&format!("class/hwmon/{hw}/temp1_input"), "52000\n");
            sys.link_device(hw, &format!("devices/i2c-20/{addr}"));
        }
        let names: Vec<String> = discover(&sys.paths())
            .into_iter()
            .map(|s| match s.instance {
                Some(i) => format!("{} ({i}) · {}", s.device, s.label),
                None => format!("{} · {}", s.device, s.label),
            })
            .collect();
        assert_eq!(
            names,
            [
                "Memory · DIMM 0",
                "Memory · DIMM 1",
                "SAME MODEL (nvme0) · Composite",
                "SAME MODEL (nvme2) · Composite",
            ]
        );
    }

    #[test]
    fn acpi_zones_supply_trip_points() {
        let sys = FakeSys::new();
        sys.write("class/hwmon/hwmon7/name", "acpitz\n");
        sys.write("class/hwmon/hwmon7/temp1_input", "45800\n");
        sys.write("class/hwmon/hwmon7/temp2_input", "48800\n");
        sys.link_device("hwmon7", "devices/virtual/thermal/thermal_zone0");
        for (z, passive) in [(0, "90000"), (1, "80000")] {
            let dir = format!("class/thermal/thermal_zone{z}");
            sys.write(&format!("{dir}/type"), "acpitz\n");
            sys.write(&format!("{dir}/trip_point_0_type"), "critical\n");
            sys.write(&format!("{dir}/trip_point_0_temp"), "170000\n");
            sys.write(&format!("{dir}/trip_point_1_type"), "passive\n");
            sys.write(&format!("{dir}/trip_point_1_temp"), &format!("{passive}\n"));
        }
        let sensors = discover(&sys.paths());
        assert_eq!(sensors[0].label, "Zone 0");
        assert_eq!(sensors[0].thresholds.throttle, 90.0);
        assert_eq!(sensors[1].label, "Zone 1");
        assert_eq!(sensors[1].thresholds.throttle, 80.0);
    }

    #[test]
    fn ids_avoid_probe_order_numbers() {
        let key = |p: &str| device_key("x", Some(Path::new(p)));
        assert_eq!(key("/sys/devices/pci0000:00/0000:00:18.3"), "0000:00:18.3");
        assert_eq!(
            key("/sys/devices/platform/FRMWC004:00/cros-ec-dev.0.auto/cros-ec-hwmon.6.auto"),
            "cros-ec-hwmon"
        );
        assert_eq!(
            key("/sys/devices/pci0000:00/0000:00:02.2/0000:03:00.0/ieee80211/phy0"),
            "0000:03:00.0/phy"
        );
        assert_eq!(
            key("/sys/devices/pci0000:00/0000:00:14.0/i2c-20/20-0050"),
            "0000:00:14.0/i2c-0050"
        );
        assert_eq!(key("/sys/devices/virtual/thermal/thermal_zone0"), "thermal_zone0");
        assert_eq!(key("/sys/devices/platform/coretemp.0"), "coretemp.0");
        assert_eq!(key("/sys/devices/LNXSYSTM:00/PNP0C0A:00/power_supply/BAT1"), "BAT1");
        assert_eq!(device_key("acpitz", None), "acpitz");
        assert_eq!(id_chip("mt7921_phy0"), "mt7921");
        assert_eq!(id_chip("iwlwifi_1"), "iwlwifi");
        assert_eq!(id_chip("nct6798"), "nct6798");
        assert_eq!(id_chip("k10temp"), "k10temp");
    }

    #[test]
    fn clashing_ids_fall_back_to_kernel_names() {
        let sys = FakeSys::new();
        // Two SPD sensors at the same address on two buses of one controller.
        for (hw, bus) in [("hwmon11", "i2c-20/20-0050"), ("hwmon12", "i2c-21/21-0050")] {
            sys.write(&format!("class/hwmon/{hw}/name"), "spd5118\n");
            sys.write(&format!("class/hwmon/{hw}/temp1_input"), "52000\n");
            sys.link_device(hw, &format!("devices/0000:00:14.0/{bus}"));
        }
        let ids: Vec<String> = discover(&sys.paths()).into_iter().map(|s| s.id).collect();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);
        assert!(ids[0].ends_with("@spd5118/20-0050/temp1"), "{ids:?}");
    }

    #[test]
    fn labels_are_readable() {
        assert_eq!(ec_label("ambient_f75303@4d"), "Ambient");
        assert_eq!(ec_label("gpu_vr_f75303@4d"), "GPU VR");
        assert_eq!(ec_label("cpu@4c"), "CPU");
        assert_eq!(cpu_label("Tccd1"), "CCD 1");
        assert_eq!(cpu_label("Package id 0"), "Package");
        assert_eq!(dimm_label("20-0050"), "DIMM 0");
        assert!(is_wireless("mt7921_phy0"));
        assert!(is_wireless("iwlwifi_1"));
    }

    /// Shows what this machine exposes: `cargo test real_sensors -- --ignored --nocapture`.
    #[test]
    #[ignore = "reads the real /sys of the machine running the test"]
    fn real_sensors() {
        for s in discover(&SysPaths::default()) {
            let mut json = serde_json::to_value(&s).unwrap();
            let Input::Hwmon(path) = &s.input else { continue };
            json["now"] = serde_json::json!(read_hwmon(path));
            println!("{json}");
        }
    }
}
