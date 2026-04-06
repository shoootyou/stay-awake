//! Application configuration management.
//!
//! Provides a serializable [`AppConfig`] struct with load/save to the
//! platform configuration directory (`non-sleep/config.json`).

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Mouse jiggle pattern mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum JiggleMode {
    /// Prevent sleep via system API only — no mouse movement.
    PowerOnly,
    /// Subtle one-pixel mouse nudge (1 px right, then back).
    MouseSubtle,
    /// Zero-delta mouse event that resets the idle timer without visible movement.
    MouseZen,
    /// Small circular mouse movement pattern.
    MouseCircle,
}

/// Application activation mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AppMode {
    /// Manual activation via toggle / hotkey.
    Manual,
    /// Always active while the app is running.
    AlwaysOn,
    /// Active during a scheduled time window.
    Scheduled,
}

/// A named settings profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub jiggle_mode: JiggleMode,
    pub interval_secs: u64,
    pub schedule_enabled: bool,
    pub schedule_start_hour: u8,
    pub schedule_start_minute: u8,
    pub schedule_end_hour: u8,
    pub schedule_end_minute: u8,
    pub schedule_days: Vec<String>,
}

/// Main application configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// How the app is activated.
    pub mode: AppMode,
    /// The jiggle pattern to use.
    pub jiggle_mode: JiggleMode,
    /// Seconds between jiggle actions.
    pub interval_secs: u64,
    /// Whether to launch at system startup.
    pub autostart: bool,
    /// UI language code (e.g. `"en"`, `"es"`).
    pub language: String,
    /// Global keyboard shortcut string.
    pub global_hotkey: String,
    // ── Schedule ──
    pub schedule_enabled: bool,
    pub schedule_start_hour: u8,
    pub schedule_start_minute: u8,
    pub schedule_end_hour: u8,
    pub schedule_end_minute: u8,
    pub schedule_days: Vec<String>,
    // ── Profiles ──
    pub profiles: Vec<Profile>,
    pub active_profile: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            mode: AppMode::Manual,
            jiggle_mode: JiggleMode::MouseSubtle,
            interval_secs: 30,
            autostart: false,
            language: String::from("en"),
            global_hotkey: String::from("CmdOrCtrl+Shift+J"),
            schedule_enabled: false,
            schedule_start_hour: 9,
            schedule_start_minute: 0,
            schedule_end_hour: 17,
            schedule_end_minute: 0,
            schedule_days: vec![
                "mon".into(),
                "tue".into(),
                "wed".into(),
                "thu".into(),
                "fri".into(),
            ],
            profiles: vec![],
            active_profile: String::from("Default"),
        }
    }
}

impl AppConfig {
    /// Returns the path to the configuration file.
    fn config_path() -> Result<PathBuf, String> {
        let dir = dirs::config_dir()
            .ok_or_else(|| "Could not determine config directory".to_string())?
            .join("non-sleep");
        Ok(dir.join("config.json"))
    }

    /// Load configuration from disk, returning an error if the file is missing
    /// or malformed.
    pub fn load() -> Result<Self, String> {
        let path = Self::config_path()?;
        if !path.exists() {
            return Err("Config file not found".to_string());
        }
        let contents =
            fs::read_to_string(&path).map_err(|e| format!("Failed to read config: {}", e))?;
        serde_json::from_str(&contents).map_err(|e| format!("Failed to parse config: {}", e))
    }

    /// Save the current configuration to disk, creating the directory if needed.
    pub fn save(&self) -> Result<(), String> {
        let path = Self::config_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create config directory: {}", e))?;
        }
        let contents = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;
        fs::write(&path, contents).map_err(|e| format!("Failed to write config: {}", e))
    }
}
