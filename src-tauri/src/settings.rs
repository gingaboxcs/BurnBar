// Persisted user settings. Lives at <app_local_data_dir>/settings.json so it
// survives across launches without depending on tauri-plugin-store.

use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tauri::Manager;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DisplayMode {
    Bar,
    Tray,
    Both,
}

impl Default for DisplayMode {
    fn default() -> Self {
        DisplayMode::Both
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BarLayout {
    Compact,
    Stacked,
}

impl Default for BarLayout {
    fn default() -> Self {
        BarLayout::Compact
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Settings {
    #[serde(default)]
    pub display_mode: DisplayMode,
    #[serde(default = "default_cap")]
    pub cap_tokens: u64,
    #[serde(default)]
    pub hide_dock_icon: bool,
    #[serde(default)]
    pub bar_position: Option<(i32, i32)>,
    /// When set, overrides our computed block_start with the user's pinned
    /// window. Active until `now > calibrated_block_start + 5h`, after which
    /// the bar reverts to the algorithm and the user is nudged to recalibrate.
    #[serde(default)]
    pub calibrated_block_start: Option<DateTime<Utc>>,
    #[serde(default)]
    pub bar_layout: BarLayout,
    #[serde(default = "default_true")]
    pub notify_thresholds: bool,
    #[serde(default = "default_true")]
    pub show_flame: bool,
    /// Show the separate Fable weekly limit on the bar when the plan has one
    /// and there's no 5-hour window (verified Ultra accounts).
    #[serde(default = "default_true")]
    pub show_fable: bool,
}

fn default_true() -> bool {
    true
}

fn default_cap() -> u64 {
    5_000_000
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            display_mode: DisplayMode::default(),
            cap_tokens: default_cap(),
            hide_dock_icon: false,
            bar_position: None,
            calibrated_block_start: None,
            bar_layout: BarLayout::default(),
            notify_thresholds: true,
            show_flame: true,
            show_fable: true,
        }
    }
}

fn settings_path(app: &tauri::AppHandle) -> Option<PathBuf> {
    let dir = app.path().app_local_data_dir().ok()?;
    let _ = fs::create_dir_all(&dir);
    Some(dir.join("settings.json"))
}

pub fn load(app: &tauri::AppHandle) -> Settings {
    let Some(path) = settings_path(app) else {
        return Settings::default();
    };
    let Ok(bytes) = fs::read(&path) else {
        return Settings::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

pub fn save(app: &tauri::AppHandle, s: &Settings) {
    let Some(path) = settings_path(app) else { return };
    if let Ok(bytes) = serde_json::to_vec_pretty(s) {
        let _ = fs::write(path, bytes);
    }
}
