//! Core anti-inactivity engine with state machine and timer.

use crate::config::{AppConfig, JiggleMode};
use crate::platform::{MouseDriver, PowerInhibitor};
use crate::runtime::{
    normalize_override, should_jiggle, JiggleInput, JigglePolicy, LifecycleEvent, NormalizeInput,
};
use crate::schedule::{evaluate_schedule, Clock, Schedule, ScheduleInput, SystemClock};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub enum EngineState {
    Idle,
    Active,
}

struct WorkerControl {
    policy: JigglePolicy,
}

pub fn is_within_schedule(config: &AppConfig) -> bool {
    if !config.schedule_enabled {
        return true;
    }
    let clock = SystemClock;
    let schedule = match Schedule::from_config(config) {
        Ok(schedule) => schedule,
        Err(_) => return false,
    };
    evaluate_schedule(ScheduleInput {
        schedule: &schedule,
        at: clock.now(),
        clock: &clock,
    })
    .map(|evaluation| evaluation.in_window())
    .unwrap_or(false)
}

pub struct Engine {
    state: EngineState,
    mouse_driver: Arc<dyn MouseDriver>,
    power_inhibitor: Box<dyn PowerInhibitor>,
    config: Arc<Mutex<AppConfig>>,
    clock: Arc<dyn Clock>,
    worker_control: Arc<Mutex<WorkerControl>>,
    running: Arc<AtomicBool>,
    thread_handle: Option<JoinHandle<()>>,
}

impl Engine {
    pub fn new(
        mouse_driver: Box<dyn MouseDriver>,
        power_inhibitor: Box<dyn PowerInhibitor>,
        config: Arc<Mutex<AppConfig>>,
    ) -> Self {
        Self::new_with_clock(mouse_driver, power_inhibitor, config, Arc::new(SystemClock))
    }

    pub(crate) fn new_with_clock(
        mouse_driver: Box<dyn MouseDriver>,
        power_inhibitor: Box<dyn PowerInhibitor>,
        config: Arc<Mutex<AppConfig>>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            state: EngineState::Idle,
            mouse_driver: Arc::from(mouse_driver),
            power_inhibitor,
            config,
            clock,
            worker_control: Arc::new(Mutex::new(WorkerControl {
                policy: JigglePolicy {
                    override_state: crate::runtime::OverrideState::None,
                },
            })),
            running: Arc::new(AtomicBool::new(false)),
            thread_handle: None,
        }
    }

    pub fn start(&mut self) -> Result<(), String> {
        if self.is_active() {
            return Ok(());
        }
        if self.thread_handle.is_some() {
            return Err("timer worker cleanup is incomplete".to_owned());
        }
        self.power_inhibitor
            .inhibit_sleep("Stay Awake: keeping system awake")?;
        self.running.store(true, Ordering::Release);
        self.state = EngineState::Active;
        let mouse = Arc::clone(&self.mouse_driver);
        let config = Arc::clone(&self.config);
        let clock = Arc::clone(&self.clock);
        let worker_control = Arc::clone(&self.worker_control);
        let running = Arc::clone(&self.running);
        match thread::Builder::new()
            .name("stay-awake-jiggle".to_owned())
            .spawn(move || jiggle_loop(mouse, config, clock, worker_control, running))
        {
            Ok(handle) => {
                self.thread_handle = Some(handle);
                Ok(())
            }
            Err(error) => {
                self.running.store(false, Ordering::Release);
                match self.power_inhibitor.release() {
                    Ok(()) => { self.state = EngineState::Idle; Err(format!("failed to spawn timer worker: {error}")) }
                    Err(release_error) => Err(format!("failed to spawn timer worker: {error}; failed to release sleep inhibition: {release_error}")),
                }
            }
        }
    }

    pub fn stop(&mut self) -> Result<(), String> {
        self.running.store(false, Ordering::Release);
        let release_error = if self.is_active() {
            self.power_inhibitor.release().err()
        } else {
            None
        };
        if release_error.is_none() {
            self.state = EngineState::Idle;
        }
        let join_error = self
            .thread_handle
            .take()
            .and_then(|handle| handle.join().err())
            .map(|_| "failed to join timer worker".to_owned());
        match (release_error, join_error) {
            (None, None) => Ok(()),
            (Some(release), None) => Err(format!("failed to release sleep inhibition: {release}")),
            (None, Some(join)) => Err(join),
            (Some(release), Some(join)) => Err(format!(
                "failed to release sleep inhibition: {release}; {join}"
            )),
        }
    }

    pub fn toggle(&mut self) -> Result<(), String> {
        if self.is_active() {
            self.stop()
        } else {
            self.start()
        }
    }
    pub fn is_active(&self) -> bool {
        self.state == EngineState::Active
    }
    pub fn state_name(&self) -> &str {
        if self.is_active() {
            "active"
        } else {
            "idle"
        }
    }

    pub(crate) fn set_jiggle_policy(&mut self, policy: JigglePolicy) {
        match self.worker_control.lock() {
            Ok(mut control) => control.policy = policy,
            Err(_) => log::error!("jiggle policy lock poisoned"),
        }
    }

    pub(crate) fn update_config(&mut self, config: AppConfig) -> Result<(), String> {
        let mut control = self
            .worker_control
            .lock()
            .map_err(|_| "jiggle policy lock poisoned".to_owned())?;
        let mut current = self
            .config
            .lock()
            .map_err(|_| "engine config lock poisoned".to_owned())?;
        let old_policy = (
            crate::runtime::effective_mode(&current.mode),
            Schedule::from_config(&current)?,
        );
        let new_policy = (
            crate::runtime::effective_mode(&config.mode),
            Schedule::from_config(&config)?,
        );
        if old_policy != new_policy {
            control.policy.override_state = crate::runtime::OverrideState::None;
        }
        *current = config;
        Ok(())
    }
}

fn jiggle_loop(
    mouse: Arc<dyn MouseDriver>,
    config: Arc<Mutex<AppConfig>>,
    clock: Arc<dyn Clock>,
    worker_control: Arc<Mutex<WorkerControl>>,
    running: Arc<AtomicBool>,
) {
    let mut last_pos = None;
    while running.load(Ordering::Acquire) {
        let interval = match config.lock() {
            Ok(config) => Duration::from_secs(config.interval_secs.max(1)),
            Err(_) => {
                log::error!("engine config lock poisoned");
                break;
            }
        };
        let mut elapsed = Duration::ZERO;
        while elapsed < interval && running.load(Ordering::Acquire) {
            let step = Duration::from_millis(250).min(interval.saturating_sub(elapsed));
            thread::sleep(step);
            elapsed += step;
        }
        if !running.load(Ordering::Acquire) {
            break;
        }
        let eligible = match worker_eligible(&config, &clock, &worker_control) {
            Ok(eligible) => eligible,
            Err(error) => {
                log::warn!("skipping jiggle: {error}");
                false
            }
        };
        if !eligible {
            continue;
        }
        let current_pos = mouse.get_position().ok();
        let user_active =
            matches!((last_pos, current_pos), (Some(last), Some(current)) if last != current);
        last_pos = current_pos;
        if user_active {
            continue;
        }
        let mode = match config.lock() {
            Ok(config) => config.jiggle_mode.clone(),
            Err(_) => break,
        };
        let result = match mode {
            JiggleMode::MouseSubtle => mouse.move_relative(1, 0).and_then(|_| {
                thread::sleep(Duration::from_millis(50));
                mouse.move_relative(-1, 0)
            }),
            JiggleMode::MouseZen => mouse.jiggle_zen(),
            JiggleMode::MouseCircle => jiggle_circle(mouse.as_ref(), &running),
            JiggleMode::PowerOnly => Ok(()),
        };
        if let Err(error) = result {
            log::warn!("jiggle failed: {error}");
        }
    }
}

fn worker_eligible(
    config: &Arc<Mutex<AppConfig>>,
    clock: &Arc<dyn Clock>,
    worker_control: &Arc<Mutex<WorkerControl>>,
) -> Result<bool, String> {
    let control = worker_control
        .lock()
        .map_err(|_| "jiggle policy lock poisoned".to_owned())?;
    let config = config
        .lock()
        .map_err(|_| "engine config lock poisoned".to_owned())?
        .clone();
    let schedule = Schedule::from_config(&config)?;
    let window = evaluate_schedule(ScheduleInput {
        schedule: &schedule,
        at: clock.now(),
        clock: clock.as_ref(),
    })?;
    let override_state = normalize_override(NormalizeInput {
        mode: &config.mode,
        schedule_enabled: schedule.enabled,
        window: &window,
        override_state: &control.policy.override_state,
        event: LifecycleEvent::Tick,
    });
    Ok(should_jiggle(JiggleInput {
        mode: &config.mode,
        jiggle_mode: &config.jiggle_mode,
        engine_active: true,
        schedule_enabled: schedule.enabled,
        window: &window,
        override_state: &override_state,
    }))
}

fn jiggle_circle(mouse: &dyn MouseDriver, running: &AtomicBool) -> Result<(), String> {
    for (dx, dy) in [(1, 0), (0, 1), (-1, 0), (0, -1)] {
        if !running.load(Ordering::Acquire) {
            break;
        }
        mouse.move_relative(dx, dy)?;
        thread::sleep(Duration::from_millis(25));
    }
    Ok(())
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
    fn set_jiggle_policy(&mut self, policy: JigglePolicy) {
        Engine::set_jiggle_policy(self, policy)
    }
    fn update_config(&mut self, config: AppConfig) -> Result<(), String> {
        Engine::update_config(self, config)
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
