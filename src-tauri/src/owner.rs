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
