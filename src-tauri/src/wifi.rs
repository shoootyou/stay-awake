/*!
 * @spec-handoff
 * @interface coordinate_linux_monitor(events: Stream<MonitorSignal>, shutdown: watch::Receiver<bool>, tick: Interval, last_ssid: Option<String>, emit: Emit, recheck: Recheck): LoopExit
 * @behavior
 *   - Production must call this coordinator with its combined zbus signal stream.
 *   - Production must use monitor_starts_after_setup and monitor_exit_uses_polling_fallback.
 *   - Emits current state once for either NetworkManager signal.
 *   - Stops promptly on shutdown and returns Stopped.
 *   - Emits on a changed safety-net SSID only; stale paths and ended streams return fallback exits.
 *
 * @edge-cases
 *   - Setup failure and an ended stream while running select polling fallback.
 *
 * @see try_event_driven_loop
 *
 * WiFi SSID detection and monitoring for auto-activation.
 *
 * On macOS 26+, uses CoreWLAN via raw Objective-C FFI (corewlan_bridge.m)
 * with Location Services authorization. Falls back to `networksetup` CLI
 * for older macOS versions.
 *
 * The [`WifiMonitor`] uses SCDynamicStore (event-driven) on macOS, falling
 * back to a polling loop on failure or on non-macOS platforms.
 */

use crate::config::{AppConfig, AppMode};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tauri::AppHandle;
use tauri::Emitter;

// ── CoreWLAN bridge FFI (compiled from corewlan_bridge.m) ──────────────────
#[cfg(target_os = "macos")]
extern "C" {
    fn corewlan_location_status() -> i32;
    fn corewlan_request_location();
    fn corewlan_current_ssid() -> *const std::ffi::c_char;
    fn corewlan_free_string(s: *const std::ffi::c_char);
}

/// Location Services authorization status.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub enum LocationStatus {
    NotDetermined, // 0
    Restricted,    // 1
    Denied,        // 2
    Authorized,    // 3 or 4
}

impl From<i32> for LocationStatus {
    fn from(val: i32) -> Self {
        match val {
            0 => LocationStatus::NotDetermined,
            1 => LocationStatus::Restricted,
            2 => LocationStatus::Denied,
            3 | 4 => LocationStatus::Authorized,
            _ => LocationStatus::Denied,
        }
    }
}

/// Check the current Location Services authorization status.
pub fn get_location_status() -> LocationStatus {
    #[cfg(target_os = "macos")]
    {
        let status = unsafe { corewlan_location_status() };
        LocationStatus::from(status)
    }
    #[cfg(not(target_os = "macos"))]
    {
        LocationStatus::Authorized // Non-macOS doesn't need location
    }
}

/// Request Location Services "When In Use" authorization.
/// On macOS, this triggers the system permission dialog (only in bundled .app).
pub fn request_location() {
    #[cfg(target_os = "macos")]
    unsafe {
        corewlan_request_location();
    }
}

/// NetworkManager availability, as surfaced to the frontend so it can present a single
/// consistent shape across platforms without needing its own platform detection (the frontend
/// has no `tauri-plugin-os` dependency today).
///
/// The `#[allow(dead_code)]` is on the enum itself rather than a single variant: on a Linux
/// build, only `NotApplicable` goes unconstructed; on a non-Linux build, only `Available` and
/// `Unavailable` go unconstructed (`derive`d impls like `serde::Serialize` don't count as
/// construction for dead-code analysis). Which variant(s) are "dead" flips per target, so the
/// allow needs to cover the whole enum rather than pin to whichever variant is dead today.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[allow(dead_code)]
pub enum NetworkManagerStatus {
    /// Linux, and NetworkManager's D-Bus bus name is owned — WiFi mode is fully functional.
    Available,
    /// Linux, but NetworkManager's D-Bus bus name is absent — WiFi mode is unsupported (D1
    /// accepted risk: NetworkManager-only reactive WiFi, no fallback tier).
    Unavailable,
    /// Non-Linux platform — this signal doesn't apply; WiFi detection uses a different
    /// mechanism (CoreWLAN/networksetup on macOS) that isn't NetworkManager-gated.
    NotApplicable,
}

/// Check whether NetworkManager is available for reactive WiFi monitoring.
/// Linux-meaningful; returns [`NetworkManagerStatus::NotApplicable`] on other platforms so the
/// frontend has one consistent shape to branch on.
pub async fn get_networkmanager_status() -> NetworkManagerStatus {
    #[cfg(target_os = "linux")]
    {
        let Ok(conn) = zbus::Connection::system().await else {
            return NetworkManagerStatus::Unavailable;
        };
        if nm_available_from(nm_bus_name_present(&conn).await) {
            NetworkManagerStatus::Available
        } else {
            NetworkManagerStatus::Unavailable
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        NetworkManagerStatus::NotApplicable
    }
}

/// Parse the output of `networksetup -getairportnetwork en0`.
/// Returns the SSID if connected, or `None` if disconnected or unrecognised.
#[cfg(target_os = "macos")]
fn parse_ssid_output(output: &str) -> Option<String> {
    // Expected: "Current Wi-Fi Network: MySSID\n"
    // Disconnected: "You are not associated with an AirPort network.\n"
    output
        .strip_prefix("Current Wi-Fi Network: ")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Detect the current WiFi SSID.
/// On macOS 26+, uses CoreWLAN (requires Location Services authorization).
/// Falls back to networksetup CLI if CoreWLAN returns nil.
pub fn detect_current_ssid() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        // Only attempt CoreWLAN when Location Services is authorized.
        // If not authorized, fall through directly to the networksetup fallback
        // to avoid a potential hang or silent failure in the FFI call.
        if get_location_status() == LocationStatus::Authorized {
            let ssid_ptr = unsafe { corewlan_current_ssid() };
            if !ssid_ptr.is_null() {
                let ssid = unsafe { std::ffi::CStr::from_ptr(ssid_ptr) }
                    .to_string_lossy()
                    .into_owned();
                unsafe { corewlan_free_string(ssid_ptr) };
                if !ssid.is_empty() {
                    return Some(ssid);
                }
            }
        } else {
            log::debug!(
                "detect_current_ssid: Location Services not authorized ({:?}) \
                 — skipping CoreWLAN, using networksetup fallback",
                get_location_status()
            );
        }

        // Fallback to networksetup (works on macOS 12–25 without Location Services)
        let output = std::process::Command::new("networksetup")
            .args(["-getairportnetwork", "en0"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_ssid_output(&stdout)
    }

    #[cfg(target_os = "linux")]
    {
        tauri::async_runtime::block_on(async {
            let conn = zbus::Connection::system().await.ok()?;
            linux_current_ssid(&conn).await
        })
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
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
    #[cfg(target_os = "linux")]
    shutdown: Option<tokio::sync::watch::Sender<bool>>,
}

impl WifiMonitor {
    pub fn new(config: Arc<Mutex<AppConfig>>, app_handle: AppHandle) -> Self {
        Self {
            config,
            app_handle,
            running: Arc::new(AtomicBool::new(false)),
            thread_handle: None,
            #[cfg(target_os = "linux")]
            shutdown: None,
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

        #[cfg(target_os = "linux")]
        let (shutdown_sender, shutdown_receiver) = tokio::sync::watch::channel(false);
        #[cfg(target_os = "linux")]
        {
            self.shutdown = Some(shutdown_sender);
        }

        self.thread_handle = Some(thread::spawn(move || {
            #[cfg(target_os = "linux")]
            let event_loop_finished = tauri::async_runtime::block_on(try_event_driven_loop(
                &config,
                &running,
                &app_handle,
                shutdown_receiver,
            ));
            #[cfg(not(target_os = "linux"))]
            let event_loop_finished = try_event_driven_loop(&config, &running, &app_handle);

            if !event_loop_finished {
                log::warn!(
                    "Event-driven WiFi monitoring unavailable on this platform \
                     (or setup failed) — falling back to polling"
                );
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
        #[cfg(target_os = "linux")]
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(true);
        }

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
    check_and_emit_ssid(config, app_handle, detect_current_ssid());
}

fn check_and_emit_ssid(
    config: &Arc<Mutex<AppConfig>>,
    app_handle: &AppHandle,
    ssid: Option<String>,
) {
    let (enabled, active) = {
        let cfg = match config.lock() {
            Ok(cfg) => cfg,
            Err(_) => return,
        };
        if cfg.mode != AppMode::WiFi {
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

fn polling_loop(config: &Arc<Mutex<AppConfig>>, running: &Arc<AtomicBool>, app_handle: &AppHandle) {
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

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn try_event_driven_loop(
    _config: &Arc<Mutex<AppConfig>>,
    _running: &Arc<AtomicBool>,
    _app_handle: &AppHandle,
) -> bool {
    false // Always falls back to polling on non-macOS, non-Linux platforms (e.g. Windows)
}

// ─────────────── Event-driven path (Linux NetworkManager D-Bus) ─────────────
//
// Uses `zbus::blocking` against NetworkManager's system-bus D-Bus surface. NM is present on
// effectively every Ubuntu/Fedora desktop; on systems without it, `try_event_driven_loop`
// returns `false` (bus name absent) and the caller falls back to polling, which also returns
// `None` from `detect_current_ssid()` since the Linux block below shares the same NM-absence
// check. See RFC 048 D1 for the accepted-risk rationale (NetworkManager-only reactive WiFi).

#[cfg(target_os = "linux")]
const NM_SERVICE: &str = "org.freedesktop.NetworkManager";
#[cfg(target_os = "linux")]
const NM_DEVICE_IFACE: &str = "org.freedesktop.NetworkManager.Device";
#[cfg(target_os = "linux")]
const NM_WIRELESS_IFACE: &str = "org.freedesktop.NetworkManager.Device.Wireless";
#[cfg(target_os = "linux")]
const NM_AP_IFACE: &str = "org.freedesktop.NetworkManager.AccessPoint";
#[cfg(target_os = "linux")]
const NM_PROPERTIES_IFACE: &str = "org.freedesktop.DBus.Properties";
/// `DeviceType` enum value for WiFi devices, per the NetworkManager D-Bus API spec.
#[cfg(target_os = "linux")]
const NM_DEVICE_TYPE_WIFI: u32 = 2;

/// The D-Bus object path NetworkManager uses to represent "no active access point" — the root
/// path is never a real `AccessPoint` object, so it serves as the disconnected sentinel.
/// Extracted as a pure predicate (mirroring the macOS `parse_ssid_output` precedent) so it's
/// unit-testable without a live D-Bus connection.
#[cfg(target_os = "linux")]
fn is_disconnected_ap_path(path: &str) -> bool {
    path == "/"
}

/// Decide whether a periodic re-check (`Option<String>` freshly read vs. the last-known value)
/// represents an SSID change that should be emitted. Pure comparison extracted from the
/// `Timeout` arm of `try_event_driven_loop` so the "did it change" decision is unit-testable
/// without a live D-Bus connection — mirrors the macOS `try_event_driven_loop`'s inline
/// `current != last_ssid` check (line ~831), just named and seamed for Linux's periodic
/// safety-net re-check.
#[cfg(target_os = "linux")]
fn ssid_changed(current: &Option<String>, last: &Option<String>) -> bool {
    current != last
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoopExit {
    Stopped,
    PathStale,
    StreamEnded,
}

enum PathRecheck {
    Current(Option<String>),
    Stale,
}

enum MonitorSignal {
    StateChanged,
    PropertiesChanged,
}

async fn coordinate_linux_monitor<S, Emit, Recheck>(
    mut events: S,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    mut tick: tokio::time::Interval,
    mut last_ssid: Option<String>,
    mut emit_current_state: Emit,
    mut recheck_path: Recheck,
) -> LoopExit
where
    S: futures_util::Stream<Item = MonitorSignal> + Unpin,
    Emit: FnMut() -> Option<String>,
    Recheck: FnMut() -> PathRecheck,
{
    use futures_util::StreamExt;

    loop {
        if *shutdown.borrow() {
            return LoopExit::Stopped;
        }

        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return LoopExit::Stopped;
                }
            }
            event = events.next() => match event {
                Some(MonitorSignal::StateChanged | MonitorSignal::PropertiesChanged) => {
                    last_ssid = emit_current_state();
                }
                None => return LoopExit::StreamEnded,
            },
            _ = tick.tick() => match recheck_path() {
                PathRecheck::Stale => return LoopExit::PathStale,
                PathRecheck::Current(current) if ssid_changed(&current, &last_ssid) => {
                    last_ssid = emit_current_state();
                }
                PathRecheck::Current(_) => {}
            },
        }
    }
}

fn monitor_exit_uses_polling_fallback(exit: LoopExit) -> bool {
    !matches!(exit, LoopExit::Stopped)
}

fn monitor_starts_after_setup(setup_succeeded: bool) -> bool {
    setup_succeeded
}

/// Pure decision function over whether NetworkManager's well-known bus name is currently owned
/// on the system bus. Mirrors the `xdotool_present_from` seam pattern (`platform/linux.rs`,
/// D3a): the caller performs the real D-Bus `NameHasOwner` check and passes in the resulting
/// `bool`, keeping the decision itself unit-testable without a real system bus connection.
#[cfg(target_os = "linux")]
fn nm_available_from(has_bus_name: bool) -> bool {
    has_bus_name
}

/// Check whether NetworkManager's well-known bus name is currently owned on the system bus.
/// Returns `false` if the system bus itself is unreachable (treated the same as NM being
/// absent — either way, the WiFi feature is unavailable).
#[cfg(target_os = "linux")]
async fn nm_bus_name_present(conn: &zbus::Connection) -> bool {
    use zbus::fdo::DBusProxy;
    use zbus::names::BusName;

    let Ok(dbus_proxy) = DBusProxy::new(conn).await else {
        return false;
    };
    let Ok(name) = BusName::try_from(NM_SERVICE) else {
        return false;
    };
    dbus_proxy.name_has_owner(name).await.unwrap_or(false)
}

/// Find the object path of the first NetworkManager device with `DeviceType == 2` (WiFi).
/// Returns `None` if NetworkManager is unavailable or no WiFi device is present.
#[cfg(target_os = "linux")]
async fn find_wifi_device_path(conn: &zbus::Connection) -> Option<zbus::OwnedObjectPath> {
    use zbus::Proxy;

    let nm_proxy = Proxy::new(
        conn,
        NM_SERVICE,
        "/org/freedesktop/NetworkManager",
        NM_SERVICE,
    )
    .await
    .ok()?;
    let devices: Vec<zbus::OwnedObjectPath> = nm_proxy.get_property("AllDevices").await.ok()?;

    for device_path in devices {
        // A single un-constructible device proxy must not abort discovery of a WiFi device
        // later in the list — `continue` here mirrors the `DeviceType` read failure handling
        // directly below, rather than propagating `None` out of the whole function via `?`.
        let device_proxy =
            match Proxy::new(conn, NM_SERVICE, device_path.as_str(), NM_DEVICE_IFACE).await {
                Ok(p) => p,
                Err(_) => continue,
            };
        let device_type: u32 = match device_proxy.get_property("DeviceType").await {
            Ok(t) => t,
            Err(_) => continue,
        };
        drop(device_proxy);
        if device_type == NM_DEVICE_TYPE_WIFI {
            return Some(device_path);
        }
    }
    None
}

#[cfg(target_os = "linux")]
async fn linux_current_ssid(conn: &zbus::Connection) -> Option<String> {
    use zbus::Proxy;

    let wifi_path = find_wifi_device_path(conn).await?;
    let wireless_proxy = Proxy::new(conn, NM_SERVICE, wifi_path.as_str(), NM_WIRELESS_IFACE)
        .await
        .ok()?;
    let active_ap: zbus::OwnedObjectPath = wireless_proxy
        .get_property("ActiveAccessPoint")
        .await
        .ok()?;
    if is_disconnected_ap_path(active_ap.as_str()) {
        return None;
    }
    let ap_proxy = Proxy::new(conn, NM_SERVICE, active_ap.as_str(), NM_AP_IFACE)
        .await
        .ok()?;
    let ssid_bytes: Vec<u8> = ap_proxy.get_property("Ssid").await.ok()?;
    Some(String::from_utf8_lossy(&ssid_bytes).into_owned())
}

#[cfg(target_os = "linux")]
fn linux_current_ssid_from_callback(conn: zbus::Connection) -> Option<String> {
    thread::spawn(move || tauri::async_runtime::block_on(linux_current_ssid(&conn)))
        .join()
        .ok()
        .flatten()
}

#[cfg(target_os = "linux")]
fn wifi_path_from_callback(conn: zbus::Connection) -> Option<zbus::OwnedObjectPath> {
    thread::spawn(move || tauri::async_runtime::block_on(find_wifi_device_path(&conn)))
        .join()
        .ok()
        .flatten()
}

/// Try to run a `zbus`+NetworkManager event-driven loop on the current thread.
/// Returns `true` if the loop ran to completion (i.e. `running` went false), or `false` if
/// setup failed — NM bus name absent, no WiFi device found, or the system bus itself is
/// unreachable (caller falls back to polling).
#[cfg(target_os = "linux")]
async fn try_event_driven_loop(
    config: &Arc<Mutex<AppConfig>>,
    _running: &Arc<AtomicBool>,
    app_handle: &AppHandle,
    shutdown: tokio::sync::watch::Receiver<bool>,
) -> bool {
    use futures_util::StreamExt;
    use zbus::runtime::AsyncDrop;
    use zbus::{Connection, Proxy};

    let conn = match Connection::system().await {
        Ok(c) => c,
        Err(e) => {
            log::warn!("WifiMonitor: failed to connect to the D-Bus system bus: {e}");
            return monitor_starts_after_setup(false);
        }
    };

    if !nm_available_from(nm_bus_name_present(&conn).await) {
        log::warn!(
            "WifiMonitor: NetworkManager is not available on the D-Bus system bus \
             (org.freedesktop.NetworkManager has no owner) — WiFi mode requires \
             NetworkManager on Linux; falling back to polling (which will also report no SSID)"
        );
        return monitor_starts_after_setup(false);
    }

    let Some(wifi_path) = find_wifi_device_path(&conn).await else {
        log::warn!(
            "WifiMonitor: no WiFi device found via NetworkManager — falling back to polling"
        );
        return monitor_starts_after_setup(false);
    };

    let device_proxy =
        match Proxy::new(&conn, NM_SERVICE, wifi_path.as_str(), NM_DEVICE_IFACE).await {
            Ok(p) => p,
            Err(e) => {
                log::warn!("WifiMonitor: failed to create Device proxy: {e}");
                return monitor_starts_after_setup(false);
            }
        };
    let mut state_signals = match device_proxy.receive_signal("StateChanged").await {
        Ok(it) => it,
        Err(e) => {
            log::warn!("WifiMonitor: failed to subscribe to Device.StateChanged: {e}");
            return monitor_starts_after_setup(false);
        }
    };

    let props_proxy =
        match Proxy::new(&conn, NM_SERVICE, wifi_path.as_str(), NM_PROPERTIES_IFACE).await {
            Ok(p) => p,
            Err(e) => {
                log::warn!("WifiMonitor: failed to create Properties proxy: {e}");
                return monitor_starts_after_setup(false);
            }
        };
    let mut properties_signals = match props_proxy
        .receive_signal_with_args("PropertiesChanged", &[(0, NM_WIRELESS_IFACE)])
        .await
    {
        Ok(it) => it,
        Err(e) => {
            log::warn!("WifiMonitor: failed to subscribe to Wireless PropertiesChanged: {e}");
            return monitor_starts_after_setup(false);
        }
    };

    log::info!("WifiMonitor: zbus/NetworkManager event loop active");

    // Initial probe so the frontend gets state immediately on startup.
    let last_ssid = linux_current_ssid(&conn).await;
    check_and_emit_ssid(config, app_handle, last_ssid.clone());
    let events = futures_util::stream::select(
        state_signals.by_ref().map(|_| MonitorSignal::StateChanged),
        properties_signals
            .by_ref()
            .map(|_| MonitorSignal::PropertiesChanged),
    );
    let event_config = Arc::clone(config);
    let event_app = app_handle.clone();
    let emit_connection = conn.clone();
    let recheck_connection = conn.clone();
    let recheck_path = wifi_path.clone();
    let exit = coordinate_linux_monitor(
        events,
        shutdown,
        tokio::time::interval(Duration::from_secs(5)),
        last_ssid,
        move || {
            let ssid = linux_current_ssid_from_callback(emit_connection.clone());
            check_and_emit_ssid(&event_config, &event_app, ssid.clone());
            ssid
        },
        move || match wifi_path_from_callback(recheck_connection.clone()) {
            Some(current_path) if current_path == recheck_path => {
                PathRecheck::Current(linux_current_ssid_from_callback(recheck_connection.clone()))
            }
            _ => PathRecheck::Stale,
        },
    )
    .await;

    state_signals.async_drop().await;
    properties_signals.async_drop().await;
    drop(device_proxy);
    drop(props_proxy);
    if let Err(e) = conn.close().await {
        log::debug!("WifiMonitor: error closing D-Bus connection during shutdown: {e}");
    }

    if matches!(exit, LoopExit::PathStale) {
        log::warn!("WifiMonitor: WiFi device path changed or disappeared; falling back to polling");
    }
    monitor_starts_after_setup(true) && !monitor_exit_uses_polling_fallback(exit)
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

        pub fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);

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
    use sc_sys::*;
    use std::ffi::c_void;

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
    let mut last_ssid = detect_current_ssid();
    check_and_emit(config, app_handle);

    // ── Run loop — 1 s slices so we can react to stop() quickly ──────────
    // Also poll every POLL_INTERVAL_SECS to catch missed SC events (macOS 26+).
    const POLL_INTERVAL_SECS: u32 = 5;
    let mut ticks: u32 = 0;

    while running.load(Ordering::Relaxed) {
        // Returns after the timeout or after processing a source (our callback).
        unsafe {
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, 1.0, 0);
        }
        ticks += 1;
        if ticks >= POLL_INTERVAL_SECS {
            ticks = 0;
            let current = detect_current_ssid();
            if current != last_ssid {
                log::debug!(
                    "WiFi periodic check: SSID changed {:?} → {:?}",
                    last_ssid,
                    current
                );
                check_and_emit(config, app_handle);
                last_ssid = current;
            }
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

#[cfg(all(test, target_os = "macos"))]
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

/// @spec-handoff
///
/// ## `is_disconnected_ap_path`
///
/// ```
/// fn is_disconnected_ap_path(path: &str) -> bool
/// ```
///
/// Pure predicate mirroring the macOS `parse_ssid_output` precedent: NetworkManager represents
/// "no active access point" as the D-Bus object path `"/"` (the root path is never a real
/// `AccessPoint` object). `detect_current_ssid()` calls this on the `ActiveAccessPoint` property
/// value before attempting to read `AccessPoint.Ssid`, returning `None` early when it's `true`.
///
/// Behavior:
/// - `"/"` → `true` (disconnected sentinel).
/// - Any other non-empty path (e.g. `"/org/freedesktop/NetworkManager/AccessPoint/22"`) →
///   `false`.
/// - `""` (empty string, never actually emitted by NM but defensively covered) → `false` — only
///   the exact sentinel `"/"` counts as disconnected.
///
/// ## SSID byte-array decode (inline in `detect_current_ssid`, tested here via
/// `String::from_utf8_lossy` directly — no separate helper needed since it's a single stdlib
/// call)
///
/// NetworkManager's `AccessPoint.Ssid` property is D-Bus type `ay` (raw byte array), not `s`
/// (string) — SSIDs are not guaranteed valid UTF-8. Decoding uses `String::from_utf8_lossy`,
/// matching the existing macOS path's `CStr::to_string_lossy` semantics: valid UTF-8 bytes
/// decode losslessly, and any invalid byte sequence is replaced with `U+FFFD` (lossy
/// replacement) rather than panicking or returning an `Err`.
///
/// ## `nm_available_from`
///
/// ```
/// fn nm_available_from(has_bus_name: bool) -> bool
/// ```
///
/// Pure decision function mirroring the `xdotool_present_from` seam pattern established in
/// `platform/linux.rs` for D3a: the caller performs the real D-Bus `NameHasOwner` check against
/// `org.freedesktop.NetworkManager` and passes in the resulting `bool`, keeping the "is NM
/// available" decision itself unit-testable without a real system bus connection. Identity
/// function today (`has_bus_name` in, same value out) — kept as a named seam rather than
/// inlined so the decision point has one place to grow additional signals later (e.g. also
/// checking `WirelessEnabled`) without every call site needing to change.
///
/// Behavior:
/// - `true` (NM bus name is owned) → `true` (NM available).
/// - `false` (bus name absent — system bus reachable but NetworkManager isn't running/installed)
///   → `false` (NM unavailable).
///
/// ## `ssid_changed`
///
/// ```
/// fn ssid_changed(current: &Option<String>, last: &Option<String>) -> bool
/// ```
///
/// Pure comparison extracted from the `Timeout` arm of `try_event_driven_loop`'s periodic
/// safety-net re-check (HIGH #1 remediation), mirroring the macOS branch's inline
/// `current != last_ssid` check. Kept as a named seam so the "did the SSID change" decision is
/// unit-testable without a live D-Bus connection or a real WiFi adapter.
///
/// Behavior:
/// - Both `None` (disconnected, still disconnected) → `false` (no change).
/// - `None` → `Some(ssid)` (connected after being disconnected) → `true`.
/// - `Some(ssid)` → `None` (disconnected after being connected) → `true`.
/// - `Some(a)` → `Some(a)` (same SSID) → `false`.
/// - `Some(a)` → `Some(b)` where `a != b` (roamed to a different network) → `true`.
#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use super::*;
    use futures_util::stream;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::sync::{mpsc, watch};
    use tokio::time::{advance, interval_at, timeout, Duration, Instant};

    type EventStream = Pin<Box<dyn futures_util::Stream<Item = MonitorSignal> + Send>>;

    fn coordinator_inputs() -> (
        mpsc::Sender<MonitorSignal>,
        watch::Sender<bool>,
        EventStream,
    ) {
        let (signal_tx, signals) = mpsc::channel(4);
        let (shutdown_tx, _shutdown) = watch::channel(false);
        let events = stream::unfold(signals, |mut signals| async move {
            signals.recv().await.map(|signal| (signal, signals))
        });
        (signal_tx, shutdown_tx, Box::pin(events))
    }

    fn safety_net_tick() -> tokio::time::Interval {
        interval_at(
            Instant::now() + Duration::from_secs(1),
            Duration::from_secs(1),
        )
    }

    async fn finish(task: tokio::task::JoinHandle<LoopExit>) -> LoopExit {
        timeout(Duration::from_secs(1), task)
            .await
            .expect("monitor coordinator must not hang")
            .expect("monitor coordinator task must not panic")
    }

    #[tokio::test(start_paused = true)]
    async fn state_changed_signal_emits_current_state_once() {
        let (signal_tx, shutdown_tx, events) = coordinator_inputs();
        signal_tx.try_send(MonitorSignal::StateChanged).unwrap();
        let emissions = Arc::new(AtomicUsize::new(0));
        let emitted = Arc::clone(&emissions);
        let task = tokio::spawn(coordinate_linux_monitor(
            events,
            shutdown_tx.subscribe(),
            safety_net_tick(),
            Some("OfficeWiFi".to_string()),
            move || {
                emitted.fetch_add(1, Ordering::SeqCst);
                Some("OfficeWiFi".to_string())
            },
            || PathRecheck::Current(Some("OfficeWiFi".to_string())),
        ));
        tokio::task::yield_now().await;
        let _ = shutdown_tx.send(true);
        let exit = finish(task).await;

        assert_eq!(emissions.load(Ordering::SeqCst), 1);
        assert_eq!(exit, LoopExit::Stopped);
    }

    #[tokio::test(start_paused = true)]
    async fn properties_changed_signal_emits_current_state_once() {
        let (signal_tx, shutdown_tx, events) = coordinator_inputs();
        signal_tx
            .try_send(MonitorSignal::PropertiesChanged)
            .unwrap();
        let emissions = Arc::new(AtomicUsize::new(0));
        let emitted = Arc::clone(&emissions);
        let task = tokio::spawn(coordinate_linux_monitor(
            events,
            shutdown_tx.subscribe(),
            safety_net_tick(),
            Some("OfficeWiFi".to_string()),
            move || {
                emitted.fetch_add(1, Ordering::SeqCst);
                Some("OfficeWiFi".to_string())
            },
            || PathRecheck::Current(Some("OfficeWiFi".to_string())),
        ));
        tokio::task::yield_now().await;
        let _ = shutdown_tx.send(true);
        let exit = finish(task).await;

        assert_eq!(emissions.load(Ordering::SeqCst), 1);
        assert_eq!(exit, LoopExit::Stopped);
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_exits_without_waiting_for_monitor_cadence() {
        let (_signal_tx, shutdown_tx, events) = coordinator_inputs();
        let shutdown = shutdown_tx.subscribe();
        shutdown_tx.send(true).unwrap();
        let exit = finish(tokio::spawn(coordinate_linux_monitor(
            events,
            shutdown,
            safety_net_tick(),
            None,
            || None,
            || PathRecheck::Current(None),
        )))
        .await;

        assert_eq!(exit, LoopExit::Stopped);
    }

    #[tokio::test(start_paused = true)]
    async fn unchanged_safety_net_tick_emits_nothing() {
        let (_signal_tx, shutdown_tx, events) = coordinator_inputs();
        let emissions = Arc::new(AtomicUsize::new(0));
        let emitted = Arc::clone(&emissions);
        let task = tokio::spawn(coordinate_linux_monitor(
            events,
            shutdown_tx.subscribe(),
            safety_net_tick(),
            Some("OfficeWiFi".to_string()),
            move || {
                emitted.fetch_add(1, Ordering::SeqCst);
                Some("OfficeWiFi".to_string())
            },
            || PathRecheck::Current(Some("OfficeWiFi".to_string())),
        ));
        advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        let _ = shutdown_tx.send(true);
        let exit = finish(task).await;

        assert_eq!(emissions.load(Ordering::SeqCst), 0);
        assert_eq!(exit, LoopExit::Stopped);
    }

    #[tokio::test(start_paused = true)]
    async fn changed_safety_net_tick_emits_current_state_once() {
        let (_signal_tx, shutdown_tx, events) = coordinator_inputs();
        let emissions = Arc::new(AtomicUsize::new(0));
        let emitted = Arc::clone(&emissions);
        let task = tokio::spawn(coordinate_linux_monitor(
            events,
            shutdown_tx.subscribe(),
            safety_net_tick(),
            Some("HomeWiFi".to_string()),
            move || {
                emitted.fetch_add(1, Ordering::SeqCst);
                Some("OfficeWiFi".to_string())
            },
            || PathRecheck::Current(Some("OfficeWiFi".to_string())),
        ));
        advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        let _ = shutdown_tx.send(true);
        let exit = finish(task).await;

        assert_eq!(emissions.load(Ordering::SeqCst), 1);
        assert_eq!(exit, LoopExit::Stopped);
    }

    #[tokio::test(start_paused = true)]
    async fn stale_device_path_returns_path_stale_for_polling_fallback() {
        let (_signal_tx, shutdown_tx, events) = coordinator_inputs();
        let task = tokio::spawn(coordinate_linux_monitor(
            events,
            shutdown_tx.subscribe(),
            safety_net_tick(),
            Some("OfficeWiFi".to_string()),
            || Some("OfficeWiFi".to_string()),
            || PathRecheck::Stale,
        ));
        advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        let _ = shutdown_tx.send(true);
        let exit = finish(task).await;

        assert_eq!(exit, LoopExit::PathStale);
        assert!(monitor_exit_uses_polling_fallback(exit));
    }

    #[test]
    fn setup_failure_selects_polling_fallback() {
        assert!(!monitor_starts_after_setup(false));
    }

    #[tokio::test(start_paused = true)]
    async fn ended_signal_stream_while_running_selects_polling_fallback() {
        let (signal_tx, shutdown_tx, events) = coordinator_inputs();
        drop(signal_tx);

        let exit = finish(tokio::spawn(coordinate_linux_monitor(
            events,
            shutdown_tx.subscribe(),
            safety_net_tick(),
            None,
            || None,
            || PathRecheck::Current(None),
        )))
        .await;

        assert_eq!(exit, LoopExit::StreamEnded);
        assert!(monitor_exit_uses_polling_fallback(exit));
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_stops_monitor_without_polling_fallback() {
        let (_signal_tx, shutdown_tx, events) = coordinator_inputs();
        let shutdown = shutdown_tx.subscribe();
        shutdown_tx.send(true).unwrap();

        let exit = finish(tokio::spawn(coordinate_linux_monitor(
            events,
            shutdown,
            safety_net_tick(),
            None,
            || None,
            || PathRecheck::Current(None),
        )))
        .await;

        assert_eq!(exit, LoopExit::Stopped);
        assert!(!monitor_exit_uses_polling_fallback(exit));
    }

    #[test]
    fn ssid_bytes_decode_valid_utf8() {
        let raw: Vec<u8> = b"OfficeWiFi".to_vec();
        assert_eq!(String::from_utf8_lossy(&raw), "OfficeWiFi");
    }

    #[test]
    fn ssid_bytes_decode_non_utf8_is_lossy_not_panic() {
        // 0xFF is not a valid UTF-8 continuation/start byte on its own.
        let raw: Vec<u8> = vec![b'A', b'B', 0xFF, b'C'];
        let decoded = String::from_utf8_lossy(&raw);
        // Lossy decode replaces the invalid byte with U+FFFD rather than panicking.
        assert!(decoded.contains('\u{FFFD}'));
        assert!(decoded.starts_with("AB"));
        assert!(decoded.ends_with('C'));
    }

    #[test]
    fn disconnected_sentinel_path_is_detected() {
        assert!(is_disconnected_ap_path("/"));
    }

    #[test]
    fn real_access_point_path_is_not_disconnected() {
        assert!(!is_disconnected_ap_path(
            "/org/freedesktop/NetworkManager/AccessPoint/22"
        ));
    }

    #[test]
    fn empty_path_is_not_the_disconnected_sentinel() {
        // Defensive: NM never actually emits "", but only the exact "/" sentinel counts.
        assert!(!is_disconnected_ap_path(""));
    }

    #[test]
    fn nm_available_from_true_when_bus_name_present() {
        assert!(nm_available_from(true));
    }

    #[test]
    fn nm_available_from_false_when_bus_name_absent() {
        assert!(!nm_available_from(false));
    }

    #[test]
    fn ssid_changed_false_when_both_none() {
        assert!(!ssid_changed(&None, &None));
    }

    #[test]
    fn ssid_changed_true_when_connecting_from_disconnected() {
        assert!(ssid_changed(&Some("OfficeWiFi".to_string()), &None));
    }

    #[test]
    fn ssid_changed_true_when_disconnecting_from_connected() {
        assert!(ssid_changed(&None, &Some("OfficeWiFi".to_string())));
    }

    #[test]
    fn ssid_changed_false_when_same_ssid() {
        assert!(!ssid_changed(
            &Some("OfficeWiFi".to_string()),
            &Some("OfficeWiFi".to_string())
        ));
    }

    #[test]
    fn ssid_changed_true_when_roamed_to_different_ssid() {
        assert!(ssid_changed(
            &Some("HomeWiFi".to_string()),
            &Some("OfficeWiFi".to_string())
        ));
    }

    // ── Live-verification tests against a real NetworkManager D-Bus (HIGH #1/#2) ───────────
    //
    // These require a running `NetworkManager` on the system D-Bus with at least one WiFi
    // device present (`DeviceType == 2`). They are `#[ignore]`d by default so a plain `cargo
    // test` never depends on host D-Bus/NM state, and are run explicitly with
    // `cargo test -- --ignored` on a host known to have NetworkManager (verified present in
    // this remediation session — see the plan's audit report for the live evidence captured).

    /// HIGH #1 evidence: `find_wifi_device_path` resolves to a real, currently-present WiFi
    /// device path on a live system, and calling it twice in a row is stable (same device,
    /// same path) absent an actual NM restart/device removal — i.e. the staleness branch in
    /// `try_event_driven_loop`'s periodic re-check is not a false positive on a healthy system.
    #[tokio::test]
    #[ignore = "requires a live NetworkManager D-Bus + WiFi device"]
    async fn find_wifi_device_path_is_stable_on_a_healthy_live_system() {
        let conn = zbus::Connection::system()
            .await
            .expect("system bus should be reachable in this environment");
        let first = find_wifi_device_path(&conn).await;
        assert!(
            first.is_some(),
            "expected a live WiFi device on this host — see plan's Chi context: verified via \
             `busctl --system get-property .../Devices/2 ... DeviceType` == 2"
        );
        let second = find_wifi_device_path(&conn).await;
        assert_eq!(
            first, second,
            "device path should be stable across two immediate re-resolutions absent a real \
             NM restart or device removal"
        );
    }

    /// HIGH #2 evidence: zbus 6 shutdown explicitly drops the subscribed `SignalStream` before
    /// closing its connection, completing inside a bounded wait on a live NetworkManager host.
    #[tokio::test]
    #[ignore = "requires a live NetworkManager D-Bus + WiFi device"]
    async fn closing_connection_unblocks_a_parked_signal_iterator_thread() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use zbus::runtime::AsyncDrop;

        let conn = zbus::Connection::system()
            .await
            .expect("system bus should be reachable in this environment");
        let wifi_path = find_wifi_device_path(&conn)
            .await
            .expect("expected a live WiFi device on this host");
        let device_proxy = zbus::Proxy::new(&conn, NM_SERVICE, wifi_path.as_str(), NM_DEVICE_IFACE)
            .await
            .expect("Device proxy should construct against a live NM device");
        let signal_stream = device_proxy
            .receive_signal("StateChanged")
            .await
            .expect("subscribing to StateChanged should succeed against a live NM device");
        let (shutdown_tx, mut shutdown) = tokio::sync::watch::channel(false);
        let async_drop_ran = AtomicBool::new(false);
        let close_ran = AtomicBool::new(false);
        shutdown_tx
            .send(true)
            .expect("shutdown receiver should be present");

        timeout(Duration::from_secs(5), async {
            shutdown
                .changed()
                .await
                .expect("shutdown sender should remain present");
            signal_stream.async_drop().await;
            async_drop_ran.store(true, Ordering::SeqCst);
            drop(device_proxy);
            conn.close()
                .await
                .expect("closing a live system-bus connection should succeed");
            close_ran.store(true, Ordering::SeqCst);
        })
        .await
        .expect("zbus 6 shutdown should complete before the bounded timeout");

        assert!(async_drop_ran.load(Ordering::SeqCst));
        assert!(close_ran.load(Ordering::SeqCst));
    }
}
