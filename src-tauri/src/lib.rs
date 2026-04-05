//! No Sleep Please! — a cross-platform tray-only mouse jiggler / anti-inactivity tool.
//!
//! This is the main Tauri application entry point. It wires together the
//! platform layer, configuration, engine, system tray, global shortcut,
//! autostart, accessibility checks, and exposes IPC commands to the
//! settings webview.

mod config;
mod engine;
mod i18n;
mod platform;

use config::{AppConfig, JiggleMode, Profile};
use engine::Engine;
use i18n::Bundle as I18nBundle;
use platform::PermissionChecker;
use std::sync::{Arc, Mutex};
use tauri::Manager;

// ───────────────────────────── Tauri commands ──────────────────────────────

/// Return the current configuration to the frontend.
#[tauri::command]
fn get_config(config: tauri::State<'_, Arc<Mutex<AppConfig>>>) -> Result<AppConfig, String> {
    let cfg = config
        .lock()
        .map_err(|e| format!("Config lock poisoned: {}", e))?;
    Ok(cfg.clone())
}

/// Persist a new configuration from the frontend and sync autostart.
#[tauri::command]
fn save_config(
    app: tauri::AppHandle,
    new_config: AppConfig,
    state: tauri::State<'_, Arc<Mutex<AppConfig>>>,
) -> Result<(), String> {
    // Sync autostart with the new config value.
    {
        use tauri_plugin_autostart::ManagerExt;
        let manager = app.autolaunch();
        if new_config.autostart {
            let _ = manager.enable();
        } else {
            let _ = manager.disable();
        }
    }

    let mut cfg = state
        .lock()
        .map_err(|e| format!("Config lock poisoned: {}", e))?;
    *cfg = new_config;
    cfg.save()
}

/// Return the engine state name (`"idle"`, `"active"`, …).
#[tauri::command]
fn get_engine_state(engine: tauri::State<'_, Arc<Mutex<Engine>>>) -> Result<String, String> {
    let eng = engine
        .lock()
        .map_err(|e| format!("Engine lock poisoned: {}", e))?;
    Ok(eng.state_name().to_string())
}

/// Toggle the engine on/off and return the new state name.
#[tauri::command]
fn toggle_engine(engine: tauri::State<'_, Arc<Mutex<Engine>>>) -> Result<String, String> {
    let mut eng = engine
        .lock()
        .map_err(|e| format!("Engine lock poisoned: {}", e))?;
    eng.toggle()?;
    Ok(eng.state_name().to_string())
}

/// Check whether accessibility permission is granted (macOS-specific).
#[tauri::command]
fn check_accessibility(
    checker: tauri::State<'_, Arc<dyn PermissionChecker>>,
) -> Result<bool, String> {
    Ok(checker.check_accessibility())
}

/// Open the OS accessibility settings so the user can grant permission.
#[tauri::command]
fn request_accessibility(
    checker: tauri::State<'_, Arc<dyn PermissionChecker>>,
) -> Result<(), String> {
    checker.request_accessibility()
}

/// Query the OS autostart manager for the current state.
#[tauri::command]
fn get_autostart_enabled(app: tauri::AppHandle) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch()
        .is_enabled()
        .map_err(|e| e.to_string())
}

/// Enable or disable launch-at-login via the OS autostart manager.
#[tauri::command]
fn set_autostart_enabled(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let manager = app.autolaunch();
    if enabled {
        manager.enable().map_err(|e| e.to_string())
    } else {
        manager.disable().map_err(|e| e.to_string())
    }
}

/// Hide the settings window (called from the settings webview on Save/Cancel).
#[tauri::command]
fn close_settings_window(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("settings") {
        win.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Return a translated string for the given message key.
#[tauri::command]
fn get_translation(
    key: String,
    bundle: tauri::State<'_, Arc<Mutex<I18nBundle>>>,
) -> Result<String, String> {
    let b = bundle
        .lock()
        .map_err(|e| format!("I18n lock poisoned: {}", e))?;
    Ok(i18n::t(&b, &key))
}

/// Replace the shared i18n bundle so the frontend can switch languages at runtime.
#[tauri::command]
fn set_language(
    language: String,
    bundle: tauri::State<'_, Arc<Mutex<I18nBundle>>>,
) -> Result<(), String> {
    let new_bundle = i18n::create_bundle(&language);
    let mut b = bundle
        .lock()
        .map_err(|e| format!("I18n lock poisoned: {}", e))?;
    *b = new_bundle;
    Ok(())
}

/// Return all saved profiles.
#[tauri::command]
fn list_profiles(config: tauri::State<'_, Arc<Mutex<AppConfig>>>) -> Result<Vec<Profile>, String> {
    let cfg = config
        .lock()
        .map_err(|e| format!("Config lock poisoned: {}", e))?;
    Ok(cfg.profiles.clone())
}

/// Save the current settings as a named profile.
#[tauri::command]
fn save_profile(
    name: String,
    config: tauri::State<'_, Arc<Mutex<AppConfig>>>,
) -> Result<(), String> {
    let mut cfg = config
        .lock()
        .map_err(|e| format!("Config lock poisoned: {}", e))?;

    let profile = Profile {
        name: name.clone(),
        jiggle_mode: cfg.jiggle_mode.clone(),
        interval_secs: cfg.interval_secs,
        schedule_enabled: cfg.schedule_enabled,
        schedule_start_hour: cfg.schedule_start_hour,
        schedule_start_minute: cfg.schedule_start_minute,
        schedule_end_hour: cfg.schedule_end_hour,
        schedule_end_minute: cfg.schedule_end_minute,
        schedule_days: cfg.schedule_days.clone(),
    };

    // Replace if a profile with the same name exists, otherwise append.
    if let Some(existing) = cfg.profiles.iter_mut().find(|p| p.name == name) {
        *existing = profile;
    } else {
        cfg.profiles.push(profile);
    }
    cfg.active_profile = name;
    cfg.save()
}

/// Load a named profile into the current settings.
#[tauri::command]
fn load_profile(
    name: String,
    config: tauri::State<'_, Arc<Mutex<AppConfig>>>,
) -> Result<(), String> {
    let mut cfg = config
        .lock()
        .map_err(|e| format!("Config lock poisoned: {}", e))?;

    let profile = cfg
        .profiles
        .iter()
        .find(|p| p.name == name)
        .cloned()
        .ok_or_else(|| format!("Profile '{}' not found", name))?;

    cfg.jiggle_mode = profile.jiggle_mode;
    cfg.interval_secs = profile.interval_secs;
    cfg.schedule_enabled = profile.schedule_enabled;
    cfg.schedule_start_hour = profile.schedule_start_hour;
    cfg.schedule_start_minute = profile.schedule_start_minute;
    cfg.schedule_end_hour = profile.schedule_end_hour;
    cfg.schedule_end_minute = profile.schedule_end_minute;
    cfg.schedule_days = profile.schedule_days;
    cfg.active_profile = name;
    cfg.save()
}

/// Delete a named profile.
#[tauri::command]
fn delete_profile(
    name: String,
    config: tauri::State<'_, Arc<Mutex<AppConfig>>>,
) -> Result<(), String> {
    let mut cfg = config
        .lock()
        .map_err(|e| format!("Config lock poisoned: {}", e))?;

    cfg.profiles.retain(|p| p.name != name);
    if cfg.active_profile == name {
        cfg.active_profile = String::from("Default");
    }
    cfg.save()
}

/// Update the global hotkey at runtime: unregister the old shortcut, register
/// the new one, and persist the change.
#[tauri::command]
fn update_global_hotkey(
    app: tauri::AppHandle,
    hotkey: String,
    config: tauri::State<'_, Arc<Mutex<AppConfig>>>,
) -> Result<(), String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    // Parse the new shortcut string.
    let new_shortcut = parse_shortcut_string(&hotkey)
        .ok_or_else(|| format!("Invalid shortcut: {}", hotkey))?;

    // Unregister all existing shortcuts, then register the new one.
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();

    gs.register(new_shortcut)
        .map_err(|e| format!("Failed to register shortcut: {}", e))?;

    // Persist to config.
    if let Ok(mut cfg) = config.lock() {
        cfg.global_hotkey = hotkey;
        cfg.save()?;
    }

    Ok(())
}

// ───────────────────────── Hotkey string parser ────────────────────────────

/// Parse a human-readable shortcut string like `"CmdOrCtrl+Shift+J"` into a
/// [`Shortcut`] that can be registered with the global-shortcut plugin.
#[cfg(desktop)]
fn parse_shortcut_string(
    s: &str,
) -> Option<tauri_plugin_global_shortcut::Shortcut> {
    use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut};

    let parts: Vec<&str> = s.split('+').map(|p| p.trim()).collect();
    if parts.is_empty() {
        return None;
    }

    let mut mods: Option<Modifiers> = None;
    let mut add_mod = |m: Modifiers| {
        mods = Some(match mods {
            Some(existing) => existing | m,
            None => m,
        });
    };

    for part in &parts[..parts.len().saturating_sub(1)] {
        match part.to_lowercase().as_str() {
            "ctrl" | "control" => add_mod(Modifiers::CONTROL),
            "cmd" | "meta" | "super" | "command" => add_mod(Modifiers::META),
            "cmdorctrl" | "commandorcontrol" => {
                #[cfg(target_os = "macos")]
                add_mod(Modifiers::META);
                #[cfg(not(target_os = "macos"))]
                add_mod(Modifiers::CONTROL);
            }
            "shift" => add_mod(Modifiers::SHIFT),
            "alt" | "option" => add_mod(Modifiers::ALT),
            _ => return None,
        }
    }

    let key_str = parts.last()?.to_uppercase();
    let code = match key_str.as_str() {
        "A" => Code::KeyA,
        "B" => Code::KeyB,
        "C" => Code::KeyC,
        "D" => Code::KeyD,
        "E" => Code::KeyE,
        "F" => Code::KeyF,
        "G" => Code::KeyG,
        "H" => Code::KeyH,
        "I" => Code::KeyI,
        "J" => Code::KeyJ,
        "K" => Code::KeyK,
        "L" => Code::KeyL,
        "M" => Code::KeyM,
        "N" => Code::KeyN,
        "O" => Code::KeyO,
        "P" => Code::KeyP,
        "Q" => Code::KeyQ,
        "R" => Code::KeyR,
        "S" => Code::KeyS,
        "T" => Code::KeyT,
        "U" => Code::KeyU,
        "V" => Code::KeyV,
        "W" => Code::KeyW,
        "X" => Code::KeyX,
        "Y" => Code::KeyY,
        "Z" => Code::KeyZ,
        "0" => Code::Digit0,
        "1" => Code::Digit1,
        "2" => Code::Digit2,
        "3" => Code::Digit3,
        "4" => Code::Digit4,
        "5" => Code::Digit5,
        "6" => Code::Digit6,
        "7" => Code::Digit7,
        "8" => Code::Digit8,
        "9" => Code::Digit9,
        "F1" => Code::F1,
        "F2" => Code::F2,
        "F3" => Code::F3,
        "F4" => Code::F4,
        "F5" => Code::F5,
        "F6" => Code::F6,
        "F7" => Code::F7,
        "F8" => Code::F8,
        "F9" => Code::F9,
        "F10" => Code::F10,
        "F11" => Code::F11,
        "F12" => Code::F12,
        "SPACE" => Code::Space,
        "ENTER" | "RETURN" => Code::Enter,
        "ESCAPE" | "ESC" => Code::Escape,
        "TAB" => Code::Tab,
        _ => return None,
    };

    Some(Shortcut::new(mods, code))
}

// ──────────────────────── Settings window helper ───────────────────────────

/// Show the settings window, creating it dynamically if the config-defined
/// instance was closed by the user.
#[cfg(desktop)]
fn show_settings_window(app: &tauri::AppHandle) {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    if let Some(win) = app.get_webview_window("settings") {
        let _ = win.show();
        let _ = win.set_focus();
    } else {
        let _ = WebviewWindowBuilder::new(
            app,
            "settings",
            WebviewUrl::App("settings.html".into()),
        )
        .title("No Sleep Please! Settings")
        .inner_size(480.0, 720.0)
        .resizable(false)
        .center()
        .build();
    }
}

// ──────────────────────── macOS app configuration ──────────────────────────

/// Configure the macOS app as an Accessory (no Dock icon, no menu bar).
/// Must be called before the Tauri builder runs.
#[cfg(target_os = "macos")]
fn configure_macos_app() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSMenu};

    if let Some(mtm) = MainThreadMarker::new() {
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        let empty_menu = NSMenu::new(mtm);
        app.setMainMenu(Some(&empty_menu));
    }
}

/// Create a dimmed version of an RGBA icon by reducing opacity.
fn create_dimmed_icon(icon: &tauri::image::Image<'_>) -> tauri::image::Image<'static> {
    let rgba = icon.rgba().to_vec();
    let mut dimmed = rgba.clone();
    // Reduce alpha channel to ~40% for a "greyed out" look
    for i in (3..dimmed.len()).step_by(4) {
        dimmed[i] = (dimmed[i] as f32 * 0.4) as u8;
    }
    tauri::image::Image::new_owned(dimmed, icon.width(), icon.height())
}

// ──────────────────────────── Application entry ────────────────────────────

/// Build and run the Tauri application (called from `main.rs`).
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize logging as early as possible.
    let _ = env_logger::try_init();

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_config,
            get_engine_state,
            toggle_engine,
            check_accessibility,
            request_accessibility,
            get_autostart_enabled,
            set_autostart_enabled,
            close_settings_window,
            get_translation,
            set_language,
            list_profiles,
            save_profile,
            load_profile,
            delete_profile,
            update_global_hotkey,
        ])
        .plugin(tauri_plugin_single_instance::init(|_app, _args, _cwd| {
            log::info!("Another instance attempted to start -- focusing existing instance");
        }))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            // ── Configuration ──────────────────────────────────────────
            let loaded_config = AppConfig::load().unwrap_or_else(|_| {
                log::info!("No config found; using defaults");
                let cfg = AppConfig::default();
                let _ = cfg.save();
                cfg
            });
            let shared_config: Arc<Mutex<AppConfig>> = Arc::new(Mutex::new(loaded_config.clone()));

            // ── i18n bundle ────────────────────────────────────────────
            let bundle = i18n::create_bundle(&loaded_config.language);
            let shared_bundle: Arc<Mutex<I18nBundle>> = Arc::new(Mutex::new(bundle));

            // ── Platform implementations ───────────────────────────────
            let mouse_driver = platform::create_mouse_driver();
            let power_inhibitor = platform::create_power_inhibitor();
            let perm_checker: Arc<dyn PermissionChecker> =
                Arc::from(platform::create_permission_checker());

            // ── Accessibility check on startup ─────────────────────────
            let has_accessibility = perm_checker.check_accessibility();
            let needs_mouse = loaded_config.jiggle_mode != JiggleMode::PowerOnly;
            if needs_mouse && !has_accessibility {
                log::warn!(
                    "Accessibility permission not granted — \
                     mouse jiggle modes will not work until access is granted"
                );
            }

            // ── Engine ─────────────────────────────────────────────────
            let engine = Engine::new(mouse_driver, power_inhibitor, Arc::clone(&shared_config));
            let shared_engine: Arc<Mutex<Engine>> = Arc::new(Mutex::new(engine));

            // Auto-start the engine on launch.
            if let Ok(mut eng) = shared_engine.lock() {
                if let Err(e) = eng.start() {
                    log::warn!("Engine auto-start failed: {}", e);
                }
            }

            // Manage state so Tauri commands can access it.
            app.manage(Arc::clone(&shared_config));
            app.manage(Arc::clone(&shared_engine));
            app.manage(Arc::clone(&perm_checker));
            app.manage(Arc::clone(&shared_bundle));

            // ── System tray ────────────────────────────────────────────
            #[cfg(desktop)]
            {
                use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
                use tauri::tray::TrayIconBuilder;

                // Resolve tray labels from the i18n bundle.
                let tray_bundle = i18n::create_bundle(&loaded_config.language);
                let tr = |id: &str| i18n::t(&tray_bundle, id);

                let text_inactive = format!("\u{25b6}\u{fe0f} {}", tr("tray-start"));
                let text_active = format!("\u{1f7e2} {}", tr("tray-running"));

                let toggle_item = MenuItem::with_id(
                    app,
                    "toggle_active",
                    &text_active,
                    true,
                    None::<&str>,
                )?;

                let mode_power =
                    MenuItem::with_id(app, "mode_power", tr("tray-mode-power"), true, None::<&str>)?;
                let mode_subtle =
                    MenuItem::with_id(app, "mode_subtle", tr("tray-mode-subtle"), true, None::<&str>)?;
                let mode_zen =
                    MenuItem::with_id(app, "mode_zen", tr("tray-mode-zen"), true, None::<&str>)?;

                let mode_circle =
                    MenuItem::with_id(app, "mode_circle", tr("tray-mode-circle"), true, None::<&str>)?;

                let mode_submenu = Submenu::with_id_and_items(
                    app,
                    "mode",
                    tr("tray-mode"),
                    true,
                    &[&mode_power, &mode_subtle, &mode_zen, &mode_circle],
                )?;

                // Accessibility permission indicator.
                let (acc_text, acc_enabled) = if has_accessibility {
                    (format!("\u{1f7e2} {}", tr("tray-accessibility-ok")), false)
                } else {
                    (format!("\u{26a0}\u{fe0f} {}", tr("tray-grant-accessibility")), true)
                };
                let accessibility_item = MenuItem::with_id(
                    app,
                    "grant_accessibility",
                    acc_text,
                    acc_enabled,
                    None::<&str>,
                )?;

                let sep1 = PredefinedMenuItem::separator(app)?;
                let sep2 = PredefinedMenuItem::separator(app)?;
                let sep3 = PredefinedMenuItem::separator(app)?;
                let settings_item =
                    MenuItem::with_id(app, "settings", tr("tray-settings"), true, None::<&str>)?;
                let quit_item =
                    MenuItem::with_id(app, "quit", tr("tray-quit"), true, None::<&str>)?;

                let menu = Menu::with_items(
                    app,
                    &[
                        &toggle_item,
                        &sep1,
                        &mode_submenu,
                        &sep2,
                        &accessibility_item,
                        &sep3,
                        &settings_item,
                        &quit_item,
                    ],
                )?;

                // Translated tooltip strings for the tray-menu event closure.
                let tip_active = tr("tray-tooltip-active");
                let tip_inactive = tr("tray-tooltip-inactive");
                let acc_ok_text = format!("\u{1f7e2} {}", tr("tray-accessibility-ok"));

                // Clones for the tray-menu closure.
                let engine_for_tray = Arc::clone(&shared_engine);
                let config_for_tray = Arc::clone(&shared_config);
                let perm_for_tray = Arc::clone(&perm_checker);
                let toggle_clone = toggle_item.clone();
                let accessibility_clone = accessibility_item.clone();
                let text_active_clone = text_active.clone();
                let text_inactive_clone = text_inactive.clone();
                // Extra clones for the global shortcut handler.
                let tip_active_for_sc = tip_active.clone();
                let tip_inactive_for_sc = tip_inactive.clone();

                let default_icon = app.default_window_icon().unwrap();
                let active_icon = tauri::image::Image::new_owned(
                    default_icon.rgba().to_vec(),
                    default_icon.width(),
                    default_icon.height(),
                );
                let inactive_icon = create_dimmed_icon(default_icon);
                let active_icon_clone = active_icon.clone();
                let inactive_icon_clone = inactive_icon.clone();
                // Extra clones for the global shortcut handler closure.
                let active_icon_for_sc = active_icon.clone();
                let inactive_icon_for_sc = inactive_icon.clone();

                TrayIconBuilder::with_id("main")
                    .icon(active_icon)
                    .tooltip(&tip_active)
                    .menu(&menu)
                    .show_menu_on_left_click(true)
                    .on_menu_event(move |app, event| {
                        match event.id().as_ref() {
                            "toggle_active" => {
                                if let Ok(mut eng) = engine_for_tray.lock() {
                                    let _ = eng.toggle();
                                    let active = eng.is_active();
                                    let label = if active {
                                        &text_active_clone
                                    } else {
                                        &text_inactive_clone
                                    };
                                    let _ = toggle_clone.set_text(label);
                                    if let Some(tray) = app.tray_by_id("main") {
                                        let tip = if active {
                                            &tip_active
                                        } else {
                                            &tip_inactive
                                        };
                                        let _ = tray.set_tooltip(Some(tip.as_str()));
                                        let icon = if active {
                                            active_icon_clone.clone()
                                        } else {
                                            inactive_icon_clone.clone()
                                        };
                                        let _ = tray.set_icon(Some(icon));
                                    }
                                }
                            }
                            "mode_power" => {
                                if let Ok(mut cfg) = config_for_tray.lock() {
                                    cfg.jiggle_mode = JiggleMode::PowerOnly;
                                    let _ = cfg.save();
                                }
                            }
                            "mode_subtle" => {
                                if let Ok(mut cfg) = config_for_tray.lock() {
                                    cfg.jiggle_mode = JiggleMode::MouseSubtle;
                                    let _ = cfg.save();
                                }
                            }
                            "mode_zen" => {
                                if let Ok(mut cfg) = config_for_tray.lock() {
                                    cfg.jiggle_mode = JiggleMode::MouseZen;
                                    let _ = cfg.save();
                                }
                            }
                            "mode_circle" => {
                                if let Ok(mut cfg) = config_for_tray.lock() {
                                    cfg.jiggle_mode = JiggleMode::MouseCircle;
                                    let _ = cfg.save();
                                }
                            }
                            "grant_accessibility" => {
                                if let Err(e) = perm_for_tray.request_accessibility() {
                                    log::error!("Failed to request accessibility: {}", e);
                                }
                                // Re-check after the user (potentially) grants access.
                                if perm_for_tray.check_accessibility() {
                                    let _ = accessibility_clone.set_text(&acc_ok_text);
                                    let _ = accessibility_clone.set_enabled(false);
                                }
                            }
                            "settings" => {
                                show_settings_window(app);
                            }
                            "quit" => {
                                // Ensure the engine is stopped before exiting.
                                if let Ok(mut eng) = engine_for_tray.lock() {
                                    let _ = eng.stop();
                                }
                                app.exit(0);
                            }
                            _ => {}
                        }
                    })
                    .build(app)?;

                // ── Global shortcut (configurable from config.global_hotkey) ──
                {
                    use tauri_plugin_global_shortcut::{
                        Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState,
                    };

                    // Try parsing the user-configured hotkey; fall back to default.
                    let shortcut = parse_shortcut_string(&loaded_config.global_hotkey)
                        .unwrap_or_else(|| {
                            log::warn!(
                                "Could not parse hotkey \"{}\"; using default CmdOrCtrl+Shift+J",
                                loaded_config.global_hotkey
                            );
                            #[cfg(target_os = "macos")]
                            let modifier = Modifiers::META;
                            #[cfg(not(target_os = "macos"))]
                            let modifier = Modifiers::CONTROL;

                            Shortcut::new(Some(modifier | Modifiers::SHIFT), Code::KeyJ)
                        });

                    // Clones for the shortcut handler closure (tray UI feedback).
                    let engine_for_shortcut = Arc::clone(&shared_engine);
                    let sc_text_active = text_active;
                    let sc_text_inactive = text_inactive;
                    let sc_tip_active = tip_active_for_sc;
                    let sc_tip_inactive = tip_inactive_for_sc;
                    let sc_active_icon = active_icon_for_sc;
                    let sc_inactive_icon = inactive_icon_for_sc;
                    let toggle_for_shortcut = toggle_item;

                    app.handle().plugin(
                        tauri_plugin_global_shortcut::Builder::new()
                            .with_handler(move |app, _shortcut, event| {
                                if matches!(event.state(), ShortcutState::Pressed) {
                                    if let Ok(mut eng) = engine_for_shortcut.lock() {
                                        let _ = eng.toggle();
                                        let active = eng.is_active();
                                        // Update toggle menu item text.
                                        let label = if active {
                                            &sc_text_active
                                        } else {
                                            &sc_text_inactive
                                        };
                                        let _ = toggle_for_shortcut.set_text(label);
                                        // Update tray icon and tooltip.
                                        if let Some(tray) = app.tray_by_id("main") {
                                            let tip = if active {
                                                &sc_tip_active
                                            } else {
                                                &sc_tip_inactive
                                            };
                                            let _ = tray.set_tooltip(Some(tip.as_str()));
                                            let icon = if active {
                                                sc_active_icon.clone()
                                            } else {
                                                sc_inactive_icon.clone()
                                            };
                                            let _ = tray.set_icon(Some(icon));
                                        }
                                        log::info!(
                                            "Global shortcut toggled engine → {}",
                                            eng.state_name()
                                        );
                                    }
                                }
                            })
                            .build(),
                    )?;

                    app.global_shortcut().register(shortcut)?;
                }
            }

            // ── Autostart plugin ───────────────────────────────────────
            #[cfg(desktop)]
            {
                app.handle().plugin(tauri_plugin_autostart::init(
                    tauri_plugin_autostart::MacosLauncher::LaunchAgent,
                    None,
                ))?;
            }

            log::info!("No Sleep Please! initialised");
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, event| {
            match &event {
                tauri::RunEvent::ExitRequested { api, code, .. } => {
                    // Only prevent exit when triggered by all windows closing (code = None).
                    // Allow explicit app.exit() calls (code = Some) so Quit works.
                    if code.is_none() {
                        api.prevent_exit();
                    }
                }
                #[cfg(target_os = "macos")]
                tauri::RunEvent::Ready => {
                    // Set Accessory policy AFTER Tauri finishes setup,
                    // otherwise Tauri overrides it during initialization.
                    configure_macos_app();
                }
                _ => {}
            }
        });
}
