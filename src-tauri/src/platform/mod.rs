//! Platform abstraction layer for Non-Sleep.
//!
//! Provides cross-platform traits for mouse control, power management,
//! and permission checking, with platform-specific implementations.

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "windows")]
pub mod windows;

/// Driver for controlling mouse cursor position and generating movement events.
pub trait MouseDriver: Send + Sync {
    /// Move the mouse cursor by a relative offset.
    fn move_relative(&self, dx: i32, dy: i32) -> Result<(), String>;

    /// Get the current absolute cursor position.
    fn get_position(&self) -> Result<(i32, i32), String>;

    /// Perform a zero-delta mouse movement to reset the system idle timer
    /// without visually moving the cursor.
    fn jiggle_zen(&self) -> Result<(), String>;
}

/// Controls system sleep and display sleep inhibition.
pub trait PowerInhibitor: Send + Sync {
    /// Prevent the system from sleeping, with the given reason.
    fn inhibit_sleep(&mut self, reason: &str) -> Result<(), String>;

    /// Release the sleep inhibition.
    fn release(&mut self) -> Result<(), String>;

    /// Returns true if sleep inhibition is currently active.
    #[allow(dead_code)]
    fn is_active(&self) -> bool;
}

/// Checks and requests platform-specific permissions needed for operation.
pub trait PermissionChecker: Send + Sync {
    /// Check if the application has accessibility permissions.
    fn check_accessibility(&self) -> bool;

    /// Request accessibility permissions from the user.
    fn request_accessibility(&self) -> Result<(), String>;
}

/// Creates the platform-specific mouse driver.
pub fn create_mouse_driver() -> Box<dyn MouseDriver> {
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::MacMouseDriver)
    }

    #[cfg(target_os = "windows")]
    {
        Box::new(windows::WinMouseDriver)
    }
}

/// Creates the platform-specific power inhibitor.
pub fn create_power_inhibitor() -> Box<dyn PowerInhibitor> {
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::MacPowerInhibitor::new())
    }

    #[cfg(target_os = "windows")]
    {
        Box::new(windows::WinPowerInhibitor::new())
    }
}

/// Creates the platform-specific permission checker.
pub fn create_permission_checker() -> Box<dyn PermissionChecker> {
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::MacPermissionChecker)
    }

    #[cfg(target_os = "windows")]
    {
        Box::new(windows::WinPermissionChecker)
    }
}
