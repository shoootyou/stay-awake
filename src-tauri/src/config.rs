//! Application configuration management.
//!
//! Provides a serializable [`AppConfig`] struct with load/save to the
//! platform configuration directory (`stay-awake/config.json`).

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};

static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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
    /// Activates automatically when connected to a registered WiFi network.
    WiFi,
}

/// WiFi-based automatic activation configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct WifiConfig {
    /// Whether WiFi-based activation is enabled.
    pub enabled: bool,
    /// SSIDs that trigger automatic activation.
    pub networks: Vec<String>,
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
    pub schedule_enabled: bool,
    pub schedule_start_hour: u8,
    pub schedule_start_minute: u8,
    pub schedule_end_hour: u8,
    pub schedule_end_minute: u8,
    pub schedule_days: Vec<String>,
    pub profiles: Vec<Profile>,
    pub active_profile: String,
    #[serde(default)]
    pub wifi: WifiConfig,
}

/// Default `jiggle_mode` for a fresh `AppConfig`.
///
/// Wayland mouse injection has not been shown to reset the idle timer, so use the verified
/// systemd-inhibit path until a real-hardware re-test proves otherwise.
#[cfg(target_os = "linux")]
fn default_jiggle_mode() -> JiggleMode {
    JiggleMode::PowerOnly
}

/// Default `jiggle_mode` for a fresh `AppConfig` on non-Linux targets.
#[cfg(not(target_os = "linux"))]
fn default_jiggle_mode() -> JiggleMode {
    JiggleMode::MouseSubtle
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            mode: AppMode::Manual,
            jiggle_mode: default_jiggle_mode(),
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
    /// Returns `true` when WiFi-based engine control is active.
    /// Derived from `mode` — the `wifi.enabled` field is retained only for
    /// config backward-compatibility and MUST NOT be used as a runtime gate.
    pub fn wifi_enabled(&self) -> bool {
        self.mode == AppMode::WiFi
    }

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
        self.save_to(&path)
    }

    /// Saves to a caller-supplied path so tests can avoid the user configuration directory.
    pub(crate) fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            create_config_dir(parent)?;
        }
        let contents = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;
        let tmp = temp_config_path(path);
        write_private_file(&tmp, contents.as_bytes())?;
        fs::rename(&tmp, path).map_err(|e| format!("Failed to commit config: {}", e))
    }
}

fn create_config_dir(path: &Path) -> Result<(), String> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder
        .create(path)
        .map_err(|error| format!("Failed to create config directory: {error}"))?;
    #[cfg(unix)]
    harden_config_dir(path)?;
    Ok(())
}

#[cfg(unix)]
fn harden_config_dir(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Failed to inspect config directory: {error}"))?;
    if metadata.file_type().is_symlink() {
        return Err("Refusing to use a symlinked config directory".to_owned());
    }
    if !metadata.is_dir() {
        return Err("Config directory path is not a directory".to_owned());
    }
    let current_uid = current_uid()?;
    if metadata.uid() != current_uid {
        return Err("Refusing to use a config directory owned by another user".to_owned());
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("Failed to secure config directory: {error}"))
}

#[cfg(unix)]
fn current_uid() -> Result<u32, String> {
    let output = std::process::Command::new("/usr/bin/id")
        .arg("-u")
        .output()
        .map_err(|error| format!("Failed to determine current user: {error}"))?;
    if !output.status.success() {
        return Err("Failed to determine current user".to_owned());
    }
    std::str::from_utf8(&output.stdout)
        .map_err(|error| format!("Current user ID was not UTF-8: {error}"))?
        .trim()
        .parse()
        .map_err(|error| format!("Current user ID was invalid: {error}"))
}

fn temp_config_path(path: &Path) -> PathBuf {
    let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    path.with_extension(format!("json.{}.{}.tmp", std::process::id(), sequence))
}

fn write_private_file(path: &Path, contents: &[u8]) -> Result<(), String> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(path)
        .map_err(|error| format!("Failed to write config: {error}"))?;
    file.write_all(contents)
        .map_err(|error| format!("Failed to write config: {error}"))
}

#[cfg(test)]
mod tests {
    /*
     * @spec-handoff
     * @interface AppConfig JSON serialization and runtime::effective_mode(&AppMode)
     * @behavior
     *   - Legacy Scheduled JSON remains persisted as Scheduled and runs as Manual.
     *   - A default config retains the established persisted key set.
     *   - Legacy JSON without `wifi` loads with the disabled empty WiFi default.
     *   - Unknown JSON fields are ignored by the current migration contract.
     * @edge-cases
     *   - No runtime-only field is persisted.
     */
    use super::*;

    #[test]
    fn legacy_scheduled_json_remains_persisted_and_uses_manual_runtime_behavior() {
        let json = r#"{
            "mode":"Scheduled", "jiggle_mode":"MouseSubtle", "interval_secs":30,
            "autostart":false, "language":"en", "global_hotkey":"CmdOrCtrl+Shift+J",
            "schedule_enabled":true, "schedule_start_hour":9, "schedule_start_minute":0,
            "schedule_end_hour":17, "schedule_end_minute":0, "schedule_days":["mon"],
            "profiles":[], "active_profile":"Default", "wifi":{"enabled":false,"networks":[]}
        }"#;
        let config: AppConfig = serde_json::from_str(json).expect("legacy shape must deserialize");
        assert_eq!(config.mode, AppMode::Scheduled);
        assert_eq!(
            serde_json::to_value(&config)
                .expect("legacy config must serialize")
                .get("mode"),
            Some(&serde_json::Value::String("Scheduled".to_owned()))
        );
        assert_eq!(
            crate::runtime::effective_mode(&config.mode),
            AppMode::Manual
        );
    }

    #[test]
    fn default_config_serializes_to_the_established_persisted_key_set() {
        let serialized =
            serde_json::to_value(AppConfig::default()).expect("default config serializes");
        let mut keys = serialized
            .as_object()
            .expect("config serializes as an object")
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "active_profile",
                "autostart",
                "global_hotkey",
                "interval_secs",
                "jiggle_mode",
                "language",
                "mode",
                "profiles",
                "schedule_days",
                "schedule_enabled",
                "schedule_end_hour",
                "schedule_end_minute",
                "schedule_start_hour",
                "schedule_start_minute",
                "wifi",
            ]
        );
    }

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
        let config = AppConfig {
            wifi: WifiConfig {
                enabled: true,
                networks: vec!["TestSSID".into()],
            },
            ..AppConfig::default()
        };
        let json = serde_json::to_string(&config).expect("serialize must succeed");
        let decoded: AppConfig = serde_json::from_str(&json).expect("deserialize must succeed");
        assert!(decoded.wifi.enabled);
        assert_eq!(decoded.wifi.networks, vec!["TestSSID".to_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn save_to_creates_private_paths_and_replaces_the_previous_config_atomically() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "stay-awake-config-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock is after the Unix epoch")
                .as_nanos()
        ));
        let path = root.join("private").join("config.json");
        let mut config = AppConfig {
            language: "en".to_owned(),
            ..AppConfig::default()
        };
        config.save_to(&path).expect("first private save succeeds");

        assert_eq!(
            std::fs::metadata(path.parent().expect("config parent"))
                .expect("config directory metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&path)
                .expect("config file metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        config.language = "es".to_owned();
        config.save_to(&path).expect("replacement save succeeds");
        assert!(
            std::fs::read_to_string(&path)
                .expect("replacement config reads")
                .contains("\"es\""),
            "replacement makes the complete new config visible"
        );
        assert_eq!(
            std::fs::metadata(&path)
                .expect("replacement file metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::read_dir(path.parent().expect("config parent"))
                .expect("config directory reads")
                .count(),
            1,
            "atomic replacement leaves no temporary file behind"
        );
        std::fs::remove_dir_all(root).expect("test directory cleanup succeeds");
    }

    #[test]
    fn wifi_enabled_returns_true_only_for_wifi_mode() {
        for mode in [AppMode::Manual, AppMode::AlwaysOn, AppMode::Scheduled] {
            let cfg = AppConfig {
                mode,
                ..Default::default()
            };
            assert!(!cfg.wifi_enabled());
        }
        let cfg = AppConfig {
            mode: AppMode::WiFi,
            ..Default::default()
        };
        assert!(cfg.wifi_enabled());
    }

    #[test]
    fn app_mode_wifi_round_trips_through_json() {
        let cfg = AppConfig {
            mode: AppMode::WiFi,
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).expect("serialize must succeed");
        assert!(
            json.contains("\"WiFi\""),
            "WiFi variant must serialize as \"WiFi\""
        );
        let decoded: AppConfig = serde_json::from_str(&json).expect("deserialize must succeed");
        assert_eq!(decoded.mode, AppMode::WiFi);
        assert!(decoded.wifi_enabled());
    }

    /// E8 fallback: on Linux, `AppConfig::default()` must default `jiggle_mode` to
    /// `PowerOnly` (A1 — Wayland idle-timer reset — was left inconclusive), while all other
    /// targets keep the original `MouseSubtle` default.
    #[test]
    fn default_jiggle_mode_is_power_only_on_linux_and_mouse_subtle_elsewhere() {
        let cfg = AppConfig::default();
        #[cfg(target_os = "linux")]
        assert_eq!(cfg.jiggle_mode, JiggleMode::PowerOnly);
        #[cfg(not(target_os = "linux"))]
        assert_eq!(cfg.jiggle_mode, JiggleMode::MouseSubtle);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_default_config_matches_the_serialization_golden() {
        let actual = serde_json::to_string_pretty(&AppConfig::default())
            .expect("default config must serialize");
        assert_eq!(
            actual,
            include_str!("../tests/fixtures/default-config-linux.json").trim_end()
        );
    }

    #[test]
    fn unknown_config_fields_are_ignored_during_current_migration() {
        let json = r#"{
            "mode":"Manual", "jiggle_mode":"MouseSubtle", "interval_secs":30,
            "autostart":false, "language":"en", "global_hotkey":"CmdOrCtrl+Shift+J",
            "schedule_enabled":false, "schedule_start_hour":9, "schedule_start_minute":0,
            "schedule_end_hour":17, "schedule_end_minute":0,
            "schedule_days":[], "profiles":[], "active_profile":"Default",
            "future_setting":"ignored"
        }"#;
        let config: AppConfig = serde_json::from_str(json).expect("unknown fields are ignored");
        assert_eq!(config.wifi, WifiConfig::default());
    }

    #[test]
    fn unknown_jiggle_mode_is_rejected() {
        let json = r#"{
            "mode":"Manual", "jiggle_mode":"FutureMode", "interval_secs":30,
            "autostart":false, "language":"en", "global_hotkey":"CmdOrCtrl+Shift+J",
            "schedule_enabled":false, "schedule_start_hour":9, "schedule_start_minute":0,
            "schedule_end_hour":17, "schedule_end_minute":0,
            "schedule_days":[], "profiles":[], "active_profile":"Default"
        }"#;
        assert!(serde_json::from_str::<AppConfig>(json).is_err());
    }
}
