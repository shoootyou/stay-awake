/*
 * @spec-handoff
 * @interface RuntimeHandle::spawn(OwnerStart) -> Result<RuntimeHandle, String>
 * @interface RuntimeHandle::{post, request, query, update_config, shutdown}
 * @behavior
 *   - The owner serializes Startup, routes, snapshots, and config revisions on one thread.
 *   - Replies report actual backend state and retain queued work after a caller timeout.
 *   - Shutdown rejects new work and becomes exit-ready only after confirmed cleanup and one join.
 * @edge-cases
 *   - Disconnected replies, poisoned mirrors, and failed cleanup return errors without defaults.
 */
#![allow(dead_code)]

use crate::config::AppConfig;
use crate::i18n::{create_bundle, Bundle};
use crate::runtime::{EngineBackend, RuntimeSnapshot, Supervisor, SUPERVISOR_PERIOD};
use crate::schedule::Clock;
use crate::{handle_runtime_route, RuntimeRouteInput, TrayRoute, TraySink};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(1);
pub(crate) const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) type BackendFactory =
    Box<dyn FnOnce(AppConfig, Arc<dyn Clock>) -> Box<dyn EngineBackend> + Send + 'static>;

pub(crate) struct OwnerStart {
    pub(crate) config: Arc<Mutex<AppConfig>>,
    pub(crate) bundle: Arc<Mutex<Bundle>>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) backend_factory: BackendFactory,
    pub(crate) sink: Box<dyn TraySink + Send>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConfigOrigin {
    Settings,
    Profile,
    Language,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WifiObservation {
    pub(crate) ssid: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct OwnerReply {
    pub(crate) snapshot: Option<RuntimeSnapshot>,
    pub(crate) error: Option<String>,
    pub(crate) terminating: bool,
}

pub(crate) struct PendingReply {
    receiver: mpsc::Receiver<OwnerReply>,
}

impl PendingReply {
    /// A timeout leaves the queued command alone; callers must query rather than replay it.
    pub(crate) fn wait(self, timeout: Duration) -> Result<OwnerReply, String> {
        self.receiver
            .recv_timeout(timeout)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => {
                    "owner reply timed out; operation outcome is unknown".to_owned()
                }
                mpsc::RecvTimeoutError::Disconnected => {
                    "owner reply channel disconnected".to_owned()
                }
            })
    }
}

enum OwnerCommand {
    Route {
        route: TrayRoute,
        reply: Option<mpsc::Sender<OwnerReply>>,
    },
    Config {
        origin: ConfigOrigin,
        config: AppConfig,
        reply: mpsc::Sender<OwnerReply>,
    },
    Wifi {
        observation: WifiObservation,
        reply: Option<mpsc::Sender<OwnerReply>>,
    },
    Query {
        reply: mpsc::Sender<OwnerReply>,
    },
}

struct OwnerAlive(std::sync::Weak<Shared>);

impl Drop for OwnerAlive {
    fn drop(&mut self) {
        if let Some(shared) = self.0.upgrade() {
            shared.alive.store(false, Ordering::Release);
        }
    }
}

struct Shared {
    sender: mpsc::Sender<OwnerCommand>,
    config: Arc<Mutex<AppConfig>>,
    bundle: Arc<Mutex<Bundle>>,
    mutations: Mutex<()>,
    owner: Mutex<Option<JoinHandle<Result<(), String>>>>,
    shutdown: Mutex<()>,
    closing: AtomicBool,
    alive: AtomicBool,
    exit_ready: AtomicBool,
    exit_coordinator_running: AtomicBool,
}

#[derive(Clone)]
pub(crate) struct RuntimeHandle {
    shared: Arc<Shared>,
}

#[derive(Debug, Clone)]
pub(crate) struct TimeSample {
    pub(crate) wall: chrono::DateTime<chrono::Utc>,
    pub(crate) monotonic: Instant,
    pub(crate) offset_seconds: i32,
}

pub(crate) struct TimeChangeInput<'a> {
    pub(crate) previous: &'a TimeSample,
    pub(crate) current: &'a TimeSample,
}

/// Treat substantial sleeping, wall-clock jumps, and offset changes as a resume hint.
pub(crate) fn detect_time_change(input: TimeChangeInput<'_>) -> bool {
    let monotonic = input
        .current
        .monotonic
        .saturating_duration_since(input.previous.monotonic);
    let wall = input
        .current
        .wall
        .signed_duration_since(input.previous.wall);
    let wall = wall.abs().to_std().unwrap_or(Duration::MAX);
    monotonic >= Duration::from_secs(2)
        || monotonic.abs_diff(wall) >= Duration::from_secs(2)
        || input.previous.offset_seconds != input.current.offset_seconds
}

/// Chooses whether the loop must reconcile before accepting another queued command.
pub(crate) fn deadline_due(now: Instant, next_tick: Instant) -> bool {
    now >= next_tick
}

/// Test seam for persistence failures without changing the production save path.
pub(crate) fn persist_config_with<F>(config: &AppConfig, persist: F) -> Result<(), String>
where
    F: FnOnce(&AppConfig) -> Result<(), String>,
{
    persist(config)
}

impl RuntimeHandle {
    pub(crate) fn spawn(input: OwnerStart) -> Result<Self, String> {
        let initial_config = input
            .config
            .lock()
            .map_err(|_| "config lock poisoned".to_owned())?
            .clone();
        let (sender, receiver) = mpsc::channel();
        let shared = Arc::new(Shared {
            sender,
            config: input.config,
            bundle: input.bundle,
            mutations: Mutex::new(()),
            owner: Mutex::new(None),
            shutdown: Mutex::new(()),
            closing: AtomicBool::new(false),
            alive: AtomicBool::new(true),
            exit_ready: AtomicBool::new(false),
            exit_coordinator_running: AtomicBool::new(false),
        });
        let weak = Arc::downgrade(&shared);
        let clock = input.clock;
        let owner = thread::Builder::new()
            .name("stay-awake-owner".to_owned())
            .spawn(move || {
                let backend = (input.backend_factory)(initial_config.clone(), Arc::clone(&clock));
                owner_loop(receiver, weak, initial_config, clock, backend, input.sink)
            })
            .map_err(|error| format!("failed to spawn owner thread: {error}"))?;
        shared
            .owner
            .lock()
            .map_err(|_| "owner lock poisoned".to_owned())?
            .replace(owner);
        Ok(Self { shared })
    }

    fn send(&self, command: OwnerCommand) -> Result<(), String> {
        if self.is_closing() || !self.shared.alive.load(Ordering::Acquire) {
            return Err("runtime is shutting down".to_owned());
        }
        self.shared
            .sender
            .send(command)
            .map_err(|_| "runtime owner channel disconnected".to_owned())
    }

    pub(crate) fn post(&self, route: TrayRoute) -> Result<(), String> {
        if !matches!(
            route,
            TrayRoute::TrayToggle | TrayRoute::Hotkey | TrayRoute::IpcToggle | TrayRoute::Resume
        ) {
            return Err("route requires an internal owner command".to_owned());
        }
        self.send(OwnerCommand::Route { route, reply: None })
    }
    pub(crate) fn post_wifi(&self, observation: WifiObservation) -> Result<(), String> {
        self.send(OwnerCommand::Wifi {
            observation,
            reply: None,
        })
    }
    pub(crate) fn request(&self, route: TrayRoute) -> Result<PendingReply, String> {
        if !matches!(
            route,
            TrayRoute::TrayToggle | TrayRoute::Hotkey | TrayRoute::IpcToggle | TrayRoute::Resume
        ) {
            return Err("route requires an internal owner command".to_owned());
        }
        let (sender, receiver) = mpsc::channel();
        self.send(OwnerCommand::Route {
            route,
            reply: Some(sender),
        })?;
        Ok(PendingReply { receiver })
    }
    pub(crate) fn query(&self) -> Result<PendingReply, String> {
        let (reply, receiver) = mpsc::channel();
        self.send(OwnerCommand::Query { reply })?;
        Ok(PendingReply { receiver })
    }

    pub(crate) fn update_config<F, E>(
        &self,
        origin: ConfigOrigin,
        edit: F,
        effects: E,
    ) -> Result<PendingReply, String>
    where
        F: FnOnce(&mut AppConfig) -> Result<(), String>,
        E: FnOnce(&AppConfig, &AppConfig),
    {
        let _mutations = self
            .shared
            .mutations
            .lock()
            .map_err(|_| "configuration mutation lock poisoned".to_owned())?;
        if self.is_closing() || !self.shared.alive.load(Ordering::Acquire) {
            return Err("runtime is shutting down".to_owned());
        }
        let previous = self
            .shared
            .config
            .lock()
            .map_err(|_| "config lock poisoned".to_owned())?
            .clone();
        let mut candidate = previous.clone();
        edit(&mut candidate)?;
        if candidate.interval_secs == 0 {
            return Err("interval must be at least one second".to_owned());
        }
        crate::schedule::Schedule::from_config(&candidate)?;
        persist_config_with(&candidate, AppConfig::save)?;
        if candidate.language != previous.language {
            let replacement = create_bundle(&candidate.language);
            *self
                .shared
                .bundle
                .lock()
                .map_err(|_| "bundle lock poisoned after configuration save".to_owned())? =
                replacement;
        }
        *self
            .shared
            .config
            .lock()
            .map_err(|_| "config lock poisoned after configuration save".to_owned())? =
            candidate.clone();
        let (reply, receiver) = mpsc::channel();
        self.shared
            .sender
            .send(OwnerCommand::Config {
                origin,
                config: candidate.clone(),
                reply,
            })
            .map_err(|_| "configuration saved, runtime unavailable".to_owned())?;
        effects(&previous, &candidate);
        Ok(PendingReply { receiver })
    }

    pub(crate) fn shutdown(&self, timeout: Duration) -> Result<(), String> {
        let _shutdown = self
            .shared
            .shutdown
            .lock()
            .map_err(|_| "shutdown lock poisoned".to_owned())?;
        if self.exit_ready() {
            return Ok(());
        }
        self.shared.closing.store(true, Ordering::Release);
        let deadline = Instant::now() + timeout;
        if self.shared.alive.load(Ordering::Acquire) {
            let (reply, receiver) = mpsc::channel();
            self.shared
                .sender
                .send(OwnerCommand::Route {
                    route: TrayRoute::Quit,
                    reply: Some(reply),
                })
                .map_err(|_| "runtime owner channel disconnected".to_owned())?;
            let remaining = deadline.saturating_duration_since(Instant::now());
            let reply = receiver
                .recv_timeout(remaining)
                .map_err(|_| "owner shutdown reply timed out".to_owned())?;
            if !reply.terminating {
                return Err(reply
                    .error
                    .unwrap_or_else(|| "owner cleanup is incomplete".to_owned()));
            }
        }
        let mut owner = self
            .shared
            .owner
            .lock()
            .map_err(|_| "owner lock poisoned".to_owned())?;
        let Some(handle) = owner.as_ref() else {
            self.shared.exit_ready.store(true, Ordering::Release);
            return Ok(());
        };
        while !handle.is_finished() {
            if Instant::now() >= deadline {
                return Err("owner shutdown join timed out".to_owned());
            }
            thread::sleep(Duration::from_millis(10));
        }
        let handle = owner
            .take()
            .ok_or_else(|| "owner join handle disappeared".to_owned())?;
        handle
            .join()
            .map_err(|_| "owner thread panicked during shutdown".to_owned())??;
        self.shared.exit_ready.store(true, Ordering::Release);
        Ok(())
    }
    pub(crate) fn is_closing(&self) -> bool {
        self.shared.closing.load(Ordering::Acquire)
    }
    pub(crate) fn exit_ready(&self) -> bool {
        self.shared.exit_ready.load(Ordering::Acquire)
    }
    pub(crate) fn claim_exit_coordinator(&self) -> bool {
        let claimed = self
            .shared
            .exit_coordinator_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        if claimed {
            self.shared.closing.store(true, Ordering::Release);
        }
        claimed
    }
    pub(crate) fn release_exit_coordinator(&self) {
        self.shared
            .exit_coordinator_running
            .store(false, Ordering::Release);
    }
}

fn owner_loop(
    receiver: mpsc::Receiver<OwnerCommand>,
    shared: std::sync::Weak<Shared>,
    mut config: AppConfig,
    clock: Arc<dyn Clock>,
    mut backend: Box<dyn EngineBackend>,
    mut sink: Box<dyn TraySink + Send>,
) -> Result<(), String> {
    let _alive = OwnerAlive(shared.clone());
    let mut supervisor = Supervisor::new();
    let mut next_tick = Instant::now() + SUPERVISOR_PERIOD;
    let _startup = route_reply(
        TrayRoute::Startup,
        &config,
        &clock,
        &mut supervisor,
        backend.as_mut(),
        sink.as_mut(),
    );
    let mut last_ssid: Option<String>;
    loop {
        let now = Instant::now();
        if deadline_due(now, next_tick) {
            let _ = route_reply(
                TrayRoute::Tick,
                &config,
                &clock,
                &mut supervisor,
                backend.as_mut(),
                sink.as_mut(),
            );
            next_tick = Instant::now() + SUPERVISOR_PERIOD;
            continue;
        }
        let wait = next_tick.saturating_duration_since(now);
        match receiver.recv_timeout(wait) {
            Ok(command) => {
                let closing = shared
                    .upgrade()
                    .is_none_or(|state| state.closing.load(Ordering::Acquire));
                match command {
                    OwnerCommand::Query { reply } => {
                        let _ = reply.send(OwnerReply {
                            snapshot: supervisor.snapshot().cloned(),
                            error: None,
                            terminating: false,
                        });
                    }
                    OwnerCommand::Route { route, reply } => {
                        if closing && route != TrayRoute::Quit {
                            send_reply(
                                reply,
                                OwnerReply {
                                    snapshot: supervisor.snapshot().cloned(),
                                    error: Some("runtime is shutting down".to_owned()),
                                    terminating: false,
                                },
                            );
                            continue;
                        }
                        let result = route_reply(
                            route,
                            &config,
                            &clock,
                            &mut supervisor,
                            backend.as_mut(),
                            sink.as_mut(),
                        );
                        let result = if route == TrayRoute::Quit && !backend.is_active() {
                            match backend.stop() {
                                Ok(()) => result,
                                Err(error) => OwnerReply {
                                    snapshot: result.snapshot,
                                    error: Some(error),
                                    terminating: false,
                                },
                            }
                        } else {
                            result
                        };
                        let terminating = route == TrayRoute::Quit
                            && !backend.is_active()
                            && result.error.is_none();
                        send_reply(
                            reply,
                            OwnerReply {
                                terminating,
                                ..result
                            },
                        );
                        if terminating {
                            return Ok(());
                        }
                    }
                    OwnerCommand::Config {
                        origin,
                        config: candidate,
                        reply,
                    } => {
                        if closing {
                            let _ = reply.send(OwnerReply {
                                snapshot: supervisor.snapshot().cloned(),
                                error: Some(
                                    "configuration saved, runtime is shutting down".to_owned(),
                                ),
                                terminating: false,
                            });
                            continue;
                        }
                        match backend.update_config(candidate.clone()) {
                            Ok(()) => {
                                config = candidate;
                                let route = match origin {
                                    ConfigOrigin::Profile => TrayRoute::ProfileLoaded {
                                        policy_changed: true,
                                    },
                                    ConfigOrigin::Language => TrayRoute::LanguageChanged,
                                    _ => TrayRoute::ConfigChanged {
                                        policy_changed: true,
                                    },
                                };
                                let result = route_reply(
                                    route,
                                    &config,
                                    &clock,
                                    &mut supervisor,
                                    backend.as_mut(),
                                    sink.as_mut(),
                                );
                                let _ = reply.send(result);
                            }
                            Err(error) => {
                                let stop = backend.stop();
                                let message = match stop { Ok(()) => format!("configuration update failed: {error}"), Err(stop_error) => format!("configuration update failed: {error}; cleanup failed: {stop_error}") };
                                let _ = reply.send(OwnerReply {
                                    snapshot: supervisor.snapshot().cloned(),
                                    error: Some(message),
                                    terminating: false,
                                });
                            }
                        }
                    }
                    OwnerCommand::Wifi { observation, reply } => {
                        last_ssid = observation.ssid;
                        let active = last_ssid.as_ref().is_some_and(|ssid| {
                            config.wifi.networks.iter().any(|network| network == ssid)
                        });
                        let result = route_reply(
                            TrayRoute::WifiChanged { active },
                            &config,
                            &clock,
                            &mut supervisor,
                            backend.as_mut(),
                            sink.as_mut(),
                        );
                        send_reply(reply, result);
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = route_reply(
                    TrayRoute::Tick,
                    &config,
                    &clock,
                    &mut supervisor,
                    backend.as_mut(),
                    sink.as_mut(),
                );
                next_tick = Instant::now() + SUPERVISOR_PERIOD;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let reply = route_reply(
                    TrayRoute::Quit,
                    &config,
                    &clock,
                    &mut supervisor,
                    backend.as_mut(),
                    sink.as_mut(),
                );
                return reply.error.map_or(Ok(()), Err);
            }
        }
    }
}

fn route_reply(
    route: TrayRoute,
    config: &AppConfig,
    clock: &Arc<dyn Clock>,
    supervisor: &mut Supervisor,
    backend: &mut dyn EngineBackend,
    sink: &mut dyn TraySink,
) -> OwnerReply {
    match handle_runtime_route(
        RuntimeRouteInput {
            route,
            config,
            clock: clock.as_ref(),
            bundle: &create_bundle(&config.language),
        },
        supervisor,
        backend,
        sink,
    ) {
        Ok(snapshot) => OwnerReply {
            snapshot: Some(snapshot),
            error: None,
            terminating: false,
        },
        Err(error) => OwnerReply {
            snapshot: supervisor.snapshot().cloned(),
            error: Some(error),
            terminating: false,
        },
    }
}
fn send_reply(reply: Option<mpsc::Sender<OwnerReply>>, value: OwnerReply) {
    if let Some(reply) = reply {
        let _ = reply.send(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, FixedOffset, LocalResult, NaiveDateTime, TimeZone, Utc};
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct FixedClock(DateTime<Utc>);

    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }

        fn to_local(&self, instant: DateTime<Utc>) -> DateTime<FixedOffset> {
            instant.with_timezone(&FixedOffset::east_opt(0).expect("UTC offset exists"))
        }

        fn resolve_local(&self, local: NaiveDateTime) -> LocalResult<DateTime<Utc>> {
            LocalResult::Single(local.and_utc())
        }
    }

    #[test]
    fn detects_a_backward_wall_clock_jump_without_a_long_monotonic_wait() {
        let monotonic = Instant::now();
        let previous = TimeSample {
            wall: Utc
                .with_ymd_and_hms(2026, 3, 2, 8, 0, 0)
                .single()
                .expect("fixed instant exists"),
            monotonic,
            offset_seconds: 0,
        };
        let current = TimeSample {
            wall: previous.wall - chrono::Duration::seconds(5),
            monotonic: monotonic + Duration::from_millis(250),
            offset_seconds: 0,
        };

        assert!(detect_time_change(TimeChangeInput {
            previous: &previous,
            current: &current,
        }));
    }

    #[test]
    fn detects_forward_offset_and_long_wait_discontinuities_but_not_a_short_steady_wait() {
        let monotonic = Instant::now();
        let previous = TimeSample {
            wall: Utc
                .with_ymd_and_hms(2026, 3, 2, 8, 0, 0)
                .single()
                .expect("fixed instant exists"),
            monotonic,
            offset_seconds: 0,
        };
        let sample = |wall, elapsed, offset_seconds| TimeSample {
            wall,
            monotonic: monotonic + elapsed,
            offset_seconds,
        };

        let forward = sample(
            previous.wall + chrono::Duration::seconds(5),
            Duration::from_millis(250),
            0,
        );
        assert!(detect_time_change(TimeChangeInput {
            previous: &previous,
            current: &forward,
        }));
        let offset = sample(
            previous.wall + chrono::Duration::milliseconds(250),
            Duration::from_millis(250),
            3600,
        );
        assert!(detect_time_change(TimeChangeInput {
            previous: &previous,
            current: &offset,
        }));
        let long_wait = sample(
            previous.wall + chrono::Duration::seconds(2),
            Duration::from_secs(2),
            0,
        );
        assert!(detect_time_change(TimeChangeInput {
            previous: &previous,
            current: &long_wait,
        }));
        let steady = sample(
            previous.wall + chrono::Duration::milliseconds(250),
            Duration::from_millis(250),
            0,
        );
        assert!(!detect_time_change(TimeChangeInput {
            previous: &previous,
            current: &steady,
        }));
    }

    #[test]
    fn deadline_due_is_inclusive_and_persistence_failure_is_returned() {
        let now = Instant::now();
        assert!(!deadline_due(now, now + Duration::from_millis(1)));
        assert!(deadline_due(now, now));
        assert!(deadline_due(now + Duration::from_millis(1), now));

        assert_eq!(
            persist_config_with(&AppConfig::default(), |_| Err("save failed".to_owned())),
            Err("save failed".to_owned())
        );
    }

    #[derive(Default)]
    struct RecordingSink {
        frames: Arc<Mutex<Vec<(TrayRoute, crate::TrayPresentation)>>>,
    }

    impl TraySink for RecordingSink {
        fn publish(
            &mut self,
            route: TrayRoute,
            presentation: &crate::TrayPresentation,
        ) -> Result<(), String> {
            self.frames
                .lock()
                .expect("test sink lock")
                .push((route, presentation.clone()));
            Ok(())
        }
    }

    struct FailingSink;

    impl TraySink for FailingSink {
        fn publish(&mut self, _: TrayRoute, _: &crate::TrayPresentation) -> Result<(), String> {
            Err("tray enqueue failed".to_owned())
        }
    }

    fn owner_start(calls: Arc<Mutex<Vec<&'static str>>>) -> OwnerStart {
        owner_start_with_config(AppConfig::default(), calls)
    }

    fn owner_start_with_config(
        captured_config: AppConfig,
        calls: Arc<Mutex<Vec<&'static str>>>,
    ) -> OwnerStart {
        let now = Utc
            .with_ymd_and_hms(2026, 3, 2, 8, 0, 0)
            .single()
            .expect("fixed instant exists");
        OwnerStart {
            config: Arc::new(Mutex::new(captured_config)),
            bundle: Arc::new(Mutex::new(crate::i18n::create_bundle("en"))),
            clock: Arc::new(FixedClock(now)),
            backend_factory: Box::new(move |_, _| {
                Box::new(SharedRecordingBackend {
                    calls,
                    active: Arc::new(Mutex::new(false)),
                }) as Box<dyn EngineBackend>
            }),
            sink: Box::new(RecordingSink::default()),
        }
    }

    fn wifi_config(networks: &[&str]) -> AppConfig {
        AppConfig {
            mode: crate::config::AppMode::WiFi,
            wifi: crate::config::WifiConfig {
                enabled: true,
                networks: networks
                    .iter()
                    .map(|network| (*network).to_owned())
                    .collect(),
            },
            ..AppConfig::default()
        }
    }

    fn send_owner_config(handle: &RuntimeHandle, config: AppConfig) -> OwnerReply {
        let (reply, receiver) = mpsc::channel();
        handle
            .shared
            .sender
            .send(OwnerCommand::Config {
                origin: ConfigOrigin::Settings,
                config,
                reply,
            })
            .expect("ordered config command sends");
        receiver
            .recv_timeout(REQUEST_TIMEOUT)
            .expect("ordered config command replies")
    }

    fn send_owner_route(handle: &RuntimeHandle, route: TrayRoute) -> OwnerReply {
        let (reply, receiver) = mpsc::channel();
        handle
            .shared
            .sender
            .send(OwnerCommand::Route {
                route,
                reply: Some(reply),
            })
            .expect("ordered route command sends");
        receiver
            .recv_timeout(REQUEST_TIMEOUT)
            .expect("ordered route command replies")
    }

    struct SharedRecordingBackend {
        calls: Arc<Mutex<Vec<&'static str>>>,
        active: Arc<Mutex<bool>>,
    }

    impl EngineBackend for SharedRecordingBackend {
        fn start(&mut self) -> Result<(), String> {
            self.calls.lock().expect("backend log lock").push("start");
            *self.active.lock().expect("backend activity lock") = true;
            Ok(())
        }

        fn stop(&mut self) -> Result<(), String> {
            self.calls.lock().expect("backend log lock").push("stop");
            *self.active.lock().expect("backend activity lock") = false;
            Ok(())
        }

        fn is_active(&self) -> bool {
            *self.active.lock().expect("backend activity lock")
        }

        fn set_jiggle_policy(&mut self, _: crate::runtime::JigglePolicy) {
            self.calls.lock().expect("backend log lock").push("policy");
        }
    }

    struct FailingConfigPublicationBackend {
        active: Arc<Mutex<bool>>,
        starts: Arc<Mutex<usize>>,
        failures_remaining: usize,
    }

    impl EngineBackend for FailingConfigPublicationBackend {
        fn start(&mut self) -> Result<(), String> {
            *self.starts.lock().expect("backend start lock") += 1;
            *self.active.lock().expect("backend activity lock") = true;
            Ok(())
        }

        fn stop(&mut self) -> Result<(), String> {
            *self.active.lock().expect("backend activity lock") = false;
            Ok(())
        }

        fn is_active(&self) -> bool {
            *self.active.lock().expect("backend activity lock")
        }

        fn set_jiggle_policy(&mut self, _: crate::runtime::JigglePolicy) {}

        fn update_config(&mut self, _: AppConfig) -> Result<(), String> {
            if self.failures_remaining == 0 {
                Ok(())
            } else {
                self.failures_remaining -= 1;
                Err("worker configuration rejected".to_owned())
            }
        }
    }

    struct FailingStopBackend {
        active: Arc<Mutex<bool>>,
    }

    impl EngineBackend for FailingStopBackend {
        fn start(&mut self) -> Result<(), String> {
            *self.active.lock().expect("backend activity lock") = true;
            Ok(())
        }

        fn stop(&mut self) -> Result<(), String> {
            Err("release failed".to_owned())
        }

        fn is_active(&self) -> bool {
            *self.active.lock().expect("backend activity lock")
        }

        fn set_jiggle_policy(&mut self, _: crate::runtime::JigglePolicy) {}
    }

    #[test]
    fn owner_processes_internal_startup_once_before_a_queued_route() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let spawned = RuntimeHandle::spawn(owner_start(calls.clone()));

        assert!(
            spawned.is_ok(),
            "owner spawn must create the transport thread"
        );
        let handle = spawned.expect("assertion above establishes owner availability");
        handle
            .post(TrayRoute::IpcToggle)
            .expect("queued toggle sends");
        let reply = handle
            .query()
            .expect("query sends")
            .wait(REQUEST_TIMEOUT)
            .expect("query reply arrives");

        assert!(
            reply.snapshot.is_some(),
            "startup initializes a queryable snapshot"
        );
        let calls = calls.lock().expect("backend log lock");
        assert_eq!(calls.first(), Some(&"policy"));
        assert_eq!(
            calls.iter().filter(|call| **call == "policy").count(),
            2,
            "one startup and one queued toggle each publish one jiggle policy"
        );
        assert_eq!(calls.iter().filter(|call| **call == "start").count(), 1);
    }

    #[test]
    fn request_timeout_does_not_replay_work_and_dropped_reply_keeps_owner_alive() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let spawned = RuntimeHandle::spawn(owner_start(calls.clone()));

        assert!(
            spawned.is_ok(),
            "owner spawn must create the transport thread"
        );
        let handle = spawned.expect("assertion above establishes owner availability");
        let pending = handle.request(TrayRoute::IpcToggle).expect("request sends");
        assert!(
            pending.wait(Duration::ZERO).is_err(),
            "zero wait reports a bounded timeout"
        );
        let reply = handle
            .query()
            .expect("query after dropped reply sends")
            .wait(REQUEST_TIMEOUT)
            .expect("owner remains alive after reply drop");

        assert!(
            reply.snapshot.is_some(),
            "later query reads the completed operation"
        );
        assert_eq!(
            calls
                .lock()
                .expect("backend log lock")
                .iter()
                .filter(|call| **call == "start")
                .count(),
            1,
            "a timed-out toggle is applied at most once"
        );
    }

    #[test]
    fn update_config_serializes_a_candidate_before_its_owner_command() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let spawned = RuntimeHandle::spawn(owner_start(calls));

        assert!(
            spawned.is_ok(),
            "owner spawn must create the transport thread"
        );
        let handle = spawned.expect("assertion above establishes owner availability");
        let pending = handle
            .update_config(
                ConfigOrigin::Language,
                |config| {
                    config.language = "es".to_owned();
                    Ok(())
                },
                |_, _| {},
            )
            .expect("durable candidate queues its matching config command");

        assert!(
            pending.wait(REQUEST_TIMEOUT).is_ok(),
            "owner accepts the saved snapshot"
        );

        let source = include_str!("owner.rs");
        let update_config = &source[source
            .find("pub(crate) fn update_config")
            .expect("update_config remains defined")..];
        assert!(
            update_config
                .find("persist_config_with(&candidate, AppConfig::save)")
                .expect("candidate is saved")
                < update_config
                    .find("OwnerCommand::Config")
                    .expect("owner receives the saved candidate"),
            "disk persistence precedes runtime configuration delivery"
        );
    }

    #[test]
    fn unrelated_config_change_keeps_the_current_skip_window() {
        let config = AppConfig {
            schedule_enabled: true,
            schedule_start_hour: 7,
            schedule_end_hour: 9,
            schedule_days: vec!["mon".to_owned()],
            ..AppConfig::default()
        };
        let calls = Arc::new(Mutex::new(Vec::new()));
        let handle = RuntimeHandle::spawn(owner_start_with_config(config, calls.clone()))
            .expect("owner spawn succeeds");

        let paused = handle
            .request(TrayRoute::IpcToggle)
            .expect("toggle request sends")
            .wait(REQUEST_TIMEOUT)
            .expect("toggle reply arrives");
        assert_eq!(
            paused.snapshot.expect("toggle returns a snapshot").status,
            crate::runtime::TrayStatus::Paused,
            "the real owner records SkipWindow after stopping inside the current window"
        );

        let unchanged_policy = handle
            .update_config(
                ConfigOrigin::Other,
                |config| {
                    config.interval_secs = 60;
                    Ok(())
                },
                |_, _| {},
            )
            .expect("interval save queues its exact config snapshot")
            .wait(REQUEST_TIMEOUT)
            .expect("unrelated config change receives a reply");
        assert_eq!(
            unchanged_policy
                .snapshot
                .expect("config reply returns a snapshot")
                .status,
            crate::runtime::TrayStatus::Paused,
            "an interval-only update must not clear SkipWindow"
        );
        assert_eq!(
            calls
                .lock()
                .expect("backend log lock")
                .iter()
                .filter(|call| **call == "start")
                .count(),
            1,
            "the unrelated update must not restart the backend"
        );
    }

    #[test]
    fn entering_wifi_does_not_restart_from_an_activation_before_the_previous_wifi_session() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let handle = RuntimeHandle::spawn(owner_start_with_config(wifi_config(&["Office"]), calls))
            .expect("owner spawn succeeds");

        handle
            .post_wifi(WifiObservation {
                ssid: Some("Office".to_owned()),
            })
            .expect("registered observation queues");
        assert!(
            handle
                .query()
                .expect("query sends")
                .wait(REQUEST_TIMEOUT)
                .expect("query replies")
                .snapshot
                .expect("startup snapshot exists")
                .engine_active,
            "the first WiFi session acquires on a registered SSID"
        );

        let mut manual = wifi_config(&["Office"]);
        manual.mode = crate::config::AppMode::Manual;
        send_owner_config(&handle, manual);
        send_owner_config(&handle, wifi_config(&["Office"]));

        assert!(
            !handle
                .query()
                .expect("query sends")
                .wait(REQUEST_TIMEOUT)
                .expect("query replies")
                .snapshot
                .expect("re-entry snapshot exists")
                .engine_active,
            "entering WiFi waits for a new SSID observation rather than reusing activation"
        );
    }

    #[test]
    fn leaving_wifi_discards_the_last_ssid_before_a_later_wifi_session() {
        let handle = RuntimeHandle::spawn(owner_start_with_config(
            wifi_config(&["Office"]),
            Arc::new(Mutex::new(Vec::new())),
        ))
        .expect("owner spawn succeeds");

        handle
            .post_wifi(WifiObservation {
                ssid: Some("Office".to_owned()),
            })
            .expect("registered observation queues");
        let mut manual = wifi_config(&["Office"]);
        manual.mode = crate::config::AppMode::Manual;
        send_owner_config(&handle, manual);
        send_owner_config(&handle, wifi_config(&[]));

        assert!(
            !handle
                .query()
                .expect("query sends")
                .wait(REQUEST_TIMEOUT)
                .expect("query replies")
                .snapshot
                .expect("re-entry snapshot exists")
                .engine_active,
            "a previous SSID is cleared when WiFi mode ends"
        );
    }

    #[test]
    fn wifi_network_list_edits_recompute_membership_from_the_last_ssid_immediately() {
        let handle = RuntimeHandle::spawn(owner_start_with_config(
            wifi_config(&["Office"]),
            Arc::new(Mutex::new(Vec::new())),
        ))
        .expect("owner spawn succeeds");
        handle
            .post_wifi(WifiObservation {
                ssid: Some("Office".to_owned()),
            })
            .expect("registered observation queues");
        let active = handle
            .query()
            .expect("query sends")
            .wait(REQUEST_TIMEOUT)
            .expect("query replies")
            .snapshot
            .expect("active snapshot exists");
        assert!(
            active.engine_active,
            "registered SSID activates the backend"
        );

        let reply = send_owner_config(&handle, wifi_config(&[]));
        assert!(
            !reply
                .snapshot
                .expect("network-list reply includes a snapshot")
                .engine_active,
            "removing the current SSID stops WiFi activation without another observation"
        );
    }

    #[test]
    fn failed_config_publication_observes_cleanup_guards_ticks_and_recovers_for_quit() {
        let active = Arc::new(Mutex::new(false));
        let starts = Arc::new(Mutex::new(0));
        let backend_active = active.clone();
        let backend_starts = starts.clone();
        let now = Utc
            .with_ymd_and_hms(2026, 3, 2, 8, 0, 0)
            .single()
            .expect("fixed instant exists");
        let initial = AppConfig {
            mode: crate::config::AppMode::AlwaysOn,
            ..AppConfig::default()
        };
        let handle = RuntimeHandle::spawn(OwnerStart {
            config: Arc::new(Mutex::new(initial.clone())),
            bundle: Arc::new(Mutex::new(crate::i18n::create_bundle("en"))),
            clock: Arc::new(FixedClock(now)),
            backend_factory: Box::new(move |_, _| {
                Box::new(FailingConfigPublicationBackend {
                    active: backend_active,
                    starts: backend_starts,
                    failures_remaining: 1,
                })
            }),
            sink: Box::new(RecordingSink::default()),
        })
        .expect("owner spawn succeeds");
        let mut candidate = initial.clone();
        candidate.interval_secs = 60;

        let failed = send_owner_config(&handle, candidate.clone());
        assert!(
            failed.error.is_some(),
            "publication failure is returned to the caller"
        );
        assert!(
            !failed
                .snapshot
                .expect("failed publication returns an observed snapshot")
                .engine_active,
            "the failed reply reports the actual post-cleanup inactivity"
        );

        let starts_after_cleanup = *starts.lock().expect("backend start lock");
        let tick = send_owner_route(&handle, TrayRoute::Tick);
        assert!(tick.error.is_none(), "the guarded tick is observable");
        assert_eq!(
            *starts.lock().expect("backend start lock"),
            starts_after_cleanup,
            "a configuration fault blocks automatic restart"
        );

        let recovered = send_owner_config(&handle, candidate);
        assert!(
            recovered.error.is_none(),
            "the next publication clears the fault guard"
        );
        assert!(
            recovered
                .snapshot
                .expect("successful publication returns a snapshot")
                .engine_active,
            "a successful publication restores automatic reconciliation"
        );
        let quit = send_owner_route(&handle, TrayRoute::Quit);
        assert!(
            quit.terminating,
            "Quit remains available while fault recovery is pending"
        );
    }

    #[test]
    fn ticks_do_not_restart_the_backend_after_quit_cleanup_with_a_tray_error() {
        let config = AppConfig {
            schedule_enabled: true,
            schedule_start_hour: 7,
            schedule_end_hour: 9,
            schedule_days: vec!["mon".to_owned()],
            ..AppConfig::default()
        };
        let calls = Arc::new(Mutex::new(Vec::new()));
        let backend_calls = calls.clone();
        let active = Arc::new(Mutex::new(false));
        let backend_active = active.clone();
        let now = Utc
            .with_ymd_and_hms(2026, 3, 2, 8, 0, 0)
            .single()
            .expect("fixed instant exists");
        let handle = RuntimeHandle::spawn(OwnerStart {
            config: Arc::new(Mutex::new(config)),
            bundle: Arc::new(Mutex::new(crate::i18n::create_bundle("en"))),
            clock: Arc::new(FixedClock(now)),
            backend_factory: Box::new(move |_, _| {
                Box::new(SharedRecordingBackend {
                    calls: backend_calls,
                    active: backend_active,
                })
            }),
            sink: Box::new(FailingSink),
        })
        .expect("owner spawn succeeds");

        assert!(
            handle.shutdown(SHUTDOWN_TIMEOUT).is_err(),
            "a tray failure after confirmed cleanup keeps shutdown retryable"
        );
        thread::sleep(SUPERVISOR_PERIOD + Duration::from_millis(50));
        assert_eq!(
            calls
                .lock()
                .expect("backend log lock")
                .iter()
                .filter(|call| **call == "start")
                .count(),
            1,
            "automatic ticks must stay quiescent after shutdown begins"
        );
        assert!(
            !*active.lock().expect("backend activity lock"),
            "the confirmed release remains inactive while quit is retryable"
        );
    }

    #[test]
    fn shutdown_timeout_includes_waiting_for_another_shutdown_caller() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let handle = RuntimeHandle::spawn(owner_start(calls)).expect("owner spawn succeeds");
        let shared = handle.shared.clone();
        let (acquired, acquired_receiver) = mpsc::channel();
        let (release, release_receiver) = mpsc::channel();
        let holder = thread::spawn(move || {
            let _gate = shared.shutdown.lock().expect("shutdown gate lock");
            acquired.send(()).expect("test receives gate acquisition");
            release_receiver
                .recv()
                .expect("test releases shutdown gate");
        });
        acquired_receiver
            .recv()
            .expect("holder acquired the shutdown gate");

        let concurrent = handle.clone();
        let (result, result_receiver) = mpsc::channel();
        let (attempting, attempt_receiver) = mpsc::channel();
        let caller = thread::spawn(move || {
            let started = Instant::now();
            attempting.send(()).expect("test receives shutdown attempt");
            let outcome = concurrent.shutdown(Duration::from_millis(10));
            result
                .send((started.elapsed(), outcome))
                .expect("test receives concurrent shutdown result");
        });
        attempt_receiver
            .recv()
            .expect("concurrent caller reached shutdown");

        let returned_within_deadline = result_receiver
            .recv_timeout(Duration::from_millis(100))
            .is_ok();
        release.send(()).expect("release shutdown gate");
        holder.join().expect("gate holder joins");
        caller.join().expect("shutdown caller joins");
        assert!(
            returned_within_deadline,
            "a caller with a 10ms deadline must not wait behind another shutdown gate holder"
        );
    }

    #[test]
    fn route_reply_builds_its_bundle_from_the_accepted_language() {
        let source = include_str!("owner.rs");
        let route_start = source
            .find("fn route_reply(")
            .expect("route reply remains defined");
        let route_reply = &source[route_start
            ..source[route_start..]
                .find("fn send_reply(")
                .map(|offset| route_start + offset)
                .expect("route reply has a bounded production scope")];

        assert!(
            route_reply.contains("bundle: &create_bundle(&config.language)"),
            "each route renders with its accepted configuration language"
        );
    }

    #[test]
    fn shutdown_is_idempotent_and_rejects_new_routes_after_cleanup() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let spawned = RuntimeHandle::spawn(owner_start(calls.clone()));

        assert!(
            spawned.is_ok(),
            "owner spawn must create the transport thread"
        );
        let handle = spawned.expect("assertion above establishes owner availability");
        handle
            .shutdown(SHUTDOWN_TIMEOUT)
            .expect("first shutdown completes");
        handle
            .shutdown(SHUTDOWN_TIMEOUT)
            .expect("second shutdown is idempotent");

        assert!(
            handle.exit_ready(),
            "only confirmed cleanup and join set exit readiness"
        );
        assert!(
            handle.post(TrayRoute::IpcToggle).is_err(),
            "closing rejects new starts"
        );
        assert_eq!(
            calls
                .lock()
                .expect("backend log lock")
                .iter()
                .filter(|call| **call == "stop")
                .count(),
            1,
            "shutdown joins and releases once"
        );
    }

    #[test]
    fn failed_shutdown_release_does_not_mark_exit_ready() {
        let now = Utc
            .with_ymd_and_hms(2026, 3, 2, 8, 0, 0)
            .single()
            .expect("fixed instant exists");
        let active = Arc::new(Mutex::new(false));
        let failing_active = active.clone();
        let handle = RuntimeHandle::spawn(OwnerStart {
            config: Arc::new(Mutex::new(AppConfig::default())),
            bundle: Arc::new(Mutex::new(crate::i18n::create_bundle("en"))),
            clock: Arc::new(FixedClock(now)),
            backend_factory: Box::new(move |_, _| {
                Box::new(FailingStopBackend {
                    active: failing_active,
                })
            }),
            sink: Box::new(RecordingSink::default()),
        })
        .expect("owner spawn succeeds");
        handle
            .request(TrayRoute::IpcToggle)
            .expect("toggle request sends")
            .wait(REQUEST_TIMEOUT)
            .expect("toggle reply arrives");
        assert!(
            *active.lock().expect("backend activity lock"),
            "the test backend must acquire before exercising a retained release failure"
        );

        assert_eq!(
            handle.shutdown(SHUTDOWN_TIMEOUT),
            Err("release failed".to_owned())
        );
        assert!(
            !handle.exit_ready(),
            "failed release must keep exit blocked"
        );
        assert!(
            *active.lock().expect("backend activity lock"),
            "a failed release retains backend activity rather than deriving it from the call log"
        );
    }

    #[test]
    fn a_second_shutdown_does_not_report_exit_ready_after_a_failed_join() {
        let (sender, _receiver) = mpsc::channel();
        let shared = Arc::new(Shared {
            sender,
            config: Arc::new(Mutex::new(AppConfig::default())),
            bundle: Arc::new(Mutex::new(crate::i18n::create_bundle("en"))),
            mutations: Mutex::new(()),
            owner: Mutex::new(Some(thread::spawn(|| Err("owner failed".to_owned())))),
            shutdown: Mutex::new(()),
            closing: AtomicBool::new(false),
            alive: AtomicBool::new(false),
            exit_ready: AtomicBool::new(false),
            exit_coordinator_running: AtomicBool::new(false),
        });
        let handle = RuntimeHandle { shared };

        assert_eq!(
            handle.shutdown(SHUTDOWN_TIMEOUT),
            Err("owner failed".to_owned()),
            "the failed join must remain visible"
        );
        assert!(
            handle.shutdown(SHUTDOWN_TIMEOUT).is_err(),
            "a consumed failed join must not become idempotent shutdown success"
        );
        assert!(
            !handle.exit_ready(),
            "only a successfully joined owner may make exit readiness true"
        );
    }
}
