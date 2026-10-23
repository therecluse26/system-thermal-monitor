# Thermite

A Tauri desktop app that charts every hardware temperature sensor over time, and sets fan speeds.
Up to version 0.2, Thermite was called Thermal Monitor.
The charts follow the look of KDE System Monitor.
Sensors of one device that share their thresholds draw as one chart, for example both DIMMs.
Each chart shows two thresholds:

- **Keep below** (amber dashed line): the recommended maximum for full performance and long hardware life.
- **Throttling likely at** (red dashed line): the temperature where the device is likely to slow itself down.

The top of the window shows the system thermal load, then each fan.
A fan card shows its speed, and a ring that fills to its share of maximum speed.
The fan icon in the ring spins faster as the fan does.

## Run it

```sh
npm install
npm run tauri dev                       # development, with hot reload
npm run tauri build -- --no-bundle      # release binary: src-tauri/target/release/thermite
```

`CARGO_TARGET_DIR` moves the release binary when you set it.
`npm run tauri build` without `--no-bundle` also makes `.deb`, `.rpm` and AppImage installers.
On NixOS, use `--no-bundle`; the installer formats do not suit NixOS.

The installers of a release also carry the fan daemon. To build them the same way, add the release config:

```sh
rustup target add x86_64-unknown-linux-musl      # once
npm run tauri build -- --config src-tauri/tauri.release.conf.json
```

If the window stays blank, start the app with `WEBKIT_DISABLE_DMABUF_RENDERER=1`.
This is a WebKitGTK problem with some GPU drivers, not a problem in this app.

## Releases

Each push to `main` publishes a GitHub release with a `.deb`, an `.rpm` and an AppImage.
The packages are built on Ubuntu 22.04, so they also run on older distributions.
Each package also carries the fan daemon, `thermite-fand`, as a static binary (see Fan daemon).
Before the build, the release workflow runs the same checks as pull requests: the frontend build, clippy and `cargo test`.

The commit messages since the last release tag decide the new version:

| Commit subject | Bump | Example |
|---|---|---|
| `feat: ...` or `feat(scope): ...` | minor | 0.1.3 → 0.2.0 |
| any type with `!`, such as `fix!: ...`, or a `BREAKING CHANGE:` line in the body | major | 0.2.0 → 1.0.0 |
| anything else, including messages without a type | patch | 0.1.0 → 0.1.1 |

- The first release uses the version in `src-tauri/tauri.conf.json`.
- To jump to a version, set it there. When it is higher than the bumped version, it wins.
- The pipeline does not write the version back into the repository. The release tag holds it.
- The release notes list the commit subjects since the last release, grouped by type.
- A push that changes only Markdown files makes no release. Its commits go into the next release.
- To push without a release, put `[skip ci]` in the commit message.
- The `thermite` packages replace the `thermal-monitor` packages of versions up to 0.2.

## Where the data comes from

The app reads `/sys/class/hwmon` once per second. It needs no root access, except for fan control.
NVIDIA GPUs on the proprietary driver come from NVML instead (see below).
It keeps 30 minutes of history in memory. The time-window buttons choose 1, 5, 10 or 30 minutes.
Every 30 seconds it looks for new sensors and fans, for example a dGPU that woke up.

Sensors that read 0 °C or less, or 200 °C or more, are treated as absent.
Example: a Framework 16 without a GPU module reports its GPU sensors as -0.15 °C.

## One chart per device

The app puts sensors in one chart when they have the same device name and the same thresholds.
If you give one sensor its own thresholds, it moves to its own chart.

- **1 sensor**: a filled area, as in KDE System Monitor.
- **2–6 sensors**: one coloured line per sensor. The legend and the hover tooltip list each one.
- **More than 6 sensors** (for example the cores of a large Intel CPU): a band from the coolest
  to the hottest sensor. The legend names the hottest and the coolest sensor.

The edit button on a chart sets thresholds for every sensor in it.
It also lists the sensors, so you can hide one of them.

## System thermal load

The card shows two numbers:

- **Average**: the plain mean of the visible sensors, in °C.
- **Load**: how close the most stressed sensor is to its own throttle point, in percent.

The average mixes parts with very different limits, so it cannot show risk on its own. The load does:

- Each sensor gets `load = (30 °C − headroom) / 30 °C`, where headroom = throttle point − current temperature.
- 0% means the sensor is 30 °C or more below its throttle point. 100% means it has reached it.
- The system thermal load is the highest sensor load. The card names that sensor.

Android's thermal headroom API uses the same method and the same 30 °C span.
The load uses your custom thresholds. Both numbers ignore hidden sensors.

## Fans

The app reads every `fanN_input` in `/sys/class/hwmon`.
Some drivers report the same fans as another driver.
Example: `framework_laptop` repeats the `cros_ec` fans, so the app shows only the `cros_ec` ones.

A fan's "share of maximum" uses the first of these that the hardware offers:

1. **Rated max**: `fanN_max`, the maximum speed the hardware reports.
2. **PWM duty**: `pwmN`, the share of full power sent to the fan. On many laptops only root can read it.
3. **Fastest seen**: the highest speed this app has recorded for the fan. It is saved between runs.

Hover a fan card to see which one applies.

A fan card appears only after the fan has turned once, or when its controller asks for a speed.
This keeps empty fan headers on desktop boards off the screen.
Hover a fan card and click the crossed-out eye to hide it.

## NVIDIA GPUs

The proprietary NVIDIA driver exposes no hwmon sensors. The app reads these GPUs through NVML.
It loads `libnvidia-ml.so.1` at run time, so machines without the NVIDIA driver need nothing extra.

- **Temperature**: the GPU core temperature.
- **Throttling likely at**: the GPU Boost temperature target, else the "GPU max" threshold,
  else the hardware slowdown threshold. Without any of these: 83 °C, marked as an estimate.
- **Fans**: the speed as a share of maximum, from the driver. RPM appears too when the driver reports it.

This support is not tested on real NVIDIA hardware yet.

## Fan control

A **fan curve** sets the PWM duty of one fan from the temperature of one sensor.
Fan control has no compatibility guarantees. It is tested only on a Framework Laptop 16.

A fan card whose speed the app can set has a mode line under the speed. It shows:

- **Automatic**: the firmware sets the speed.
- **Fixed 40%**, or **Curve · 58%**: the duty that this app or the fan daemon set in the last second.
- **Full speed: failsafe**: see the safety rules below.
- Otherwise, why the setting does not apply now, for example "Waiting for your password".

Click **Fan control** in the top bar to set the speeds. The dialog has two views:

- **All fans**: one setting for every fan whose speed the app can set.
- **Individual fans**: a list with a setting for each fan.

A setting is Automatic, a fixed level from 0% to 100%, or a curve.
A fixed level is saved as a curve with one point.

- The dialog opens on "All fans" while every fan has the same setting. Otherwise it opens on "Individual fans".
- If you change a setting and then switch the view, the change carries over.
  In "All fans", all fans take it. In "Individual fans", each fan starts from it.
- Apply saves the view that you see.
- The dialog shows a fan after it has turned once, or when it has a setting. This drops empty fan headers.
  Hidden fans stay in the dialog, because hiding changes only the display.

The curve editor shows the duty at each temperature of the sensor you choose:

- Drag a point, or focus it and use the arrow keys (Shift moves in steps of 5).
- Double-click the grid to add a point. Delete removes the focused point. A curve keeps at least 2 points.
- The list under the chart takes exact numbers. "Add point" halves the widest gap.
- A dotted line marks the sensor's temperature now, with the duty the curve gives there.
- The red zone starts at the sensor's "Throttling likely at" temperature, where the failsafe sets full speed.

Only root can set fan speeds, so the app splits the work:

- When there are saved curves, the app starts a small **fan helper** as root through `pkexec`.
  After a cancelled password dialog, Apply in the fan control asks again.
  While the fan daemon runs your settings, the app starts no helper and shows no password dialog.
- Your desktop shows its own password dialog. The app never sees the password.
- In a sandbox that sets "no new privileges", `pkexec` cannot become root.
  On NixOS, `appimage-run` runs each AppImage in such a sandbox.
  There the app starts `pkexec` through `systemd-run --user`, outside the sandbox.
- If `pkexec` fails, the fan control shows its message, for example "pkexec must be setuid root".
- The fan helper runs the curves once per second. The window sends it changes through a pipe.
- When the app closes or crashes, the pipe closes. The helper then gives every fan back to its automatic mode.
- So curves run only while the app runs, unless the fan daemon runs them (see below).
- Without curves, the app shows no password dialog and needs no root.

Safety rules:

- At or above the sensor's "Throttling likely at" temperature, the fan runs at full speed. A custom value applies.
- When the sensor stops reading, the fan runs at full speed.
- The duty rises at once. It falls only after the temperature drops 3 °C, so the fan does not hunt.
- A curve that starts at 0% stops the fan below its first point. Never do this for a pump.
- The helper sets no fan while another fan controller runs: `fw-fanctrl run`, `fancontrol`, `thinkfan`,
  `coolercontrold`, `fan2go`, `nbfc_service`, or Thermite's own fan daemon `thermite-fand`. It checks every 30 seconds.
  Client commands such as `fw-fanctrl print`, which `fw-fanctrl-gui` runs, do not count.
- If the helper gets `SIGKILL`, the fans stay at their last duty. Only root or the out-of-memory killer can do that.

### Which fans the app can control

The driver must have both `pwmN` and `pwmN_enable`.
`cargo test real_fans -- --ignored --nocapture` shows `"controllable"` for each fan.

- Drivers that follow the hwmon ABI: the helper writes `1` to `pwmN_enable`, then the duty to `pwmN`.
  When it lets go, it writes back the mode it found.
  Firmware can turn its own mode back on, for example on resume from suspend.
  The helper then writes `1` again on its next tick, and the fan daemon does too.
- `framework_laptop`: any write to `pwmN_enable` gives control back to the EC.
  So the helper writes only `pwmN`, from 0 to 100. When it lets go, it writes `2` to `pwmN_enable`.
- On a Framework, `cros_ec` reports the fans and `framework_laptop` owns the PWM files.
  The fan cards keep the `cros_ec` fans; their control comes from `framework_laptop`.
- NVIDIA fans cannot be controlled yet.

### How curves are saved

```json
"fanCurves": {
  "cros_ec/cros-ec-hwmon/fan1": {
    "sensor": "k10temp/0000:00:18.3/temp1",
    "points": [
      { "temp": 45, "duty": 20 },
      { "temp": 60, "duty": 35 },
      { "temp": 85, "duty": 100 }
    ]
  }
}
```

- "All fans" saves the same curve under each fan id.
- The editor writes this. To edit `settings.json` by hand, close the app first: it writes its own copy back when it saves.
- Between two points the duty follows a straight line. Outside the points, the nearest point's duty applies.
- `cargo test real_fans` and `cargo test real_sensors` (with `-- --ignored --nocapture`) print the ids.
- From an AppImage, the helper runs the AppImage file (`$APPIMAGE`). Root cannot read the user's AppImage mount.

### Fan daemon

`thermite-fand` runs the fan settings of one user from boot, without the window and without a password dialog.

On NixOS, the module in this repository's flake starts it at boot (see On NixOS below).
The packages for other distributions carry it, but nothing installs or starts it there yet: there is no install switch.

- Each package puts it at `/usr/libexec/thermite/thermite-fand`. In the AppImage it is at `usr/libexec/thermite/thermite-fand`.
- It is a static musl binary. It needs no glibc and no loader, so it runs on every distribution.
- It is not in `usr/bin`: linuxdeploy rewrites the ELF files there for the AppImage, and a rewritten static binary crashes.
  The release workflow runs the daemon from the AppImage and the `.deb` before it publishes them.
- `.github/scripts/build-daemon.sh` builds it. `src-tauri/tauri.release.conf.json` runs that script and adds the
  binary to each package through `files`. That config is for release builds only, so other builds need no musl target.

To try it, build it and start it as root:

```sh
cd src-tauri
cargo build --release -p thermite-core --bin thermite-fand
sudo target/release/thermite-fand "$USER"      # Ctrl+C gives every fan back
```

- It reads that user's `~/.config/local.thermite/settings.json` once per second.
  So a setting that you apply in the window reaches the fans within a second.
- It does not follow `XDG_CONFIG_HOME`.
- When it cannot read the settings, every fan goes back to its automatic mode. Its log says why.
- It looks for new sensors every 30 seconds. A sensor that appears late at boot joins the plan then.
- The safety rules above apply to it too. It also sets no fan while another fan controller runs.
- It writes each new status to `/run/thermite/status.json`, and deletes the file when it stops.
- Closing the window does not stop it. `SIGTERM`, `SIGINT` and `SIGHUP` stop it, and it gives every fan back first.
- The window checks for it once per second, through that file and the process id in it.
  A file that a killed daemon left behind does not count.
- While it runs your settings file, the window starts no fan helper and shows no password dialog.
  The fan cards show the duty that the daemon set. Fan control says that the daemon applies the settings.
- When it starts while the window's fan helper runs, the window closes the helper within a second.
- When it stops, the window shows "The fan daemon stopped". The window does not ask for a password by itself.
  Apply in Fan control starts the fan helper.
- A daemon that follows another settings file counts as another fan controller: "thermite-fand controls the fans".
  That happens when `XDG_CONFIG_HOME` moves the window's settings.

#### On NixOS

The flake in this repository has a NixOS module, `services.thermite-fand`, that starts the daemon at boot.
Add it to your system flake:

```nix
{
  inputs.thermite = {
    url = "github:therecluse26/thermite";   # or "git+file:///path/to/thermite" for a local clone
    inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { nixpkgs, thermite, ... }: {
    nixosConfigurations.my-machine = nixpkgs.lib.nixosSystem {
      modules = [
        thermite.nixosModules.default
        {
          services.thermite-fand = {
            enable = true;
            user = "alice";                  # the user whose fan settings the daemon runs
          };
        }
        # ...your other modules
      ];
    };
  };
}
```

- The module builds the daemon from source with the system's `nixpkgs`. It is a glibc binary, not the static one of the packages.
  `nix build .#thermite-fand` builds the same binary: `result/bin/thermite-fand`.
- The service is `thermite-fand.service`. Its log: `journalctl -u thermite-fand`.
- The module warns when `users.users` has no user of the name in `user`.
- When `nixos-rebuild switch` brings a new daemon, systemd restarts it. The old daemon gives every fan back first.
- The unit has no order after other fan controllers. The NixOS fw-fanctrl unit starts after `multi-user.target`,
  so that order would be a cycle, and systemd would then not start the daemon at boot.
  When another fan controller also starts at boot, the daemon finds it at its next check, within 30 seconds.
- The service is hardened. Everything except `/proc`, `/sys` and `/run/thermite` is read-only to it, also the home directories.
  It has no network. Its only capability is `CAP_DAC_READ_SEARCH`, to read the settings in a home directory of mode 700.
  `systemd-analyze security thermite-fand` rates it 1.5.

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

The edit button on each chart sets custom values or hides the chart.
The app saves these choices in `~/.config/local.thermite/settings.json`.
On its first start, Thermite copies the settings of Thermal Monitor from `~/.config/local.thermal-monitor/`.

## Saved settings

Settings are keyed by sensor and fan ids. The ids avoid numbers that the kernel gives out in probe order:

- NVMe drives use their serial number; NVIDIA GPUs use their UUID.
- Devices on PCI use their PCI address. Below it, numbered names lose the number (`phy0` → `phy`).
- I²C sensors keep their address but not the bus number (`20-0050` → `i2c-0050`).
- Platform devices drop automatic instance numbers (`cros-ec-hwmon.6.auto` → `cros-ec-hwmon`).

At start-up, the app moves settings saved under the ids of earlier versions to the new ids.

## Code layout

The Rust code is a Cargo workspace in `src-tauri`, with two crates:

- `thermite-core` (`src-tauri/core`): hardware access that the window, the fan helper and the fan daemon share.
  It has no Tauri and no NVML code. It also builds the fan daemon, `thermite-fand`.
- `thermite` (`src-tauri/src`): the Tauri app.

| Path | Contents |
|---|---|
| `src-tauri/core/src/sensors.rs` | Finds hwmon sensors, names them, reads them |
| `src-tauri/core/src/fans.rs` | Finds and reads fans, drops duplicate drivers |
| `src-tauri/core/src/control.rs` | Sets fan speeds through hwmon; driver exceptions; finds other fan controllers |
| `src-tauri/core/src/curve.rs` | Fan curves: duty per temperature, hysteresis |
| `src-tauri/core/src/helper.rs` | The root fan helper and the plan it runs |
| `src-tauri/core/src/daemon.rs` | The fan daemon `thermite-fand`: follows the user's settings file, writes its status |
| `src-tauri/core/src/thresholds.rs` | Default thresholds per sensor driver |
| `src-tauri/core/src/settings.rs` | Saved overrides, hidden sensors, time window, fastest fan speeds, fan curves |
| `src-tauri/src/link.rs` | The window's end of the pipe: starts the fan helper through `pkexec`, or follows the fan daemon |
| `src-tauri/src/nvidia.rs` | NVIDIA GPU temperatures and fans through NVML |
| `src-tauri/src/monitor.rs` | 1 Hz sampler and the 30-minute history |
| `src-tauri/src/main.rs` | Tauri commands (`snapshot`, `get_settings`, `save_settings`), the `sample` event, `--fan-helper` |
| `src/chart.ts` | Canvas chart: area, lines or band; grid, threshold lines, hover crosshair |
| `src/main.ts` | Groups sensors into chart cards; threshold editor, hidden list, summary bar |
| `src/gauge.ts` | Ring gauge card shared by the top cards |
| `src/load.ts` | System thermal load card |
| `src/fans.ts` | Fan cards: share of maximum speed and spinning rotor |
| `src/fan-control.ts` | Fan control panel: Automatic, fixed level or curve, for one fan or all fans |
| `src/curve-editor.ts` | Fan curve editor: drag points, exact numbers, temperature now |
| `.github/workflows/ci.yml` | Checks for pull requests: frontend build, clippy, tests |
| `.github/workflows/release.yml` | Builds the packages and publishes the release |
| `.github/scripts/` | Chooses the next version; writes the release notes; builds the static fan daemon |
| `flake.nix`, `nix/package.nix` | The fan daemon as a Nix package |
| `nix/module.nix` | The NixOS module `services.thermite-fand` |
| `nix/test.nix` | NixOS VM test of the module |

## Tests

In `src-tauri`, `cargo test` and `cargo clippy` cover both crates of the workspace.

```sh
cd src-tauri
cargo test                                           # unit tests, against a fake /sys tree
cargo test real_sensors -- --ignored --nocapture     # prints the sensors of this machine as JSON
cargo test real_fans -- --ignored --nocapture        # prints the fans of this machine as JSON
cargo test real_monitor -- --ignored --nocapture     # NVML status, counts, and the ids that settings move between
cargo test leaves_a_sandbox -- --ignored             # needs bwrap: pkexec can become root from a sandbox
```

The flake has its own checks. They need Nix, and the VM test needs KVM:

```sh
nix build .#checks.x86_64-linux.module    # NixOS VM test of the module, about 2 minutes
nix flake check                           # that test, and the package build with the core tests
```

- The VM has no fans. So the test covers the service around the daemon, with the hardening on.
- It checks that the user can read the status file, and that the daemon reads settings in a home directory of mode 700.
  It checks that the daemon gives way to a process that looks like `fw-fanctrl run`, and that a stop removes `/run/thermite`.
- The package build skips `root_has_a_home`. The build sandbox's `/etc/passwd` gives root the home `/build`.
