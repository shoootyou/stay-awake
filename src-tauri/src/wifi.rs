//! WiFi SSID detection and monitoring for auto-activation.
//!
//! Shells out to `networksetup -getairportnetwork en0` to detect the current
//! WiFi SSID. Does not require Location Services on macOS 12+.
//!
//! The [`WifiMonitor`] uses SCDynamicStore (event-driven) on macOS, falling
//! back to a polling loop on failure or on non-macOS platforms.

use crate::config::AppConfig;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tauri::AppHandle;
use tauri::Emitter;

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

// ─────────────────────────────── Payload ───────────────────────────────────

/// Payload emitted on the `wifi-state-changed` Tauri event.
#[derive(Clone, serde::Serialize)]
pub struct WifiStatePayload {
    /// Current SSID, or `None` if disconnected.
    pub ssid: Option<String>,
    /// Whether the current SSID matches a registered network.
    pub active: bool,
}

// ───────────────────────────── WifiMonitor ─────────────────────────────────

pub struct WifiMonitor {
    config: Arc<Mutex<AppConfig>>,
    app_handle: AppHandle,
    running: Arc<AtomicBool>,
    thread_handle: Option<JoinHandle<()>>,
}

impl WifiMonitor {
    pub fn new(config: Arc<Mutex<AppConfig>>, app_handle: AppHandle) -> Self {
        Self {
            config,
            app_handle,
            running: Arc::new(AtomicBool::new(false)),
            thread_handle: None,
        }
    }

    pub fn start(&mut self) -> Result<(), String> {
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }
        self.running.store(true, Ordering::SeqCst);

        let config = Arc::clone(&self.config);
        let running = Arc::clone(&self.running);
        let app_handle = self.app_handle.clone();

        self.thread_handle = Some(thread::spawn(move || {
            if !try_event_driven_loop(&config, &running, &app_handle) {
                log::warn!("SCDynamicStore setup failed — falling back to polling");
                polling_loop(&config, &running, &app_handle);
            }
        }));

        log::info!("WifiMonitor started");
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), String> {
        if !self.running.load(Ordering::SeqCst) {
            return Ok(());
        }
        self.running.store(false, Ordering::SeqCst);

        if let Some(handle) = self.thread_handle.take() {
            handle
                .join()
                .map_err(|_| "Failed to join WiFi monitor thread".to_string())?;
        }

        log::info!("WifiMonitor stopped");
        Ok(())
    }

    pub fn restart(&mut self) -> Result<(), String> {
        self.stop()?;
        self.start()
    }
}

impl Drop for WifiMonitor {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

// ──────────────────────────── Shared logic ─────────────────────────────────

/// Detect the current SSID, check it against config, and emit a Tauri event.
fn check_and_emit(config: &Arc<Mutex<AppConfig>>, app_handle: &AppHandle) {
    let ssid = detect_current_ssid();

    let (enabled, active) = {
        let cfg = match config.lock() {
            Ok(cfg) => cfg,
            Err(_) => return,
        };
        if !cfg.wifi.enabled {
            return;
        }
        let is_registered = match &ssid {
            Some(s) => cfg.wifi.networks.iter().any(|n| n == s),
            None => false,
        };
        (true, is_registered)
    };

    if enabled {
        let payload = WifiStatePayload {
            ssid: ssid.clone(),
            active,
        };
        if let Err(e) = app_handle.emit("wifi-state-changed", payload) {
            log::error!("Failed to emit wifi-state-changed event: {}", e);
        }
        log::debug!("WiFi state: ssid={:?}, active={}", ssid, active);
    }
}

// ──────────────────────────── Polling fallback ──────────────────────────────

fn polling_loop(
    config: &Arc<Mutex<AppConfig>>,
    running: &Arc<AtomicBool>,
    app_handle: &AppHandle,
) {
    let mut last_ssid: Option<String> = None;

    while running.load(Ordering::Relaxed) {
        let current_ssid = detect_current_ssid();

        // Only emit when SSID changes
        if current_ssid != last_ssid {
            check_and_emit(config, app_handle);
            last_ssid = current_ssid;
        }

        // Sleep 15 seconds in 250 ms increments so we can react to stop() quickly.
        let total = Duration::from_secs(15);
        let step = Duration::from_millis(250);
        let mut elapsed = Duration::ZERO;
        while elapsed < total && running.load(Ordering::Relaxed) {
            thread::sleep(step);
            elapsed += step;
        }
    }
}

// ─────────────────── Event-driven path (non-macOS stub) ─────────────────────

#[cfg(not(target_os = "macos"))]
fn try_event_driven_loop(
    _config: &Arc<Mutex<AppConfig>>,
    _running: &Arc<AtomicBool>,
    _app_handle: &AppHandle,
) -> bool {
    false // Always falls back to polling on non-macOS
}

// ─────────────── Event-driven path (macOS SCDynamicStore) ───────────────────
//
// We use raw FFI against the SystemConfiguration and CoreFoundation frameworks
// directly, rather than the `system-configuration` crate.  The reason: that
// crate pins `core-foundation ^0.9`, but our project already depends on
// `core-foundation 0.10` (pulled in by `core-graphics 0.24`).  The two
// versions have incompatible wrapper types (`CFRunLoopSource`, `CFArray`, …),
// so using the high-level crate would produce type errors that cannot be
// resolved without changing unrelated platform code.  Raw FFI is the correct
// pragmatic fix — the ABI never changes.

#[cfg(target_os = "macos")]
mod sc_sys {
    use std::ffi::c_void;

    // ── Basic CF scalar types ──────────────────────────────────────────────
    pub type CFIndex = isize;
    pub type CFTimeInterval = f64;
    pub type Boolean = u8;
    pub type CFStringEncoding = u32;

    /// UTF-8 encoding constant for `CFStringCreateWithCString`.
    pub const K_CF_STRING_ENCODING_UTF8: CFStringEncoding = 0x0800_0100;

    // ── Opaque CF / SC object types ────────────────────────────────────────
    #[repr(C)]
    pub struct __CFRunLoop(c_void);
    pub type CFRunLoopRef = *mut __CFRunLoop;

    #[repr(C)]
    pub struct __CFRunLoopSource(c_void);
    pub type CFRunLoopSourceRef = *mut __CFRunLoopSource;

    #[repr(C)]
    pub struct __CFString(c_void);
    pub type CFStringRef = *const __CFString;

    #[repr(C)]
    pub struct __CFArray(c_void);
    pub type CFArrayRef = *const __CFArray;

    #[repr(C)]
    pub struct __SCDynamicStore(c_void);
    pub type SCDynamicStoreRef = *mut __SCDynamicStore;

    // ── SCDynamicStore callback ────────────────────────────────────────────

    /// Nullable C function pointer for the SCDynamicStore change callback.
    pub type SCDynamicStoreCallBack =
        Option<unsafe extern "C" fn(SCDynamicStoreRef, CFArrayRef, *mut c_void)>;

    /// Context passed to `SCDynamicStoreCreate`.
    #[repr(C)]
    pub struct SCDynamicStoreContext {
        pub version: CFIndex,
        pub info: *mut c_void,
        pub retain: Option<unsafe extern "C" fn(*const c_void) -> *const c_void>,
        pub release: Option<unsafe extern "C" fn(*const c_void)>,
        pub copy_description: Option<unsafe extern "C" fn(*const c_void) -> CFStringRef>,
    }

    // ── CoreFoundation framework bindings ─────────────────────────────────

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        /// Pass this to CF allocation functions to use the default allocator
        /// (equivalent to passing `NULL`; in practice `NULL` also works fine).
        #[allow(dead_code)]
        pub static kCFAllocatorDefault: *const c_void;

        /// The run-loop mode constant for the default mode.
        pub static kCFRunLoopDefaultMode: CFStringRef;

        /// Standard CF retain/release callbacks for arrays of CF objects.
        /// Declared as `u8` so we can take its address without caring about
        /// the internal struct layout.
        pub static kCFTypeArrayCallBacks: u8;

        pub fn CFStringCreateWithCString(
            alloc: *const c_void,
            c_str: *const u8,
            encoding: CFStringEncoding,
        ) -> CFStringRef;

        pub fn CFArrayCreate(
            allocator: *const c_void,
            values: *const *const c_void,
            num_values: CFIndex,
            call_backs: *const u8,
        ) -> CFArrayRef;

        pub fn CFRunLoopGetCurrent() -> CFRunLoopRef;

        pub fn CFRunLoopAddSource(
            rl: CFRunLoopRef,
            source: CFRunLoopSourceRef,
            mode: CFStringRef,
        );

        pub fn CFRunLoopRunInMode(
            mode: CFStringRef,
            seconds: CFTimeInterval,
            return_after_source_handled: Boolean,
        ) -> i32;

        pub fn CFRelease(cf: *const c_void);
    }

    // ── SystemConfiguration framework bindings ────────────────────────────

    #[link(name = "SystemConfiguration", kind = "framework")]
    extern "C" {
        pub fn SCDynamicStoreCreate(
            allocator: *const c_void,
            name: CFStringRef,
            callout: SCDynamicStoreCallBack,
            context: *mut SCDynamicStoreContext,
        ) -> SCDynamicStoreRef;

        pub fn SCDynamicStoreSetNotificationKeys(
            store: SCDynamicStoreRef,
            keys: CFArrayRef,
            patterns: CFArrayRef,
        ) -> Boolean;

        pub fn SCDynamicStoreCreateRunLoopSource(
            allocator: *const c_void,
            store: SCDynamicStoreRef,
            order: CFIndex,
        ) -> CFRunLoopSourceRef;
    }
}

/// Context data shared between the event-driven loop and the SC callback.
/// Heap-allocated and passed as a raw `*mut c_void` through the C boundary.
#[cfg(target_os = "macos")]
struct WifiCallbackData {
    config: Arc<Mutex<AppConfig>>,
    app_handle: AppHandle,
}

/// SAFETY: `WifiCallbackData` is only accessed on the single monitor thread
/// that owns the run loop, so no data race can occur.
#[cfg(target_os = "macos")]
unsafe impl Send for WifiCallbackData {}

/// Raw C callback invoked by SCDynamicStore when watched keys change.
#[cfg(target_os = "macos")]
unsafe extern "C" fn wifi_change_callback(
    _store: sc_sys::SCDynamicStoreRef,
    _changed_keys: sc_sys::CFArrayRef,
    info: *mut std::ffi::c_void,
) {
    if info.is_null() {
        return;
    }
    // SAFETY: `info` was cast from `&WifiCallbackData` that lives for the
    // entire duration of `try_event_driven_loop` (guaranteed by the loop
    // structure — we only release the store after the run loop exits).
    let data = &*(info as *const WifiCallbackData);
    check_and_emit(&data.config, &data.app_handle);
}

/// Try to run an SCDynamicStore-based event loop on the current thread.
/// Returns `true` if the loop ran to completion (i.e. `running` went false),
/// or `false` if setup failed (caller should fall back to polling).
#[cfg(target_os = "macos")]
fn try_event_driven_loop(
    config: &Arc<Mutex<AppConfig>>,
    running: &Arc<AtomicBool>,
    app_handle: &AppHandle,
) -> bool {
    use std::ffi::c_void;
    use sc_sys::*;

    // ── Callback data (lives for the full duration of this function) ──────
    let callback_data = WifiCallbackData {
        config: Arc::clone(config),
        app_handle: app_handle.clone(),
    };
    let mut context = SCDynamicStoreContext {
        version: 0,
        info: &callback_data as *const WifiCallbackData as *mut c_void,
        retain: None,
        release: None,
        copy_description: None,
    };

    // ── Create the SCDynamicStore ─────────────────────────────────────────
    let store_name = b"stay-awake-wifi\0";
    let name_cf = unsafe {
        CFStringCreateWithCString(
            std::ptr::null(),
            store_name.as_ptr(),
            K_CF_STRING_ENCODING_UTF8,
        )
    };
    if name_cf.is_null() {
        log::error!("CFStringCreateWithCString failed for store name");
        return false;
    }

    let store = unsafe {
        SCDynamicStoreCreate(
            std::ptr::null(),
            name_cf,
            Some(wifi_change_callback),
            &mut context,
        )
    };
    unsafe { CFRelease(name_cf as *const c_void) };

    if store.is_null() {
        log::error!("SCDynamicStoreCreate returned null");
        return false;
    }

    // ── Register notification keys ────────────────────────────────────────
    // Watch keys: empty (we don't care about exact key values, only patterns).
    let keys_array = unsafe {
        CFArrayCreate(
            std::ptr::null(),
            std::ptr::null(),
            0,
            &kCFTypeArrayCallBacks as *const u8,
        )
    };

    // Pattern matches any AirPort interface state change.
    let pattern_str = b"State:/Network/Interface/.*/AirPort\0";
    let pattern_cf = unsafe {
        CFStringCreateWithCString(
            std::ptr::null(),
            pattern_str.as_ptr(),
            K_CF_STRING_ENCODING_UTF8,
        )
    };
    let patterns_array = if !pattern_cf.is_null() {
        let val = pattern_cf as *const c_void;
        unsafe {
            CFArrayCreate(
                std::ptr::null(),
                &val as *const *const c_void,
                1,
                &kCFTypeArrayCallBacks as *const u8,
            )
        }
    } else {
        std::ptr::null()
    };

    let registered = !keys_array.is_null()
        && !patterns_array.is_null()
        && unsafe { SCDynamicStoreSetNotificationKeys(store, keys_array, patterns_array) != 0 };

    if !keys_array.is_null() {
        unsafe { CFRelease(keys_array as *const c_void) };
    }
    if !pattern_cf.is_null() {
        unsafe { CFRelease(pattern_cf as *const c_void) };
    }
    if !patterns_array.is_null() {
        unsafe { CFRelease(patterns_array as *const c_void) };
    }

    if !registered {
        log::error!("SCDynamicStoreSetNotificationKeys failed");
        unsafe { CFRelease(store as *const c_void) };
        return false;
    }

    // ── Attach store to the current thread's run loop ─────────────────────
    let source = unsafe { SCDynamicStoreCreateRunLoopSource(std::ptr::null(), store, 0) };
    if source.is_null() {
        log::error!("SCDynamicStoreCreateRunLoopSource returned null");
        unsafe { CFRelease(store as *const c_void) };
        return false;
    }

    let run_loop = unsafe { CFRunLoopGetCurrent() };
    unsafe { CFRunLoopAddSource(run_loop, source, kCFRunLoopDefaultMode) };

    log::info!("WifiMonitor: SCDynamicStore event loop active");

    // Initial probe so the frontend gets state immediately on startup.
    check_and_emit(config, app_handle);

    // ── Run loop — 1 s slices so we can react to stop() quickly ──────────
    while running.load(Ordering::Relaxed) {
        // Returns after the timeout or after processing a source (our callback).
        unsafe {
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, 1.0, 0);
        }
    }

    // ── Cleanup ───────────────────────────────────────────────────────────
    unsafe {
        CFRelease(source as *const c_void);
        CFRelease(store as *const c_void);
    }

    true
}

// ───────────────────────────────── Tests ───────────────────────────────────

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
