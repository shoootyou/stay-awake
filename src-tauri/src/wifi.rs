//! WiFi SSID detection for auto-activation.
//!
//! Shells out to `networksetup -getairportnetwork en0` to detect the current
//! WiFi SSID. Does not require Location Services on macOS 12+.

use std::process::Command;

/// Parse the output of `networksetup -getairportnetwork en0`.
/// Returns the SSID if connected, or `None` if disconnected or unrecognised.
fn parse_ssid_output(output: &str) -> Option<String> {
    // Expected: "Current Wi-Fi Network: MySSID\n"
    // Disconnected: "You are not associated with an AirPort network.\n"
    output
        .strip_prefix("Current Wi-Fi Network: ")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Detect the current WiFi SSID by shelling out to `networksetup`.
/// Returns `None` if WiFi is disconnected or the command fails.
pub fn detect_current_ssid() -> Option<String> {
    let output = Command::new("networksetup")
        .args(["-getairportnetwork", "en0"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_ssid_output(&stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_connected_ssid() {
        assert_eq!(
            parse_ssid_output("Current Wi-Fi Network: OfficeWiFi\n"),
            Some("OfficeWiFi".to_string())
        );
    }

    #[test]
    fn parse_disconnected() {
        assert_eq!(
            parse_ssid_output("You are not associated with an AirPort network.\n"),
            None
        );
    }

    #[test]
    fn parse_empty_output() {
        assert_eq!(parse_ssid_output(""), None);
    }

    #[test]
    fn parse_ssid_with_spaces() {
        assert_eq!(
            parse_ssid_output("Current Wi-Fi Network: My Home Network\n"),
            Some("My Home Network".to_string())
        );
    }
}
