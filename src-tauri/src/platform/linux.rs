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
        jiggle_zen_with(|| self.get_position(), move_absolute)
    }
}

/// The `jiggle_zen` sequence with the position-reader and move-sink injected, so the
/// return-move arithmetic is unit-testable without spawning `xdotool`.
///
/// Re-reads the position *after* the `+1` move rather than reusing the position captured
/// before it: if the user moved the mouse during the two subprocess spawns, this closes the
/// TOCTOU window that would otherwise teleport the cursor back to a stale pre-move value.
/// Critically, the return move must undo *this function's own* `+1` delta relative to that
/// fresh read (`fresh - 1`) — it must NOT move to the fresh position unchanged, which would
/// be an unconditional no-op that leaves the cursor permanently displaced `+1px` per call.
fn jiggle_zen_with<F, M>(get_pos: F, move_to: M) -> Result<(), String>
where
    F: Fn() -> Result<(i32, i32), String>,
    M: Fn(i32, i32) -> Result<(), String>,
{
    let (x, y) = get_pos()?;
    let (x1, y1) = rmw_target((x, y), 1, 0);
    move_to(x1, y1)?;
    let fresh = get_pos()?;
    let (back_x, back_y) = rmw_target(fresh, -1, 0);
    move_to(back_x, back_y)
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
/// Tests for the pure-function seams that make the `xdotool`-shelling logic in this
/// module unit-testable without spawning a real subprocess:
///
/// - `xdotool_present_from(probe: Result<std::process::ExitStatus, std::io::Error>) -> bool` —
///   see doc comment above its definition for full behavior.
/// - `rmw_target(base: (i32, i32), dx: i32, dy: i32) -> (i32, i32)` — see doc comment above its
///   definition for full behavior, including the `jiggle_zen` `+1`/immediately-back special
///   case covered by its own test below.
/// - `jiggle_zen_with(get_pos, move_to) -> Result<(), String>` — the `jiggle_zen` sequence
///   with its position-reader and move-sink injected. Tests assert the exact final resting
///   position after a fresh mid-sequence read, distinguishing a correct undo (`fresh - 1`)
///   from both known regressions: a no-op undo (`fresh` unchanged) and a stale-cache undo
///   (the pre-move position, ignoring the fresh read entirely).
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

    /// Regression test for the round-1/round-2 `jiggle_zen` TOCTOU saga.
    ///
    /// Simulates the exact scenario both bugs mishandled: the user nudges the mouse in the
    /// gap between `jiggle_zen`'s first `move_absolute` and its second `get_position` call, so
    /// the fresh read (`850, 400`) genuinely differs from the pre-move cached position
    /// (`800, 400`). This distinguishes all three implementations, which would otherwise be
    /// indistinguishable under a test that only asserts "some move happened":
    ///
    /// - **Original TOCTOU bug** (pre-`c8f1161`): moved back to the *stale cached* position
    ///   `(800, 400)` — a real teleport, ignoring the user's nudge entirely.
    /// - **No-op regression** (`c8f1161`, this round's finding): moved back to the *fresh*
    ///   position `(850, 400)` unchanged — a guaranteed no-op that leaves the `+1` this
    ///   function itself applied permanently in place.
    /// - **Correct fix**: moves back to `fresh - 1 = (849, 400)` — undoing exactly this
    ///   function's own delta relative to the fresh read, closing the TOCTOU window without
    ///   either teleporting or drifting.
    ///
    /// Only the correct fix's final move lands on `(849, 400)`; both bugs land elsewhere.
    #[test]
    fn jiggle_zen_with_undoes_its_own_delta_relative_to_a_fresh_read() {
        let call_count = std::cell::Cell::new(0);
        let get_pos = || -> Result<(i32, i32), String> {
            let n = call_count.get();
            call_count.set(n + 1);
            // First read: pre-move cached position. Second read: fresh position after the
            // user nudged the mouse during the first `move_absolute` subprocess spawn.
            Ok(if n == 0 { (800, 400) } else { (850, 400) })
        };

        let moves = std::cell::RefCell::new(Vec::new());
        let move_to = |x: i32, y: i32| -> Result<(), String> {
            moves.borrow_mut().push((x, y));
            Ok(())
        };

        jiggle_zen_with(get_pos, move_to).expect("jiggle_zen_with should succeed");

        let recorded = moves.borrow();
        assert_eq!(
            recorded.len(),
            2,
            "expected exactly two moves: +1, then undo"
        );
        assert_eq!(
            recorded[0],
            (801, 400),
            "first move applies +1 to the pre-move position"
        );

        let final_move = recorded[1];
        assert_ne!(
            final_move,
            (800, 400),
            "must not undo to the stale cached pre-move position (the original TOCTOU bug)"
        );
        assert_ne!(
            final_move,
            (850, 400),
            "must not undo to the fresh position unchanged (the no-op regression)"
        );
        assert_eq!(
            final_move,
            (849, 400),
            "must undo exactly this function's own +1 delta relative to the fresh read"
        );
    }
}
