//! macOS platform implementations using Core Graphics, IOKit, and Accessibility APIs.

use super::{MouseDriver, PermissionChecker, PowerInhibitor};
use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use core_graphics::event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;
use std::ffi::c_void;

// ---------------------------------------------------------------------------
// IOKit Power Management FFI
// ---------------------------------------------------------------------------

const K_IO_RETURN_SUCCESS: i32 = 0;
const K_IOPM_ASSERTION_LEVEL_ON: u32 = 255;

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOPMAssertionCreateWithName(
        assertion_type: *const c_void,
        assertion_level: u32,
        assertion_name: *const c_void,
        assertion_id: *mut u32,
    ) -> i32;

    fn IOPMAssertionRelease(assertion_id: u32) -> i32;
}

// ---------------------------------------------------------------------------
// Accessibility FFI
// ---------------------------------------------------------------------------

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

// ---------------------------------------------------------------------------
// MacMouseDriver
// ---------------------------------------------------------------------------

/// macOS mouse driver using Core Graphics event system.
///
/// Creates `CGEvent` mouse-moved events and posts them to the HID event tap
/// to simulate cursor movement and reset the system idle timer.
pub struct MacMouseDriver;

impl MacMouseDriver {
    fn create_source() -> Result<CGEventSource, String> {
        CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|_| "Failed to create CGEventSource".to_string())
    }

    fn post_mouse_moved(dest: CGPoint) -> Result<(), String> {
        let source = Self::create_source()?;
        let event =
            CGEvent::new_mouse_event(source, CGEventType::MouseMoved, dest, CGMouseButton::Left)
                .map_err(|_| "Failed to create mouse-moved event".to_string())?;
        event.post(CGEventTapLocation::HID);
        Ok(())
    }
}

impl MouseDriver for MacMouseDriver {
    fn move_relative(&self, dx: i32, dy: i32) -> Result<(), String> {
        let (x, y) = self.get_position()?;
        let dest = CGPoint::new((x + dx) as f64, (y + dy) as f64);
        Self::post_mouse_moved(dest)
    }

    fn get_position(&self) -> Result<(i32, i32), String> {
        let source = Self::create_source()?;
        let event = CGEvent::new(source)
            .map_err(|_| "Failed to create CGEvent for position query".to_string())?;
        let point = event.location();
        Ok((point.x as i32, point.y as i32))
    }

    fn jiggle_zen(&self) -> Result<(), String> {
        let (x, y) = self.get_position()?;
        Self::post_mouse_moved(CGPoint::new(x as f64, y as f64))
    }
}

// ---------------------------------------------------------------------------
// MacPowerInhibitor
// ---------------------------------------------------------------------------

/// macOS power-sleep inhibitor backed by IOKit power assertions.
///
/// Calls `IOPMAssertionCreateWithName` with `PreventUserIdleSystemSleep` to
/// keep the system (and display) awake. The assertion is automatically
/// released on [`Drop`].
pub struct MacPowerInhibitor {
    assertion_id: Option<u32>,
}

impl MacPowerInhibitor {
    pub fn new() -> Self {
        Self { assertion_id: None }
    }
}

impl PowerInhibitor for MacPowerInhibitor {
    fn inhibit_sleep(&mut self, reason: &str) -> Result<(), String> {
        if self.assertion_id.is_some() {
            return Ok(());
        }

        let assertion_type = CFString::new("PreventUserIdleSystemSleep");
        let assertion_name = CFString::new(reason);
        let mut assertion_id: u32 = 0;

        let result = unsafe {
            IOPMAssertionCreateWithName(
                assertion_type.as_concrete_TypeRef() as *const c_void,
                K_IOPM_ASSERTION_LEVEL_ON,
                assertion_name.as_concrete_TypeRef() as *const c_void,
                &mut assertion_id,
            )
        };

        if result == K_IO_RETURN_SUCCESS {
            self.assertion_id = Some(assertion_id);
            log::info!(
                "Sleep inhibition activated (assertion ID: {})",
                assertion_id
            );
            Ok(())
        } else {
            Err(format!(
                "IOPMAssertionCreateWithName failed with code: {}",
                result
            ))
        }
    }

    fn release(&mut self) -> Result<(), String> {
        if let Some(id) = self.assertion_id.take() {
            let result = unsafe { IOPMAssertionRelease(id) };
            if result != K_IO_RETURN_SUCCESS {
                return Err(format!("IOPMAssertionRelease failed with code: {}", result));
            }
            log::info!("Sleep inhibition released");
        }
        Ok(())
    }

    fn is_active(&self) -> bool {
        self.assertion_id.is_some()
    }
}

impl Drop for MacPowerInhibitor {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

// ---------------------------------------------------------------------------
// MacPermissionChecker
// ---------------------------------------------------------------------------

/// macOS accessibility permission checker.
///
/// Uses `AXIsProcessTrusted()` to test whether the app has been granted
/// Accessibility access in System Settings → Privacy & Security.
pub struct MacPermissionChecker;

impl PermissionChecker for MacPermissionChecker {
    fn check_accessibility(&self) -> bool {
        unsafe { AXIsProcessTrusted() }
    }

    fn request_accessibility(&self) -> Result<(), String> {
        // Open System Settings at the Privacy → Accessibility pane.
        std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
            .spawn()
            .map_err(|e| format!("Failed to open System Settings: {}", e))?;
        Ok(())
    }
}
