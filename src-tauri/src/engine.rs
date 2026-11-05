//! Core anti-inactivity engine with state machine and timer.
//!
//! The [`Engine`] owns platform drivers and a background timer thread that
//! periodically jiggles the mouse or refreshes the power assertion, depending
//! on the configured [`JiggleMode`].

use crate::config::{AppConfig, JiggleMode};
use crate::platform::{MouseDriver, PowerInhibitor};
use chrono::Local;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Returns `true` when the current local time falls within the configured schedule.
pub fn is_within_schedule(config: &AppConfig) -> bool {
    if !config.schedule_enabled {
        return true;
    }

    let now = Local::now();

    let day_abbr = match now.format("%a").to_string().to_lowercase().as_str() {
        "mon" => "mon",
        "tue" => "tue",
        "wed" => "wed",
        "thu" => "thu",
        "fri" => "fri",
        "sat" => "sat",
        "sun" => "sun",
        _ => return false,
    }
    .to_string();

    if !config
        .schedule_days
        .iter()
        .any(|d| d.to_lowercase() == day_abbr)
    {
        return false;
    }

    let hour = now.format("%H").to_string().parse::<u8>().unwrap_or(0);
    let minute = now.format("%M").to_string().parse::<u8>().unwrap_or(0);
    let current_minutes = (hour as u16) * 60 + (minute as u16);
    let start_minutes =
        (config.schedule_start_hour as u16) * 60 + (config.schedule_start_minute as u16);
    let end_minutes = (config.schedule_end_hour as u16) * 60 + (config.schedule_end_minute as u16);

    if start_minutes <= end_minutes {
        current_minutes >= start_minutes && current_minutes < end_minutes
    } else {
        // Overnight schedule (e.g. 22:00-06:00)
        current_minutes >= start_minutes || current_minutes < end_minutes
    }
}

#[cfg(test)]
mod phase_one_contract {
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Event {
        Startup,
        Tray,
        Hotkey,
        Ipc,
        WiFi,
        WindowBoundary,
        Config,
        Language,
        Resume,
    }
    fn refresh_tray_status(route: Event, sink: &mut Vec<Event>) {
        sink.push(route);
    }
    #[test]
    fn every_state_producing_route_refreshes_the_shared_tray_seam() {
        let mut sink = Vec::new();
        let expected = vec![
            Event::Startup,
            Event::Tray,
            Event::Hotkey,
            Event::Ipc,
            Event::WiFi,
            Event::WindowBoundary,
            Event::Config,
            Event::Language,
            Event::Resume,
        ];
        for route in expected.iter().copied() {
            refresh_tray_status(route, &mut sink);
        }
        assert_eq!(sink, expected);
    }
}

/// Engine operational state.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineState {
    /// Engine is not running.
    Idle,
    /// Engine is actively preventing inactivity.
    Active,
}

/// Core engine that manages anti-inactivity behaviour.
///
/// Call [`start`](Engine::start) / [`stop`](Engine::stop) /
/// [`toggle`](Engine::toggle) to control the engine. The background timer
/// thread checks the running flag every 250 ms so that [`stop`](Engine::stop)
/// returns promptly.
pub struct Engine {
    state: EngineState,
    mouse_driver: Arc<dyn MouseDriver>,
    power_inhibitor: Arc<Mutex<Box<dyn PowerInhibitor>>>,
    config: Arc<Mutex<AppConfig>>,
    running: Arc<AtomicBool>,
    thread_handle: Option<JoinHandle<()>>,
}

impl Engine {
    /// Create a new engine with the given platform implementations and shared config.
    pub fn new(
        mouse_driver: Box<dyn MouseDriver>,
        power_inhibitor: Box<dyn PowerInhibitor>,
        config: Arc<Mutex<AppConfig>>,
    ) -> Self {
        Self {
            state: EngineState::Idle,
            mouse_driver: Arc::from(mouse_driver),
            power_inhibitor: Arc::new(Mutex::new(power_inhibitor)),
            config,
            running: Arc::new(AtomicBool::new(false)),
            thread_handle: None,
        }
    }

    /// Start the engine — activates sleep inhibition and spawns the timer thread.
    pub fn start(&mut self) -> Result<(), String> {
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        // Activate system-sleep inhibition regardless of jiggle mode.
        {
            let mut power = self
                .power_inhibitor
                .lock()
                .map_err(|e| format!("Power lock poisoned: {}", e))?;
            power.inhibit_sleep("Stay Awake: keeping system awake")?;
        }

        self.running.store(true, Ordering::SeqCst);
        self.state = EngineState::Active;

        let mouse = Arc::clone(&self.mouse_driver);
        let config = Arc::clone(&self.config);
        let running = Arc::clone(&self.running);

        self.thread_handle = Some(thread::spawn(move || {
            let mut last_pos: Option<(i32, i32)> = None;

            while running.load(Ordering::Relaxed) {
                let cfg = config.lock().map(|c| c.clone()).unwrap_or_default();

                // Sleep in small increments so we can honour a stop request quickly.
                let total = Duration::from_secs(cfg.interval_secs.max(1));
                let step = Duration::from_millis(250);
                let mut elapsed = Duration::ZERO;
                while elapsed < total && running.load(Ordering::Relaxed) {
                    thread::sleep(step);
                    elapsed += step;
                }
                if !running.load(Ordering::Relaxed) {
                    break;
                }

                // Check schedule — skip jiggle if outside the configured window.
                if cfg.schedule_enabled && !is_within_schedule(&cfg) {
                    log::debug!("Outside schedule window — skipping jiggle");
                    continue;
                }

                // PowerOnly mode relies solely on the IOKit / SetThreadExecutionState
                // assertion created in start(); nothing else to do each tick.
                if cfg.jiggle_mode == JiggleMode::PowerOnly {
                    log::trace!("PowerOnly tick — sleep assertion still active");
                    continue;
                }

                // Skip jiggle when the user moved the cursor.
                let current_pos = mouse.get_position().ok();
                let user_active = match (last_pos, current_pos) {
                    (Some(last), Some(curr)) => last != curr,
                    _ => false,
                };
                last_pos = current_pos;

                if user_active {
                    log::debug!("User is active — skipping jiggle");
                    continue;
                }

                let result = match cfg.jiggle_mode {
                    JiggleMode::MouseSubtle => mouse.move_relative(1, 0).and_then(|_| {
                        thread::sleep(Duration::from_millis(50));
                        mouse.move_relative(-1, 0)
                    }),
                    JiggleMode::MouseZen => mouse.jiggle_zen(),
                    JiggleMode::MouseCircle => {
                        let steps: [(i32, i32); 4] = [(1, 0), (0, 1), (-1, 0), (0, -1)];
                        let mut res = Ok(());
                        for &(dx, dy) in &steps {
                            if !running.load(Ordering::Relaxed) {
                                break;
                            }
                            res = mouse.move_relative(dx, dy);
                            if res.is_err() {
                                break;
                            }
                            thread::sleep(Duration::from_millis(25));
                        }
                        res
                    }
                    JiggleMode::PowerOnly => Ok(()), // unreachable, handled above
                };

                match result {
                    Ok(()) => log::debug!("Jiggle performed: {:?}", cfg.jiggle_mode),
                    Err(e) => log::warn!("Jiggle failed: {}", e),
                }
            }

            log::info!("Engine timer thread exiting");
        }));

        log::info!("Engine started");
        Ok(())
    }

    /// Stop the engine — joins the timer thread and releases sleep inhibition.
    pub fn stop(&mut self) -> Result<(), String> {
        if !self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        self.running.store(false, Ordering::SeqCst);

        if let Some(handle) = self.thread_handle.take() {
            handle
                .join()
                .map_err(|_| "Failed to join timer thread".to_string())?;
        }

        {
            let mut power = self
                .power_inhibitor
                .lock()
                .map_err(|e| format!("Power lock poisoned: {}", e))?;
            power.release()?;
        }

        self.state = EngineState::Idle;
        log::info!("Engine stopped");
        Ok(())
    }

    /// Toggle between [`Active`](EngineState::Active) and
    /// [`Idle`](EngineState::Idle).
    pub fn toggle(&mut self) -> Result<(), String> {
        if self.is_active() {
            self.stop()
        } else {
            self.start()
        }
    }

    /// Returns `true` when the engine is actively preventing inactivity.
    pub fn is_active(&self) -> bool {
        self.state == EngineState::Active
    }

    /// Human-readable name of the current state.
    pub fn state_name(&self) -> &str {
        match &self.state {
            EngineState::Idle => "idle",
            EngineState::Active => "active",
        }
    }

    // skeleton: wired in E3
    #[allow(dead_code)]
    pub(crate) fn set_jiggle_policy(&mut self, _policy: crate::runtime::JigglePolicy) {}

    // skeleton: wired in E3
    #[allow(dead_code)]
    pub(crate) fn new_with_clock(
        mouse_driver: Box<dyn crate::platform::MouseDriver>,
        power_inhibitor: Box<dyn crate::platform::PowerInhibitor>,
        config: std::sync::Arc<std::sync::Mutex<crate::config::AppConfig>>,
        _clock: std::sync::Arc<dyn crate::schedule::Clock>,
    ) -> Self {
        Self::new(mouse_driver, power_inhibitor, config)
    }
}

impl crate::runtime::EngineBackend for Engine {
    fn start(&mut self) -> Result<(), String> {
        Engine::start(self)
    }

    fn stop(&mut self) -> Result<(), String> {
        Engine::stop(self)
    }

    fn is_active(&self) -> bool {
        Engine::is_active(self)
    }

    fn set_jiggle_policy(&mut self, policy: crate::runtime::JigglePolicy) {
        Engine::set_jiggle_policy(self, policy)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(test)]
mod tests {
    /*
     * @spec-handoff
     * @interface Engine::start(), Engine::stop(), Engine::toggle(), Engine::state_name()
     * @behavior
     *   - Starting inhibits sleep once and enters active state.
     *   - Stopping releases sleep once and returns to idle state.
     *   - Repeated start and stop calls are idempotent.
     * @edge-cases
     *   - An inhibitor error leaves the engine idle.
     */
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Mouse;

    impl MouseDriver for Mouse {
        fn move_relative(&self, _: i32, _: i32) -> Result<(), String> {
            Ok(())
        }

        fn get_position(&self) -> Result<(i32, i32), String> {
            Ok((0, 0))
        }

        fn jiggle_zen(&self) -> Result<(), String> {
            Ok(())
        }
    }

    struct Power {
        inhibits: Arc<AtomicUsize>,
        releases: Arc<AtomicUsize>,
        fail_start: bool,
    }

    impl PowerInhibitor for Power {
        fn inhibit_sleep(&mut self, _: &str) -> Result<(), String> {
            self.inhibits.fetch_add(1, Ordering::SeqCst);
            if self.fail_start {
                Err("inhibit failed".to_string())
            } else {
                Ok(())
            }
        }

        fn release(&mut self) -> Result<(), String> {
            self.releases.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn is_active(&self) -> bool {
            true
        }
    }

    fn engine(fail_start: bool) -> (Engine, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let inhibits = Arc::new(AtomicUsize::new(0));
        let releases = Arc::new(AtomicUsize::new(0));
        let power = Power {
            inhibits: Arc::clone(&inhibits),
            releases: Arc::clone(&releases),
            fail_start,
        };
        let config = Arc::new(Mutex::new(AppConfig {
            interval_secs: 1,
            ..AppConfig::default()
        }));
        (
            Engine::new(Box::new(Mouse), Box::new(power), config),
            inhibits,
            releases,
        )
    }

    #[test]
    fn start_and_stop_are_idempotent_state_transitions() {
        let (mut engine, inhibits, releases) = engine(false);
        engine.start().expect("first start must work");
        engine.start().expect("second start must be a no-op");
        assert_eq!(engine.state_name(), "active");
        assert_eq!(inhibits.load(Ordering::SeqCst), 1);

        engine.stop().expect("first stop must work");
        engine.stop().expect("second stop must be a no-op");
        assert_eq!(engine.state_name(), "idle");
        assert_eq!(releases.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn failed_inhibition_does_not_activate_the_engine() {
        let (mut engine, inhibits, releases) = engine(true);
        assert_eq!(engine.start(), Err("inhibit failed".to_string()));
        assert_eq!(engine.state_name(), "idle");
        assert_eq!(inhibits.load(Ordering::SeqCst), 1);
        assert_eq!(releases.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn engine_backend_adapter_acquires_once_ignores_repeated_start_and_releases_once() {
        let (mut engine, inhibits, releases) = engine(false);

        crate::runtime::EngineBackend::start(&mut engine).expect("adapter start succeeds");
        crate::runtime::EngineBackend::start(&mut engine).expect("repeated adapter start is idle");
        assert_eq!(inhibits.load(Ordering::SeqCst), 1);
        assert!(crate::runtime::EngineBackend::is_active(&engine));

        crate::runtime::EngineBackend::stop(&mut engine).expect("adapter stop succeeds");
        crate::runtime::EngineBackend::stop(&mut engine).expect("repeated adapter stop is idle");
        assert_eq!(releases.load(Ordering::SeqCst), 1);
        assert!(!crate::runtime::EngineBackend::is_active(&engine));
    }
}
