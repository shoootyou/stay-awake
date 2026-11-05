#![allow(dead_code)] // skeleton: wired in E3

use crate::config::{AppConfig, AppMode, JiggleMode};
use crate::schedule::{Clock, Occurrence, Schedule, ScheduleEvaluation};
use chrono::{DateTime, Utc};
use std::time::Duration;

pub(crate) const SUPERVISOR_PERIOD: Duration = Duration::from_millis(250);

pub(crate) fn effective_mode(mode: &AppMode) -> AppMode {
    mode.clone()
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RuntimePolicy {
    pub(crate) mode: AppMode,
    pub(crate) schedule: Schedule,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum OverrideState {
    #[default]
    None,
    SkipWindow(Occurrence),
    ForceOn {
        until: Option<DateTime<Utc>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NormalizedOverride(OverrideState);

impl NormalizedOverride {
    pub(crate) fn as_state(&self) -> &OverrideState {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LifecycleEvent {
    Startup,
    TrayToggle,
    Tick,
    WifiChanged { active: bool },
    ConfigChanged { policy_changed: bool },
    Resume,
    Quit,
}

pub(crate) struct NormalizeInput<'a> {
    pub(crate) mode: &'a AppMode,
    pub(crate) schedule_enabled: bool,
    pub(crate) window: &'a ScheduleEvaluation,
    pub(crate) override_state: &'a OverrideState,
    pub(crate) event: LifecycleEvent,
}

pub(crate) fn normalize_override(_input: NormalizeInput<'_>) -> NormalizedOverride {
    NormalizedOverride(OverrideState::None)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EngineAction {
    Start,
    Stop,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrayStatus {
    Active,
    Inactive,
    Waiting,
    Paused,
    ActiveOverride,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrayIcon {
    Active,
    Inactive,
    Waiting,
}

impl TrayStatus {
    pub(crate) fn menu_key(self) -> &'static str {
        "tray-start"
    }

    pub(crate) fn tooltip_key(self) -> &'static str {
        "tray-tooltip-inactive"
    }

    pub(crate) fn icon(self) -> TrayIcon {
        TrayIcon::Inactive
    }
}

pub(crate) struct TrayInput<'a> {
    pub(crate) mode: &'a AppMode,
    pub(crate) engine_active: bool,
    pub(crate) schedule_enabled: bool,
    pub(crate) window: &'a ScheduleEvaluation,
    pub(crate) override_state: &'a NormalizedOverride,
}

pub(crate) fn derive_tray_status(_input: TrayInput<'_>) -> TrayStatus {
    TrayStatus::Inactive
}

pub(crate) struct LifecycleInput<'a> {
    pub(crate) mode: &'a AppMode,
    pub(crate) schedule_enabled: bool,
    pub(crate) window: &'a ScheduleEvaluation,
    pub(crate) engine_active: bool,
    pub(crate) override_state: &'a NormalizedOverride,
    pub(crate) event: LifecycleEvent,
    pub(crate) previous_policy: Option<&'a RuntimePolicy>,
    pub(crate) wifi_active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LifecycleDecision {
    pub(crate) action: EngineAction,
    pub(crate) override_state: NormalizedOverride,
    pub(crate) presentation: TrayStatus,
}

pub(crate) fn decide_lifecycle(_input: LifecycleInput<'_>) -> LifecycleDecision {
    LifecycleDecision {
        action: EngineAction::None,
        override_state: NormalizedOverride(OverrideState::None),
        presentation: TrayStatus::Inactive,
    }
}

pub(crate) struct JiggleInput<'a> {
    pub(crate) mode: &'a AppMode,
    pub(crate) jiggle_mode: &'a JiggleMode,
    pub(crate) engine_active: bool,
    pub(crate) schedule_enabled: bool,
    pub(crate) window: &'a ScheduleEvaluation,
    pub(crate) override_state: &'a NormalizedOverride,
}

pub(crate) fn should_jiggle(_input: JiggleInput<'_>) -> bool {
    false
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JigglePolicy {
    pub(crate) override_state: OverrideState,
}

pub(crate) trait EngineBackend: Send {
    fn start(&mut self) -> Result<(), String>;
    fn stop(&mut self) -> Result<(), String>;
    fn is_active(&self) -> bool;
    fn set_jiggle_policy(&mut self, policy: JigglePolicy);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeSnapshot {
    pub(crate) engine_active: bool,
    pub(crate) override_state: NormalizedOverride,
    pub(crate) window: ScheduleEvaluation,
    pub(crate) status: TrayStatus,
}

pub(crate) struct ReconcileInput<'a> {
    pub(crate) config: &'a AppConfig,
    pub(crate) clock: &'a dyn Clock,
    pub(crate) event: LifecycleEvent,
}

#[derive(Debug, Default)]
pub(crate) struct Supervisor {
    override_state: OverrideState,
    previous_policy: Option<RuntimePolicy>,
    previous_current: Option<Occurrence>,
    wifi_active: bool,
    failed_action: Option<EngineAction>,
    last_error: Option<String>,
    last_snapshot: Option<RuntimeSnapshot>,
}

impl Supervisor {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn reconcile(
        &mut self,
        input: ReconcileInput<'_>,
        _backend: &mut dyn EngineBackend,
    ) -> Result<RuntimeSnapshot, String> {
        let snapshot = RuntimeSnapshot {
            engine_active: false,
            override_state: NormalizedOverride(OverrideState::None),
            window: ScheduleEvaluation {
                at: input.clock.now(),
                current: None,
                next: None,
            },
            status: TrayStatus::Inactive,
        };
        self.last_snapshot = Some(snapshot.clone());
        Ok(snapshot)
    }

    pub(crate) fn snapshot(&self) -> Option<&RuntimeSnapshot> {
        self.last_snapshot.as_ref()
    }
}
