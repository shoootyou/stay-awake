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
#![allow(dead_code)] // skeleton: wired in boundary 3/4

use crate::config::AppConfig;
use crate::i18n::Bundle;
use crate::runtime::{EngineBackend, RuntimeSnapshot};
use crate::schedule::Clock;
use crate::{TrayRoute, TraySink};
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

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
    pub(crate) fn wait(self, _timeout: Duration) -> Result<OwnerReply, String> {
        Err("owner reply skeleton".to_owned())
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

impl RuntimeHandle {
    pub(crate) fn spawn(_input: OwnerStart) -> Result<Self, String> {
        Err("owner spawn skeleton".to_owned())
    }

    pub(crate) fn post(&self, _route: TrayRoute) -> Result<(), String> {
        Err("owner post skeleton".to_owned())
    }

    pub(crate) fn post_wifi(&self, _observation: WifiObservation) -> Result<(), String> {
        Err("owner wifi skeleton".to_owned())
    }

    pub(crate) fn request(&self, _route: TrayRoute) -> Result<PendingReply, String> {
        Err("owner request skeleton".to_owned())
    }

    pub(crate) fn query(&self) -> Result<PendingReply, String> {
        Err("owner query skeleton".to_owned())
    }

    pub(crate) fn update_config<F, E>(
        &self,
        _origin: ConfigOrigin,
        _edit: F,
        _effects: E,
    ) -> Result<PendingReply, String>
    where
        F: FnOnce(&mut AppConfig) -> Result<(), String>,
        E: FnOnce(&AppConfig, &AppConfig),
    {
        Err("owner config skeleton".to_owned())
    }

    pub(crate) fn shutdown(&self, _timeout: Duration) -> Result<(), String> {
        Err("owner shutdown skeleton".to_owned())
    }

    pub(crate) fn is_closing(&self) -> bool {
        self.shared
            .closing
            .load(std::sync::atomic::Ordering::Acquire)
    }

    pub(crate) fn exit_ready(&self) -> bool {
        self.shared
            .exit_ready
            .load(std::sync::atomic::Ordering::Acquire)
    }

    pub(crate) fn claim_exit_coordinator(&self) -> bool {
        let claimed = self
            .shared
            .exit_coordinator_running
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_ok();
        if claimed {
            self.shared
                .closing
                .store(true, std::sync::atomic::Ordering::Release);
        }
        claimed
    }

    pub(crate) fn release_exit_coordinator(&self) {
        self.shared
            .exit_coordinator_running
            .store(false, std::sync::atomic::Ordering::Release);
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

    fn owner_start(calls: Arc<Mutex<Vec<&'static str>>>) -> OwnerStart {
        let now = Utc
            .with_ymd_and_hms(2026, 3, 2, 8, 0, 0)
            .single()
            .expect("fixed instant exists");
        OwnerStart {
            config: Arc::new(Mutex::new(AppConfig::default())),
            bundle: Arc::new(Mutex::new(crate::i18n::create_bundle("en"))),
            clock: Arc::new(FixedClock(now)),
            backend_factory: Box::new(move |_, _| {
                Box::new(SharedRecordingBackend { calls }) as Box<dyn EngineBackend>
            }),
            sink: Box::new(RecordingSink::default()),
        }
    }

    struct SharedRecordingBackend {
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    impl EngineBackend for SharedRecordingBackend {
        fn start(&mut self) -> Result<(), String> {
            self.calls.lock().expect("backend log lock").push("start");
            Ok(())
        }

        fn stop(&mut self) -> Result<(), String> {
            self.calls.lock().expect("backend log lock").push("stop");
            Ok(())
        }

        fn is_active(&self) -> bool {
            self.calls
                .lock()
                .expect("backend log lock")
                .last()
                .is_some_and(|call| *call == "start")
        }

        fn set_jiggle_policy(&mut self, _: crate::runtime::JigglePolicy) {
            self.calls.lock().expect("backend log lock").push("policy");
        }
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
}
