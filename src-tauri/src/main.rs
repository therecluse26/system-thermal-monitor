// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod control;
mod curve;
mod fans;
mod helper;
mod monitor;
mod nvidia;
mod sensors;
mod settings;
#[cfg(test)]
mod testsys;
mod thresholds;

use std::path::PathBuf;
use std::sync::Mutex;
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Emitter, Manager, State};

use helper::{ControlState, Link, Plan};
use monitor::{Monitor, Snapshot, INTERVAL};
use sensors::SysPaths;
use settings::Settings;

struct AppState {
    monitor: Mutex<Monitor>,
    settings: Mutex<Settings>,
    settings_path: PathBuf,
    fan_helper: Mutex<Link>,
}

/// Sends the saved fan curves to the fan helper when they changed.
/// `retry` starts the helper again for unchanged curves, as `Link::apply` says.
fn sync_fan_curves(state: &AppState, retry: bool) {
    let plan = Plan::new(&state.settings.lock().unwrap(), state.monitor.lock().unwrap().sensors());
    state.fan_helper.lock().unwrap().apply(plan, &SysPaths::default().proc, retry);
}

#[tauri::command]
fn snapshot(state: State<'_, AppState>) -> Snapshot {
    state.monitor.lock().unwrap().snapshot()
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
fn save_settings(state: State<'_, AppState>, settings: Settings) -> Result<(), String> {
    settings
        .save(&state.settings_path)
        .map_err(|e| format!("Could not save {}: {e}", state.settings_path.display()))?;
    *state.settings.lock().unwrap() = settings;
    sync_fan_curves(&state, false);
    Ok(())
}

#[tauri::command]
fn fan_control(state: State<'_, AppState>) -> ControlState {
    state.fan_helper.lock().unwrap().state()
}

/// Starts the fan helper again after a cancelled password dialog or a block.
#[tauri::command]
fn retry_fan_control(state: State<'_, AppState>) {
    sync_fan_curves(&state, true);
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// Samples on a fixed schedule and pushes each sample to the UI as a "sample" event.
/// It keeps sampling while the window is hidden, so the history has no holes.
fn spawn_sampler(app: AppHandle) {
    thread::spawn(move || {
        let mut next = Instant::now();
        loop {
            let sample = app
                .state::<AppState>()
                .monitor
                .lock()
                .unwrap()
                .sample(unix_ms());
            // Fails only while the window is closing; nothing to do about it.
            let _ = app.emit("sample", &sample);

            next += INTERVAL;
            let now = Instant::now();
            if next > now {
                thread::sleep(next - now);
            } else {
                // Fell behind (e.g. after suspend): restart the schedule instead of bursting.
                next = now;
            }
        }
    });
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some(helper::HELPER_FLAG) {
        helper::main();
        return;
    }
    tauri::Builder::default()
        .setup(|app| {
            let settings_path = app.path().app_config_dir()?.join("settings.json");
            let monitor = Monitor::new(SysPaths::default());
            eprintln!(
                "thermite: {} temperature sensors and {} fans found; settings at {}",
                monitor.sensor_count(),
                monitor.fan_count(),
                settings_path.display()
            );
            let handle = app.handle().clone();
            // Thermite was called Thermal Monitor, and kept its settings here.
            let old_path = app.path().config_dir()?.join("local.thermal-monitor/settings.json");
            let (mut settings, adopted) = Settings::load_or_adopt(&settings_path, &old_path);
            let migrated = settings.migrate(&monitor.aliases());
            if adopted || migrated {
                if let Err(e) = settings.save(&settings_path) {
                    eprintln!("thermite: could not save the moved settings: {e}");
                }
            }
            app.manage(AppState {
                monitor: Mutex::new(monitor),
                settings: Mutex::new(settings),
                settings_path,
                fan_helper: Mutex::new(Link::new(move |control| {
                    // Fails only while the window is closing; nothing to do about it.
                    let _ = handle.emit("fan-control", control);
                })),
            });
            sync_fan_curves(&app.state::<AppState>(), false);
            spawn_sampler(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            snapshot,
            get_settings,
            save_settings,
            fan_control,
            retry_fan_control
        ])
        .run(tauri::generate_context!())
        .expect("error while running Thermite");
}
