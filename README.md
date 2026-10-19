# Thermal Monitor

A Tauri desktop app that charts every hardware temperature sensor over time.
The charts follow the look of KDE System Monitor.
Each chart shows two thresholds:

- **Keep below** (amber dashed line): the recommended maximum for full performance and long hardware life.
- **Throttling likely at** (red dashed line): the temperature where the device is likely to slow itself down.

The top of the window shows each fan: its speed, and a ring that fills to its share of maximum speed.
The fan icon in the ring spins faster as the fan does.

## Run it

```sh
npm install
npm run tauri dev                       # development, with hot reload
npm run tauri build -- --no-bundle      # release binary: src-tauri/target/release/thermal-monitor
```

`CARGO_TARGET_DIR` moves the release binary when you set it.
`npm run tauri build` without `--no-bundle` also makes `.deb`, `.rpm` and AppImage installers.
On NixOS, use `--no-bundle`; the installer formats do not suit NixOS.

If the window stays blank, start the app with `WEBKIT_DISABLE_DMABUF_RENDERER=1`.
This is a WebKitGTK problem with some GPU drivers, not a problem in this app.

## Where the data comes from

The app reads `/sys/class/hwmon` once per second. It needs no root access.
It keeps 30 minutes of history in memory. The time-window buttons choose 1, 5, 10 or 30 minutes.
Every 30 seconds it looks for new sensors and fans, for example a dGPU that woke up.

Sensors that read 0 °C or less, or 200 °C or more, are treated as absent.
Example: a Framework 16 without a GPU module reports its GPU sensors as -0.15 °C.

## Fans

The app reads every `fanN_input` in `/sys/class/hwmon`.
Some drivers report the same fans as another driver.
Example: `framework_laptop` repeats the `cros_ec` fans, so the app shows only the `cros_ec` ones.

A fan's "share of maximum" uses the first of these that the hardware offers:

1. **Rated max**: `fanN_max`, the maximum speed the hardware reports.
2. **PWM duty**: `pwmN`, the share of full power sent to the fan. On many laptops only root can read it.
3. **Fastest seen**: the highest speed this app has recorded for the fan. It is saved between runs.

Hover a fan card to see which one applies.

## How the thresholds are chosen

The device's own limit wins when it reports a believable one (30–150 °C).
Otherwise the app uses a typical value for that kind of device and marks it **estimate**.
The tag on each chart (hardware / estimate / custom) shows which applies.
Hover the tag, or open the editor, to see the reason.

| Sensor driver | Throttling likely at | Keep below |
|---|---|---|
| `nvme` | WCTEMP (warning temperature) from the drive | 10 °C lower |
| `spd5118`, `jc42` (DDR5/DDR4) | `crit`, else 85 °C (DDR refreshes twice as often above it) | 15 °C lower |
| `cros_ec` (Framework embedded controller) | EC warning limit; the EC throttles the CPU above it | 15 °C lower |
| `k10temp`, `zenpower` (AMD CPU) | 95 °C estimate (Tjmax is 95–100 °C on most Ryzen models) | 85 °C |
| `coretemp` (Intel CPU) | TjMax from the CPU | 15 °C lower |
| `amdgpu` and other GPUs | `crit` from the driver, else 95 °C estimate | 10–15 °C lower |
| ACPI thermal zones | `passive` trip point, else general values | 10 °C lower |
| Wi-Fi chips | 100 °C estimate | 80 °C |
| Anything else | `max`, then `crit` − 5 °C, else 95 °C estimate | 10–15 °C lower |

The edit button on each chart sets custom values or hides the sensor.
The app saves these choices in `~/.config/local.thermal-monitor/settings.json`.

## Code layout

| Path | Contents |
|---|---|
| `src-tauri/src/sensors.rs` | Finds hwmon sensors, names them, reads them |
| `src-tauri/src/fans.rs` | Finds and reads fans, drops duplicate drivers |
| `src-tauri/src/thresholds.rs` | Default thresholds per sensor driver |
| `src-tauri/src/monitor.rs` | 1 Hz sampler and the 30-minute history |
| `src-tauri/src/settings.rs` | Saved overrides, hidden sensors, time window, fastest fan speeds |
| `src-tauri/src/main.rs` | Tauri commands (`snapshot`, `get_settings`, `save_settings`) and the `sample` event |
| `src/chart.ts` | Canvas chart: area line, grid, threshold lines, hover crosshair |
| `src/main.ts` | Chart cards, threshold editor, summary bar |
| `src/fans.ts` | Fan cards: ring gauge and spinning rotor |

## Tests

```sh
cd src-tauri
cargo test                                           # unit tests, against a fake /sys tree
cargo test real_sensors -- --ignored --nocapture     # prints the sensors of this machine as JSON
cargo test real_fans -- --ignored --nocapture        # prints the fans of this machine as JSON
```
