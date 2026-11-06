/**
 * @spec-handoff
 * @interface Engine::new_with_clock(), set_jiggle_policy(), update_config(), start(), stop()
 * @behavior
 *   - A worker samples its injected clock and jiggles outside a schedule only with ForceOn.
 *   - Config and policy updates expose a coherent worker snapshot.
 *   - Stop releases inhibition before joining a blocked worker and prevents overlapping workers.
 *
 * @edge-cases
 *   - Non-policy config changes retain a valid ForceOn override.
 *   - A blocked worker does not permit a second acquisition before it is joined.
 */
// Core anti-inactivity engine with state machine and timer.
use crate::config::{AppConfig, JiggleMode};
use crate::platform::{MouseDriver, PowerInhibitor};
use crate::runtime::{
    normalize_override, should_jiggle, JiggleInput, JigglePolicy, LifecycleEvent, NormalizeInput,
};
#[cfg(test)]
use crate::schedule::SystemClock;
use crate::schedule::{evaluate_schedule, Clock, Schedule, ScheduleInput};
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

struct WorkerSnapshot {
    config: AppConfig,
    policy: JigglePolicy,
}

#[cfg(test)]
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
    #[cfg(test)]
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
                    Ok(()) => {
                        self.state = EngineState::Idle;
                        Err(format!("failed to spawn timer worker: {error}"))
                    }
                    Err(release_error) => Err(format!(
                        "failed to spawn timer worker: {error}; failed to release sleep inhibition: {release_error}"
                    )),
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

    pub fn is_active(&self) -> bool {
        self.state == EngineState::Active
    }
    #[cfg(test)]
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
        let snapshot = match worker_snapshot(&config, &worker_control) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                log::warn!("skipping jiggle: {error}");
                continue;
            }
        };
        let eligible = match worker_eligible(&snapshot, &clock) {
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
        let snapshot = match worker_snapshot(&config, &worker_control) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                log::warn!("skipping jiggle: {error}");
                continue;
            }
        };
        if !running.load(Ordering::Acquire)
            || !worker_eligible(&snapshot, &clock).unwrap_or_else(|error| {
                log::warn!("skipping jiggle: {error}");
                false
            })
        {
            continue;
        }
        let result = match snapshot.config.jiggle_mode {
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

fn worker_snapshot(
    config: &Arc<Mutex<AppConfig>>,
    worker_control: &Arc<Mutex<WorkerControl>>,
) -> Result<WorkerSnapshot, String> {
    let control = worker_control
        .lock()
        .map_err(|_| "jiggle policy lock poisoned".to_owned())?;
    let config = config
        .lock()
        .map_err(|_| "engine config lock poisoned".to_owned())?
        .clone();
    Ok(WorkerSnapshot {
        config,
        policy: control.policy.clone(),
    })
}

fn worker_eligible(snapshot: &WorkerSnapshot, clock: &Arc<dyn Clock>) -> Result<bool, String> {
    let schedule = Schedule::from_config(&snapshot.config)?;
    let window = evaluate_schedule(ScheduleInput {
        schedule: &schedule,
        at: clock.now(),
        clock: clock.as_ref(),
    })?;
    let override_state = normalize_override(NormalizeInput {
        mode: &snapshot.config.mode,
        schedule_enabled: schedule.enabled,
        window: &window,
        override_state: &snapshot.policy.override_state,
        event: LifecycleEvent::Tick,
    });
    Ok(should_jiggle(JiggleInput {
        mode: &snapshot.config.mode,
        jiggle_mode: &snapshot.config.jiggle_mode,
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
    use crate::runtime::OverrideState;
    use chrono::{TimeZone, Utc};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::Instant;

    const WORKER_EVENT_TIMEOUT: Duration = Duration::from_secs(10);

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

    struct CountingMouse {
        positions: Arc<AtomicUsize>,
        moves: Arc<AtomicUsize>,
    }

    struct SignalingMouse {
        jiggles: mpsc::Sender<()>,
    }

    impl MouseDriver for SignalingMouse {
        fn move_relative(&self, _: i32, _: i32) -> Result<(), String> {
            self.jiggles
                .send(())
                .map_err(|_| "jiggle observer disconnected".to_owned())
        }

        fn get_position(&self) -> Result<(i32, i32), String> {
            Ok((0, 0))
        }

        fn jiggle_zen(&self) -> Result<(), String> {
            self.jiggles
                .send(())
                .map_err(|_| "jiggle observer disconnected".to_owned())
        }
    }

    struct FixedClock(chrono::DateTime<Utc>);

    impl Clock for FixedClock {
        fn now(&self) -> chrono::DateTime<Utc> {
            self.0
        }

        fn to_local(
            &self,
            instant: chrono::DateTime<Utc>,
        ) -> chrono::DateTime<chrono::FixedOffset> {
            instant.with_timezone(&chrono::FixedOffset::east_opt(0).expect("UTC offset exists"))
        }

        fn resolve_local(
            &self,
            local: chrono::NaiveDateTime,
        ) -> chrono::LocalResult<chrono::DateTime<Utc>> {
            chrono::LocalResult::Single(local.and_utc())
        }
    }

    struct CountingFixedClock {
        instant: chrono::DateTime<Utc>,
        ticks: Arc<AtomicUsize>,
    }

    impl Clock for CountingFixedClock {
        fn now(&self) -> chrono::DateTime<Utc> {
            self.ticks.fetch_add(1, Ordering::SeqCst);
            self.instant
        }

        fn to_local(
            &self,
            instant: chrono::DateTime<Utc>,
        ) -> chrono::DateTime<chrono::FixedOffset> {
            instant.with_timezone(&chrono::FixedOffset::east_opt(0).expect("UTC offset exists"))
        }

        fn resolve_local(
            &self,
            local: chrono::NaiveDateTime,
        ) -> chrono::LocalResult<chrono::DateTime<Utc>> {
            chrono::LocalResult::Single(local.and_utc())
        }
    }

    impl MouseDriver for CountingMouse {
        fn move_relative(&self, _: i32, _: i32) -> Result<(), String> {
            self.moves.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn get_position(&self) -> Result<(i32, i32), String> {
            self.positions.fetch_add(1, Ordering::SeqCst);
            Ok((0, 0))
        }

        fn jiggle_zen(&self) -> Result<(), String> {
            self.moves.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct BarrierMouse {
        entered: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
        moves: Arc<AtomicUsize>,
    }

    impl MouseDriver for BarrierMouse {
        fn move_relative(&self, _: i32, _: i32) -> Result<(), String> {
            self.moves.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn get_position(&self) -> Result<(i32, i32), String> {
            self.entered
                .send(())
                .map_err(|_| "barrier observer disconnected".to_owned())?;
            self.release
                .lock()
                .map_err(|_| "barrier release lock poisoned".to_owned())?
                .recv_timeout(WORKER_EVENT_TIMEOUT)
                .map_err(|_| "barrier release timed out".to_owned())?;
            Ok((0, 0))
        }

        fn jiggle_zen(&self) -> Result<(), String> {
            self.moves.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct Power {
        inhibits: Arc<AtomicUsize>,
        releases: Arc<AtomicUsize>,
        fail_start: bool,
    }

    struct FailingReleasePower {
        releases: Arc<AtomicUsize>,
        failures_remaining: usize,
    }

    impl PowerInhibitor for FailingReleasePower {
        fn inhibit_sleep(&mut self, _: &str) -> Result<(), String> {
            Ok(())
        }

        fn release(&mut self) -> Result<(), String> {
            self.releases.fetch_add(1, Ordering::SeqCst);
            if self.failures_remaining > 0 {
                self.failures_remaining -= 1;
                Err("release failed".to_owned())
            } else {
                Ok(())
            }
        }

        fn is_active(&self) -> bool {
            true
        }
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

    struct SignalingPower {
        released: mpsc::Sender<()>,
        starts: Arc<AtomicUsize>,
    }

    impl PowerInhibitor for SignalingPower {
        fn inhibit_sleep(&mut self, _: &str) -> Result<(), String> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn release(&mut self) -> Result<(), String> {
            self.released
                .send(())
                .map_err(|_| "release observer disconnected".to_owned())
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

    #[test]
    fn a_second_stop_retries_a_failed_release_without_starting_another_worker() {
        let releases = Arc::new(AtomicUsize::new(0));
        let config = Arc::new(Mutex::new(AppConfig {
            interval_secs: 1,
            ..AppConfig::default()
        }));
        let mut engine = Engine::new(
            Box::new(Mouse),
            Box::new(FailingReleasePower {
                releases: Arc::clone(&releases),
                failures_remaining: 1,
            }),
            config,
        );

        engine.start().expect("initial acquisition succeeds");
        assert_eq!(
            engine.stop(),
            Err("failed to release sleep inhibition: release failed".to_owned())
        );
        assert!(
            engine.is_active(),
            "unconfirmed release retains active ownership"
        );
        engine
            .stop()
            .expect("a later stop retries the retained release");

        assert_eq!(releases.load(Ordering::SeqCst), 2);
        assert!(!engine.is_active());
    }

    #[test]
    fn jiggle_policy_retains_force_on_for_the_worker() {
        let (mut engine, _, _) = engine(false);
        let until = chrono::Utc::now();

        engine.set_jiggle_policy(JigglePolicy {
            override_state: OverrideState::ForceOn { until: Some(until) },
        });

        assert_eq!(
            engine.worker_control.lock().expect("policy lock").policy,
            JigglePolicy {
                override_state: OverrideState::ForceOn { until: Some(until) },
            }
        );
    }

    #[test]
    fn injected_clock_allows_outside_schedule_jiggles_only_for_force_on() {
        let outside = Utc
            .with_ymd_and_hms(2026, 3, 2, 18, 0, 0)
            .single()
            .expect("fixed instant exists");
        let config = Arc::new(Mutex::new(AppConfig {
            jiggle_mode: JiggleMode::MouseZen,
            interval_secs: 1,
            schedule_enabled: true,
            schedule_days: vec!["mon".to_owned()],
            ..AppConfig::default()
        }));
        let (without_force_sender, without_force_receiver) = mpsc::channel();
        let without_force_ticks = Arc::new(AtomicUsize::new(0));
        let (mut without_force, _, _) = engine_with_clock(
            Box::new(SignalingMouse {
                jiggles: without_force_sender,
            }),
            Arc::clone(&config),
            Arc::new(CountingFixedClock {
                instant: outside,
                ticks: Arc::clone(&without_force_ticks),
            }),
        );
        without_force
            .start()
            .expect("engine starts without ForceOn");
        wait_until(WORKER_EVENT_TIMEOUT, || {
            without_force_ticks.load(Ordering::SeqCst) >= 2
        });
        without_force.stop().expect("engine stops without ForceOn");
        assert!(
            without_force_receiver.try_recv().is_err(),
            "outside-window worker must not jiggle without ForceOn"
        );

        let (with_force_sender, with_force_receiver) = mpsc::channel();
        let (mut with_force, _, _) = engine_with_clock(
            Box::new(SignalingMouse {
                jiggles: with_force_sender,
            }),
            config,
            Arc::new(FixedClock(outside)),
        );
        with_force.set_jiggle_policy(JigglePolicy {
            override_state: OverrideState::ForceOn {
                until: Some(outside + chrono::Duration::hours(1)),
            },
        });
        with_force.start().expect("engine starts with ForceOn");
        with_force_receiver
            .recv_timeout(WORKER_EVENT_TIMEOUT)
            .expect("ForceOn permits the outside-window jiggle");
        with_force.stop().expect("engine stops with ForceOn");
    }

    #[test]
    fn config_updates_clear_force_on_only_when_the_runtime_policy_changes() {
        let outside = Utc
            .with_ymd_and_hms(2026, 3, 2, 18, 0, 0)
            .single()
            .expect("fixed instant exists");
        let config = Arc::new(Mutex::new(AppConfig {
            schedule_enabled: true,
            schedule_days: vec!["mon".to_owned()],
            ..AppConfig::default()
        }));
        let (mut engine, _, _) = engine_with_clock(
            Box::new(Mouse),
            Arc::clone(&config),
            Arc::new(FixedClock(outside)),
        );
        let force_on = JigglePolicy {
            override_state: OverrideState::ForceOn {
                until: Some(outside + chrono::Duration::hours(1)),
            },
        };
        engine.set_jiggle_policy(force_on.clone());

        let mut changed_schedule = config.lock().expect("config lock").clone();
        changed_schedule.schedule_days = vec!["tue".to_owned()];
        engine
            .update_config(changed_schedule.clone())
            .expect("policy change updates the worker snapshot");
        let snapshot = worker_snapshot(&engine.config, &engine.worker_control)
            .expect("worker snapshot is readable");
        assert_eq!(
            snapshot.config.schedule_days,
            changed_schedule.schedule_days
        );
        assert_eq!(snapshot.policy.override_state, OverrideState::None);

        engine.set_jiggle_policy(force_on.clone());
        let mut non_policy = changed_schedule;
        non_policy.interval_secs = 2;
        non_policy.jiggle_mode = JiggleMode::MouseZen;
        engine
            .update_config(non_policy.clone())
            .expect("non-policy update succeeds");
        let snapshot = worker_snapshot(&engine.config, &engine.worker_control)
            .expect("worker snapshot is readable");
        assert_eq!(snapshot.config.interval_secs, non_policy.interval_secs);
        assert_eq!(snapshot.config.jiggle_mode, non_policy.jiggle_mode);
        assert_eq!(snapshot.policy, force_on);
    }

    #[test]
    fn power_only_never_calls_the_mouse_driver_while_mouse_subtle_does() {
        let power_only_positions = Arc::new(AtomicUsize::new(0));
        let power_only_moves = Arc::new(AtomicUsize::new(0));
        let power_config = Arc::new(Mutex::new(AppConfig {
            jiggle_mode: JiggleMode::PowerOnly,
            interval_secs: 1,
            ..AppConfig::default()
        }));
        let power_only_ticks = Arc::new(AtomicUsize::new(0));
        let (mut power_only, _, _) = engine_with_clock(
            Box::new(CountingMouse {
                positions: power_only_positions.clone(),
                moves: power_only_moves.clone(),
            }),
            power_config,
            Arc::new(CountingFixedClock {
                instant: Utc::now(),
                ticks: Arc::clone(&power_only_ticks),
            }),
        );
        power_only.start().expect("PowerOnly engine starts");
        wait_until(WORKER_EVENT_TIMEOUT, || {
            power_only_ticks.load(Ordering::SeqCst) >= 2
        });
        power_only.stop().expect("PowerOnly engine stops");
        assert_eq!(power_only_positions.load(Ordering::SeqCst), 0);
        assert_eq!(power_only_moves.load(Ordering::SeqCst), 0);

        let mouse_positions = Arc::new(AtomicUsize::new(0));
        let mouse_moves = Arc::new(AtomicUsize::new(0));
        let mouse_config = Arc::new(Mutex::new(AppConfig {
            jiggle_mode: JiggleMode::MouseSubtle,
            interval_secs: 1,
            ..AppConfig::default()
        }));
        let (mut mouse_engine, _, _) = engine_with_mouse(
            Box::new(CountingMouse {
                positions: mouse_positions.clone(),
                moves: mouse_moves.clone(),
            }),
            mouse_config,
        );
        mouse_engine.start().expect("mouse engine starts");
        wait_until(WORKER_EVENT_TIMEOUT, || {
            mouse_moves.load(Ordering::SeqCst) > 0
        });
        mouse_engine.stop().expect("mouse engine stops");
        assert!(mouse_positions.load(Ordering::SeqCst) > 0);
        assert!(mouse_moves.load(Ordering::SeqCst) > 0);
    }

    #[test]
    fn stop_releases_inhibition_before_a_blocked_probe_and_starts_no_new_mouse_move() {
        let (entered_sender, entered_receiver) = mpsc::channel();
        let (continue_sender, continue_receiver) = mpsc::channel();
        let (released_sender, released_receiver) = mpsc::channel();
        let moves = Arc::new(AtomicUsize::new(0));
        let starts = Arc::new(AtomicUsize::new(0));
        let config = Arc::new(Mutex::new(AppConfig {
            jiggle_mode: JiggleMode::MouseSubtle,
            interval_secs: 1,
            ..AppConfig::default()
        }));
        let engine = Arc::new(Mutex::new(Engine::new(
            Box::new(BarrierMouse {
                entered: entered_sender,
                release: Mutex::new(continue_receiver),
                moves: moves.clone(),
            }),
            Box::new(SignalingPower {
                released: released_sender,
                starts: Arc::clone(&starts),
            }),
            config,
        )));
        engine
            .lock()
            .expect("engine lock")
            .start()
            .expect("engine starts");
        entered_receiver
            .recv_timeout(WORKER_EVENT_TIMEOUT)
            .expect("worker reaches its blocked activity probe");

        let stopping_engine = engine.clone();
        let (stopped_sender, stopped_receiver) = mpsc::channel();
        let stopper = thread::spawn(move || {
            stopped_sender.send(stopping_engine.lock().expect("engine lock for stop").stop())
        });
        released_receiver
            .recv_timeout(WORKER_EVENT_TIMEOUT)
            .expect("inhibitor releases before waiting for the blocked worker");
        let attempted_starts = Arc::new(AtomicUsize::new(0));
        let attempted_starts_in_thread = Arc::clone(&attempted_starts);
        let starting_engine = Arc::clone(&engine);
        let (started_sender, started_receiver) = mpsc::channel();
        let starter = thread::spawn(move || {
            attempted_starts_in_thread.fetch_add(1, Ordering::SeqCst);
            let result = starting_engine
                .lock()
                .expect("engine lock for start")
                .start();
            started_sender.send(result)
        });
        wait_until(WORKER_EVENT_TIMEOUT, || {
            attempted_starts.load(Ordering::SeqCst) == 1
        });
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        continue_sender
            .send(())
            .expect("worker probe is allowed to return");
        stopped_receiver
            .recv_timeout(WORKER_EVENT_TIMEOUT)
            .expect("stop completes after the probe returns")
            .expect("stop succeeds");
        assert_eq!(stopper.join().expect("stopper joins"), Ok(()));
        started_receiver
            .recv_timeout(WORKER_EVENT_TIMEOUT)
            .expect("second start completes only after the blocked worker joins")
            .expect("second start succeeds after cleanup");
        assert!(starter.join().expect("starter joins").is_ok());
        engine
            .lock()
            .expect("engine lock for final stop")
            .stop()
            .expect("replacement worker stops");

        assert_eq!(
            moves.load(Ordering::SeqCst),
            0,
            "no mouse movement may begin after inhibitor release"
        );
    }

    #[test]
    fn start_rejects_an_unfinished_worker_without_acquiring_a_second_lease() {
        let (mut engine, inhibits, _) = engine(false);
        let (release_sender, release_receiver) = mpsc::channel();
        engine.thread_handle = Some(thread::spawn(move || {
            release_receiver
                .recv_timeout(WORKER_EVENT_TIMEOUT)
                .expect("test releases the unfinished worker");
        }));

        assert_eq!(
            engine.start(),
            Err("timer worker cleanup is incomplete".to_owned())
        );
        assert_eq!(inhibits.load(Ordering::SeqCst), 0);

        release_sender
            .send(())
            .expect("unfinished worker is released");
        engine.stop().expect("cleanup joins the released worker");
    }

    fn engine_with_mouse(
        mouse: Box<dyn MouseDriver>,
        config: Arc<Mutex<AppConfig>>,
    ) -> (Engine, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let inhibits = Arc::new(AtomicUsize::new(0));
        let releases = Arc::new(AtomicUsize::new(0));
        let power = Power {
            inhibits: inhibits.clone(),
            releases: releases.clone(),
            fail_start: false,
        };
        (
            Engine::new(mouse, Box::new(power), config),
            inhibits,
            releases,
        )
    }

    fn engine_with_clock(
        mouse: Box<dyn MouseDriver>,
        config: Arc<Mutex<AppConfig>>,
        clock: Arc<dyn Clock>,
    ) -> (Engine, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let inhibits = Arc::new(AtomicUsize::new(0));
        let releases = Arc::new(AtomicUsize::new(0));
        let power = Power {
            inhibits: Arc::clone(&inhibits),
            releases: Arc::clone(&releases),
            fail_start: false,
        };
        (
            Engine::new_with_clock(mouse, Box::new(power), config, clock),
            inhibits,
            releases,
        )
    }

    fn wait_until(timeout: Duration, ready: impl Fn() -> bool) {
        let deadline = Instant::now() + timeout;
        while !ready() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(ready(), "condition did not become true before its timeout");
    }
}
