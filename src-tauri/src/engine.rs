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
    /*
     * @spec-handoff
     * @interface evaluate_schedule(schedule, clock) -> ScheduleResult; decide_lifecycle(input) -> Decision
     * @behavior
     *   - Schedule windows are start-inclusive, end-exclusive, and owned by their start day.
     *   - Lifecycle decisions reconcile schedule-owned Manual and AlwaysOn modes without changing WiFi lifecycle.
     *   - Tray presentation is derived from actual engine state and every route refreshes it.
     * @edge-cases
     *   - Empty days and equal bounds have no occurrence; DST gaps/folds resolve deterministically.
     *   - Overrides expire at their boundary and policy changes clear them.
     */
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Mode {
        Manual,
        AlwaysOn,
        WiFi,
        Scheduled,
    }
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Override {
        None,
        SkipWindow,
        ForceOn,
    }
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Presentation {
        Inactive,
        Active,
        Waiting,
        Paused,
        ActiveOverride,
    }
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum LocalResolution {
        Earlier,
        Later,
        FirstAfterGap,
    }
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Event {
        Startup,
        WindowBoundary,
        Tray,
        Hotkey,
        Ipc,
        WiFi,
        Config,
        Language,
        Resume,
    }

    // Fields are consumed only after Kou promotes this test seam to production.
    #[allow(dead_code)]
    #[derive(Clone, Copy, Debug)]
    struct Schedule {
        start: u16,
        end: u16,
        start_day: u8,
        days: &'static [u8],
    }
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct ScheduleResult {
        in_window: bool,
        next_start: Option<u16>,
    }
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct Decision {
        active: bool,
        override_state: Override,
        presentation: Presentation,
    }

    // Test-only placeholders use production-final names; Kou promotes these seams unchanged.
    fn evaluate_schedule(_: Schedule, _: u8, _: u16, _: LocalResolution) -> ScheduleResult {
        ScheduleResult {
            in_window: false,
            next_start: None,
        }
    }
    fn effective_mode(mode: Mode) -> Mode {
        mode
    }
    fn derive_tray_status(_: Mode, _: bool, _: bool, _: bool, _: Override) -> Presentation {
        Presentation::Inactive
    }
    fn should_jiggle(_: Mode, _: bool, _: bool, _: bool, _: Override) -> bool {
        false
    }
    fn decide_lifecycle(
        _: Mode,
        _: bool,
        _: ScheduleResult,
        _: bool,
        _: Override,
        _: Event,
    ) -> Decision {
        Decision {
            active: false,
            override_state: Override::None,
            presentation: Presentation::Inactive,
        }
    }
    fn refresh_tray_status(route: Event, sink: &mut Vec<Event>) {
        sink.push(route);
    }

    #[derive(Default)]
    struct FakeBackend {
        acquires: usize,
        releases: usize,
        mouse_calls: usize,
        active: bool,
    }

    fn reconcile_supervisor(
        _: &mut FakeBackend,
        _: ScheduleResult,
        _: Override,
        _: u64,
        _: bool,
    ) -> Result<Presentation, &'static str> {
        Ok(Presentation::Inactive)
    }

    const MON: &[u8] = &[1];
    const NONE: &[u8] = &[];

    #[test]
    fn schedule_is_start_inclusive_end_exclusive_and_empty_for_equal_bounds() {
        let weekday = Schedule {
            start: 9 * 60,
            end: 17 * 60,
            start_day: 1,
            days: MON,
        };
        assert!(evaluate_schedule(weekday, 1, 9 * 60, LocalResolution::Earlier).in_window);
        assert!(!evaluate_schedule(weekday, 1, 17 * 60, LocalResolution::Earlier).in_window);
        let empty = Schedule {
            start: 9 * 60,
            end: 9 * 60,
            start_day: 1,
            days: MON,
        };
        assert!(!evaluate_schedule(empty, 1, 10 * 60, LocalResolution::Earlier).in_window);
    }

    #[test]
    fn overnight_window_is_owned_by_its_selected_start_day_and_empty_days_have_no_window() {
        let overnight = Schedule {
            start: 22 * 60,
            end: 6 * 60,
            start_day: 1,
            days: MON,
        };
        assert!(evaluate_schedule(overnight, 2, 5 * 60, LocalResolution::Earlier).in_window);
        assert!(
            !evaluate_schedule(
                Schedule {
                    days: NONE,
                    ..overnight
                },
                2,
                5 * 60,
                LocalResolution::Earlier
            )
            .in_window
        );
    }

    #[test]
    fn schedule_resolution_has_explicit_gap_fold_and_next_window_contracts() {
        let schedule = Schedule {
            start: 2 * 60 + 30,
            end: 4 * 60,
            start_day: 1,
            days: MON,
        };
        assert_eq!(
            evaluate_schedule(schedule, 1, 60, LocalResolution::FirstAfterGap).next_start,
            Some(3 * 60)
        );
        assert!(evaluate_schedule(schedule, 1, 3 * 60, LocalResolution::Earlier).in_window);
        assert_eq!(
            evaluate_schedule(schedule, 1, 3 * 60, LocalResolution::Later).next_start,
            Some(2 * 60 + 30)
        );
    }

    #[test]
    fn scheduled_normalizes_to_manual_and_always_on_is_schedule_owned() {
        assert_eq!(effective_mode(Mode::Scheduled), Mode::Manual);
        let outside = ScheduleResult {
            in_window: false,
            next_start: Some(9 * 60),
        };
        assert!(
            !decide_lifecycle(
                Mode::AlwaysOn,
                true,
                outside,
                true,
                Override::None,
                Event::Startup
            )
            .active
        );
    }

    #[test]
    fn tray_truth_table_uses_actual_state_and_wifi_never_has_schedule_presentations() {
        let rows = [
            (
                Mode::WiFi,
                true,
                false,
                false,
                Override::None,
                Presentation::Active,
            ),
            (
                Mode::WiFi,
                false,
                true,
                true,
                Override::None,
                Presentation::Inactive,
            ),
            (
                Mode::Manual,
                true,
                false,
                false,
                Override::None,
                Presentation::Active,
            ),
            (
                Mode::AlwaysOn,
                false,
                true,
                false,
                Override::None,
                Presentation::Waiting,
            ),
            (
                Mode::Manual,
                false,
                true,
                true,
                Override::SkipWindow,
                Presentation::Paused,
            ),
            (
                Mode::Manual,
                true,
                true,
                false,
                Override::ForceOn,
                Presentation::ActiveOverride,
            ),
        ];
        for (mode, active, enabled, in_window, override_state, expected) in rows {
            assert_eq!(
                derive_tray_status(mode, active, enabled, in_window, override_state),
                expected
            );
        }
    }

    #[test]
    fn jiggle_policy_separates_wifi_and_force_on_and_never_allows_inactive_engine() {
        assert!(should_jiggle(
            Mode::Manual,
            true,
            true,
            false,
            Override::ForceOn
        ));
        assert!(!should_jiggle(
            Mode::WiFi,
            true,
            true,
            false,
            Override::ForceOn
        ));
        assert!(!should_jiggle(
            Mode::Manual,
            false,
            false,
            true,
            Override::None
        ));
    }

    #[test]
    fn lifecycle_creates_cancels_expires_and_resets_overrides() {
        let inside = ScheduleResult {
            in_window: true,
            next_start: Some(9 * 60),
        };
        let outside = ScheduleResult {
            in_window: false,
            next_start: Some(9 * 60),
        };
        assert_eq!(
            decide_lifecycle(
                Mode::Manual,
                true,
                inside,
                false,
                Override::None,
                Event::Tray
            )
            .override_state,
            Override::SkipWindow
        );
        assert_eq!(
            decide_lifecycle(
                Mode::Manual,
                true,
                outside,
                false,
                Override::None,
                Event::Tray
            )
            .override_state,
            Override::ForceOn
        );
        assert_eq!(
            decide_lifecycle(
                Mode::Manual,
                true,
                inside,
                true,
                Override::ForceOn,
                Event::WindowBoundary
            )
            .override_state,
            Override::None
        );
        assert_eq!(
            decide_lifecycle(
                Mode::Manual,
                true,
                outside,
                false,
                Override::ForceOn,
                Event::Config
            )
            .override_state,
            Override::None
        );
    }

    #[test]
    fn every_state_producing_route_refreshes_the_shared_tray_seam() {
        let mut sink = Vec::new();
        for route in [
            Event::Startup,
            Event::Tray,
            Event::Hotkey,
            Event::Ipc,
            Event::WiFi,
            Event::WindowBoundary,
            Event::Config,
            Event::Language,
            Event::Resume,
        ] {
            refresh_tray_status(route, &mut sink);
        }
        assert_eq!(
            sink,
            vec![
                Event::Startup,
                Event::Tray,
                Event::Hotkey,
                Event::Ipc,
                Event::WiFi,
                Event::WindowBoundary,
                Event::Config,
                Event::Language,
                Event::Resume
            ]
        );
    }

    #[test]
    fn supervisor_reconciles_idle_windows_independently_of_jiggle_interval() {
        let mut backend = FakeBackend::default();
        let inside = ScheduleResult {
            in_window: true,
            next_start: None,
        };
        assert_eq!(
            reconcile_supervisor(&mut backend, inside, Override::None, 300, false),
            Ok(Presentation::Active)
        );
        assert_eq!(backend.acquires, 1, "opening a window acquires once");
        assert!(backend.active);
        assert_eq!(
            reconcile_supervisor(&mut backend, inside, Override::None, 300, false),
            Ok(Presentation::Active)
        );
        assert_eq!(backend.acquires, 1, "repeated reconciliation is a no-op");
    }

    #[test]
    fn supervisor_releases_on_close_and_power_only_never_calls_mouse() {
        let mut backend = FakeBackend {
            active: true,
            ..FakeBackend::default()
        };
        let outside = ScheduleResult {
            in_window: false,
            next_start: Some(9 * 60),
        };
        assert_eq!(
            reconcile_supervisor(&mut backend, outside, Override::None, 300, true),
            Ok(Presentation::Waiting)
        );
        assert_eq!(
            backend.releases, 1,
            "window close releases even after worker cleanup error"
        );
        assert_eq!(backend.mouse_calls, 0, "PowerOnly has no mouse action");
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
}
