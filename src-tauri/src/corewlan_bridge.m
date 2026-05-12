// CoreWLAN + CoreLocation bridge for WiFi SSID detection on macOS 26+.
// macOS 26 requires Location Services authorization for SSID reading.
// This file is compiled by build.rs via cc::Build.

#import <CoreWLAN/CoreWLAN.h>
#import <CoreLocation/CoreLocation.h>
#import <dispatch/dispatch.h>
#import <string.h>
#import <stdlib.h>

// ── Location Services delegate ──────────────────────────────────────────────

@interface WifiLocationDelegate : NSObject <CLLocationManagerDelegate>
@end

@implementation WifiLocationDelegate
- (void)locationManagerDidChangeAuthorization:(CLLocationManager *)manager {
    // This callback fires after the user grants/denies permission.
    // We don't need to do anything here — the caller will re-check status.
    (void)manager;
}
@end

// ── Static state (lazy-initialized) ──────────────────────────────────────────

static CLLocationManager *_locationManager = nil;
static WifiLocationDelegate *_locationDelegate = nil;

static void ensure_location_manager(void) {
    if (_locationManager) return;
    _locationManager = [[CLLocationManager alloc] init];
    _locationDelegate = [[WifiLocationDelegate alloc] init];
    _locationManager.delegate = _locationDelegate;
}

// ── Public C API (called from Rust via extern "C") ───────────────────────────

/// Returns the CLAuthorizationStatus as an int.
/// 0 = notDetermined, 1 = restricted, 2 = denied, 3 = authorizedAlways, 4 = authorizedWhenInUse
int corewlan_location_status(void) {
    __block int status = 0;
    if ([NSThread isMainThread]) {
        ensure_location_manager();
        status = (int)[_locationManager authorizationStatus];
    } else {
        dispatch_sync(dispatch_get_main_queue(), ^{
            ensure_location_manager();
            status = (int)[_locationManager authorizationStatus];
        });
    }
    return status;
}

/// Request Location Services "When In Use" authorization.
/// Must be called to trigger the macOS permission dialog.
/// NOTE: The permission dialog only appears in a properly bundled .app (tauri build),
/// NOT in tauri dev mode.
void corewlan_request_location(void) {
    if ([NSThread isMainThread]) {
        ensure_location_manager();
        if ([_locationManager authorizationStatus] == kCLAuthorizationStatusNotDetermined) {
            [_locationManager requestWhenInUseAuthorization];
        }
    } else {
        dispatch_sync(dispatch_get_main_queue(), ^{
            ensure_location_manager();
            if ([_locationManager authorizationStatus] == kCLAuthorizationStatusNotDetermined) {
                [_locationManager requestWhenInUseAuthorization];
            }
        });
    }
}

/// Get the current WiFi SSID via CoreWLAN.
/// Returns a malloc'd C string (caller must free with corewlan_free_string),
/// or NULL if disconnected, not authorized, or on error.
const char *corewlan_current_ssid(void) {
    @autoreleasepool {
        CWWiFiClient *client = [CWWiFiClient sharedWiFiClient];
        CWInterface *iface = [client interface];
        if (!iface) return NULL;
        NSString *ssid = [iface ssid];
        if (!ssid || [ssid length] == 0) return NULL;
        return strdup([ssid UTF8String]);
    }
}

/// Free a string returned by corewlan_current_ssid.
void corewlan_free_string(const char *s) {
    free((void *)s);
}
