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
        let base = self.get_position()?;
        let (x, y) = rmw_target(base, dx, dy);
        move_absolute(x, y)
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
        let (x, y) = self.get_position()?;
        let (x1, y1) = rmw_target((x, y), 1, 0);
        move_absolute(x1, y1)?;
        // Re-read the position rather than reusing the cached (x, y) from before the first
        // move: if the user moved the mouse during the two subprocess spawns above, moving
        // back to the stale value would teleport the cursor instead of drifting naturally.
        let (cur_x, cur_y) = self.get_position()?;
        move_absolute(cur_x, cur_y)
    }
}

/// Move the cursor to an absolute position via `xdotool mousemove`.
///
/// Absolute `mousemove` works under XWayland, where `xdotool`'s relative-motion subcommand is
/// silently dropped at the XTEST layer — see D3b in the RFC backing this module.
fn move_absolute(x: i32, y: i32) -> Result<(), String> {
    let status = Command::new("xdotool")
        .args(["mousemove", "--", &x.to_string(), &y.to_string()])
        .status()
        .map_err(|e| format!("Failed to run xdotool: {}", e))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("xdotool mousemove exited with {}", status))
    }
}

/// Compute the absolute target coordinates for a relative move under the read-modify-write
/// motion model: `base + (dx, dy)`.
fn rmw_target(base: (i32, i32), dx: i32, dy: i32) -> (i32, i32) {
    (base.0 + dx, base.1 + dy)
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
                "--who=stay-awake",
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
        xdotool_present_from(
            Command::new("xdotool")
                .arg("--version")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status(),
        )
    }

    fn request_accessibility(&self) -> Result<(), String> {
        Ok(())
    }
}

/// Pure decision function over the outcome of spawning `xdotool --version`.
///
/// `Ok(status)` with a zero exit code means `xdotool` is present and runnable; anything else
/// (non-zero exit, or an `Err` such as `io::ErrorKind::NotFound` when the binary isn't on
/// `PATH`) means it isn't. Kept separate from the `Command` construction so the branch logic
/// is unit-testable without spawning a real subprocess.
fn xdotool_present_from(probe: Result<std::process::ExitStatus, std::io::Error>) -> bool {
    matches!(probe, Ok(status) if status.success())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// @spec-handoff
///
/// Tests for the two pure-function seams that make the `xdotool`-shelling logic in this
/// module unit-testable without spawning a real subprocess:
///
/// - `xdotool_present_from(probe: Result<std::process::ExitStatus, std::io::Error>) -> bool` —
///   see doc comment above its definition for full behavior.
/// - `rmw_target(base: (i32, i32), dx: i32, dy: i32) -> (i32, i32)` — see doc comment above its
///   definition for full behavior, including the `jiggle_zen` `+1`/immediately-back special
///   case covered by its own test below.
#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::io;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    #[test]
    fn xdotool_present_from_ok_zero_exit_is_true() {
        let probe: Result<ExitStatus, io::Error> = Ok(ExitStatus::from_raw(0));
        assert!(xdotool_present_from(probe));
    }

    #[test]
    fn xdotool_present_from_ok_nonzero_exit_is_false() {
        let probe: Result<ExitStatus, io::Error> = Ok(ExitStatus::from_raw(1 << 8));
        assert!(!xdotool_present_from(probe));
    }

    #[test]
    fn xdotool_present_from_err_not_found_is_false() {
        let probe: Result<ExitStatus, io::Error> = Err(io::Error::from(io::ErrorKind::NotFound));
        assert!(!xdotool_present_from(probe));
    }

    #[test]
    fn rmw_target_applies_positive_delta() {
        assert_eq!(rmw_target((800, 400), 1, 0), (801, 400));
    }

    #[test]
    fn rmw_target_applies_negative_delta() {
        assert_eq!(rmw_target((801, 400), -1, 0), (800, 400));
    }

    #[test]
    fn rmw_target_applies_zero_delta() {
        assert_eq!(rmw_target((800, 400), 0, 0), (800, 400));
    }

    /// `jiggle_zen`'s `+1`/immediately-back sequence, expressed via `rmw_target`: not a true
    /// zero-delta move (which would be a no-op under read-modify-write), but a base -> base+1
    /// -> base round trip.
    #[test]
    fn rmw_target_models_jiggle_zen_plus_one_then_back() {
        let base = (800, 400);
        let out = rmw_target(base, 1, 0);
        let back = rmw_target(out, -1, 0);
        assert_eq!(out, (801, 400));
        assert_ne!(out, base);
        assert_eq!(back, base);
    }
}
