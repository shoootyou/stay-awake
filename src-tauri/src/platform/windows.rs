//! Windows platform implementations using Win32 APIs.

use super::{MouseDriver, PermissionChecker, PowerInhibitor};
use std::mem;
use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::System::Power::SetThreadExecutionState;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

// Execution-state flags for `SetThreadExecutionState`.
const ES_CONTINUOUS: u32 = 0x8000_0000;
const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;
const ES_DISPLAY_REQUIRED: u32 = 0x0000_0002;

// ---------------------------------------------------------------------------
// WinMouseDriver
// ---------------------------------------------------------------------------

/// Windows mouse driver using `SendInput` for synthetic mouse events.
pub struct WinMouseDriver;

impl WinMouseDriver {
    fn send_mouse_input(dx: i32, dy: i32) -> Result<(), String> {
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: unsafe {
                let mut u: windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT_0 =
                    mem::zeroed();
                *u.Mouse_mut() = MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_MOVE,
                    time: 0,
                    dwExtraInfo: 0,
                };
                u
            },
        };

        let sent = unsafe { SendInput(1, &input, mem::size_of::<INPUT>() as i32) };
        if sent == 0 {
            Err("SendInput failed".to_string())
        } else {
            Ok(())
        }
    }
}

impl MouseDriver for WinMouseDriver {
    fn move_relative(&self, dx: i32, dy: i32) -> Result<(), String> {
        Self::send_mouse_input(dx, dy)
    }

    fn get_position(&self) -> Result<(i32, i32), String> {
        let mut point = POINT { x: 0, y: 0 };
        let ok = unsafe { GetCursorPos(&mut point) };
        if ok == 0 {
            Err("GetCursorPos failed".to_string())
        } else {
            Ok((point.x, point.y))
        }
    }

    fn jiggle_zen(&self) -> Result<(), String> {
        Self::send_mouse_input(0, 0)
    }
}

// ---------------------------------------------------------------------------
// WinPowerInhibitor
// ---------------------------------------------------------------------------

/// Windows power-sleep inhibitor using `SetThreadExecutionState`.
///
/// Sets `ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED` to
/// prevent both system sleep and display turn-off while active.
pub struct WinPowerInhibitor {
    active: bool,
}

impl WinPowerInhibitor {
    pub fn new() -> Self {
        Self { active: false }
    }
}

impl PowerInhibitor for WinPowerInhibitor {
    fn inhibit_sleep(&mut self, _reason: &str) -> Result<(), String> {
        if self.active {
            return Ok(());
        }
        let result = unsafe {
            SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED)
        };
        if result == 0 {
            Err("SetThreadExecutionState failed".to_string())
        } else {
            self.active = true;
            log::info!("Sleep inhibition activated via SetThreadExecutionState");
            Ok(())
        }
    }

    fn release(&mut self) -> Result<(), String> {
        if !self.active {
            return Ok(());
        }
        let result = unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
        if result == 0 {
            Err("SetThreadExecutionState (release) failed".to_string())
        } else {
            self.active = false;
            log::info!("Sleep inhibition released");
            Ok(())
        }
    }

    fn is_active(&self) -> bool {
        self.active
    }
}

impl Drop for WinPowerInhibitor {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

// ---------------------------------------------------------------------------
// WinPermissionChecker
// ---------------------------------------------------------------------------

/// Windows permission checker — no special permissions are required on Windows,
/// so all checks succeed unconditionally.
pub struct WinPermissionChecker;

impl PermissionChecker for WinPermissionChecker {
    fn check_accessibility(&self) -> bool {
        true
    }

    fn request_accessibility(&self) -> Result<(), String> {
        Ok(())
    }
}
