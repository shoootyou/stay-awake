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
use crate::config::AppConfig;
use crate::i18n::{create_bundle, Bundle};
use crate::runtime::{
    effective_mode, runtime_policy, EngineBackend, RuntimeSnapshot, Supervisor, SUPERVISOR_PERIOD,
};
use crate::schedule::Clock;
use crate::{
    build_tray_presentation, handle_runtime_route, refresh_tray_status, PresentationInput,
    RuntimeRouteInput, TrayRefreshInput, TrayRoute, TraySink,
};
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
    /// Injected presentation and diagnostics sink; its default diagnostic path logs errors.
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

pub(crate) enum OwnerCommand {
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

pub(crate) struct Shared {
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

/// Configuration transaction pieces can be supplied independently for failure tests.
pub(crate) struct OwnerShared {
    pub(crate) config: Arc<Mutex<AppConfig>>,
    pub(crate) bundle: Arc<Mutex<Bundle>>,
    pub(crate) mutations: Mutex<()>,
}

impl OwnerShared {
    pub(crate) fn new(
        config: Arc<Mutex<AppConfig>>,
        bundle: Arc<Mutex<Bundle>>,
        mutations: Mutex<()>,
    ) -> Self {
        Self {
            config,
            bundle,
            mutations,
        }
    }
}

/// A test can drop this receiver after persistence to simulate owner disconnection.
#[allow(dead_code)]
pub(crate) struct ConfigTransactionReceiver(mpsc::Receiver<OwnerCommand>);

#[allow(dead_code)]
impl ConfigTransactionReceiver {
    pub(crate) fn disconnect(self) {
        drop(self.0);
    }
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

/// Samples immediately before and after each receive wait for resume detection.
pub(crate) trait TimeSource: Send {
    fn sample(&mut self, clock: &dyn Clock) -> TimeSample;
}

/// The loop's monotonic deadline source is injectable without changing wall-clock sampling.
pub(crate) trait MonotonicClock: Send {
    fn now(&mut self) -> Instant;
}

struct SystemTimeSource;

struct SystemMonotonicClock;

impl TimeSource for SystemTimeSource {
    fn sample(&mut self, clock: &dyn Clock) -> TimeSample {
        let wall = clock.now();
        TimeSample {
            offset_seconds: clock.to_local(wall).offset().local_minus_utc(),
            wall,
            monotonic: Instant::now(),
        }
    }
}

impl MonotonicClock for SystemMonotonicClock {
    fn now(&mut self) -> Instant {
        Instant::now()
    }
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
        let transaction = OwnerShared::new(input.config, input.bundle, Mutex::new(()));
        let shared = Arc::new(Shared {
            sender,
            config: transaction.config,
            bundle: transaction.bundle,
            mutations: transaction.mutations,
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

    /// Builds a transaction-only handle without starting an owner thread.
    #[allow(dead_code)]
    pub(crate) fn for_config_transaction(
        transaction: OwnerShared,
    ) -> (Self, ConfigTransactionReceiver) {
        let (sender, receiver) = mpsc::channel();
        let shared = Shared {
            sender,
            config: transaction.config,
            bundle: transaction.bundle,
            mutations: transaction.mutations,
            owner: Mutex::new(None),
            shutdown: Mutex::new(()),
            closing: AtomicBool::new(false),
            alive: AtomicBool::new(true),
            exit_ready: AtomicBool::new(false),
            exit_coordinator_running: AtomicBool::new(false),
        };
        (
            Self {
                shared: Arc::new(shared),
            },
            ConfigTransactionReceiver(receiver),
        )
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
        self.update_config_with(origin, edit, effects, |candidate| {
            persist_config_with(&candidate, AppConfig::save)
        })
    }

    /// Uses an injected durable write so transaction failure ordering is testable.
    pub(crate) fn update_config_with<F, E, P>(
        &self,
        origin: ConfigOrigin,
        edit: F,
        effects: E,
        persist: P,
    ) -> Result<PendingReply, String>
    where
        F: FnOnce(&mut AppConfig) -> Result<(), String>,
        E: FnOnce(&AppConfig, &AppConfig),
        P: FnOnce(AppConfig) -> Result<(), String>,
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
        persist(candidate.clone())?;
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
        let deadline = Instant::now() + timeout;
        let _shutdown = loop {
            match self.shared.shutdown.try_lock() {
                Ok(lock) => break lock,
                Err(std::sync::TryLockError::Poisoned(_)) => {
                    return Err("shutdown lock poisoned".to_owned());
                }
                Err(std::sync::TryLockError::WouldBlock) if Instant::now() >= deadline => {
                    return Err("shutdown gate timed out".to_owned());
                }
                Err(std::sync::TryLockError::WouldBlock) => {
                    thread::sleep(Duration::from_millis(1));
                }
            }
        };
        if self.exit_ready() {
            return Ok(());
        }
        self.shared.closing.store(true, Ordering::Release);
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
        loop {
            let finished = self
                .shared
                .owner
                .lock()
                .map_err(|_| "owner lock poisoned".to_owned())?
                .as_ref()
                .map(JoinHandle::is_finished);
            let Some(finished) = finished else {
                return Err("owner shutdown outcome was not verified".to_owned());
            };
            if finished {
                break;
            }
            if Instant::now() >= deadline {
                return Err("owner shutdown join timed out".to_owned());
            }
            thread::sleep(Duration::from_millis(10));
        }
        let handle = self
            .shared
            .owner
            .lock()
            .map_err(|_| "owner lock poisoned".to_owned())?
            .take()
            .ok_or_else(|| "owner join handle disappeared".to_owned())?;
        let joined = handle
            .join()
            .map_err(|_| "owner thread panicked during shutdown".to_owned())
            .and_then(|result| result);
        joined?;
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
    config: AppConfig,
    clock: Arc<dyn Clock>,
    backend: Box<dyn EngineBackend>,
    sink: Box<dyn TraySink + Send>,
) -> Result<(), String> {
    owner_loop_with_time_source(
        receiver,
        shared,
        config,
        clock,
        backend,
        sink,
        &mut SystemTimeSource,
    )
}

fn owner_loop_with_time_source(
    receiver: mpsc::Receiver<OwnerCommand>,
    shared: std::sync::Weak<Shared>,
    config: AppConfig,
    clock: Arc<dyn Clock>,
    backend: Box<dyn EngineBackend>,
    sink: Box<dyn TraySink + Send>,
    time_source: &mut dyn TimeSource,
) -> Result<(), String> {
    owner_loop_with_time_sources(
        receiver,
        shared,
        config,
        clock,
        backend,
        sink,
        time_source,
        &mut SystemMonotonicClock,
    )
}

/// Runs the owner loop with explicit wall and monotonic sources for deadline tests.
#[allow(dead_code)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn owner_loop_with_time_sources(
    receiver: mpsc::Receiver<OwnerCommand>,
    shared: std::sync::Weak<Shared>,
    mut config: AppConfig,
    clock: Arc<dyn Clock>,
    mut backend: Box<dyn EngineBackend>,
    mut sink: Box<dyn TraySink + Send>,
    time_source: &mut dyn TimeSource,
    monotonic_clock: &mut dyn MonotonicClock,
) -> Result<(), String> {
    let _alive = OwnerAlive(shared.clone());
    let mut supervisor = Supervisor::new();
    let mut next_tick = monotonic_clock.now() + SUPERVISOR_PERIOD;
    let startup = route_reply(
        TrayRoute::Startup,
        &config,
        &clock,
        &mut supervisor,
        backend.as_mut(),
        sink.as_mut(),
    );
    report_unreplied_error(sink.as_mut(), &startup);
    let mut last_ssid = None;
    let mut configuration_fault = false;
    let mut last_automatic_error = None;
    let mut before_wait = time_source.sample(clock.as_ref());
    let mut last_observed_offset = before_wait.offset_seconds;
    let mut pending_resume = false;
    loop {
        let now = monotonic_clock.now();
        let closing = shared
            .upgrade()
            .is_none_or(|state| state.closing.load(Ordering::Acquire));
        if deadline_due(now, next_tick) && !closing {
            let after_wait = time_source.sample(clock.as_ref());
            last_observed_offset = after_wait.offset_seconds;
            let route = if pending_resume
                || detect_time_change(TimeChangeInput {
                    previous: &before_wait,
                    current: &after_wait,
                }) {
                TrayRoute::Resume
            } else {
                TrayRoute::Tick
            };
            let result = if configuration_fault {
                observe_configuration_fault(
                    &config,
                    &clock,
                    &mut supervisor,
                    backend.as_ref(),
                    sink.as_mut(),
                )
            } else {
                route_reply(
                    route,
                    &config,
                    &clock,
                    &mut supervisor,
                    backend.as_mut(),
                    sink.as_mut(),
                )
            };
            report_automatic_error(sink.as_mut(), &mut last_automatic_error, &result);
            pending_resume = false;
            next_tick = monotonic_clock.now() + SUPERVISOR_PERIOD;
            before_wait = time_source.sample(clock.as_ref());
            pending_resume |= last_observed_offset != before_wait.offset_seconds;
            last_observed_offset = before_wait.offset_seconds;
            continue;
        }
        let wait = next_tick.saturating_duration_since(now);
        match receiver.recv_timeout(wait) {
            Ok(command) => {
                let after_wait = time_source.sample(clock.as_ref());
                let closing = shared
                    .upgrade()
                    .is_none_or(|state| state.closing.load(Ordering::Acquire));
                let offset_changed = last_observed_offset != after_wait.offset_seconds;
                last_observed_offset = after_wait.offset_seconds;
                if !closing
                    && !offset_changed
                    && (pending_resume
                        || detect_time_change(TimeChangeInput {
                            previous: &before_wait,
                            current: &after_wait,
                        }))
                {
                    let result = if configuration_fault {
                        observe_configuration_fault(
                            &config,
                            &clock,
                            &mut supervisor,
                            backend.as_ref(),
                            sink.as_mut(),
                        )
                    } else {
                        route_reply(
                            TrayRoute::Resume,
                            &config,
                            &clock,
                            &mut supervisor,
                            backend.as_mut(),
                            sink.as_mut(),
                        )
                    };
                    report_automatic_error(sink.as_mut(), &mut last_automatic_error, &result);
                    pending_resume = false;
                }
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
                        let fault_blocks_route = configuration_fault
                            && route != TrayRoute::Quit
                            && !(matches!(
                                route,
                                TrayRoute::TrayToggle | TrayRoute::Hotkey | TrayRoute::IpcToggle
                            ) && backend.is_active());
                        let result = if fault_blocks_route {
                            let mut observed = observe_configuration_fault(
                                &config,
                                &clock,
                                &mut supervisor,
                                backend.as_ref(),
                                sink.as_mut(),
                            );
                            if matches!(
                                route,
                                TrayRoute::TrayToggle | TrayRoute::Hotkey | TrayRoute::IpcToggle
                            ) {
                                observed.error = Some(
                                    "configuration fault blocks starting a new backend lease"
                                        .to_owned(),
                                );
                            }
                            observed
                        } else {
                            route_reply(
                                route,
                                &config,
                                &clock,
                                &mut supervisor,
                                backend.as_mut(),
                                sink.as_mut(),
                            )
                        };
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
                        let terminating = route == TrayRoute::Quit && !backend.is_active();
                        if terminating {
                            report_unreplied_error(sink.as_mut(), &result);
                        }
                        let no_reply = reply.is_none();
                        if no_reply {
                            report_unreplied_error(sink.as_mut(), &result);
                        }
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
                        let previous_policy = runtime_policy(&config)?;
                        let candidate_policy = runtime_policy(&candidate)?;
                        let policy_changed = previous_policy != candidate_policy;
                        let networks_changed = config.wifi.networks != candidate.wifi.networks;
                        let entered_wifi = effective_mode(&config.mode)
                            != crate::config::AppMode::WiFi
                            && effective_mode(&candidate.mode) == crate::config::AppMode::WiFi;
                        let left_wifi = effective_mode(&config.mode)
                            == crate::config::AppMode::WiFi
                            && effective_mode(&candidate.mode) != crate::config::AppMode::WiFi;
                        if entered_wifi {
                            supervisor.reset_wifi_activation();
                            last_ssid = None;
                        }
                        if left_wifi {
                            last_ssid = None;
                        }
                        match backend.update_config(candidate.clone()) {
                            Ok(()) => {
                                let recovering = configuration_fault;
                                config = candidate;
                                configuration_fault = false;
                                let route = if recovering {
                                    TrayRoute::Startup
                                } else {
                                    match origin {
                                        ConfigOrigin::Profile => {
                                            TrayRoute::ProfileLoaded { policy_changed }
                                        }
                                        ConfigOrigin::Language => TrayRoute::LanguageChanged,
                                        _ => TrayRoute::ConfigChanged { policy_changed },
                                    }
                                };
                                let mut result = route_reply(
                                    route,
                                    &config,
                                    &clock,
                                    &mut supervisor,
                                    backend.as_mut(),
                                    sink.as_mut(),
                                );
                                if effective_mode(&config.mode) == crate::config::AppMode::WiFi
                                    && !entered_wifi
                                    && networks_changed
                                {
                                    let active = last_ssid.as_ref().is_some_and(|ssid| {
                                        config.wifi.networks.iter().any(|network| network == ssid)
                                    });
                                    result = route_reply(
                                        TrayRoute::WifiChanged { active },
                                        &config,
                                        &clock,
                                        &mut supervisor,
                                        backend.as_mut(),
                                        sink.as_mut(),
                                    );
                                }
                                let _ = reply.send(result);
                            }
                            Err(error) => {
                                configuration_fault = true;
                                let stop = backend.stop();
                                let message = match stop {
                                    Ok(()) => format!("configuration update failed: {error}"),
                                    Err(stop_error) => {
                                        format!("configuration update failed: {error}; cleanup failed: {stop_error}")
                                    }
                                };
                                let observed = observe_configuration_fault(
                                    &config,
                                    &clock,
                                    &mut supervisor,
                                    backend.as_ref(),
                                    sink.as_mut(),
                                );
                                let error = match observed.error {
                                    Some(observation_error) => {
                                        format!("{message}; {observation_error}")
                                    }
                                    None => message,
                                };
                                let _ = reply.send(OwnerReply {
                                    snapshot: observed.snapshot,
                                    error: Some(error),
                                    terminating: false,
                                });
                            }
                        }
                    }
                    OwnerCommand::Wifi { observation, reply } => {
                        if closing {
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
                        last_ssid = observation.ssid;
                        let active = last_ssid.as_ref().is_some_and(|ssid| {
                            config.wifi.networks.iter().any(|network| network == ssid)
                        });
                        let result = if configuration_fault {
                            observe_configuration_fault(
                                &config,
                                &clock,
                                &mut supervisor,
                                backend.as_ref(),
                                sink.as_mut(),
                            )
                        } else {
                            route_reply(
                                TrayRoute::WifiChanged { active },
                                &config,
                                &clock,
                                &mut supervisor,
                                backend.as_mut(),
                                sink.as_mut(),
                            )
                        };
                        let no_reply = reply.is_none();
                        if no_reply {
                            report_unreplied_error(sink.as_mut(), &result);
                        }
                        send_reply(reply, result);
                    }
                }
                if !closing && offset_changed {
                    let result = if configuration_fault {
                        observe_configuration_fault(
                            &config,
                            &clock,
                            &mut supervisor,
                            backend.as_ref(),
                            sink.as_mut(),
                        )
                    } else {
                        route_reply(
                            TrayRoute::Resume,
                            &config,
                            &clock,
                            &mut supervisor,
                            backend.as_mut(),
                            sink.as_mut(),
                        )
                    };
                    report_automatic_error(sink.as_mut(), &mut last_automatic_error, &result);
                    pending_resume = false;
                }
                before_wait = time_source.sample(clock.as_ref());
                pending_resume |= last_observed_offset != before_wait.offset_seconds;
                last_observed_offset = before_wait.offset_seconds;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let after_wait = time_source.sample(clock.as_ref());
                last_observed_offset = after_wait.offset_seconds;
                let closing = shared
                    .upgrade()
                    .is_none_or(|state| state.closing.load(Ordering::Acquire));
                if !closing {
                    let route = if pending_resume
                        || detect_time_change(TimeChangeInput {
                            previous: &before_wait,
                            current: &after_wait,
                        }) {
                        TrayRoute::Resume
                    } else {
                        TrayRoute::Tick
                    };
                    let result = if configuration_fault {
                        observe_configuration_fault(
                            &config,
                            &clock,
                            &mut supervisor,
                            backend.as_ref(),
                            sink.as_mut(),
                        )
                    } else {
                        route_reply(
                            route,
                            &config,
                            &clock,
                            &mut supervisor,
                            backend.as_mut(),
                            sink.as_mut(),
                        )
                    };
                    report_automatic_error(sink.as_mut(), &mut last_automatic_error, &result);
                    pending_resume = false;
                }
                next_tick = monotonic_clock.now() + SUPERVISOR_PERIOD;
                before_wait = time_source.sample(clock.as_ref());
                pending_resume |= last_observed_offset != before_wait.offset_seconds;
                last_observed_offset = before_wait.offset_seconds;
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

fn observe_configuration_fault(
    config: &AppConfig,
    clock: &Arc<dyn Clock>,
    supervisor: &mut Supervisor,
    backend: &dyn EngineBackend,
    sink: &mut dyn TraySink,
) -> OwnerReply {
    if let Err(error) = supervisor.observe(
        crate::runtime::ReconcileInput {
            config,
            clock: clock.as_ref(),
            event: crate::runtime::LifecycleEvent::Tick,
        },
        backend,
    ) {
        return OwnerReply {
            snapshot: None,
            error: Some(format!("configuration fault observation failed: {error}")),
            terminating: false,
        };
    }
    let snapshot = supervisor.snapshot().cloned();
    let error = snapshot.as_ref().and_then(|snapshot| {
        let bundle = create_bundle(&config.language);
        let presentation = build_tray_presentation(PresentationInput {
            snapshot,
            bundle: &bundle,
            locale: &config.language,
            clock: clock.as_ref(),
        });
        refresh_tray_status(
            TrayRefreshInput {
                route: TrayRoute::LanguageChanged,
                presentation: &presentation,
            },
            sink,
        )
        .err()
    });
    OwnerReply {
        snapshot,
        error,
        terminating: false,
    }
}

fn send_reply(reply: Option<mpsc::Sender<OwnerReply>>, value: OwnerReply) {
    if let Some(reply) = reply {
        let _ = reply.send(value);
    }
}

fn report_unreplied_error(sink: &mut dyn TraySink, reply: &OwnerReply) {
    if let Some(error) = &reply.error {
        sink.report_error(error);
    }
}

fn report_automatic_error(
    sink: &mut dyn TraySink,
    last_error: &mut Option<String>,
    reply: &OwnerReply,
) {
    match &reply.error {
        Some(error) if last_error.as_deref() != Some(error) => {
            sink.report_error(error);
            *last_error = Some(error.clone());
        }
        Some(_) => {}
        None => *last_error = None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, FixedOffset, LocalResult, NaiveDateTime, TimeZone, Utc};
    use std::collections::VecDeque;
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
        errors: Arc<Mutex<Vec<String>>>,
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

        fn report_error(&mut self, error: &str) {
            self.errors
                .lock()
                .expect("test sink error lock")
                .push(error.to_owned());
        }
    }

    struct ScriptedTimeSource {
        samples: VecDeque<TimeSample>,
    }

    impl TimeSource for ScriptedTimeSource {
        fn sample(&mut self, _: &dyn Clock) -> TimeSample {
            self.samples
                .pop_front()
                .expect("owner loop must not sample beyond the test script")
        }
    }

    struct ScriptedMonotonicClock {
        instants: VecDeque<Instant>,
    }

    impl MonotonicClock for ScriptedMonotonicClock {
        fn now(&mut self) -> Instant {
            self.instants
                .pop_front()
                .expect("owner loop must not read beyond the monotonic clock script")
        }
    }

    struct RouteSink {
        routes: mpsc::Sender<TrayRoute>,
        errors: Arc<Mutex<Vec<String>>>,
        tick_results: Arc<Mutex<VecDeque<Result<(), String>>>>,
    }

    impl TraySink for RouteSink {
        fn publish(&mut self, route: TrayRoute, _: &crate::TrayPresentation) -> Result<(), String> {
            self.routes
                .send(route)
                .map_err(|_| "test route observer disconnected".to_owned())?;
            if route == TrayRoute::Tick {
                self.tick_results
                    .lock()
                    .expect("test tick result lock")
                    .pop_front()
                    .unwrap_or(Ok(()))
            } else {
                Ok(())
            }
        }

        fn report_error(&mut self, error: &str) {
            self.errors
                .lock()
                .expect("test sink error lock")
                .push(error.to_owned());
        }
    }

    fn steady_time_samples(count: usize) -> VecDeque<TimeSample> {
        let base = Instant::now();
        let wall = Utc
            .with_ymd_and_hms(2026, 3, 2, 8, 0, 0)
            .single()
            .expect("fixed instant exists");
        (0..count)
            .map(|index| TimeSample {
                wall: wall + chrono::Duration::milliseconds((index * 250) as i64),
                monotonic: base + Duration::from_millis((index * 250) as u64),
                offset_seconds: 0,
            })
            .collect()
    }

    fn scheduled_active_config() -> AppConfig {
        AppConfig {
            schedule_enabled: true,
            schedule_start_hour: 7,
            schedule_end_hour: 9,
            schedule_days: vec!["mon".to_owned()],
            ..AppConfig::default()
        }
    }

    fn owner_loop_shared(sender: mpsc::Sender<OwnerCommand>) -> Arc<Shared> {
        Arc::new(Shared {
            sender,
            config: Arc::new(Mutex::new(AppConfig::default())),
            bundle: Arc::new(Mutex::new(crate::i18n::create_bundle("en"))),
            mutations: Mutex::new(()),
            owner: Mutex::new(None),
            shutdown: Mutex::new(()),
            closing: AtomicBool::new(false),
            alive: AtomicBool::new(true),
            exit_ready: AtomicBool::new(false),
            exit_coordinator_running: AtomicBool::new(false),
        })
    }

    fn stop_owner_loop(
        sender: &mpsc::Sender<OwnerCommand>,
        owner: thread::JoinHandle<Result<(), String>>,
    ) {
        let (reply, receiver) = mpsc::channel();
        sender
            .send(OwnerCommand::Route {
                route: TrayRoute::Quit,
                reply: Some(reply),
            })
            .expect("Quit reaches the owner loop");
        assert!(
            receiver
                .recv_timeout(REQUEST_TIMEOUT)
                .expect("Quit reply arrives")
                .terminating,
            "Quit must terminate the direct owner loop"
        );
        owner
            .join()
            .expect("owner loop thread joins")
            .expect("owner loop exits cleanly");
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
        stop_failures_remaining: usize,
    }

    impl EngineBackend for FailingConfigPublicationBackend {
        fn start(&mut self) -> Result<(), String> {
            *self.starts.lock().expect("backend start lock") += 1;
            *self.active.lock().expect("backend activity lock") = true;
            Ok(())
        }

        fn stop(&mut self) -> Result<(), String> {
            if self.stop_failures_remaining > 0 {
                self.stop_failures_remaining -= 1;
                return Err("release failed".to_owned());
            }
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

    struct FailingStartBackend {
        active: Arc<Mutex<bool>>,
        starts: Arc<Mutex<usize>>,
        failures_remaining: usize,
    }

    impl EngineBackend for FailingStartBackend {
        fn start(&mut self) -> Result<(), String> {
            *self.starts.lock().expect("backend start lock") += 1;
            if self.failures_remaining == 0 {
                *self.active.lock().expect("backend activity lock") = true;
                Ok(())
            } else {
                self.failures_remaining -= 1;
                Err("backend start failed".to_owned())
            }
        }

        fn stop(&mut self) -> Result<(), String> {
            *self.active.lock().expect("backend activity lock") = false;
            Ok(())
        }

        fn is_active(&self) -> bool {
            *self.active.lock().expect("backend activity lock")
        }

        fn set_jiggle_policy(&mut self, _: crate::runtime::JigglePolicy) {}
    }

    struct BlockingFailingStartBackend {
        entered: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
        blocking_starts_remaining: usize,
    }

    impl EngineBackend for BlockingFailingStartBackend {
        fn start(&mut self) -> Result<(), String> {
            if self.blocking_starts_remaining > 0 {
                self.blocking_starts_remaining -= 1;
                self.entered
                    .send(())
                    .map_err(|_| "test start observer disconnected".to_owned())?;
                self.release
                    .recv_timeout(REQUEST_TIMEOUT)
                    .map_err(|_| "test start release timed out".to_owned())?;
            }
            Err("backend start failed".to_owned())
        }

        fn stop(&mut self) -> Result<(), String> {
            Ok(())
        }

        fn is_active(&self) -> bool {
            false
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
    fn config_transaction_reports_saved_but_not_applied_when_owner_disconnects_after_save() {
        let initial = AppConfig::default();
        let config = Arc::new(Mutex::new(initial.clone()));
        let bundle = Arc::new(Mutex::new(crate::i18n::create_bundle("en")));
        let (handle, receiver) = RuntimeHandle::for_config_transaction(OwnerShared::new(
            config.clone(),
            bundle,
            Mutex::new(()),
        ));
        let persisted = Arc::new(AtomicBool::new(false));
        let saved = persisted.clone();

        let error = handle
            .update_config_with(
                ConfigOrigin::Settings,
                |candidate| {
                    candidate.interval_secs = 60;
                    Ok(())
                },
                |_, _| panic!("effects must not run when runtime delivery disconnects"),
                move |_| {
                    saved.store(true, Ordering::Release);
                    receiver.disconnect();
                    Ok(())
                },
            )
            .err()
            .expect("a disconnected owner cannot accept a saved configuration");

        assert!(
            persisted.load(Ordering::Acquire),
            "the candidate was saved first"
        );
        assert_eq!(error, "configuration saved, runtime unavailable");
        assert_eq!(
            config
                .lock()
                .expect("mirror remains readable")
                .interval_secs,
            60,
            "the durable saved mirror is retained without inventing runtime acceptance"
        );
        assert_eq!(
            initial.interval_secs, 30,
            "the fixture starts with its prior value"
        );
    }

    #[test]
    fn config_transaction_rejects_a_poisoned_mutation_lock_without_persisting_a_default() {
        let mutations = Mutex::new(());
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = mutations.lock().expect("fresh mutation lock");
            panic!("poison mutation lock");
        }));
        let (handle, _receiver) = RuntimeHandle::for_config_transaction(OwnerShared::new(
            Arc::new(Mutex::new(AppConfig::default())),
            Arc::new(Mutex::new(crate::i18n::create_bundle("en"))),
            mutations,
        ));
        let persisted = Arc::new(AtomicBool::new(false));
        let saved = persisted.clone();

        let error = handle
            .update_config_with(
                ConfigOrigin::Settings,
                |_| Ok(()),
                |_, _| {},
                move |_| {
                    saved.store(true, Ordering::Release);
                    Ok(())
                },
            )
            .err()
            .expect("a poisoned mutation gate is an explicit transaction failure");

        assert_eq!(error, "configuration mutation lock poisoned");
        assert!(
            !persisted.load(Ordering::Acquire),
            "a poisoned gate must not save a fabricated default configuration"
        );
    }

    #[test]
    fn config_transaction_rejects_a_poisoned_config_mirror_without_persisting() {
        let config = Mutex::new(AppConfig::default());
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = config.lock().expect("fresh config mirror");
            panic!("poison config mirror");
        }));
        let (handle, _receiver) = RuntimeHandle::for_config_transaction(OwnerShared::new(
            Arc::new(config),
            Arc::new(Mutex::new(crate::i18n::create_bundle("en"))),
            Mutex::new(()),
        ));
        let persisted = Arc::new(AtomicBool::new(false));
        let saved = persisted.clone();

        let error = handle
            .update_config_with(
                ConfigOrigin::Settings,
                |_| Ok(()),
                |_, _| {},
                move |_| {
                    saved.store(true, Ordering::Release);
                    Ok(())
                },
            )
            .err()
            .expect("a poisoned config mirror is an explicit transaction failure");

        assert_eq!(error, "config lock poisoned");
        assert!(!persisted.load(Ordering::Acquire));
    }

    #[test]
    fn config_transaction_reports_saved_but_not_applied_for_a_poisoned_bundle_mirror() {
        let bundle = Mutex::new(crate::i18n::create_bundle("en"));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = bundle.lock().expect("fresh bundle mirror");
            panic!("poison bundle mirror");
        }));
        let config = Arc::new(Mutex::new(AppConfig::default()));
        let (handle, _receiver) = RuntimeHandle::for_config_transaction(OwnerShared::new(
            config.clone(),
            Arc::new(bundle),
            Mutex::new(()),
        ));
        let persisted = Arc::new(AtomicBool::new(false));
        let saved = persisted.clone();

        let error = handle
            .update_config_with(
                ConfigOrigin::Language,
                |candidate| {
                    candidate.language = "es".to_owned();
                    Ok(())
                },
                |_, _| {},
                move |_| {
                    saved.store(true, Ordering::Release);
                    Ok(())
                },
            )
            .err()
            .expect("a poisoned bundle mirror cannot publish a saved language change");

        assert!(persisted.load(Ordering::Acquire));
        assert_eq!(error, "bundle lock poisoned after configuration save");
        assert_eq!(
            config
                .lock()
                .expect("config mirror remains readable")
                .language,
            "en",
            "a failed mirror publication must not claim the runtime accepted the new language"
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

    fn assert_unrelated_wifi_save_keeps_manual_activity(
        label: &str,
        edit: impl FnOnce(&mut AppConfig),
        expected_active: bool,
    ) {
        let handle = RuntimeHandle::spawn(owner_start_with_config(
            wifi_config(&["Office"]),
            Arc::new(Mutex::new(Vec::new())),
        ))
        .expect("owner spawn succeeds");
        handle
            .post_wifi(WifiObservation {
                ssid: Some("Guest".to_owned()),
            })
            .expect("unregistered observation queues");
        let initial = handle
            .query()
            .expect("query sends")
            .wait(REQUEST_TIMEOUT)
            .expect("query replies")
            .snapshot
            .expect("startup snapshot exists");
        if expected_active {
            assert!(
                !initial.engine_active,
                "an unregistered network begins inactive before a manual start"
            );
            assert!(
                send_owner_route(&handle, TrayRoute::TrayToggle)
                    .snapshot
                    .expect("manual start returns a snapshot")
                    .engine_active,
                "manual start establishes the activity to preserve"
            );
        } else {
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
                    .expect("registered snapshot exists")
                    .engine_active,
                "a registered network begins active before a manual stop"
            );
            assert!(
                !send_owner_route(&handle, TrayRoute::TrayToggle)
                    .snapshot
                    .expect("manual stop returns a snapshot")
                    .engine_active,
                "manual stop establishes the activity to preserve"
            );
        }

        let mut candidate = wifi_config(&["Office"]);
        edit(&mut candidate);
        let reply = send_owner_config(&handle, candidate);
        assert_eq!(
            reply
                .snapshot
                .expect("unrelated-save reply includes a snapshot")
                .engine_active,
            expected_active,
            "an unrelated {label} save must preserve manual WiFi activity"
        );
        assert!(
            send_owner_route(&handle, TrayRoute::Quit).terminating,
            "Quit ends the owner after the unrelated save"
        );
    }

    #[test]
    fn unrelated_wifi_saves_keep_a_manual_stop_on_a_registered_network() {
        assert_unrelated_wifi_save_keeps_manual_activity(
            "interval",
            |config| config.interval_secs = 60,
            false,
        );
        assert_unrelated_wifi_save_keeps_manual_activity(
            "language",
            |config| config.language = "es".to_owned(),
            false,
        );
        assert_unrelated_wifi_save_keeps_manual_activity(
            "jiggle method",
            |config| config.jiggle_mode = crate::config::JiggleMode::MouseZen,
            false,
        );
        assert_unrelated_wifi_save_keeps_manual_activity(
            "profile metadata",
            |config| config.active_profile = "Focus".to_owned(),
            false,
        );
    }

    #[test]
    fn unrelated_wifi_saves_keep_a_manual_start_on_an_unregistered_network() {
        assert_unrelated_wifi_save_keeps_manual_activity(
            "interval",
            |config| config.interval_secs = 60,
            true,
        );
    }

    #[test]
    fn failed_config_publication_blocks_scheduled_automatic_routes_until_recovery() {
        for mode in [
            crate::config::AppMode::Manual,
            crate::config::AppMode::AlwaysOn,
        ] {
            let active = Arc::new(Mutex::new(false));
            let starts = Arc::new(Mutex::new(0));
            let backend_active = active.clone();
            let backend_starts = starts.clone();
            let now = Utc
                .with_ymd_and_hms(2026, 3, 2, 8, 0, 0)
                .single()
                .expect("fixed instant exists");
            let initial = AppConfig {
                mode,
                ..scheduled_active_config()
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
                        stop_failures_remaining: 1,
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
                "publication failure reaches the caller"
            );
            assert!(
                failed
                    .snapshot
                    .expect("failed publication returns an observed snapshot")
                    .engine_active,
                "a failed cleanup retains the active lease for an explicit stop"
            );

            let starts_after_fault = *starts.lock().expect("backend start lock");
            for route in [TrayRoute::Tick, TrayRoute::Resume] {
                let automatic = send_owner_route(&handle, route);
                assert!(
                    automatic.error.is_none(),
                    "fault-guard observation succeeds"
                );
                assert_eq!(
                    *starts.lock().expect("backend start lock"),
                    starts_after_fault,
                    "a configuration fault blocks {route:?} from restarting a scheduled policy"
                );
            }
            assert!(
                handle
                    .query()
                    .expect("query sends while the fault is latched")
                    .wait(REQUEST_TIMEOUT)
                    .expect("query still replies")
                    .snapshot
                    .is_some(),
                "Query remains available under the fault guard"
            );

            let user_stop = send_owner_route(&handle, TrayRoute::TrayToggle);
            assert!(
                user_stop.error.is_none(),
                "an explicit toggle may release a lease retained after failed cleanup"
            );
            assert_eq!(
                *starts.lock().expect("backend start lock"),
                starts_after_fault,
                "an explicit stop does not start another backend lease"
            );
            for route in [TrayRoute::TrayToggle, TrayRoute::IpcToggle] {
                let blocked = send_owner_route(&handle, route);
                assert!(
                    blocked
                        .error
                        .as_deref()
                        .is_some_and(|error| error.contains("configuration fault")),
                    "{route:?} reports that the configuration fault blocks a new backend lease"
                );
                assert_eq!(
                    *starts.lock().expect("backend start lock"),
                    starts_after_fault,
                    "{route:?} cannot start the backend while the configuration fault is latched"
                );
            }

            let recovered = send_owner_config(&handle, candidate);
            assert!(
                recovered.error.is_none(),
                "a successful publication clears the guard"
            );
            assert_eq!(
                *starts.lock().expect("backend start lock"),
                starts_after_fault + 1,
                "the accepted scheduled policy resumes automatic activation after recovery"
            );
            assert!(
                send_owner_route(&handle, TrayRoute::Quit).terminating,
                "Quit remains available while the fault guard is latched or recovering"
            );
        }
    }

    #[test]
    fn owner_loop_fault_guard_blocks_deadline_reconciliation_until_config_recovery() {
        for mode in [
            crate::config::AppMode::Manual,
            crate::config::AppMode::AlwaysOn,
        ] {
            let (sender, receiver) = mpsc::channel();
            let shared = owner_loop_shared(sender.clone());
            let (route_sender, routes) = mpsc::channel();
            let active = Arc::new(Mutex::new(false));
            let starts = Arc::new(Mutex::new(0));
            let backend_active = active.clone();
            let backend_starts = starts.clone();
            let base = Instant::now();
            let wall = Utc
                .with_ymd_and_hms(2026, 3, 2, 8, 0, 0)
                .single()
                .expect("fixed instant exists");
            let initial = AppConfig {
                mode: mode.clone(),
                ..scheduled_active_config()
            };
            let candidate = AppConfig {
                interval_secs: 60,
                ..initial.clone()
            };
            let mut instants = VecDeque::from(vec![base, base]);
            instants.extend(vec![base + SUPERVISOR_PERIOD; 32]);
            let owner = thread::spawn(move || {
                owner_loop_with_time_sources(
                    receiver,
                    Arc::downgrade(&shared),
                    initial,
                    Arc::new(FixedClock(wall)),
                    Box::new(FailingConfigPublicationBackend {
                        active: backend_active,
                        starts: backend_starts,
                        failures_remaining: 1,
                        stop_failures_remaining: 0,
                    }),
                    Box::new(RouteSink {
                        routes: route_sender,
                        errors: Arc::new(Mutex::new(Vec::new())),
                        tick_results: Arc::new(Mutex::new(VecDeque::new())),
                    }),
                    &mut ScriptedTimeSource {
                        samples: steady_time_samples(16),
                    },
                    &mut ScriptedMonotonicClock { instants },
                )
            });

            assert_eq!(routes.recv_timeout(REQUEST_TIMEOUT), Ok(TrayRoute::Startup));
            let (failed_reply, failed_receiver) = mpsc::channel();
            sender
                .send(OwnerCommand::Config {
                    origin: ConfigOrigin::Settings,
                    config: candidate.clone(),
                    reply: failed_reply,
                })
                .expect("failing configuration reaches the owner loop");
            assert!(
                failed_receiver
                    .recv_timeout(REQUEST_TIMEOUT)
                    .expect("failing configuration replies")
                    .error
                    .is_some(),
                "a failed publication latches the configuration fault"
            );
            assert_eq!(
                *starts.lock().expect("backend start lock"),
                1,
                "the failed publication stops the original automatic lease"
            );
            routes
                .recv_timeout(REQUEST_TIMEOUT)
                .expect("fault observation is published");
            routes
                .recv_timeout(REQUEST_TIMEOUT)
                .expect("deadline reconciliation is published");
            assert_eq!(
                *starts.lock().expect("backend start lock"),
                1,
                "the deadline path must not start {mode:?} while the fault is latched"
            );

            let (recovered_reply, recovered_receiver) = mpsc::channel();
            sender
                .send(OwnerCommand::Config {
                    origin: ConfigOrigin::Settings,
                    config: candidate,
                    reply: recovered_reply,
                })
                .expect("recovering configuration reaches the owner loop");
            assert!(
                recovered_receiver
                    .recv_timeout(REQUEST_TIMEOUT)
                    .expect("recovering configuration replies")
                    .error
                    .is_none(),
                "a successful publication clears the fault"
            );
            assert_eq!(
                *starts.lock().expect("backend start lock"),
                2,
                "recovery may start {mode:?} only after publication succeeds"
            );
            assert_eq!(
                routes.recv_timeout(REQUEST_TIMEOUT),
                Ok(TrayRoute::Startup),
                "the successful recovery publishes its Startup route"
            );
            *active.lock().expect("backend activity lock") = false;
            assert_eq!(
                routes.recv_timeout(REQUEST_TIMEOUT),
                Ok(TrayRoute::Tick),
                "the next deadline reconciliation is published after recovery"
            );
            assert_eq!(
                *starts.lock().expect("backend start lock"),
                3,
                "the recovered deadline path starts inactive {mode:?} exactly once"
            );
            stop_owner_loop(&sender, owner);
        }
    }

    #[test]
    fn owner_loop_fault_guard_blocks_timeout_reconciliation_until_config_recovery() {
        for mode in [
            crate::config::AppMode::Manual,
            crate::config::AppMode::AlwaysOn,
        ] {
            let (sender, receiver) = mpsc::channel();
            let shared = owner_loop_shared(sender.clone());
            let (route_sender, routes) = mpsc::channel();
            let active = Arc::new(Mutex::new(false));
            let starts = Arc::new(Mutex::new(0));
            let backend_active = active.clone();
            let backend_starts = starts.clone();
            let wall = Utc
                .with_ymd_and_hms(2026, 3, 2, 8, 0, 0)
                .single()
                .expect("fixed instant exists");
            let initial = AppConfig {
                mode: mode.clone(),
                ..scheduled_active_config()
            };
            let candidate = AppConfig {
                interval_secs: 60,
                ..initial.clone()
            };
            let owner = thread::spawn(move || {
                owner_loop_with_time_sources(
                    receiver,
                    Arc::downgrade(&shared),
                    initial,
                    Arc::new(FixedClock(wall)),
                    Box::new(FailingConfigPublicationBackend {
                        active: backend_active,
                        starts: backend_starts,
                        failures_remaining: 1,
                        stop_failures_remaining: 0,
                    }),
                    Box::new(RouteSink {
                        routes: route_sender,
                        errors: Arc::new(Mutex::new(Vec::new())),
                        tick_results: Arc::new(Mutex::new(VecDeque::new())),
                    }),
                    &mut ScriptedTimeSource {
                        samples: steady_time_samples(16),
                    },
                    &mut ScriptedMonotonicClock {
                        instants: VecDeque::from([Instant::now(); 8]),
                    },
                )
            });

            assert_eq!(routes.recv_timeout(REQUEST_TIMEOUT), Ok(TrayRoute::Startup));
            let (failed_reply, failed_receiver) = mpsc::channel();
            sender
                .send(OwnerCommand::Config {
                    origin: ConfigOrigin::Settings,
                    config: candidate.clone(),
                    reply: failed_reply,
                })
                .expect("failing configuration reaches the owner loop");
            assert!(
                failed_receiver
                    .recv_timeout(REQUEST_TIMEOUT)
                    .expect("failing configuration replies")
                    .error
                    .is_some(),
                "a failed publication latches the configuration fault"
            );
            assert_eq!(
                *starts.lock().expect("backend start lock"),
                1,
                "the failed publication stops the original automatic lease"
            );
            routes
                .recv_timeout(REQUEST_TIMEOUT)
                .expect("fault observation is published");
            routes
                .recv_timeout(REQUEST_TIMEOUT)
                .expect("timeout reconciliation is published");
            assert_eq!(
                *starts.lock().expect("backend start lock"),
                1,
                "the timeout path must not start {mode:?} while the fault is latched"
            );

            let (recovered_reply, recovered_receiver) = mpsc::channel();
            sender
                .send(OwnerCommand::Config {
                    origin: ConfigOrigin::Settings,
                    config: candidate,
                    reply: recovered_reply,
                })
                .expect("recovering configuration reaches the owner loop");
            assert!(
                recovered_receiver
                    .recv_timeout(REQUEST_TIMEOUT)
                    .expect("recovering configuration replies")
                    .error
                    .is_none(),
                "a successful publication clears the fault"
            );
            assert_eq!(
                *starts.lock().expect("backend start lock"),
                2,
                "recovery may start {mode:?} only after publication succeeds"
            );
            stop_owner_loop(&sender, owner);
        }
    }

    #[test]
    fn quit_completes_after_backend_release_despite_a_persistent_tray_error() {
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

        handle
            .shutdown(SHUTDOWN_TIMEOUT)
            .expect("tray publication errors do not block a released backend from terminating");
        assert!(handle.exit_ready(), "a joined owner becomes exit-ready");
        assert!(
            !*active.lock().expect("backend activity lock"),
            "the successful release remains inactive after the owner joins"
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

    fn assert_resume_retries_a_latched_start(script: VecDeque<TimeSample>) {
        let (sender, receiver) = mpsc::channel();
        let shared = owner_loop_shared(sender.clone());
        let (route_sender, routes) = mpsc::channel();
        let starts = Arc::new(Mutex::new(0));
        let backend_starts = starts.clone();
        let backend = Box::new(FailingStartBackend {
            active: Arc::new(Mutex::new(false)),
            starts: backend_starts,
            failures_remaining: 1,
        });
        let clock: Arc<dyn Clock> = Arc::new(FixedClock(
            Utc.with_ymd_and_hms(2026, 3, 2, 8, 0, 0)
                .single()
                .expect("fixed instant exists"),
        ));
        let errors = Arc::new(Mutex::new(Vec::new()));
        let owner = thread::spawn(move || {
            owner_loop_with_time_source(
                receiver,
                Arc::downgrade(&shared),
                scheduled_active_config(),
                clock,
                backend,
                Box::new(RouteSink {
                    routes: route_sender,
                    errors,
                    tick_results: Arc::new(Mutex::new(VecDeque::new())),
                }),
                &mut ScriptedTimeSource { samples: script },
            )
        });

        assert_eq!(
            routes
                .recv_timeout(REQUEST_TIMEOUT)
                .expect("Startup presentation arrives"),
            TrayRoute::Startup
        );
        assert_eq!(
            routes
                .recv_timeout(REQUEST_TIMEOUT)
                .expect("ordinary tick presentation arrives"),
            TrayRoute::Tick
        );
        assert_eq!(
            *starts.lock().expect("backend start lock"),
            1,
            "an ordinary tick must not retry the latched failed start"
        );
        assert_eq!(
            routes
                .recv_timeout(REQUEST_TIMEOUT)
                .expect("resume presentation arrives"),
            TrayRoute::Resume
        );
        assert_eq!(
            *starts.lock().expect("backend start lock"),
            2,
            "Resume must retry the previously latched failed start"
        );
        stop_owner_loop(&sender, owner);
    }

    #[test]
    fn owner_loop_reconciles_resume_before_commands_that_wake_a_discontinuous_wait() {
        for command in ["query", "wifi", "toggle"] {
            let (sender, receiver) = mpsc::channel();
            let shared = owner_loop_shared(sender.clone());
            let (route_sender, routes) = mpsc::channel();
            let starts = Arc::new(Mutex::new(0));
            let backend_starts = starts.clone();
            let base = Instant::now();
            let wall = Utc
                .with_ymd_and_hms(2026, 3, 2, 8, 0, 0)
                .single()
                .expect("fixed instant exists");
            let samples = VecDeque::from([
                TimeSample {
                    wall,
                    monotonic: base,
                    offset_seconds: 0,
                },
                TimeSample {
                    wall: wall + chrono::Duration::seconds(6),
                    monotonic: base + Duration::from_millis(250),
                    offset_seconds: 0,
                },
                TimeSample {
                    wall: wall + chrono::Duration::seconds(6) + chrono::Duration::milliseconds(250),
                    monotonic: base + Duration::from_millis(500),
                    offset_seconds: 0,
                },
                TimeSample {
                    wall: wall + chrono::Duration::seconds(6) + chrono::Duration::milliseconds(500),
                    monotonic: base + Duration::from_millis(750),
                    offset_seconds: 0,
                },
                TimeSample {
                    wall: wall + chrono::Duration::seconds(6) + chrono::Duration::milliseconds(750),
                    monotonic: base + Duration::from_secs(1),
                    offset_seconds: 0,
                },
            ]);
            let owner = thread::spawn(move || {
                owner_loop_with_time_source(
                    receiver,
                    Arc::downgrade(&shared),
                    scheduled_active_config(),
                    Arc::new(FixedClock(wall)),
                    Box::new(FailingStartBackend {
                        active: Arc::new(Mutex::new(false)),
                        starts: backend_starts,
                        failures_remaining: 1,
                    }),
                    Box::new(RouteSink {
                        routes: route_sender,
                        errors: Arc::new(Mutex::new(Vec::new())),
                        tick_results: Arc::new(Mutex::new(VecDeque::new())),
                    }),
                    &mut ScriptedTimeSource { samples },
                )
            });

            assert_eq!(routes.recv_timeout(REQUEST_TIMEOUT), Ok(TrayRoute::Startup));
            let query_reply = match command {
                "query" => {
                    let (reply, reply_receiver) = mpsc::channel();
                    sender
                        .send(OwnerCommand::Query { reply })
                        .expect("query wakes the owner wait");
                    Some(reply_receiver)
                }
                "wifi" => {
                    sender
                        .send(OwnerCommand::Wifi {
                            observation: WifiObservation {
                                ssid: Some("Office".to_owned()),
                            },
                            reply: None,
                        })
                        .expect("WiFi observation wakes the owner wait");
                    None
                }
                "toggle" => {
                    sender
                        .send(OwnerCommand::Route {
                            route: TrayRoute::TrayToggle,
                            reply: None,
                        })
                        .expect("toggle wakes the owner wait");
                    None
                }
                _ => unreachable!("the test enumerates supported command wakes"),
            };

            assert_eq!(
                routes
                    .recv_timeout(REQUEST_TIMEOUT)
                    .expect("a discontinuity is reconciled before the woken command"),
                TrayRoute::Resume,
                "{command} must not discard a suspend-like discontinuity"
            );
            assert_eq!(
                *starts.lock().expect("backend start lock"),
                2,
                "Resume retries the failed scheduled start after a {command} wake"
            );
            if let Some(reply) = query_reply {
                assert!(
                    reply
                        .recv_timeout(REQUEST_TIMEOUT)
                        .expect("query reply arrives after Resume")
                        .snapshot
                        .is_some(),
                    "Query still returns the reconciled snapshot"
                );
            }
            stop_owner_loop(&sender, owner);
        }
    }

    #[test]
    fn owner_loop_retains_an_offset_change_observed_after_command_processing() {
        let (sender, receiver) = mpsc::channel();
        let shared = owner_loop_shared(sender.clone());
        let (route_sender, routes) = mpsc::channel();
        let (entered, entered_receiver) = mpsc::channel();
        let (release, release_receiver) = mpsc::channel();
        let base = Instant::now();
        let wall = Utc
            .with_ymd_and_hms(2026, 3, 2, 8, 0, 0)
            .single()
            .expect("fixed instant exists");
        let mut samples = VecDeque::from([
            TimeSample {
                wall,
                monotonic: base,
                offset_seconds: 0,
            },
            TimeSample {
                wall: wall + chrono::Duration::milliseconds(250),
                monotonic: base + Duration::from_millis(250),
                offset_seconds: 0,
            },
            TimeSample {
                wall: wall + chrono::Duration::milliseconds(500),
                monotonic: base + Duration::from_millis(500),
                offset_seconds: 3600,
            },
            TimeSample {
                wall: wall + chrono::Duration::milliseconds(750),
                monotonic: base + Duration::from_millis(750),
                offset_seconds: 3600,
            },
        ]);
        samples.extend(steady_time_samples(8));
        let owner = thread::spawn(move || {
            owner_loop_with_time_source(
                receiver,
                Arc::downgrade(&shared),
                scheduled_active_config(),
                Arc::new(FixedClock(wall)),
                Box::new(BlockingFailingStartBackend {
                    entered,
                    release: release_receiver,
                    blocking_starts_remaining: 2,
                }),
                Box::new(RouteSink {
                    routes: route_sender,
                    errors: Arc::new(Mutex::new(Vec::new())),
                    tick_results: Arc::new(Mutex::new(VecDeque::new())),
                }),
                &mut ScriptedTimeSource { samples },
            )
        });

        entered_receiver
            .recv_timeout(REQUEST_TIMEOUT)
            .expect("startup enters the controlled backend call");
        release.send(()).expect("startup backend call releases");
        assert_eq!(routes.recv_timeout(REQUEST_TIMEOUT), Ok(TrayRoute::Startup));
        sender
            .send(OwnerCommand::Route {
                route: TrayRoute::TrayToggle,
                reply: None,
            })
            .expect("toggle reaches the owner loop");
        entered_receiver
            .recv_timeout(REQUEST_TIMEOUT)
            .expect("toggle enters the controlled backend call");
        release.send(()).expect("toggle backend call releases");
        assert_eq!(
            routes.recv_timeout(REQUEST_TIMEOUT),
            Ok(TrayRoute::TrayToggle)
        );
        assert_eq!(
            routes
                .recv_timeout(Duration::from_millis(500))
                .expect("post-processing reconciliation arrives"),
            TrayRoute::Resume,
            "an offset change during backend processing must survive until the next reconciliation"
        );
        stop_owner_loop(&sender, owner);
    }

    #[test]
    fn owner_loop_retries_a_latched_start_after_a_forward_wall_clock_jump() {
        let mut samples = steady_time_samples(2);
        let base = Instant::now();
        let wall = Utc
            .with_ymd_and_hms(2026, 3, 2, 8, 0, 0)
            .single()
            .expect("fixed instant exists");
        samples.push_back(TimeSample {
            wall: wall + chrono::Duration::milliseconds(500),
            monotonic: base + Duration::from_millis(500),
            offset_seconds: 0,
        });
        samples.push_back(TimeSample {
            wall: wall + chrono::Duration::seconds(6),
            monotonic: base + Duration::from_millis(750),
            offset_seconds: 0,
        });
        samples.extend(steady_time_samples(8));
        assert_resume_retries_a_latched_start(samples);
    }

    #[test]
    fn owner_loop_retries_a_latched_start_after_a_backward_wall_clock_jump() {
        let mut samples = steady_time_samples(2);
        let base = Instant::now();
        let wall = Utc
            .with_ymd_and_hms(2026, 3, 2, 8, 0, 0)
            .single()
            .expect("fixed instant exists");
        samples.push_back(TimeSample {
            wall: wall + chrono::Duration::seconds(5),
            monotonic: base + Duration::from_millis(500),
            offset_seconds: 0,
        });
        samples.push_back(TimeSample {
            wall,
            monotonic: base + Duration::from_millis(750),
            offset_seconds: 0,
        });
        samples.extend(steady_time_samples(8));
        assert_resume_retries_a_latched_start(samples);
    }

    #[test]
    fn owner_loop_retries_a_latched_start_after_a_timezone_offset_change() {
        let mut samples = steady_time_samples(2);
        let base = Instant::now();
        let wall = Utc
            .with_ymd_and_hms(2026, 3, 2, 8, 0, 0)
            .single()
            .expect("fixed instant exists");
        samples.push_back(TimeSample {
            wall: wall + chrono::Duration::milliseconds(500),
            monotonic: base + Duration::from_millis(500),
            offset_seconds: 0,
        });
        samples.push_back(TimeSample {
            wall: wall + chrono::Duration::milliseconds(750),
            monotonic: base + Duration::from_millis(750),
            offset_seconds: 3600,
        });
        samples.extend(steady_time_samples(8));
        assert_resume_retries_a_latched_start(samples);
    }

    #[test]
    fn deadline_due_retries_a_latched_start_only_for_time_discontinuities() {
        let base = Instant::now();
        let wall = Utc
            .with_ymd_and_hms(2026, 3, 2, 8, 0, 0)
            .single()
            .expect("fixed instant exists");

        for (label, current, expected_route, expected_starts) in [
            (
                "forward wall jump",
                TimeSample {
                    wall: wall + chrono::Duration::seconds(3),
                    monotonic: base + SUPERVISOR_PERIOD,
                    offset_seconds: 0,
                },
                TrayRoute::Resume,
                2,
            ),
            (
                "backward wall jump",
                TimeSample {
                    wall: wall - chrono::Duration::seconds(3),
                    monotonic: base + SUPERVISOR_PERIOD,
                    offset_seconds: 0,
                },
                TrayRoute::Resume,
                2,
            ),
            (
                "offset change",
                TimeSample {
                    wall: wall + chrono::Duration::milliseconds(250),
                    monotonic: base + SUPERVISOR_PERIOD,
                    offset_seconds: 3600,
                },
                TrayRoute::Resume,
                2,
            ),
            (
                "steady deadline",
                TimeSample {
                    wall: wall + chrono::Duration::milliseconds(250),
                    monotonic: base + SUPERVISOR_PERIOD,
                    offset_seconds: 0,
                },
                TrayRoute::Tick,
                1,
            ),
        ] {
            let (sender, receiver) = mpsc::channel();
            let shared = owner_loop_shared(sender.clone());
            let (route_sender, routes) = mpsc::channel();
            let starts = Arc::new(Mutex::new(0));
            let backend_starts = starts.clone();
            let before_wait = TimeSample {
                wall,
                monotonic: base,
                offset_seconds: 0,
            };
            let mut time_source = ScriptedTimeSource {
                samples: VecDeque::from([
                    before_wait,
                    current.clone(),
                    current.clone(),
                    current.clone(),
                    current,
                ]),
            };
            let mut monotonic_clock = ScriptedMonotonicClock {
                instants: VecDeque::from([
                    base,
                    base + SUPERVISOR_PERIOD,
                    base + SUPERVISOR_PERIOD,
                    base + SUPERVISOR_PERIOD,
                ]),
            };
            let owner = thread::spawn(move || {
                owner_loop_with_time_sources(
                    receiver,
                    Arc::downgrade(&shared),
                    scheduled_active_config(),
                    Arc::new(FixedClock(wall)),
                    Box::new(FailingStartBackend {
                        active: Arc::new(Mutex::new(false)),
                        starts: backend_starts,
                        failures_remaining: 1,
                    }),
                    Box::new(RouteSink {
                        routes: route_sender,
                        errors: Arc::new(Mutex::new(Vec::new())),
                        tick_results: Arc::new(Mutex::new(VecDeque::new())),
                    }),
                    &mut time_source,
                    &mut monotonic_clock,
                )
            });

            assert_eq!(
                routes.recv_timeout(REQUEST_TIMEOUT),
                Ok(TrayRoute::Startup),
                "{label} begins with one startup"
            );
            assert_eq!(
                routes.recv_timeout(REQUEST_TIMEOUT),
                Ok(expected_route),
                "{label} selects the deadline route"
            );
            assert_eq!(
                *starts.lock().expect("backend start lock"),
                expected_starts,
                "{label} has the expected failed-start retry count"
            );
            stop_owner_loop(&sender, owner);
        }
    }

    #[test]
    fn owner_loop_keeps_a_sub_threshold_clock_gap_as_an_ordinary_tick() {
        let mut samples = steady_time_samples(2);
        let base = Instant::now();
        let wall = Utc
            .with_ymd_and_hms(2026, 3, 2, 8, 0, 0)
            .single()
            .expect("fixed instant exists");
        samples.push_back(TimeSample {
            wall: wall + chrono::Duration::milliseconds(500),
            monotonic: base + Duration::from_millis(500),
            offset_seconds: 0,
        });
        samples.push_back(TimeSample {
            wall: wall + chrono::Duration::milliseconds(1500),
            monotonic: base + Duration::from_millis(750),
            offset_seconds: 0,
        });
        samples.extend(steady_time_samples(8));

        let (sender, receiver) = mpsc::channel();
        let shared = owner_loop_shared(sender.clone());
        let (route_sender, routes) = mpsc::channel();
        let starts = Arc::new(Mutex::new(0));
        let backend_starts = starts.clone();
        let owner = thread::spawn(move || {
            owner_loop_with_time_source(
                receiver,
                Arc::downgrade(&shared),
                scheduled_active_config(),
                Arc::new(FixedClock(wall)),
                Box::new(FailingStartBackend {
                    active: Arc::new(Mutex::new(false)),
                    starts: backend_starts,
                    failures_remaining: 1,
                }),
                Box::new(RouteSink {
                    routes: route_sender,
                    errors: Arc::new(Mutex::new(Vec::new())),
                    tick_results: Arc::new(Mutex::new(VecDeque::new())),
                }),
                &mut ScriptedTimeSource { samples },
            )
        });

        assert_eq!(routes.recv_timeout(REQUEST_TIMEOUT), Ok(TrayRoute::Startup));
        assert_eq!(routes.recv_timeout(REQUEST_TIMEOUT), Ok(TrayRoute::Tick));
        assert_eq!(
            *starts.lock().expect("backend start lock"),
            1,
            "a gap below the resume threshold must not retry the failed start"
        );
        stop_owner_loop(&sender, owner);
    }

    #[test]
    fn owner_loop_does_not_classify_slow_backend_processing_as_resume() {
        let (sender, receiver) = mpsc::channel();
        let shared = owner_loop_shared(sender.clone());
        let (route_sender, routes) = mpsc::channel();
        let (entered, entered_receiver) = mpsc::channel();
        let (release, release_receiver) = mpsc::channel();
        let samples = steady_time_samples(10);
        let wall = Utc
            .with_ymd_and_hms(2026, 3, 2, 8, 0, 0)
            .single()
            .expect("fixed instant exists");
        let owner = thread::spawn(move || {
            owner_loop_with_time_source(
                receiver,
                Arc::downgrade(&shared),
                scheduled_active_config(),
                Arc::new(FixedClock(wall)),
                Box::new(BlockingFailingStartBackend {
                    entered,
                    release: release_receiver,
                    blocking_starts_remaining: 1,
                }),
                Box::new(RouteSink {
                    routes: route_sender,
                    errors: Arc::new(Mutex::new(Vec::new())),
                    tick_results: Arc::new(Mutex::new(VecDeque::new())),
                }),
                &mut ScriptedTimeSource { samples },
            )
        });

        entered_receiver
            .recv_timeout(REQUEST_TIMEOUT)
            .expect("startup enters the controlled backend call");
        release.send(()).expect("startup backend call releases");
        assert_eq!(routes.recv_timeout(REQUEST_TIMEOUT), Ok(TrayRoute::Startup));
        assert_eq!(routes.recv_timeout(REQUEST_TIMEOUT), Ok(TrayRoute::Tick));
        stop_owner_loop(&sender, owner);
    }

    fn run_unreplied_error_case(route: Option<TrayRoute>) -> Vec<String> {
        let (sender, receiver) = mpsc::channel();
        let shared = owner_loop_shared(sender.clone());
        let (route_sender, routes) = mpsc::channel();
        let errors = Arc::new(Mutex::new(Vec::new()));
        let sink_errors = errors.clone();
        let mode = if route.is_some() {
            crate::config::AppMode::Manual
        } else {
            crate::config::AppMode::AlwaysOn
        };
        let owner = thread::spawn(move || {
            owner_loop_with_time_source(
                receiver,
                Arc::downgrade(&shared),
                AppConfig {
                    mode,
                    ..AppConfig::default()
                },
                Arc::new(FixedClock(
                    Utc.with_ymd_and_hms(2026, 3, 2, 8, 0, 0)
                        .single()
                        .expect("fixed instant exists"),
                )),
                Box::new(FailingStartBackend {
                    active: Arc::new(Mutex::new(false)),
                    starts: Arc::new(Mutex::new(0)),
                    failures_remaining: 1,
                }),
                Box::new(RouteSink {
                    routes: route_sender,
                    errors: sink_errors,
                    tick_results: Arc::new(Mutex::new(VecDeque::new())),
                }),
                &mut ScriptedTimeSource {
                    samples: steady_time_samples(12),
                },
            )
        });

        assert_eq!(routes.recv_timeout(REQUEST_TIMEOUT), Ok(TrayRoute::Startup));
        if let Some(route) = route {
            sender
                .send(OwnerCommand::Route { route, reply: None })
                .expect("unreplied route reaches the owner loop");
            assert_eq!(
                routes.recv_timeout(REQUEST_TIMEOUT),
                Ok(route),
                "the requested no-reply route is processed"
            );
        }
        stop_owner_loop(&sender, owner);
        let captured = errors.lock().expect("test sink error lock").clone();
        captured
    }

    #[test]
    fn owner_reports_startup_and_no_reply_tray_hotkey_errors_once() {
        let startup_errors = run_unreplied_error_case(None);
        let tray_errors = run_unreplied_error_case(Some(TrayRoute::TrayToggle));
        let hotkey_errors = run_unreplied_error_case(Some(TrayRoute::Hotkey));

        assert_eq!(startup_errors, vec!["backend start failed"]);
        assert_eq!(tray_errors, vec!["backend start failed"]);
        assert_eq!(hotkey_errors, vec!["backend start failed"]);
    }

    #[test]
    fn owner_coalesces_identical_tick_errors_until_a_success_clears_them() {
        let (sender, receiver) = mpsc::channel();
        let shared = owner_loop_shared(sender.clone());
        let (route_sender, routes) = mpsc::channel();
        let errors = Arc::new(Mutex::new(Vec::new()));
        let sink_errors = errors.clone();
        let owner = thread::spawn(move || {
            owner_loop_with_time_source(
                receiver,
                Arc::downgrade(&shared),
                AppConfig::default(),
                Arc::new(FixedClock(
                    Utc.with_ymd_and_hms(2026, 3, 2, 8, 0, 0)
                        .single()
                        .expect("fixed instant exists"),
                )),
                Box::new(SharedRecordingBackend {
                    calls: Arc::new(Mutex::new(Vec::new())),
                    active: Arc::new(Mutex::new(false)),
                }),
                Box::new(RouteSink {
                    routes: route_sender,
                    errors: sink_errors,
                    tick_results: Arc::new(Mutex::new(VecDeque::from([
                        Err("tray enqueue failed".to_owned()),
                        Err("tray enqueue failed".to_owned()),
                        Ok(()),
                        Err("tray enqueue failed".to_owned()),
                    ]))),
                }),
                &mut ScriptedTimeSource {
                    samples: steady_time_samples(16),
                },
            )
        });

        assert_eq!(routes.recv_timeout(REQUEST_TIMEOUT), Ok(TrayRoute::Startup));
        for _ in 0..4 {
            assert_eq!(routes.recv_timeout(REQUEST_TIMEOUT), Ok(TrayRoute::Tick));
        }
        stop_owner_loop(&sender, owner);
        assert_eq!(
            *errors.lock().expect("test sink error lock"),
            vec![
                "tray enqueue failed".to_owned(),
                "tray enqueue failed".to_owned(),
            ],
            "the first and post-success automatic failures report, but the duplicate does not"
        );
    }

    #[test]
    fn owner_returns_explicit_request_errors_without_reporting_them_to_the_sink() {
        let (sender, receiver) = mpsc::channel();
        let shared = owner_loop_shared(sender.clone());
        let (route_sender, routes) = mpsc::channel();
        let errors = Arc::new(Mutex::new(Vec::new()));
        let sink_errors = errors.clone();
        let owner = thread::spawn(move || {
            owner_loop_with_time_source(
                receiver,
                Arc::downgrade(&shared),
                AppConfig::default(),
                Arc::new(FixedClock(
                    Utc.with_ymd_and_hms(2026, 3, 2, 8, 0, 0)
                        .single()
                        .expect("fixed instant exists"),
                )),
                Box::new(FailingStartBackend {
                    active: Arc::new(Mutex::new(false)),
                    starts: Arc::new(Mutex::new(0)),
                    failures_remaining: 1,
                }),
                Box::new(RouteSink {
                    routes: route_sender,
                    errors: sink_errors,
                    tick_results: Arc::new(Mutex::new(VecDeque::new())),
                }),
                &mut ScriptedTimeSource {
                    samples: steady_time_samples(12),
                },
            )
        });

        assert_eq!(routes.recv_timeout(REQUEST_TIMEOUT), Ok(TrayRoute::Startup));
        let (reply, reply_receiver) = mpsc::channel();
        sender
            .send(OwnerCommand::Route {
                route: TrayRoute::TrayToggle,
                reply: Some(reply),
            })
            .expect("explicit request reaches the owner loop");
        assert_eq!(
            routes.recv_timeout(REQUEST_TIMEOUT),
            Ok(TrayRoute::TrayToggle)
        );
        assert_eq!(
            reply_receiver
                .recv_timeout(REQUEST_TIMEOUT)
                .expect("explicit request reply arrives")
                .error,
            Some("backend start failed".to_owned()),
            "the waiting caller receives its operation error"
        );
        stop_owner_loop(&sender, owner);
        assert!(
            errors.lock().expect("test sink error lock").is_empty(),
            "an explicit request error must not be duplicated into unattended diagnostics"
        );
    }
}
