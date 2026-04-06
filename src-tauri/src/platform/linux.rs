//! Linux platform implementations using xdotool and systemd-inhibit.
//!
//! Mouse control delegates to `xdotool` (X11). Wayland support is limited
//! and may require additional configuration.
//! Sleep inhibition uses `systemd-inhibit` to block the idle action.

use super::{MouseDriver, PermissionChecker, PowerInhibitor};
use std::process::{Child, Command};

// ---------------------------------------------------------------------------
// LinuxMouseDriver
// ---------------------------------------------------------------------------

/// Linux mouse driver using `xdotool` for X11 cursor control.
pub struct LinuxMouseDriver;

impl MouseDriver for LinuxMouseDriver {
    fn move_relative(&self, dx: i32, dy: i32) -> Result<(), String> {
        let status = Command::new("xdotool")
            .args(["mousemove_relative", "--", &dx.to_string(), &dy.to_string()])
            .status()
            .map_err(|e| format!("Failed to run xdotool: {}", e))?;

        if status.success() {
            Ok(())
        } else {
            Err(format!("xdotool mousemove_relative exited with {}", status))
        }
    }

    fn get_position(&self) -> Result<(i32, i32), String> {
        let output = Command::new("xdotool")
            .arg("getmouselocation")
            .output()
            .map_err(|e| format!("Failed to run xdotool: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "xdotool getmouselocation exited with {}",
                output.status
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        // Output format: "x:123 y:456 screen:0 window:12345"
        let mut x: Option<i32> = None;
        let mut y: Option<i32> = None;

        for part in stdout.split_whitespace() {
            if let Some(val) = part.strip_prefix("x:") {
                x = val.parse().ok();
            } else if let Some(val) = part.strip_prefix("y:") {
                y = val.parse().ok();
            }
        }

        match (x, y) {
            (Some(x), Some(y)) => Ok((x, y)),
            _ => Err(format!("Failed to parse xdotool output: {}", stdout)),
        }
    }

    fn jiggle_zen(&self) -> Result<(), String> {
        let status = Command::new("xdotool")
            .args(["mousemove_relative", "0", "0"])
            .status()
            .map_err(|e| format!("Failed to run xdotool: {}", e))?;

        if status.success() {
            Ok(())
        } else {
            Err(format!("xdotool mousemove_relative exited with {}", status))
        }
    }
}

// ---------------------------------------------------------------------------
// LinuxPowerInhibitor
// ---------------------------------------------------------------------------

/// Linux power-sleep inhibitor using `systemd-inhibit`.
///
/// Spawns `systemd-inhibit --what=idle ... sleep infinity` as a child process.
/// Killing the child releases the inhibition lock.
pub struct LinuxPowerInhibitor {
    child: Option<Child>,
}

impl LinuxPowerInhibitor {
    pub fn new() -> Self {
        Self { child: None }
    }
}

impl PowerInhibitor for LinuxPowerInhibitor {
    fn inhibit_sleep(&mut self, reason: &str) -> Result<(), String> {
        if self.child.is_some() {
            return Ok(());
        }

        let child = Command::new("systemd-inhibit")
            .args([
                "--what=idle",
                "--who=non-sleep",
                &format!("--why={}", reason),
                "sleep",
                "infinity",
            ])
            .spawn()
            .map_err(|e| format!("Failed to spawn systemd-inhibit: {}", e))?;

        log::info!(
            "Sleep inhibition activated via systemd-inhibit (pid: {})",
            child.id()
        );
        self.child = Some(child);
        Ok(())
    }

    fn release(&mut self) -> Result<(), String> {
        if let Some(mut child) = self.child.take() {
            child
                .kill()
                .map_err(|e| format!("Failed to kill systemd-inhibit process: {}", e))?;
            child
                .wait()
                .map_err(|e| format!("Failed to wait on systemd-inhibit process: {}", e))?;
            log::info!("Sleep inhibition released");
        }
        Ok(())
    }

    fn is_active(&self) -> bool {
        self.child.is_some()
    }
}

impl Drop for LinuxPowerInhibitor {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

// ---------------------------------------------------------------------------
// LinuxPermissionChecker
// ---------------------------------------------------------------------------

/// Linux permission checker.
///
/// On X11, `xdotool` does not require special accessibility permissions,
/// so all checks succeed unconditionally.
pub struct LinuxPermissionChecker;

impl PermissionChecker for LinuxPermissionChecker {
    fn check_accessibility(&self) -> bool {
        true
    }

    fn request_accessibility(&self) -> Result<(), String> {
        Ok(())
    }
}
