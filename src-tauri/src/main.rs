// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod fans;
mod monitor;
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

use monitor::{Monitor, Snapshot, INTERVAL};
use sensors::SysPaths;
use settings::Settings;

struct AppState {
    monitor: Mutex<Monitor>,
    settings: Mutex<Settings>,
    settings_path: PathBuf,
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
    Ok(())
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
    tauri::Builder::default()
        .setup(|app| {
            let settings_path = app.path().app_config_dir()?.join("settings.json");
            let monitor = Monitor::new(SysPaths::default());
            eprintln!(
                "thermal-monitor: {} temperature sensors and {} fans found; settings at {}",
                monitor.sensor_count(),
                monitor.fan_count(),
                settings_path.display()
            );
            app.manage(AppState {
                monitor: Mutex::new(monitor),
                settings: Mutex::new(Settings::load(&settings_path)),
                settings_path,
            });
            spawn_sampler(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![snapshot, get_settings, save_settings])
        .run(tauri::generate_context!())
        .expect("error while running Thermal Monitor");
}
