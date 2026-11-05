#![allow(dead_code)] // wired in E3b

use crate::config::{AppConfig, AppMode, JiggleMode};
use crate::schedule::{Clock, Occurrence, Schedule, ScheduleEvaluation};
use chrono::{DateTime, Utc};
use std::time::Duration;

pub(crate) const SUPERVISOR_PERIOD: Duration = Duration::from_millis(250);

pub(crate) fn effective_mode(mode: &AppMode) -> AppMode {
    match mode {
        AppMode::Scheduled => AppMode::Manual,
        _ => mode.clone(),
    }
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

pub(crate) fn normalize_override(input: NormalizeInput<'_>) -> NormalizedOverride {
    let reset = matches!(input.event, LifecycleEvent::Startup | LifecycleEvent::Quit)
        || matches!(
            input.event,
            LifecycleEvent::ConfigChanged {
                policy_changed: true
            }
        );
    if reset || !input.schedule_enabled || effective_mode(input.mode) == AppMode::WiFi {
        return NormalizedOverride(OverrideState::None);
    }
    match input.override_state {
        OverrideState::None => NormalizedOverride(OverrideState::None),
        OverrideState::SkipWindow(occurrence)
            if input.window.current.as_ref() == Some(occurrence)
                && input.window.at < occurrence.end =>
        {
            NormalizedOverride(OverrideState::SkipWindow(occurrence.clone()))
        }
        OverrideState::SkipWindow(_) => NormalizedOverride(OverrideState::None),
        OverrideState::ForceOn { .. } if input.window.in_window() => {
            NormalizedOverride(OverrideState::None)
        }
        OverrideState::ForceOn { until: Some(until) } if input.window.at >= *until => {
            NormalizedOverride(OverrideState::None)
        }
        OverrideState::ForceOn { until: Some(until) } => {
            NormalizedOverride(OverrideState::ForceOn {
                until: Some(*until),
            })
        }
        OverrideState::ForceOn { until: None } => NormalizedOverride(OverrideState::ForceOn {
            until: input.window.next.as_ref().map(|next| next.start),
        }),
    }
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
        match self {
            Self::Active => "tray-running",
            Self::Inactive => "tray-start",
            Self::Waiting => "tray-waiting",
            Self::Paused => "tray-paused",
            Self::ActiveOverride => "tray-active-override",
        }
    }

    pub(crate) fn tooltip_key(self) -> &'static str {
        match self {
            Self::Active => "tray-tooltip-active",
            Self::Inactive => "tray-tooltip-inactive",
            Self::Waiting => "tray-tooltip-waiting",
            Self::Paused => "tray-tooltip-paused",
            Self::ActiveOverride => "tray-tooltip-override",
        }
    }

    pub(crate) fn icon(self) -> TrayIcon {
        match self {
            Self::Active | Self::ActiveOverride => TrayIcon::Active,
            Self::Waiting => TrayIcon::Waiting,
            Self::Inactive | Self::Paused => TrayIcon::Inactive,
        }
    }
}

pub(crate) struct TrayInput<'a> {
    pub(crate) mode: &'a AppMode,
    pub(crate) engine_active: bool,
    pub(crate) schedule_enabled: bool,
    pub(crate) window: &'a ScheduleEvaluation,
    pub(crate) override_state: &'a NormalizedOverride,
}

pub(crate) fn derive_tray_status(input: TrayInput<'_>) -> TrayStatus {
    let schedule_owned = input.schedule_enabled
        && matches!(
            effective_mode(input.mode),
            AppMode::Manual | AppMode::AlwaysOn
        );
    if !schedule_owned {
        return if input.engine_active {
            TrayStatus::Active
        } else {
            TrayStatus::Inactive
        };
    }
    match (
        input.engine_active,
        input.window.in_window(),
        input.override_state.as_state(),
    ) {
        (true, false, OverrideState::ForceOn { .. }) => TrayStatus::ActiveOverride,
        (true, _, _) => TrayStatus::Active,
        (false, true, OverrideState::SkipWindow(_)) => TrayStatus::Paused,
        (false, false, OverrideState::None) => TrayStatus::Waiting,
        _ => TrayStatus::Inactive,
    }
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

pub(crate) fn decide_lifecycle(input: LifecycleInput<'_>) -> LifecycleDecision {
    let mode = effective_mode(input.mode);
    let schedule_owned =
        input.schedule_enabled && matches!(mode, AppMode::Manual | AppMode::AlwaysOn);
    let status = |active, override_state: NormalizedOverride| {
        let presentation = derive_tray_status(TrayInput {
            mode: input.mode,
            engine_active: active,
            schedule_enabled: input.schedule_enabled,
            window: input.window,
            override_state: &override_state,
        });
        (presentation, override_state)
    };
    let target = |wanted| match (wanted, input.engine_active) {
        (true, false) => EngineAction::Start,
        (false, true) => EngineAction::Stop,
        _ => EngineAction::None,
    };
    if matches!(input.event, LifecycleEvent::Quit) {
        return LifecycleDecision {
            action: target(false),
            override_state: NormalizedOverride(OverrideState::None),
            presentation: TrayStatus::Inactive,
        };
    }
    if matches!(input.event, LifecycleEvent::WifiChanged { .. }) && mode != AppMode::WiFi {
        let (presentation, override_state) =
            status(input.engine_active, input.override_state.clone());
        return LifecycleDecision {
            action: EngineAction::None,
            override_state,
            presentation,
        };
    }
    if schedule_owned {
        let mut override_state = if input.window.in_window()
            && matches!(
                input.override_state.as_state(),
                OverrideState::ForceOn { .. }
            ) {
            NormalizedOverride(OverrideState::None)
        } else {
            input.override_state.clone()
        };
        if matches!(input.event, LifecycleEvent::TrayToggle) {
            if input.engine_active {
                override_state = match input.window.current.clone() {
                    Some(current) => NormalizedOverride(OverrideState::SkipWindow(current)),
                    None => NormalizedOverride(OverrideState::None),
                };
                let (presentation, override_state) = status(false, override_state);
                return LifecycleDecision {
                    action: EngineAction::Stop,
                    override_state,
                    presentation,
                };
            }
            override_state = if input.window.in_window() {
                NormalizedOverride(OverrideState::None)
            } else {
                NormalizedOverride(OverrideState::ForceOn {
                    until: input.window.next.as_ref().map(|next| next.start),
                })
            };
            let (presentation, override_state) = status(true, override_state);
            return LifecycleDecision {
                action: EngineAction::Start,
                override_state,
                presentation,
            };
        }
        let wanted = matches!(override_state.as_state(), OverrideState::ForceOn { .. })
            || (input.window.in_window()
                && !matches!(override_state.as_state(), OverrideState::SkipWindow(_)));
        let (presentation, override_state) = status(wanted, override_state);
        return LifecycleDecision {
            action: target(wanted),
            override_state,
            presentation,
        };
    }
    let override_state = NormalizedOverride(OverrideState::None);
    let wanted = match input.event {
        LifecycleEvent::Startup => match mode {
            AppMode::AlwaysOn => true,
            AppMode::WiFi => input.wifi_active,
            _ => false,
        },
        LifecycleEvent::TrayToggle => !input.engine_active,
        LifecycleEvent::WifiChanged { active } if mode == AppMode::WiFi => active,
        LifecycleEvent::ConfigChanged {
            policy_changed: true,
        } => match mode {
            AppMode::AlwaysOn => input.previous_policy.is_none_or(|previous| {
                previous.mode != AppMode::AlwaysOn || previous.schedule.enabled
            }),
            AppMode::WiFi => {
                if input
                    .previous_policy
                    .is_none_or(|previous| previous.mode != AppMode::WiFi)
                {
                    input.wifi_active
                } else {
                    input.engine_active
                }
            }
            _ => input.engine_active,
        },
        _ => input.engine_active,
    };
    let (presentation, override_state) = status(wanted, override_state);
    LifecycleDecision {
        action: target(wanted),
        override_state,
        presentation,
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

pub(crate) fn should_jiggle(input: JiggleInput<'_>) -> bool {
    if !input.engine_active || matches!(input.jiggle_mode, JiggleMode::PowerOnly) {
        return false;
    }
    if matches!(
        input.override_state.as_state(),
        OverrideState::SkipWindow(_)
    ) {
        return false;
    }
    if !input.schedule_enabled {
        return true;
    }
    match effective_mode(input.mode) {
        AppMode::WiFi => input.window.in_window(),
        AppMode::Manual | AppMode::AlwaysOn => {
            input.window.in_window()
                || matches!(
                    input.override_state.as_state(),
                    OverrideState::ForceOn { .. }
                )
        }
        AppMode::Scheduled => false,
    }
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
    fn update_config(&mut self, _config: AppConfig) -> Result<(), String> {
        Ok(())
    }
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
        backend: &mut dyn EngineBackend,
    ) -> Result<RuntimeSnapshot, String> {
        let at = input.clock.now();
        let previous_wifi_active = self.wifi_active;
        if let LifecycleEvent::WifiChanged { active } = input.event {
            self.wifi_active = active;
        }
        let schedule = match Schedule::from_config(input.config) {
            Ok(schedule) => schedule,
            Err(error) => return self.fail_closed(input, backend, at, error),
        };
        let policy = RuntimePolicy {
            mode: effective_mode(&input.config.mode),
            schedule: schedule.clone(),
        };
        let policy_changed = self
            .previous_policy
            .as_ref()
            .is_some_and(|previous| previous != &policy);
        let window = match crate::schedule::evaluate_schedule(crate::schedule::ScheduleInput {
            schedule: &schedule,
            at,
            clock: input.clock,
        }) {
            Ok(window) => window,
            Err(error) => return self.fail_closed(input, backend, at, error),
        };
        let current_changed = self.previous_current != window.current;
        let normalized = normalize_override(NormalizeInput {
            mode: &input.config.mode,
            schedule_enabled: schedule.enabled,
            window: &window,
            override_state: &self.override_state,
            event: if policy_changed {
                LifecycleEvent::ConfigChanged {
                    policy_changed: true,
                }
            } else {
                input.event
            },
        });
        let decision = decide_lifecycle(LifecycleInput {
            mode: &input.config.mode,
            schedule_enabled: schedule.enabled,
            window: &window,
            engine_active: backend.is_active(),
            override_state: &normalized,
            event: if policy_changed {
                LifecycleEvent::ConfigChanged {
                    policy_changed: true,
                }
            } else {
                input.event
            },
            previous_policy: self.previous_policy.as_ref(),
            wifi_active: self.wifi_active,
        });
        self.override_state = decision.override_state.0.clone();
        backend.set_jiggle_policy(JigglePolicy {
            override_state: self.override_state.clone(),
        });

        let explicit_retry = matches!(
            input.event,
            LifecycleEvent::TrayToggle | LifecycleEvent::Resume | LifecycleEvent::Quit
        ) || policy_changed
            || current_changed
            || matches!(input.event, LifecycleEvent::WifiChanged { active } if active != previous_wifi_active);
        if decision.action != self.failed_action.unwrap_or(EngineAction::None) || explicit_retry {
            self.failed_action = None;
        }
        let mut error = None;
        if decision.action != EngineAction::None && self.failed_action != Some(decision.action) {
            let result = match decision.action {
                EngineAction::Start => backend.start(),
                EngineAction::Stop => backend.stop(),
                EngineAction::None => Ok(()),
            };
            if let Err(message) = result {
                self.failed_action = Some(decision.action);
                self.last_error = Some(message.clone());
                error = Some(message);
            } else {
                self.failed_action = None;
                self.last_error = None;
            }
        } else if self.failed_action == Some(decision.action) {
            error = self.last_error.clone();
        }
        let engine_active = backend.is_active();
        let status = derive_tray_status(TrayInput {
            mode: &input.config.mode,
            engine_active,
            schedule_enabled: schedule.enabled,
            window: &window,
            override_state: &decision.override_state,
        });
        let snapshot = RuntimeSnapshot {
            engine_active,
            override_state: decision.override_state,
            window,
            status,
        };
        self.last_snapshot = Some(snapshot.clone());
        self.previous_policy = Some(policy);
        self.previous_current = snapshot.window.current.clone();
        match error {
            Some(message) => Err(message),
            None => Ok(snapshot),
        }
    }

    fn fail_closed(
        &mut self,
        input: ReconcileInput<'_>,
        backend: &mut dyn EngineBackend,
        at: DateTime<Utc>,
        evaluation_error: String,
    ) -> Result<RuntimeSnapshot, String> {
        self.override_state = OverrideState::None;
        let stop_error = if backend.is_active() {
            backend.stop().err()
        } else {
            None
        };
        let engine_active = backend.is_active();
        let window = ScheduleEvaluation {
            at,
            current: None,
            next: None,
        };
        let schedule_owned = matches!(
            effective_mode(&input.config.mode),
            AppMode::Manual | AppMode::AlwaysOn
        ) && input.config.schedule_enabled;
        let status = if engine_active {
            TrayStatus::Active
        } else if schedule_owned {
            TrayStatus::Waiting
        } else {
            TrayStatus::Inactive
        };
        let snapshot = RuntimeSnapshot {
            engine_active,
            override_state: NormalizedOverride(OverrideState::None),
            window,
            status,
        };
        self.last_snapshot = Some(snapshot);
        self.failed_action = stop_error.as_ref().map(|_| EngineAction::Stop);
        let message = match stop_error {
            Some(error) => format!("{evaluation_error}; fail-closed stop failed: {error}"),
            None => evaluation_error,
        };
        self.last_error = Some(message.clone());
        Err(message)
    }

    pub(crate) fn snapshot(&self) -> Option<&RuntimeSnapshot> {
        self.last_snapshot.as_ref()
    }

    #[allow(dead_code)] // skeleton: wired in boundary 3/4
    pub(crate) fn reset_wifi_activation(&mut self) {
        self.wifi_active = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, TimeZone};

    fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, 2, hour, 0, 0).unwrap()
    }
    fn occurrence() -> Occurrence {
        Occurrence {
            start_day: NaiveDate::from_ymd_opt(2026, 3, 2).unwrap(),
            start: at(9),
            end: at(17),
        }
    }
    fn evaluation(inside: bool) -> ScheduleEvaluation {
        let occurrence = occurrence();
        ScheduleEvaluation {
            at: if inside { at(10) } else { at(18) },
            current: inside.then_some(occurrence.clone()),
            next: Some(Occurrence {
                start: at(18) + chrono::Duration::days(1),
                ..occurrence
            }),
        }
    }
    fn normalized(state: OverrideState) -> NormalizedOverride {
        NormalizedOverride(state)
    }
    fn expected_keys(status: TrayStatus) -> (&'static str, &'static str, TrayIcon) {
        match status {
            TrayStatus::Active => ("tray-running", "tray-tooltip-active", TrayIcon::Active),
            TrayStatus::Inactive => ("tray-start", "tray-tooltip-inactive", TrayIcon::Inactive),
            TrayStatus::Waiting => ("tray-waiting", "tray-tooltip-waiting", TrayIcon::Waiting),
            TrayStatus::Paused => ("tray-paused", "tray-tooltip-paused", TrayIcon::Inactive),
            TrayStatus::ActiveOverride => (
                "tray-active-override",
                "tray-tooltip-override",
                TrayIcon::Active,
            ),
        }
    }

    #[test]
    fn normalizes_scheduled_and_invalid_override_lifetimes() {
        assert_eq!(effective_mode(&AppMode::Scheduled), AppMode::Manual);
        assert_eq!(effective_mode(&AppMode::AlwaysOn), AppMode::AlwaysOn);
        let current = occurrence();
        let inside = evaluation(true);
        let outside = evaluation(false);
        let skip = OverrideState::SkipWindow(current.clone());
        assert_eq!(
            normalize_override(NormalizeInput {
                mode: &AppMode::Manual,
                schedule_enabled: true,
                window: &inside,
                override_state: &skip,
                event: LifecycleEvent::Tick
            })
            .as_state(),
            &skip
        );
        assert_eq!(
            normalize_override(NormalizeInput {
                mode: &AppMode::Manual,
                schedule_enabled: true,
                window: &outside,
                override_state: &skip,
                event: LifecycleEvent::Tick
            })
            .as_state(),
            &OverrideState::None
        );
        let expired = OverrideState::ForceOn {
            until: Some(at(18)),
        };
        assert_eq!(
            normalize_override(NormalizeInput {
                mode: &AppMode::Manual,
                schedule_enabled: true,
                window: &outside,
                override_state: &expired,
                event: LifecycleEvent::Tick
            })
            .as_state(),
            &OverrideState::None
        );
        assert_eq!(
            normalize_override(NormalizeInput {
                mode: &AppMode::Manual,
                schedule_enabled: true,
                window: &inside,
                override_state: &OverrideState::ForceOn { until: None },
                event: LifecycleEvent::Tick
            })
            .as_state(),
            &OverrideState::None
        );
        assert_eq!(
            normalize_override(NormalizeInput {
                mode: &AppMode::Manual,
                schedule_enabled: true,
                window: &outside,
                override_state: &OverrideState::ForceOn { until: None },
                event: LifecycleEvent::ConfigChanged {
                    policy_changed: true
                }
            })
            .as_state(),
            &OverrideState::None
        );
    }

    #[test]
    fn derives_status_for_each_normalized_mode_activity_schedule_window_and_override_row() {
        let current = occurrence();
        for mode in [
            AppMode::Manual,
            AppMode::AlwaysOn,
            AppMode::WiFi,
            AppMode::Scheduled,
        ] {
            for active in [false, true] {
                for enabled in [false, true] {
                    for inside in [false, true] {
                        for override_state in [
                            OverrideState::None,
                            OverrideState::SkipWindow(current.clone()),
                            OverrideState::ForceOn {
                                until: Some(at(18) + chrono::Duration::days(1)),
                            },
                        ] {
                            let window = evaluation(inside);
                            let normalized = normalize_override(NormalizeInput {
                                mode: &mode,
                                schedule_enabled: enabled,
                                window: &window,
                                override_state: &override_state,
                                event: LifecycleEvent::Tick,
                            });
                            let actual = derive_tray_status(TrayInput {
                                mode: &mode,
                                engine_active: active,
                                schedule_enabled: enabled,
                                window: &window,
                                override_state: &normalized,
                            });
                            let expected = if mode == AppMode::WiFi || !enabled {
                                if active {
                                    TrayStatus::Active
                                } else {
                                    TrayStatus::Inactive
                                }
                            } else if active
                                && !inside
                                && matches!(normalized.as_state(), OverrideState::ForceOn { .. })
                            {
                                TrayStatus::ActiveOverride
                            } else if active {
                                TrayStatus::Active
                            } else if inside
                                && matches!(normalized.as_state(), OverrideState::SkipWindow(_))
                            {
                                TrayStatus::Paused
                            } else if !inside
                                && matches!(normalized.as_state(), OverrideState::None)
                            {
                                TrayStatus::Waiting
                            } else {
                                TrayStatus::Inactive
                            };
                            assert_eq!(actual, expected);
                            assert_eq!(
                                (actual.menu_key(), actual.tooltip_key(), actual.icon()),
                                expected_keys(expected)
                            );
                        }
                    }
                }
            }
        }
    }

    fn decide(
        mode: AppMode,
        enabled: bool,
        inside: bool,
        active: bool,
        override_state: OverrideState,
        event: LifecycleEvent,
        wifi_active: bool,
    ) -> LifecycleDecision {
        let window = evaluation(inside);
        let override_state = normalized(override_state);
        decide_lifecycle(LifecycleInput {
            mode: &mode,
            schedule_enabled: enabled,
            window: &window,
            engine_active: active,
            override_state: &override_state,
            event,
            previous_policy: None,
            wifi_active,
        })
    }

    #[test]
    fn decides_startup_toggle_tick_config_resume_wifi_and_quit_from_the_contract_table() {
        let cases = [
            (
                AppMode::Manual,
                false,
                false,
                false,
                OverrideState::None,
                LifecycleEvent::Startup,
                false,
                EngineAction::None,
                TrayStatus::Inactive,
            ),
            (
                AppMode::AlwaysOn,
                false,
                false,
                false,
                OverrideState::None,
                LifecycleEvent::Startup,
                false,
                EngineAction::Start,
                TrayStatus::Active,
            ),
            (
                AppMode::Scheduled,
                true,
                true,
                false,
                OverrideState::None,
                LifecycleEvent::Startup,
                false,
                EngineAction::Start,
                TrayStatus::Active,
            ),
            (
                AppMode::AlwaysOn,
                true,
                false,
                true,
                OverrideState::None,
                LifecycleEvent::Startup,
                false,
                EngineAction::Stop,
                TrayStatus::Waiting,
            ),
            (
                AppMode::Manual,
                true,
                true,
                false,
                OverrideState::None,
                LifecycleEvent::TrayToggle,
                false,
                EngineAction::Start,
                TrayStatus::Active,
            ),
            (
                AppMode::Manual,
                true,
                true,
                true,
                OverrideState::None,
                LifecycleEvent::TrayToggle,
                false,
                EngineAction::Stop,
                TrayStatus::Paused,
            ),
            (
                AppMode::Manual,
                true,
                false,
                false,
                OverrideState::None,
                LifecycleEvent::TrayToggle,
                false,
                EngineAction::Start,
                TrayStatus::ActiveOverride,
            ),
            (
                AppMode::Manual,
                true,
                false,
                true,
                OverrideState::ForceOn {
                    until: Some(at(18) + chrono::Duration::days(1)),
                },
                LifecycleEvent::TrayToggle,
                false,
                EngineAction::Stop,
                TrayStatus::Waiting,
            ),
            (
                AppMode::Manual,
                true,
                true,
                false,
                OverrideState::None,
                LifecycleEvent::Tick,
                false,
                EngineAction::Start,
                TrayStatus::Active,
            ),
            (
                AppMode::Manual,
                true,
                false,
                true,
                OverrideState::None,
                LifecycleEvent::Resume,
                false,
                EngineAction::Stop,
                TrayStatus::Waiting,
            ),
            (
                AppMode::Manual,
                true,
                false,
                true,
                OverrideState::None,
                LifecycleEvent::WifiChanged { active: false },
                false,
                EngineAction::None,
                TrayStatus::Active,
            ),
            (
                AppMode::WiFi,
                true,
                false,
                false,
                OverrideState::None,
                LifecycleEvent::WifiChanged { active: true },
                false,
                EngineAction::Start,
                TrayStatus::Active,
            ),
            (
                AppMode::Manual,
                true,
                false,
                true,
                OverrideState::None,
                LifecycleEvent::ConfigChanged {
                    policy_changed: true,
                },
                false,
                EngineAction::Stop,
                TrayStatus::Waiting,
            ),
            (
                AppMode::Manual,
                true,
                true,
                true,
                OverrideState::None,
                LifecycleEvent::Quit,
                false,
                EngineAction::Stop,
                TrayStatus::Inactive,
            ),
        ];
        for (mode, enabled, inside, active, override_state, event, wifi, action, status) in cases {
            let decision = decide(mode, enabled, inside, active, override_state, event, wifi);
            assert_eq!((decision.action, decision.presentation), (action, status));
        }
    }

    #[test]
    fn retains_valid_overrides_and_preserves_non_owned_manual_activity() {
        let paused = decide(
            AppMode::Manual,
            true,
            true,
            false,
            OverrideState::SkipWindow(occurrence()),
            LifecycleEvent::Tick,
            false,
        );
        assert_eq!(
            (paused.action, paused.presentation),
            (EngineAction::None, TrayStatus::Paused)
        );

        for event in [LifecycleEvent::Tick, LifecycleEvent::Resume] {
            let forced = decide(
                AppMode::AlwaysOn,
                true,
                false,
                false,
                OverrideState::ForceOn {
                    until: Some(at(18) + chrono::Duration::days(1)),
                },
                event,
                false,
            );
            assert_eq!(
                (forced.action, forced.presentation),
                (EngineAction::Start, TrayStatus::ActiveOverride)
            );
        }

        for active in [false, true] {
            let toggled = decide(
                AppMode::Manual,
                false,
                false,
                active,
                OverrideState::None,
                LifecycleEvent::TrayToggle,
                false,
            );
            assert_eq!(
                toggled.action,
                if active {
                    EngineAction::Stop
                } else {
                    EngineAction::Start
                }
            );
        }
    }

    #[test]
    fn non_entry_always_on_config_change_preserves_actual_activity() {
        let disabled = Schedule {
            enabled: false,
            start: chrono::NaiveTime::from_hms_opt(9, 0, 0).expect("valid time"),
            end: chrono::NaiveTime::from_hms_opt(10, 0, 0).expect("valid time"),
            days: vec![chrono::Weekday::Mon],
        };
        let previous = RuntimePolicy {
            mode: AppMode::AlwaysOn,
            schedule: disabled.clone(),
        };
        let window = evaluation(false);
        let override_state = normalized(OverrideState::None);

        let decision = decide_lifecycle(LifecycleInput {
            mode: &AppMode::AlwaysOn,
            schedule_enabled: false,
            window: &window,
            engine_active: false,
            override_state: &override_state,
            event: LifecycleEvent::ConfigChanged {
                policy_changed: true,
            },
            previous_policy: Some(&previous),
            wifi_active: false,
        });

        assert_eq!(
            decision.action,
            EngineAction::None,
            "a disabled-schedule edit is not an AlwaysOn entry and preserves actual inactivity"
        );
    }

    struct UnresolvableClock;

    impl Clock for UnresolvableClock {
        fn now(&self) -> DateTime<Utc> {
            at(10)
        }

        fn to_local(&self, instant: DateTime<Utc>) -> chrono::DateTime<chrono::FixedOffset> {
            instant.with_timezone(&chrono::FixedOffset::east_opt(0).expect("UTC offset"))
        }

        fn resolve_local(&self, _: chrono::NaiveDateTime) -> chrono::LocalResult<DateTime<Utc>> {
            chrono::LocalResult::None
        }
    }

    #[test]
    fn schedule_evaluation_failure_stops_active_backend_and_keeps_a_truthful_snapshot() {
        let config = scheduled_config(AppMode::Manual, JiggleMode::MouseSubtle);
        let mut supervisor = Supervisor::new();
        let mut backend = RecordingBackend {
            active: true,
            ..RecordingBackend::inactive()
        };
        let clock = UnresolvableClock;

        assert!(
            supervisor
                .reconcile(
                    ReconcileInput {
                        config: &config,
                        clock: &clock,
                        event: LifecycleEvent::Tick,
                    },
                    &mut backend,
                )
                .is_err(),
            "an unresolvable enabled schedule reports its evaluation error"
        );
        let snapshot = supervisor.snapshot();
        assert!(
            snapshot.is_some(),
            "fail-closed reconciliation stores the post-stop state"
        );
        if let Some(snapshot) = snapshot {
            assert_eq!(
                backend.stop_calls, 1,
                "fail-closed evaluation releases an active lease"
            );
            assert!(
                !snapshot.engine_active,
                "snapshot records the confirmed release"
            );
            assert_eq!(snapshot.status, TrayStatus::Waiting);
        }
    }

    #[test]
    fn clears_skipwindow_and_forceon_at_their_required_boundaries() {
        let skip = decide(
            AppMode::Manual,
            true,
            true,
            false,
            OverrideState::SkipWindow(occurrence()),
            LifecycleEvent::TrayToggle,
            false,
        );
        assert_eq!(
            (skip.action, skip.override_state.as_state()),
            (EngineAction::Start, &OverrideState::None)
        );
        let force = decide(
            AppMode::Manual,
            true,
            true,
            true,
            OverrideState::ForceOn {
                until: Some(at(18) + chrono::Duration::days(1)),
            },
            LifecycleEvent::Tick,
            false,
        );
        assert_eq!(
            (
                force.action,
                force.override_state.as_state(),
                force.presentation
            ),
            (EngineAction::None, &OverrideState::None, TrayStatus::Active)
        );
    }

    #[test]
    fn requires_the_full_jiggle_policy_matrix() {
        for mode in [
            AppMode::Manual,
            AppMode::AlwaysOn,
            AppMode::WiFi,
            AppMode::Scheduled,
        ] {
            for active in [false, true] {
                for enabled in [false, true] {
                    for inside in [false, true] {
                        let window = evaluation(inside);
                        let mut raw_overrides = vec![OverrideState::None];
                        if enabled && mode != AppMode::WiFi {
                            raw_overrides.push(if inside {
                                OverrideState::SkipWindow(occurrence())
                            } else {
                                OverrideState::ForceOn {
                                    until: Some(at(18) + chrono::Duration::days(1)),
                                }
                            });
                        }
                        for raw_override in raw_overrides {
                            let override_state = normalize_override(NormalizeInput {
                                mode: &mode,
                                schedule_enabled: enabled,
                                window: &window,
                                override_state: &raw_override,
                                event: LifecycleEvent::Tick,
                            });
                            let expected = active
                                && match JiggleMode::MouseSubtle {
                                    JiggleMode::PowerOnly => false,
                                    _ if !enabled => true,
                                    _ if effective_mode(&mode) == AppMode::WiFi => inside,
                                    _ => {
                                        inside
                                            && !matches!(
                                                override_state.as_state(),
                                                OverrideState::SkipWindow(_)
                                            )
                                            || matches!(
                                                override_state.as_state(),
                                                OverrideState::ForceOn { .. }
                                            )
                                    }
                                };
                            assert_eq!(
                                should_jiggle(JiggleInput {
                                    mode: &mode,
                                    jiggle_mode: &JiggleMode::MouseSubtle,
                                    engine_active: active,
                                    schedule_enabled: enabled,
                                    window: &window,
                                    override_state: &override_state,
                                }),
                                expected,
                                "{mode:?}, active={active}, enabled={enabled}, inside={inside}, {raw_override:?}"
                            );
                            assert!(!should_jiggle(JiggleInput {
                                mode: &mode,
                                jiggle_mode: &JiggleMode::PowerOnly,
                                engine_active: active,
                                schedule_enabled: enabled,
                                window: &window,
                                override_state: &override_state,
                            }));
                        }
                    }
                }
            }
        }
    }

    struct AdvancingClock {
        now: std::sync::Mutex<DateTime<Utc>>,
    }

    impl AdvancingClock {
        fn new(now: DateTime<Utc>) -> Self {
            Self {
                now: std::sync::Mutex::new(now),
            }
        }

        fn advance_periods(&self, periods: i64) {
            let mut now = self.now.lock().expect("test clock lock");
            *now += chrono::Duration::milliseconds(250 * periods);
        }

        fn jump_to(&self, instant: DateTime<Utc>) {
            *self.now.lock().expect("test clock lock") = instant;
        }
    }

    impl Clock for AdvancingClock {
        fn now(&self) -> DateTime<Utc> {
            *self.now.lock().expect("test clock lock")
        }

        fn to_local(&self, instant: DateTime<Utc>) -> chrono::DateTime<chrono::FixedOffset> {
            instant.with_timezone(&chrono::FixedOffset::east_opt(0).expect("UTC offset"))
        }

        fn resolve_local(
            &self,
            local: chrono::NaiveDateTime,
        ) -> chrono::LocalResult<DateTime<Utc>> {
            chrono::LocalResult::Single(local.and_utc())
        }
    }

    struct RecordingBackend {
        active: bool,
        start_calls: usize,
        stop_calls: usize,
        start_failures: usize,
        stop_failures: usize,
        cleanup_failure_releases: bool,
        policies: usize,
    }

    impl RecordingBackend {
        fn inactive() -> Self {
            Self {
                active: false,
                start_calls: 0,
                stop_calls: 0,
                start_failures: 0,
                stop_failures: 0,
                cleanup_failure_releases: false,
                policies: 0,
            }
        }
    }

    impl EngineBackend for RecordingBackend {
        fn start(&mut self) -> Result<(), String> {
            self.start_calls += 1;
            if self.start_failures > 0 {
                self.start_failures -= 1;
                return Err("start failed".to_owned());
            }
            self.active = true;
            Ok(())
        }

        fn stop(&mut self) -> Result<(), String> {
            self.stop_calls += 1;
            if self.stop_failures > 0 {
                self.stop_failures -= 1;
                return Err("stop failed".to_owned());
            }
            self.active = false;
            if self.cleanup_failure_releases {
                return Err("cleanup failed after release".to_owned());
            }
            Ok(())
        }

        fn is_active(&self) -> bool {
            self.active
        }

        fn set_jiggle_policy(&mut self, _: JigglePolicy) {
            self.policies += 1;
        }
    }

    fn scheduled_config(mode: AppMode, jiggle_mode: JiggleMode) -> AppConfig {
        AppConfig {
            mode,
            jiggle_mode,
            interval_secs: 300,
            schedule_enabled: true,
            schedule_start_hour: 9,
            schedule_start_minute: 0,
            schedule_end_hour: 10,
            schedule_end_minute: 0,
            schedule_days: vec!["mon".to_owned()],
            ..AppConfig::default()
        }
    }

    fn reconcile(
        supervisor: &mut Supervisor,
        backend: &mut RecordingBackend,
        config: &AppConfig,
        clock: &AdvancingClock,
        event: LifecycleEvent,
    ) -> Result<RuntimeSnapshot, String> {
        supervisor.reconcile(
            ReconcileInput {
                config,
                clock,
                event,
            },
            backend,
        )
    }

    #[test]
    fn idle_supervisor_starts_the_next_window_on_its_first_tick() {
        let clock = AdvancingClock::new(
            at(8)
                + chrono::Duration::minutes(59)
                + chrono::Duration::seconds(59)
                + chrono::Duration::milliseconds(750),
        );
        let config = scheduled_config(AppMode::Manual, JiggleMode::MouseSubtle);
        let mut supervisor = Supervisor::new();
        let mut backend = RecordingBackend::inactive();

        reconcile(
            &mut supervisor,
            &mut backend,
            &config,
            &clock,
            LifecycleEvent::Startup,
        )
        .unwrap();
        clock.advance_periods(1);
        let snapshot = reconcile(
            &mut supervisor,
            &mut backend,
            &config,
            &clock,
            LifecycleEvent::Tick,
        )
        .expect("window opening reconciles");

        assert_eq!(SUPERVISOR_PERIOD, Duration::from_millis(250));
        assert_eq!(backend.start_calls, 1);
        assert!(snapshot.engine_active);
        assert_eq!(snapshot.status, TrayStatus::Active);
    }

    #[test]
    fn window_close_stops_once_and_publishes_waiting() {
        let clock = AdvancingClock::new(at(9));
        let config = scheduled_config(AppMode::Manual, JiggleMode::MouseSubtle);
        let mut supervisor = Supervisor::new();
        let mut backend = RecordingBackend::inactive();

        reconcile(
            &mut supervisor,
            &mut backend,
            &config,
            &clock,
            LifecycleEvent::Startup,
        )
        .unwrap();
        clock.advance_periods(14_400);
        let snapshot = reconcile(
            &mut supervisor,
            &mut backend,
            &config,
            &clock,
            LifecycleEvent::Tick,
        )
        .expect("window close reconciles");
        reconcile(
            &mut supervisor,
            &mut backend,
            &config,
            &clock,
            LifecycleEvent::Tick,
        )
        .unwrap();

        assert_eq!(backend.stop_calls, 1);
        assert!(!snapshot.engine_active);
        assert_eq!(snapshot.status, TrayStatus::Waiting);
    }

    #[test]
    fn unchanged_reconciliation_makes_no_lifecycle_calls() {
        let clock = AdvancingClock::new(at(8));
        let config = scheduled_config(AppMode::Manual, JiggleMode::MouseSubtle);
        let mut supervisor = Supervisor::new();
        let mut backend = RecordingBackend::inactive();

        reconcile(
            &mut supervisor,
            &mut backend,
            &config,
            &clock,
            LifecycleEvent::Startup,
        )
        .unwrap();
        reconcile(
            &mut supervisor,
            &mut backend,
            &config,
            &clock,
            LifecycleEvent::Tick,
        )
        .unwrap();

        assert_eq!((backend.start_calls, backend.stop_calls), (0, 0));
        assert_eq!(
            supervisor.snapshot().expect("published snapshot").status,
            TrayStatus::Waiting
        );
    }

    #[test]
    fn force_on_crossing_a_window_start_stays_active_without_a_stop_or_restart() {
        let clock = AdvancingClock::new(at(8));
        let config = scheduled_config(AppMode::Manual, JiggleMode::MouseSubtle);
        let mut supervisor = Supervisor::new();
        let mut backend = RecordingBackend::inactive();

        reconcile(
            &mut supervisor,
            &mut backend,
            &config,
            &clock,
            LifecycleEvent::Startup,
        )
        .unwrap();
        reconcile(
            &mut supervisor,
            &mut backend,
            &config,
            &clock,
            LifecycleEvent::TrayToggle,
        )
        .unwrap();
        clock.advance_periods(14_400);
        let snapshot = reconcile(
            &mut supervisor,
            &mut backend,
            &config,
            &clock,
            LifecycleEvent::Tick,
        )
        .expect("ForceOn boundary reconciles");

        assert_eq!((backend.start_calls, backend.stop_calls), (1, 0));
        assert!(snapshot.engine_active);
        assert_eq!(snapshot.status, TrayStatus::Active);
    }

    #[test]
    fn expired_skip_window_leaves_the_engine_idle_and_publishes_waiting() {
        let clock = AdvancingClock::new(at(9));
        let config = scheduled_config(AppMode::Manual, JiggleMode::MouseSubtle);
        let mut supervisor = Supervisor::new();
        let mut backend = RecordingBackend::inactive();

        reconcile(
            &mut supervisor,
            &mut backend,
            &config,
            &clock,
            LifecycleEvent::Startup,
        )
        .unwrap();
        reconcile(
            &mut supervisor,
            &mut backend,
            &config,
            &clock,
            LifecycleEvent::TrayToggle,
        )
        .unwrap();
        clock.advance_periods(14_400);
        let snapshot = reconcile(
            &mut supervisor,
            &mut backend,
            &config,
            &clock,
            LifecycleEvent::Tick,
        )
        .expect("expired suppression reconciles");

        assert_eq!((backend.start_calls, backend.stop_calls), (1, 1));
        assert!(!snapshot.engine_active);
        assert_eq!(snapshot.status, TrayStatus::Waiting);
    }

    #[test]
    fn failed_start_is_inactive_is_not_retried_each_tick_and_retries_on_resume() {
        let clock = AdvancingClock::new(at(9));
        let config = scheduled_config(AppMode::Manual, JiggleMode::MouseSubtle);
        let mut supervisor = Supervisor::new();
        let mut backend = RecordingBackend {
            start_failures: 1,
            ..RecordingBackend::inactive()
        };

        assert_eq!(
            reconcile(
                &mut supervisor,
                &mut backend,
                &config,
                &clock,
                LifecycleEvent::Startup
            ),
            Err("start failed".to_owned())
        );
        for _ in 0..4 {
            assert_eq!(
                reconcile(
                    &mut supervisor,
                    &mut backend,
                    &config,
                    &clock,
                    LifecycleEvent::Tick
                ),
                Err("start failed".to_owned())
            );
        }
        let snapshot = reconcile(
            &mut supervisor,
            &mut backend,
            &config,
            &clock,
            LifecycleEvent::Resume,
        )
        .expect("resume rearms a failed start");

        assert_eq!(backend.start_calls, 2);
        assert!(snapshot.engine_active);
        assert_eq!(snapshot.status, TrayStatus::Active);
    }

    #[test]
    fn failed_stop_keeps_active_ownership_and_retries_on_later_reconciliation() {
        let clock = AdvancingClock::new(at(10));
        let config = scheduled_config(AppMode::Manual, JiggleMode::MouseSubtle);
        let mut supervisor = Supervisor::new();
        let mut backend = RecordingBackend {
            active: true,
            stop_failures: 1,
            ..RecordingBackend::inactive()
        };

        assert_eq!(
            reconcile(
                &mut supervisor,
                &mut backend,
                &config,
                &clock,
                LifecycleEvent::Tick
            ),
            Err("stop failed".to_owned())
        );
        let failed = supervisor.snapshot().expect("failure snapshot");
        assert!(failed.engine_active);
        assert_eq!(failed.status, TrayStatus::Active);
        let recovered = reconcile(
            &mut supervisor,
            &mut backend,
            &config,
            &clock,
            LifecycleEvent::Resume,
        )
        .expect("resume retries failed stop");

        assert_eq!(backend.stop_calls, 2);
        assert!(!recovered.engine_active);
        assert_eq!(recovered.status, TrayStatus::Waiting);
    }

    #[test]
    fn cleanup_error_does_not_skip_the_release_attempt() {
        let clock = AdvancingClock::new(at(10));
        let config = scheduled_config(AppMode::Manual, JiggleMode::MouseSubtle);
        let mut supervisor = Supervisor::new();
        let mut backend = RecordingBackend {
            active: true,
            cleanup_failure_releases: true,
            ..RecordingBackend::inactive()
        };

        assert_eq!(
            reconcile(
                &mut supervisor,
                &mut backend,
                &config,
                &clock,
                LifecycleEvent::Tick
            ),
            Err("cleanup failed after release".to_owned())
        );

        assert_eq!(backend.stop_calls, 1);
        assert!(!backend.active);
        assert_eq!(
            supervisor.snapshot().expect("cleanup snapshot").status,
            TrayStatus::Waiting
        );
    }

    #[test]
    fn resume_after_a_whole_missed_window_does_not_replay_its_start() {
        let clock = AdvancingClock::new(at(8));
        let config = scheduled_config(AppMode::Manual, JiggleMode::MouseSubtle);
        let mut supervisor = Supervisor::new();
        let mut backend = RecordingBackend::inactive();

        reconcile(
            &mut supervisor,
            &mut backend,
            &config,
            &clock,
            LifecycleEvent::Startup,
        )
        .unwrap();
        clock.jump_to(at(11));
        let snapshot = reconcile(
            &mut supervisor,
            &mut backend,
            &config,
            &clock,
            LifecycleEvent::Resume,
        )
        .expect("resume reconciles current time");

        assert_eq!((backend.start_calls, backend.stop_calls), (0, 0));
        assert_eq!(snapshot.status, TrayStatus::Waiting);
    }

    #[test]
    fn always_on_with_an_enabled_schedule_stays_stopped_outside_the_window() {
        let clock = AdvancingClock::new(at(8));
        let config = scheduled_config(AppMode::AlwaysOn, JiggleMode::MouseSubtle);
        let mut supervisor = Supervisor::new();
        let mut backend = RecordingBackend::inactive();

        let snapshot = reconcile(
            &mut supervisor,
            &mut backend,
            &config,
            &clock,
            LifecycleEvent::Startup,
        )
        .expect("startup reconciles");

        assert_eq!((backend.start_calls, backend.stop_calls), (0, 0));
        assert!(!snapshot.engine_active);
        assert_eq!(snapshot.status, TrayStatus::Waiting);
    }

    #[test]
    fn wifi_mode_is_never_started_or_stopped_by_schedule_reconciliation() {
        let clock = AdvancingClock::new(at(8));
        let config = scheduled_config(AppMode::WiFi, JiggleMode::MouseSubtle);
        let mut supervisor = Supervisor::new();
        let mut backend = RecordingBackend {
            active: true,
            ..RecordingBackend::inactive()
        };

        let snapshot = reconcile(
            &mut supervisor,
            &mut backend,
            &config,
            &clock,
            LifecycleEvent::Tick,
        )
        .expect("WiFi tick preserves network lifecycle");

        assert_eq!((backend.start_calls, backend.stop_calls), (0, 0));
        assert!(snapshot.engine_active);
        assert_eq!(snapshot.status, TrayStatus::Active);
    }

    #[test]
    fn power_only_follows_the_same_scheduled_start_and_stop_lifecycle() {
        let clock = AdvancingClock::new(at(9));
        let config = scheduled_config(AppMode::Manual, JiggleMode::PowerOnly);
        let mut supervisor = Supervisor::new();
        let mut backend = RecordingBackend::inactive();

        reconcile(
            &mut supervisor,
            &mut backend,
            &config,
            &clock,
            LifecycleEvent::Startup,
        )
        .unwrap();
        clock.advance_periods(14_400);
        let snapshot = reconcile(
            &mut supervisor,
            &mut backend,
            &config,
            &clock,
            LifecycleEvent::Tick,
        )
        .expect("PowerOnly window close reconciles");

        assert_eq!((backend.start_calls, backend.stop_calls), (1, 1));
        assert_eq!(snapshot.status, TrayStatus::Waiting);
    }
}
