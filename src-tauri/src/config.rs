//! Application configuration management.
//!
//! Provides a serializable [`AppConfig`] struct with load/save to the
//! platform configuration directory (`stay-awake/config.json`).

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

/// WiFi-based automatic activation configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WifiConfig {
    /// Whether WiFi-based activation is enabled.
    pub enabled: bool,
    /// SSIDs that trigger automatic activation.
    pub networks: Vec<String>,
}

impl Default for WifiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            networks: vec![],
        }
    }
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
    // ── WiFi ──
    #[serde(default)]
    pub wifi: WifiConfig,
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
            wifi: WifiConfig::default(),
        }
    }
}

impl AppConfig {
    /// Returns the path to the configuration file.
    fn config_path() -> Result<PathBuf, String> {
        let dir = dirs::config_dir()
            .ok_or_else(|| "Could not determine config directory".to_string())?
            .join("stay-awake");
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
    ///
    /// Uses atomic write-then-rename to prevent data loss on crash.
    pub fn save(&self) -> Result<(), String> {
        let path = Self::config_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create config directory: {}", e))?;
        }
        let contents = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, contents).map_err(|e| format!("Failed to write config: {}", e))?;
        fs::rename(&tmp, &path).map_err(|e| format!("Failed to commit config: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wifi_config_default_is_disabled_with_empty_networks() {
        let cfg = WifiConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.networks.is_empty());
    }

    #[test]
    fn app_config_without_wifi_key_deserializes_with_wifi_default() {
        // Minimal JSON that matches the current AppConfig fields but omits "wifi"
        let json = r#"{
            "mode": "Manual",
            "jiggle_mode": "MouseSubtle",
            "interval_secs": 30,
            "autostart": false,
            "language": "en",
            "global_hotkey": "CmdOrCtrl+Shift+J",
            "schedule_enabled": false,
            "schedule_start_hour": 9,
            "schedule_start_minute": 0,
            "schedule_end_hour": 17,
            "schedule_end_minute": 0,
            "schedule_days": ["mon","tue","wed","thu","fri"],
            "profiles": [],
            "active_profile": "Default"
        }"#;
        let config: AppConfig = serde_json::from_str(json).expect("deserialization must succeed");
        assert_eq!(config.wifi, WifiConfig::default());
    }

    #[test]
    fn wifi_config_round_trips_through_json() {
        let original = WifiConfig {
            enabled: true,
            networks: vec!["HomeNet".into(), "OfficeNet".into()],
        };
        let json = serde_json::to_string(&original).expect("serialize must succeed");
        let decoded: WifiConfig = serde_json::from_str(&json).expect("deserialize must succeed");
        assert_eq!(original, decoded);
    }

    #[test]
    fn app_config_with_wifi_key_round_trips_correctly() {
        let mut config = AppConfig::default();
        config.wifi = WifiConfig {
            enabled: true,
            networks: vec!["TestSSID".into()],
        };
        let json = serde_json::to_string(&config).expect("serialize must succeed");
        let decoded: AppConfig = serde_json::from_str(&json).expect("deserialize must succeed");
        assert_eq!(decoded.wifi.enabled, true);
        assert_eq!(decoded.wifi.networks, vec!["TestSSID".to_string()]);
    }
}
